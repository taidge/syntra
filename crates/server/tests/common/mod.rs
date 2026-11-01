use std::sync::Arc;

use horme::prelude::*;

use contrix_collab::CollabManager;
use contrix_core::config::{
    AiConfig, AppConfig, AuthConfig, DatabaseConfig, RateLimitConfig, ServerConfig, StorageConfig,
    TrashConfig,
};
use contrix_server::routes::create_router;
use contrix_server::services::{AiService, NotificationBus, StorageBackend, WopiSessionStore};
use contrix_server::state::{AppState, DbPool};
use salvo::conn::Acceptor;
use salvo::prelude::*;

/// A self-contained test server backed by a temporary SQLite database.
///
/// Each instance has its own database, storage directory, and HTTP listener so
/// tests remain fully isolated from one another.
pub struct TestServer {
    pub base_url: String,
    pub client: reqwest::Client,
    pub db: DbPool,
    pub notifications: Arc<NotificationBus>,
    // Hold the tempdir so it is not deleted until the TestServer is dropped.
    pub _temp_dir: tempfile::TempDir,
}

impl TestServer {
    /// Spin up a fresh test server on a random available port.
    pub async fn new() -> Self {
        Self::new_with_ai(AiConfig::default()).await
    }

    pub async fn new_with_ai(ai_config: AiConfig) -> Self {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let storage_path = temp_dir.path().join("files");

        std::fs::create_dir_all(&storage_path).unwrap();

        let db_url = db_path.to_string_lossy().to_string();
        let db = DbPool::with_timeouts(
            &db_url,
            2,
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(3),
        )
        .unwrap();

        // Run migrations by splitting the SQL file on semicolons.
        db.run(move |conn| {
            let migration_sql = std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../migrations/001_init.sql"),
            )
            .unwrap();

            for stmt in migration_sql.split(';') {
                // Strip comment-only lines from the chunk before deciding whether
                // to execute it.
                let cleaned: String = stmt
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("--"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let trimmed = cleaned.trim();
                if trimmed.is_empty() {
                    continue;
                }
                diesel::RunQueryDsl::execute(diesel::sql_query(trimmed), conn)
                    .unwrap_or_else(|e| panic!("migration failed on:\n{trimmed}\nerror: {e}"));
            }
            Ok::<_, diesel::result::Error>(())
        })
        .await
        .unwrap();

        let config = AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 0,
                cors_origins: vec![],
            },
            database: DatabaseConfig {
                url: db_url,
                max_connections: 2,
                checkout_timeout_ms: 3_000,
                operation_timeout_ms: 15_000,
                busy_timeout_ms: 3_000,
            },
            auth: AuthConfig {
                jwt_secret: "test-secret-key-for-testing-only".to_string(),
                jwt_expiry_hours: 24,
                refresh_expiry_days: 30,
            },
            storage: StorageConfig {
                base_path: storage_path.to_string_lossy().into_owned(),
                max_upload_size_mb: 10,
                default_quota_mb: 100,
                backend: None,
                s3_bucket: None,
                s3_region: None,
                s3_access_key: None,
                s3_secret_key: None,
                s3_endpoint: None,
            },
            rate_limit: RateLimitConfig::default(),
            ai: ai_config,
            trash: TrashConfig::default(),
        };

        // Run additional migrations for new tables.
        db.run(move |conn| {
            let migration_dir =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
            let additional_migrations = [
                "002_rooms_groups.sql",
                "003_tags_favorites_recent.sql",
                "004_security.sql",
                "005_webhooks.sql",
                "006_admin.sql",
                "007_templates_invitations.sql",
                "008_upload_thumbnails.sql",
                "009_wopi_locking_presence.sql",
                "010_password_resets.sql",
                "011_share_passwords.sql",
                "012_notification_preferences.sql",
                "013_file_versions.sql",
                "014_operation_log.sql",
                "015_api_keys.sql",
                "016_login_attempts.sql",
                "017_file_properties.sql",
                "018_file_ordering.sql",
                "019_read_status.sql",
                "020_room_enhancements.sql",
                "021_document_signatures.sql",
                "022_tenants.sql",
                "023_remove_oauth.sql",
                "024_remove_integration_settings.sql",
            ];
            for migration_file in &additional_migrations {
                if let Ok(sql) = std::fs::read_to_string(migration_dir.join(migration_file)) {
                    for stmt in sql.split(';') {
                        let cleaned: String = stmt
                            .lines()
                            .filter(|line| !line.trim_start().starts_with("--"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let trimmed = cleaned.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let _ = diesel::RunQueryDsl::execute(diesel::sql_query(trimmed), conn);
                    }
                }
            }
            Ok::<_, diesel::result::Error>(())
        })
        .await
        .unwrap();

        let db_clone = db.clone();
        let storage = Arc::new(StorageBackend::from_config(&config.storage).unwrap());
        let ai = config
            .ai
            .enabled
            .then(|| AiService::new(config.ai.clone()).unwrap());
        let notifications = Arc::new(NotificationBus::new());
        let state = Arc::new(AppState {
            db: db_clone,
            config,
            storage,
            collab: CollabManager::new(),
            started_at: chrono::Utc::now(),
            ai,
            thumbnail: None,
            notifications: notifications.clone(),
            wopi_sessions: Arc::new(WopiSessionStore::new()),
        });

        let router = create_router(state);

        // Bind to port 0 so the OS assigns an available port. Extract the
        // actual address from the acceptor *before* spawning the server so
        // there is no race.
        let acceptor = TcpListener::new("127.0.0.1:0").bind().await;
        let addr = acceptor.holdings()[0]
            .local_addr
            .clone()
            .into_std()
            .unwrap();
        let base_url = format!("http://{}", addr);

        // Spawn the Salvo server in the background.
        tokio::spawn(async move {
            Server::new(acceptor).serve(router).await;
        });

        // Give the server a moment to start accepting connections.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();

        TestServer {
            base_url,
            client,
            db,
            notifications,
            _temp_dir: temp_dir,
        }
    }

    /// Build a full URL from a path.
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Register a new user and return (token, response_body).
    pub async fn register(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> (String, serde_json::Value) {
        let resp = self
            .client
            .post(self.url("/api/v1/auth/register"))
            .json(&serde_json::json!({
                "username": username,
                "email": email,
                "password": password
            }))
            .send()
            .await
            .unwrap();

        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(status, 201, "register failed with status {status}: {body}");
        let token = body["data"]["token"].as_str().unwrap().to_string();
        (token, body)
    }

    /// Log in an existing user and return (token, response_body).
    pub async fn login(&self, email: &str, password: &str) -> (String, serde_json::Value) {
        let resp = self
            .client
            .post(self.url("/api/v1/auth/login"))
            .json(&serde_json::json!({
                "email": email,
                "password": password
            }))
            .send()
            .await
            .unwrap();

        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(status, 200, "login failed with status {status}: {body}");
        let token = body["data"]["token"].as_str().unwrap().to_string();
        (token, body)
    }

    /// Build an `Authorization: Bearer <token>` header map.
    pub fn auth_header(&self, token: &str) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {}", token).parse().unwrap(),
        );
        headers
    }

    /// Helper: create a document and return (doc_id, file_id, response_body).
    pub async fn create_document(
        &self,
        token: &str,
        name: &str,
        document_type: &str,
        parent_id: Option<&str>,
    ) -> (String, String, serde_json::Value) {
        let mut payload = serde_json::json!({
            "name": name,
            "document_type": document_type,
        });
        if let Some(pid) = parent_id {
            payload["parent_id"] = serde_json::Value::String(pid.to_string());
        }

        let resp = self
            .client
            .post(self.url("/api/v1/documents"))
            .headers(self.auth_header(token))
            .json(&payload)
            .send()
            .await
            .unwrap();

        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            status, 201,
            "create_document failed with status {status}: {body}"
        );
        let doc_id = body["data"]["id"].as_str().unwrap().to_string();
        let file_id = body["data"]["file_id"].as_str().unwrap().to_string();
        (doc_id, file_id, body)
    }

    /// Helper: create a folder and return (folder_id, response_body).
    pub async fn create_folder(
        &self,
        token: &str,
        name: &str,
        parent_id: Option<&str>,
    ) -> (String, serde_json::Value) {
        let mut payload = serde_json::json!({ "name": name });
        if let Some(pid) = parent_id {
            payload["parent_id"] = serde_json::Value::String(pid.to_string());
        }

        let resp = self
            .client
            .post(self.url("/api/v1/folders"))
            .headers(self.auth_header(token))
            .json(&payload)
            .send()
            .await
            .unwrap();

        let status = resp.status();
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(
            status, 201,
            "create_folder failed with status {status}: {body}"
        );
        let folder_id = body["data"]["id"].as_str().unwrap().to_string();
        (folder_id, body)
    }

    /// Promote a user to admin role directly via SQL.
    pub async fn make_admin(&self, user_id_val: &str) {
        let mut conn = self.db.clone();
        use contrix_core::schema::users::dsl::*;
        diesel::update(users.filter(id.eq(user_id_val)))
            .set(role.eq("admin"))
            .execute(&mut conn)
            .await
            .unwrap();
    }

    /// Insert a notification directly via SQL (useful for testing read/delete).
    pub async fn insert_notification(
        &self,
        user_id_val: &str,
        title_val: &str,
        message_val: &str,
    ) -> String {
        let notification_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let mut conn = self.db.clone();

        use contrix_core::models::NewNotification;
        use contrix_core::schema::notifications::dsl::*;

        let new_notif = NewNotification {
            id: notification_id.clone(),
            user_id: user_id_val.to_string(),
            notification_type: "system".to_string(),
            title: title_val.to_string(),
            message: Some(message_val.to_string()),
            link: None,
            is_read: false,
            created_at: now,
        };

        diesel::insert_into(notifications)
            .values(&new_notif)
            .execute(&mut conn)
            .await
            .unwrap();

        notification_id
    }
}
