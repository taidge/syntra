use std::sync::Arc;

use anyhow::{Context, bail};
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use horme::prelude::*;
use salvo::prelude::*;
use tracing_subscriber::EnvFilter;

use contrix_collab::CollabManager;
use contrix_core::config::AppConfig;

use contrix_server::{database, handlers, routes, services, state};

use state::AppState;

const DEFAULT_JWT_SECRET: &str = "change-me-in-production-use-a-long-random-string";

fn load_config() -> anyhow::Result<(AppConfig, String)> {
    let environment = std::env::var("CONTRIX_ENV")
        .or_else(|_| std::env::var("APP_ENV"))
        .unwrap_or_else(|_| "development".to_string());
    let environment = environment.to_ascii_lowercase();

    let mut figment = Figment::new().merge(Toml::file("config/default.toml"));
    let environment_config = format!("config/{environment}.toml");
    if std::path::Path::new(&environment_config).exists() {
        figment = figment.merge(Toml::file(&environment_config));
    }

    let mut config: AppConfig = figment
        .merge(Env::prefixed("CONTRIX_").split("_"))
        .extract()
        .context("Failed to load application configuration")?;

    if config.auth.jwt_secret == "${JWT_SECRET}" {
        config.auth.jwt_secret = std::env::var("JWT_SECRET")
            .context("JWT_SECRET must be set when config uses ${JWT_SECRET}")?;
    }

    validate_config(&config, &environment)?;

    Ok((config, environment))
}

