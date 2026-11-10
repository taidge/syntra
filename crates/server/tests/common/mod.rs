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
    #[allow(dead_code)] // Isolated handler backpressure tests reuse the real authenticated state.
    pub state: Arc<AppState>,
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
        Self::new_configured(ai_config, 30, 900).await
    }

    #[allow(dead_code)] // Only transport-specific integration binaries use this constructor.
    pub async fn new_with_body_timeouts(idle_secs: u64, total_secs: u64) -> Self {
        Self::new_configured(AiConfig::default(), idle_secs, total_secs).await
    }

    async fn new_configured(ai_config: AiConfig, idle_secs: u64, total_secs: u64) -> Self {
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

        // Use the same SQLite parser, atomic runner and catalog as production.
        db.run(move |conn| {
            let directory =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
            contrix_server::migrations::run_directory(conn, &directory)
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
                upload_idle_timeout_seconds: idle_secs,
                upload_total_timeout_seconds: total_secs,
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
            maintenance: Default::default(),
        };

        let db_clone = db.clone();
        let storage = Arc::new(StorageBackend::from_config(&config.storage).unwrap());
        storage.attach_database(db.clone()).unwrap();
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

        let router = create_router(state.clone());

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

        // This client only talks to this test's loopback listener. Inheriting
        // system/environment proxies lets unrelated network software replace
        // application responses (including large-upload errors) with a 502.
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("build isolated loopback test client");

        TestServer {
            base_url,
            client,
            db,
            state,
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

/// Own all real connections and queue positions after prior SQL/lease cleanup
/// is genuinely idle. Pending admissions are polled inline in one executor
/// turn, so a WS timer cannot take a slot between spawning queued tasks.
#[allow(dead_code)]
pub struct OccupiedPool {
    db: DbPool,
    leases: Vec<contrix_server::database::DatabaseLease>,
    pending: Vec<
        futures_util::future::BoxFuture<
            'static,
            std::result::Result<(), contrix_core::error::AppError>,
        >,
    >,
}
#[allow(dead_code)]
impl OccupiedPool {
    pub async fn hold(server: &TestServer) -> Self {
        use horme::GetConnection;
        use std::time::Duration;
        tokio::time::timeout(Duration::from_secs(3), async {
            let mut quiet = None;
            let mut checkouts = server.db.state().checkouts;
            loop {
                let state = server.db.state();
                if state.admitted_operations == 0
                    && state.connections == state.idle_connections
                    && state.checkouts == checkouts
                {
                    let since = quiet.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= Duration::from_millis(20) {
                        break;
                    }
                } else {
                    quiet = None;
                    checkouts = state.checkouts;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("prior queries, initial WS timer, and connection cleanup retire before saturation");
        let mut db = server.db.clone();
        let leases = vec![
            db.get_mut_conn().await.unwrap(),
            db.get_mut_conn().await.unwrap(),
        ];
        assert_eq!(
            server.db.state().admitted_operations,
            2,
            "both real connections are ours before queueing"
        );
        let mut pending: Vec<
            futures_util::future::BoxFuture<
                'static,
                std::result::Result<(), contrix_core::error::AppError>,
            >,
        > = Vec::new();
        for _ in 0..2 {
            let db = server.db.clone();
            let mut request =
                Box::pin(
                    async move { db.run(|_| Ok::<_, contrix_core::error::AppError>(())).await },
                );
            assert!(
                futures_util::poll!(request.as_mut()).is_pending(),
                "each queued request is actually admitted and waiting for our held connections"
            );
            pending.push(request);
        }
        assert_eq!(
            server.db.state().admitted_operations,
            4,
            "all four slots belong to this fixture"
        );
        Self {
            db: server.db.clone(),
            leases,
            pending,
        }
    }
    pub async fn release(self) {
        drop(self.leases);
        for pending in self.pending {
            pending.await.unwrap();
        }
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let state = self.db.state();
                if state.admitted_operations == 0 && state.connections == state.idle_connections {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("every owned queued query and lease cleanup retires after release");
    }
}
