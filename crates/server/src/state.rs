use std::sync::Arc;

use diesel::SqliteConnection;
use diesel::r2d2::{ConnectionManager, Pool};

use contrix_collab::CollabManager;
use contrix_core::config::AppConfig;

use crate::services::NotificationBus;
use crate::services::SearchService;
use crate::services::StorageBackend;
use crate::services::ThumbnailService;
use crate::services::WopiSessionStore;

pub type DbPool = Pool<ConnectionManager<SqliteConnection>>;

/// Shared application state injected into Salvo's [`Depot`] for every request.
pub struct AppState {
    pub db: DbPool,
    pub config: AppConfig,
    pub storage: Arc<StorageBackend>,
    pub collab: CollabManager,
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Tantivy-backed full-text search index (None if initialization failed).
    pub search: Option<SearchService>,
    /// Image thumbnail generator.
    pub thumbnail: Option<ThumbnailService>,
    pub notifications: Arc<NotificationBus>,
    pub wopi_sessions: Arc<WopiSessionStore>,
}
