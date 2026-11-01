use std::sync::Arc;

use contrix_collab::CollabManager;
use contrix_core::config::AppConfig;

use crate::services::AiService;
use crate::services::NotificationBus;
use crate::services::StorageBackend;
use crate::services::ThumbnailService;
use crate::services::WopiSessionStore;

pub type DbPool = crate::database::Database;

/// Shared application state injected into Salvo's [`Depot`] for every request.
pub struct AppState {
    pub db: DbPool,
    pub config: AppConfig,
    pub storage: Arc<StorageBackend>,
    pub collab: CollabManager,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub ai: Option<AiService>,
    /// Image thumbnail generator.
    pub thumbnail: Option<ThumbnailService>,
    pub notifications: Arc<NotificationBus>,
    pub wopi_sessions: Arc<WopiSessionStore>,
}
