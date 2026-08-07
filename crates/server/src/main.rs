use std::sync::Arc;

use anyhow::{Context, bail};
use diesel::SqliteConnection;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool};
use diesel::result::Error as DieselError;
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use salvo::prelude::*;
use tracing_subscriber::EnvFilter;

use contrix_collab::CollabManager;
use contrix_core::config::AppConfig;

mod handlers;
mod middleware;
mod routes;
mod services;
mod state;

use state::AppState;

const DEFAULT_JWT_SECRET: &str = "change-me-in-production-use-a-long-random-string";

/// Run a single migration SQL file, splitting by semicolons.
fn run_migration_file(
    conn: &mut SqliteConnection,
    migration_path: &str,
    sql: &str,
) -> anyhow::Result<()> {
    for statement in sql.split(';') {
        let cleaned: String = statement
            .lines()
            .filter(|line| !line.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        let trimmed = cleaned.trim();
        if trimmed.is_empty() {
            continue;
        }
        match diesel::sql_query(trimmed).execute(conn) {
            Ok(_) => {}
            Err(e) if is_idempotent_migration_error(&e, trimmed) => {
                tracing::debug!("Ignoring idempotent migration statement in {migration_path}: {e}");
            }
            Err(e) => {
                bail!(
                    "Migration {migration_path} failed: {e}\nSQL: {}",
                    &trimmed[..trimmed.len().min(240)]
                );
            }
        }
    }

    Ok(())
}

fn is_idempotent_migration_error(err: &DieselError, statement: &str) -> bool {
    let lower_statement = statement.trim_start().to_ascii_lowercase();
    let lower_error = err.to_string().to_ascii_lowercase();

    lower_statement.starts_with("alter table")
        && lower_statement.contains(" add column ")
        && lower_error.contains("duplicate column")
}

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
    // 4. Create Diesel r2d2 connection pool
    // -----------------------------------------------------------------------
    let manager = ConnectionManager::<SqliteConnection>::new(db_path.to_string_lossy());
    let db = Pool::builder()
        .max_size(config.database.max_connections)
        .build(manager)?;

    tracing::info!("Database pool created");

    // -----------------------------------------------------------------------
    // 5. Run all migrations (glob migration files in order)
    // -----------------------------------------------------------------------
    let migration_files = [
        "migrations/001_init.sql",
        "migrations/002_rooms_groups.sql",
        "migrations/003_tags_favorites_recent.sql",
        "migrations/004_security.sql",
        "migrations/005_webhooks.sql",
        "migrations/006_admin.sql",
        "migrations/007_templates_invitations.sql",
        "migrations/008_upload_thumbnails.sql",
        "migrations/009_wopi_locking_presence.sql",
        "migrations/010_password_resets.sql",
        "migrations/011_share_passwords.sql",
        "migrations/012_notification_preferences.sql",
        "migrations/013_file_versions.sql",
        "migrations/014_operation_log.sql",
        "migrations/015_api_keys.sql",
        "migrations/016_login_attempts.sql",
        "migrations/017_file_properties.sql",
        "migrations/018_file_ordering.sql",
        "migrations/019_read_status.sql",
        "migrations/020_room_enhancements.sql",
        "migrations/021_document_signatures.sql",
        "migrations/022_tenants.sql",
    ];

    let mut conn = db.get()?;
    for migration_path in &migration_files {
        let sql = tokio::fs::read_to_string(migration_path)
            .await
            .with_context(|| format!("Could not read required migration file {migration_path}"))?;
        run_migration_file(&mut conn, migration_path, &sql)?;
        tracing::info!("Migration applied: {migration_path}");
    }
    drop(conn);
    tracing::info!("All migrations applied successfully");

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

    let search_path = &config.search.index_path;
    tokio::fs::create_dir_all(search_path).await?;

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

    // -----------------------------------------------------------------------
    // 7b. Initialize Tantivy search index
    // -----------------------------------------------------------------------
    let search_service = match services::SearchService::new(search_path) {
        Ok(svc) => {
            tracing::info!("Tantivy search index initialized at {search_path}");
            Some(svc)
        }
        Err(e) => {
            tracing::warn!("Failed to initialize search index: {e}. Falling back to SQL search.");
            None
        }
    };

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
        search: search_service,
        thumbnail: thumbnail_service,
        notifications: Arc::new(services::NotificationBus::new()),
        wopi_sessions: Arc::new(services::WopiSessionStore::new()),
    });

    // -----------------------------------------------------------------------
    // 8b. Spawn trash auto-cleanup background task
    // -----------------------------------------------------------------------
    if config.trash.auto_delete_enabled {
        let cleanup_db = state.db.clone();
        let cleanup_storage = state.storage.clone();
        let cleanup_days = config.trash.auto_delete_days;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                interval.tick().await;
                if let Ok(mut conn) = cleanup_db.get() {
                    let cutoff = (chrono::Utc::now() - chrono::Duration::days(cleanup_days as i64))
                        .to_rfc3339();
                    use contrix_core::schema::files::dsl as f;
                    let old_files: Vec<contrix_core::models::File> = f::files
                        .filter(f::is_trashed.eq(true))
                        .filter(f::trashed_at.lt(&cutoff))
                        .load(&mut conn)
                        .unwrap_or_default();
                    for file in &old_files {
                        if !file.is_folder {
                            let _ = cleanup_storage.delete_file(&file.path).await;
                        }
                    }
                    let _ = diesel::delete(
                        f::files
                            .filter(f::is_trashed.eq(true))
                            .filter(f::trashed_at.lt(&cutoff)),
                    )
                    .execute(&mut conn);
                    if !old_files.is_empty() {
                        tracing::info!("Trash cleanup: removed {} expired items", old_files.len());
                    }
                }
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