fn validate_config(config: &AppConfig, environment: &str) -> anyhow::Result<()> {
    if config.storage.upload_idle_timeout_seconds == 0
        || config.storage.upload_total_timeout_seconds == 0
        || config.storage.upload_idle_timeout_seconds > config.storage.upload_total_timeout_seconds
    {
        bail!("Upload timeouts must be positive and idle timeout must not exceed total timeout");
    }
    let maintenance = &config.maintenance;
    if !(1..=10000).contains(&maintenance.history_max_versions)
        || !(1..=1000).contains(&maintenance.history_batch_size)
        || !(1..=1000).contains(&maintenance.scan_page_size)
        || maintenance.interval_seconds == 0
        || maintenance.orphan_grace_seconds < 300
    {
        bail!(
            "Maintenance requires max_versions 1..10000, batch/page 1..1000, positive interval and orphan grace >=300 seconds"
        );
    }
    services::database_backup::sqlite_url_to_path(&config.database.url)?;

    let storage_backend = config
        .storage
        .backend
        .as_deref()
        .unwrap_or("local")
        .to_ascii_lowercase();
    match storage_backend.as_str() {
        "local" => {}
        "s3" => {
            if config.storage.s3_bucket.as_deref().unwrap_or("").is_empty()
                || config
                    .storage
                    .s3_access_key
                    .as_deref()
                    .unwrap_or("")
                    .is_empty()
                || config
                    .storage
                    .s3_secret_key
                    .as_deref()
                    .unwrap_or("")
                    .is_empty()
            {
                bail!(
                    "S3 storage requires storage.s3_bucket, storage.s3_access_key, and storage.s3_secret_key"
                );
            }
        }
        backend => bail!("Unsupported storage backend '{backend}'"),
    }

    if config.auth.jwt_secret.trim().is_empty() {
        bail!("auth.jwt_secret must not be empty");
    }

    if environment == "production"
        && (config.auth.jwt_secret == DEFAULT_JWT_SECRET || config.auth.jwt_secret.len() < 32)
    {
        bail!(
            "Production auth.jwt_secret must be overridden with a strong secret of at least 32 bytes"
        );
    }

    if config.ai.enabled {
        let endpoint = reqwest::Url::parse(&config.ai.endpoint)
            .context("ai.endpoint must be an absolute HTTP(S) URL")?;
        if !matches!(endpoint.scheme(), "http" | "https") {
            bail!("ai.endpoint must use http or https");
        }
        if environment == "production" && endpoint.scheme() != "https" {
            bail!("Production ai.endpoint must use https");
        }
        if config.ai.api_key.trim().is_empty() || config.ai.model.trim().is_empty() {
            bail!("Enabled AI requires ai.api_key and ai.model");
        }
        if config.ai.timeout_seconds == 0
            || config.ai.max_requests_per_hour == 0
            || config.ai.max_prompt_chars == 0
            || config.ai.max_document_chars == 0
            || config.ai.max_output_chars == 0
        {
            bail!("Enabled AI limits and timeout must be greater than zero");
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // -----------------------------------------------------------------------
    // 1. Load configuration
    // -----------------------------------------------------------------------
    let (config, environment) = load_config()?;

    // -----------------------------------------------------------------------
    // 2. Set up tracing
    // -----------------------------------------------------------------------
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,contrix_server=debug,salvo=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!("Starting Contrix Office server in {environment} environment");

    // -----------------------------------------------------------------------
    // 3. Ensure data directories exist before connecting
    // -----------------------------------------------------------------------
    let db_path = services::database_backup::sqlite_url_to_path(&config.database.url)?;
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    if services::database_backup::is_file_backed(&db_path)
        && services::database_backup::apply_pending_restore(&db_path).await?
    {
        tracing::warn!(
            database = %db_path.display(),
            "Applied staged database restore before opening the connection pool"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Create an on-demand Horme / Diesel-async SQLite provider
    // -----------------------------------------------------------------------
    let db = database::Database::connect(&config.database)
        .context("Failed to configure startup database pool")?;

    tracing::info!("Database pool created");

    // -----------------------------------------------------------------------
    // 5. Run the shared migration catalog in order
    // -----------------------------------------------------------------------
    let mut migrations = Vec::new();
    for filename in contrix_server::migrations::MIGRATION_FILES {
        let migration_path = format!("migrations/{filename}");
        let sql = tokio::fs::read_to_string(&migration_path)
            .await
            .with_context(|| format!("Could not read required migration file {migration_path}"))?;
        migrations.push((migration_path, sql));
    }
    let migration_started_at = std::time::Instant::now();
    db.run_startup_migrations(move |conn| {
        for (migration_path, sql) in migrations {
            contrix_server::migrations::run_migration_file(conn, &migration_path, &sql)?;
            tracing::info!("Migration applied: {migration_path}");
        }
        Ok(())
    })
    .await?;
    tracing::info!(
        elapsed_ms = migration_started_at.elapsed().as_millis() as u64,
        "Startup database connection and migrations completed"
    );

    // -----------------------------------------------------------------------
    // 6. Ensure storage directories exist
    // -----------------------------------------------------------------------
    let storage_path = &config.storage.base_path;
    tokio::fs::create_dir_all(storage_path).await?;
    tracing::info!("Local storage workspace ready: {storage_path}");
    let storage_backend = Arc::new(
        services::StorageBackend::from_config(&config.storage)
            .context("Failed to initialize storage backend")?,
    );
    storage_backend.attach_database(db.clone())?;
    handlers::convert::resume_pending_conversions(&db, storage_backend.clone()).await;

    // Ensure the database-specific backup directory exists.
    if services::database_backup::is_file_backed(&db_path) {
        tokio::fs::create_dir_all(services::database_backup::backup_directory(&db_path)).await?;
    }

    // Ensure chunks directory exists for chunked uploads
    tokio::fs::create_dir_all(format!("{}/chunks", storage_path)).await?;

    // -----------------------------------------------------------------------
    // 7. Initialize collaboration manager
    // -----------------------------------------------------------------------
    let collab = CollabManager::new();

    let ai_service = config
        .ai
        .enabled
        .then(|| services::AiService::new(config.ai.clone()))
        .transpose()?;

    // -----------------------------------------------------------------------
    // 7c. Initialize thumbnail service
    // -----------------------------------------------------------------------
    let thumbnail_service = Some(services::ThumbnailService::new());
    tracing::info!("Thumbnail service initialized");

    // -----------------------------------------------------------------------
    // 8. Build application state and router
    // -----------------------------------------------------------------------
    let state = Arc::new(AppState {
        db,
        config: config.clone(),
        storage: storage_backend,
        collab,
        started_at: chrono::Utc::now(),
        ai: ai_service,
        thumbnail: thumbnail_service,
        notifications: Arc::new(services::NotificationBus::new()),
        wopi_sessions: Arc::new(services::WopiSessionStore::new()),
    });

    // -----------------------------------------------------------------------
    // 8b. Shared transactional trash purge and retryable object cleanup.
    // -----------------------------------------------------------------------
    if config.maintenance.history_enabled {
        let history_db = state.db.clone();
        let history_config = config.maintenance.clone();
        tokio::spawn(async move {
            let mut cursor = services::maintenance::NativeHistoryCountCursor::default();
            let mut sweep_removed_rows = false;
            let idle = std::time::Duration::from_secs(history_config.interval_seconds.max(1));
            let active = std::time::Duration::from_millis(250);
            loop {
                let delay = match services::maintenance::retain_native_history_count_page(
                    &history_db,
                    &mut cursor,
                    &history_config,
                )
                .await
                {
                    Ok(page) => {
                        sweep_removed_rows |= page.removed != 0;
                        if page.sweep_finished && !std::mem::take(&mut sweep_removed_rows) {
                            idle
                        } else {
                            active
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Native history count catch-up will retry");
                        idle.min(std::time::Duration::from_secs(1))
                    }
                };
                tokio::time::sleep(delay).await;
            }
        });
    }
    {
        let maintenance_db = state.db.clone();
        let maintenance_storage = state.storage.clone();
        let maintenance_config = config.maintenance.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(
                maintenance_config.interval_seconds.max(1),
            ));
            let mut history = services::maintenance::HistoryCursor::default();
            let mut expired_writes = services::maintenance::ExpiredWriteCursor::default();
            let mut scan = None;
            loop {
                interval.tick().await;
                if let Err(error) = services::maintenance::expire_uploads_page(
                    &maintenance_db,
                    &maintenance_config,
                    chrono::Utc::now(),
                )
                .await
                {
                    tracing::warn!(%error,"Expired upload cleanup will retry");
                }
                if maintenance_config.history_enabled
                    && let Err(error) = services::maintenance::retain_history_page(
                        &maintenance_db,
                        &mut history,
                        &maintenance_config,
                        chrono::Utc::now(),
                    )
                    .await
                {
                    tracing::warn!(%error,"History retention page will retry");
                }
                if let Err(error) = services::maintenance::expire_blob_writes_page(
                    &maintenance_db,
                    &mut expired_writes,
                    &maintenance_config,
                    chrono::Utc::now().timestamp(),
                )
                .await
                {
                    tracing::warn!(%error,"Expired object write recovery will retry");
                }
                if maintenance_config.reconcile_enabled {
                    if scan
                        .as_ref()
                        .is_none_or(services::storage::ScanCursor::finished)
                    {
                        match maintenance_storage.begin_scan().await {
                            Ok(cursor) => scan = Some(cursor),
                            Err(error) => {
                                tracing::warn!(%error,"Storage reconciliation scan will retry");
                                continue;
                            }
                        }
                    }
                    if let Some(cursor) = scan.as_mut()
                        && let Err(error) = services::maintenance::reconcile_page(
                            &maintenance_db,
                            &maintenance_storage,
                            cursor,
                            &maintenance_config,
                            chrono::Utc::now().timestamp(),
                        )
                        .await
                    {
                        tracing::warn!(%error,"Storage reconciliation page will retry");
                        scan = None;
                    }
                }
            }
        });
        let cleanup_db = state.db.clone();
        let cleanup_storage = state.storage.clone();
        let cleanup_days = config.trash.auto_delete_days;
        let auto_delete = config.trash.auto_delete_enabled;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            let mut ticks = 0_u64;
            loop {
                interval.tick().await;
                if auto_delete && ticks.is_multiple_of(60) {
                    let cutoff = (chrono::Utc::now() - chrono::Duration::days(cleanup_days as i64))
                        .to_rfc3339();
                    let mut conn = cleanup_db.clone();
                    use contrix_core::schema::files::dsl as f;
                    match f::files
                        .filter(f::is_trashed.eq(true))
                        .filter(f::trashed_at.lt(&cutoff))
                        .select(f::id)
                        .load::<String>(&mut conn)
                        .await
                    {
                        Ok(ids) => {
                            for id in ids {
                                match contrix_server::services::persistence::purge_trashed(
                                    &cleanup_db,
                                    &id,
                                    None,
                                    Some(&cutoff),
                                )
                                .await
                                {
                                    Ok(count) => tracing::info!(count, "Expired trash removed"),
                                    Err(contrix_core::error::AppError::NotFound(_)) => {}
                                    Err(error) => {
                                        tracing::warn!(%error, file_id = %id, "Trash purge will retry")
                                    }
                                }
                            }
                        }
                        Err(error) => tracing::error!(%error, "Could not enumerate expired trash"),
                    }
                }
                ticks = ticks.wrapping_add(1);
                contrix_server::services::persistence::request_gc(&cleanup_db, &cleanup_storage);
            }
        });
    }

    let router = routes::create_router(state);

    // -----------------------------------------------------------------------
    // 9. Start the server
    // -----------------------------------------------------------------------
    let bind_addr = format!("{}:{}", config.server.host, config.server.port);
    tracing::info!("Listening on {bind_addr}");

    let acceptor = TcpListener::new(bind_addr).bind().await;
    Server::new(acceptor).serve(router).await;

    Ok(())
}
