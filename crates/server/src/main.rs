use std::sync::Arc;

use diesel::SqliteConnection;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool};
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

/// Convert a `sqlite://path/to/db?mode=rwc` URL to a plain file path
/// that Diesel's SQLite backend expects.
fn sqlite_url_to_path(url: &str) -> String {
    let path = url.strip_prefix("sqlite://").unwrap_or(url);
    path.split('?').next().unwrap_or(path).to_string()
}

/// Run a single migration SQL file, splitting by semicolons.
fn run_migration_file(conn: &mut SqliteConnection, sql: &str) {
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
        diesel::sql_query(trimmed)
            .execute(conn)
            .unwrap_or_else(|e| {
                tracing::warn!(
                    "Migration statement failed: {e}\n  SQL: {}",
                    &trimmed[..trimmed.len().min(80)]
                );
                0
            });
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // -----------------------------------------------------------------------
    // 1. Load configuration
    // -----------------------------------------------------------------------
    let config: AppConfig = Figment::new()
        .merge(Toml::file("config/default.toml"))
        .merge(Env::prefixed("CONTRIX_").split("_"))
        .extract()?;

    // -----------------------------------------------------------------------
    // 2. Set up tracing
    // -----------------------------------------------------------------------
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,contrix_server=debug,salvo=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    tracing::info!("Starting Contrix Office server");

    // -----------------------------------------------------------------------
    // 3. Ensure data directories exist before connecting
    // -----------------------------------------------------------------------
    let db_path = sqlite_url_to_path(&config.database.url);
    if let Some(parent) = std::path::Path::new(&db_path).parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    // -----------------------------------------------------------------------
    // 4. Create Diesel r2d2 connection pool
    // -----------------------------------------------------------------------
    let manager = ConnectionManager::<SqliteConnection>::new(&db_path);
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
        match tokio::fs::read_to_string(migration_path).await {
            Ok(sql) => {
                run_migration_file(&mut conn, &sql);
                tracing::info!("Migration applied: {migration_path}");
            }
            Err(e) => {
                tracing::warn!("Could not read {migration_path}: {e}. Skipping.");
            }
        }
    }
    drop(conn);
    tracing::info!("All migrations applied successfully");

    // -----------------------------------------------------------------------
    // 6. Ensure storage directories exist
    // -----------------------------------------------------------------------
    let storage_path = &config.storage.base_path;
    tokio::fs::create_dir_all(storage_path).await?;
    tracing::info!("Storage directory ready: {storage_path}");

    let search_path = &config.search.index_path;
    tokio::fs::create_dir_all(search_path).await?;

    // Ensure backup directory exists
    tokio::fs::create_dir_all("data/backups").await?;

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
    let thumbnail_service = Some(services::ThumbnailService::new(storage_path));
    tracing::info!("Thumbnail service initialized");

    // -----------------------------------------------------------------------
    // 8. Build application state and router
    // -----------------------------------------------------------------------
    let state = Arc::new(AppState {
        db,
        config: config.clone(),
        collab,
        started_at: chrono::Utc::now(),
        search: search_service,
        thumbnail: thumbnail_service,
        notifications: Arc::new(services::NotificationBus::new()),
    });

    // -----------------------------------------------------------------------
    // 8b. Spawn trash auto-cleanup background task
    // -----------------------------------------------------------------------
    if config.trash.auto_delete_enabled {
        let cleanup_db = state.db.clone();
        let cleanup_days = config.trash.auto_delete_days;
        let cleanup_base_path = config.storage.base_path.clone();
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
                            let full_path =
                                std::path::Path::new(&cleanup_base_path).join(&file.path);
                            let _ = std::fs::remove_file(&full_path);
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
