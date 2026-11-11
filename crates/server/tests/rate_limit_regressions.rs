use std::{sync::Arc, time::Duration};

use contrix_collab::CollabManager;
use contrix_core::config::AppConfig;
use contrix_server::routes::create_router;
use contrix_server::services::{NotificationBus, StorageBackend, WopiSessionStore};
use contrix_server::state::{AppState, DbPool};
use diesel::connection::SimpleConnection;
use figment::providers::{Format, Toml};
use reqwest::{Response, StatusCode};
use salvo::conn::Acceptor;
use salvo::prelude::*;
use serde_json::{Value, json};

const LIMIT: u32 = 3;

/// Bind before spawning so the first real HTTP request queues on the listener.
/// No readiness request, startup sleep or retry consumes a limiter bucket.
struct LimitedServer {
    client: reqwest::Client,
    base_url: String,
    task: Option<tokio::task::JoinHandle<()>>,
    handle: salvo::server::ServerHandle,
    db: Option<DbPool>,
    directory: Option<tempfile::TempDir>,
    operation_timeout: Duration,
}

impl LimitedServer {
    async fn new(trust_forwarded_headers: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut config: AppConfig = figment::Figment::new()
            .merge(Toml::string(include_str!("../../../config/default.toml")))
            .extract()
            .unwrap();
        assert!(config.rate_limit.enabled);
        assert!(!config.rate_limit.trust_forwarded_headers);
        config.rate_limit.max_requests = LIMIT;
        // Expiry is outside these request-count tests, even on a slow runner.
        config.rate_limit.window_seconds = 3600;
        config.rate_limit.trust_forwarded_headers = trust_forwarded_headers;
        config.database.url = directory
            .path()
            .join("office.sqlite")
            .to_string_lossy()
            .into();
        config.database.max_connections = 2;
        config.storage.base_path = directory.path().join("files").to_string_lossy().into();
        config.auth.jwt_secret = "rate-limit-regression-secret-for-isolated-tests".into();
        std::fs::create_dir(&config.storage.base_path).unwrap();
        let operation_timeout = Duration::from_millis(config.database.operation_timeout_ms);
        let db = DbPool::connect(&config.database).unwrap();
        db.run_startup_migrations(|conn| {
            contrix_server::migrations::run_directory(
                conn,
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations"),
            )
        })
        .await
        .unwrap();
        let storage = Arc::new(StorageBackend::from_config(&config.storage).unwrap());
        storage.attach_database(db.clone()).unwrap();
        let state = Arc::new(AppState {
            db: db.clone(),
            config,
            storage,
            collab: CollabManager::new(),
            started_at: chrono::Utc::now(),
            ai: None,
            thumbnail: None,
            notifications: Arc::new(NotificationBus::new()),
            wopi_sessions: Arc::new(WopiSessionStore::new()),
        });
        let acceptor = TcpListener::new("127.0.0.1:0").bind().await;
        let address = acceptor.holdings()[0]
            .local_addr
            .clone()
            .into_std()
            .unwrap();
        let router = create_router(state);
        let server = Server::new(acceptor);
        let handle = server.handle();
        let task = tokio::spawn(async move {
            server.serve(router).await;
        });
        Self {
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            base_url: format!("http://{address}"),
            task: Some(task),
            handle,
            db: Some(db),
            directory: Some(directory),
            operation_timeout,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn get(&self, path: &str) -> Response {
        self.client.get(self.url(path)).send().await.unwrap()
    }

    async fn shutdown(mut self) {
        self.handle.stop_graceful(None);
        // Keep ownership until the await completes so timeout/panic still
        // reaches Drop's abort fallback.
        tokio::time::timeout(self.operation_timeout, self.task.as_mut().unwrap())
            .await
            .expect("HTTP connections must finish graceful shutdown")
            .expect("owned HTTP server task must exit normally");
        drop(self.task.take());
        let db = self.db.take().unwrap();
        // A request can return before DatabaseLease's blocking cleanup retires.
        // Observe its existing admission/pool state without issuing more SQL.
        tokio::time::timeout(self.operation_timeout, async {
            loop {
                let status = db.state();
                if status.admitted_operations == 0 && status.connections == status.idle_connections
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("database lease cleanup must retire before deleting its files");
        drop(db);
        self.directory
            .take()
            .unwrap()
            .close()
            .expect("test database and storage files must be released");
    }
}

impl Drop for LimitedServer {
    fn drop(&mut self) {
        // Panic/timeout fallback only. Passing tests use shutdown and verify
        // that the server, database cleanup and directory removal completed.
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[tokio::test]
async fn api_page_and_asset_edge_budgets_are_independent() {
    let server = LimitedServer::new(false).await;
    let absent_asset = format!("/assets/{}.js", uuid::Uuid::new_v4());
    for (path, expected) in [
        ("/api/v1/capabilities", StatusCode::OK),
        ("/robots.txt", StatusCode::OK),
        (absent_asset.as_str(), StatusCode::NOT_FOUND),
    ] {
        for _ in 0..LIMIT {
            assert_eq!(
                server.get(path).await.status(),
                expected,
                "{path} must retain its budget after other edge classes are exhausted"
            );
        }
        assert_eq!(
            server.get(path).await.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "{path} must enforce its own configured request limit"
        );
    }
    server.shutdown().await;
}

#[tokio::test]
async fn asset_page_and_api_edge_budgets_are_independent() {
    let server = LimitedServer::new(false).await;
    let absent_asset = format!("/assets/{}.js", uuid::Uuid::new_v4());
    for (path, expected) in [
        (absent_asset.as_str(), StatusCode::NOT_FOUND),
        ("/robots.txt", StatusCode::OK),
        ("/api/v1/capabilities", StatusCode::OK),
    ] {
        for _ in 0..LIMIT {
            assert_eq!(
                server.get(path).await.status(),
                expected,
                "{path} must retain its budget after other edge classes are exhausted"
            );
        }
        assert_eq!(
            server.get(path).await.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "{path} must enforce its own configured request limit"
        );
    }
    server.shutdown().await;
}

#[tokio::test]
async fn root_robots_and_unknown_spa_routes_share_one_page_budget() {
    let server = LimitedServer::new(false).await;
    // This router uses the caller's normal static root. Root and SPA HTML
    // may be absent. Their requests still share the page budget with robots.
    assert!(matches!(
        server.get("/").await.status(),
        StatusCode::OK | StatusCode::NOT_FOUND
    ));
    assert_eq!(server.get("/robots.txt").await.status(), StatusCode::OK);
    let unknown_spa = format!("/rate-limit-spa-{}", uuid::Uuid::new_v4());
    assert!(matches!(
        server.get(&unknown_spa).await.status(),
        StatusCode::OK | StatusCode::NOT_FOUND
    ));
    assert_eq!(
        server.get("/robots.txt").await.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the SPA catch-all must share the root and robots page bucket"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn authenticated_user_limit_survives_requests_from_different_trusted_proxy_ips() {
    let server = LimitedServer::new(true).await;
    let registered = server
        .client
        .post(server.url("/api/v1/auth/register"))
        .header("x-forwarded-for", "192.0.2.100")
        .json(&json!({
            "username": "limited",
            "email": "limited@test.invalid",
            "password": "Password123!"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), StatusCode::CREATED);
    let registered: Value = registered.json().await.unwrap();
    let token = registered["data"]["token"].as_str().unwrap();
    for index in 1..=LIMIT {
        let response = server
            .client
            .get(server.url("/api/v1/auth/me"))
            .header("x-forwarded-for", format!("198.51.100.{index}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let rejected = server
        .client
        .get(server.url("/api/v1/auth/me"))
        .header("x-forwarded-for", "198.51.100.4")
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::TOO_MANY_REQUESTS);
    let public = server
        .client
        .get(server.url("/api/v1/health"))
        .header("x-forwarded-for", "198.51.100.4")
        .send()
        .await
        .unwrap();
    assert_eq!(
        public.status(),
        StatusCode::OK,
        "the fresh IP's API edge budget is available, so the JWT user bucket caused 429"
    );
    let invalid = server
        .client
        .get(server.url("/api/v1/auth/me"))
        .header("x-forwarded-for", "198.51.100.5")
        .bearer_auth("invalid.jwt.token")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
    drop(invalid);
    drop(public);
    drop(rejected);
    server.shutdown().await;
}

#[tokio::test]
async fn default_untrusted_forwarded_headers_cannot_rotate_api_edge_buckets() {
    let server = LimitedServer::new(false).await;
    for index in 1..=LIMIT + 1 {
        let response = server
            .client
            .get(server.url("/api/v1/health"))
            .header("x-forwarded-for", format!("198.51.100.{index}"))
            .header("x-real-ip", format!("203.0.113.{index}"))
            .send()
            .await
            .unwrap();
        let expected = if index <= LIMIT {
            StatusCode::OK
        } else {
            StatusCode::TOO_MANY_REQUESTS
        };
        assert_eq!(
            response.status(),
            expected,
            "untrusted headers must not replace the real loopback client IP"
        );
    }
    server.shutdown().await;
}

#[tokio::test]
async fn exhausted_edge_budgets_reject_before_ip_filter_database_checkout() {
    let server = LimitedServer::new(false).await;
    let db = server.db.as_ref().unwrap();
    let absent_asset = format!("/assets/{}.js", uuid::Uuid::new_v4());
    // These handlers do not use the database. Only the IP filter may check out
    // a connection on the requests that are still within their edge budget.
    for (path, expected) in [
        ("/robots.txt", StatusCode::OK),
        ("/api/v1/capabilities", StatusCode::OK),
        (absent_asset.as_str(), StatusCode::NOT_FOUND),
    ] {
        let before_allowed = db.state().checkouts;
        for _ in 0..LIMIT {
            assert_eq!(server.get(path).await.status(), expected);
        }
        let before_rejected = db.state().checkouts;
        assert_eq!(
            before_rejected - before_allowed,
            u64::from(LIMIT),
            "{path} must pass through exactly one IP rule lookup per allowed request"
        );
        assert_eq!(
            server.get(path).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            db.state().checkouts,
            before_rejected,
            "{path} must reject excess requests before touching the database"
        );
    }
    server.shutdown().await;
}

#[tokio::test]
async fn api_pages_and_assets_enforce_ip_denial_within_their_budgets() {
    let server = LimitedServer::new(false).await;
    let db = server.db.as_ref().unwrap();
    db.run(|conn| {
        conn.batch_execute(
            "INSERT INTO users(id,username,email,password_hash)
             VALUES ('limiter-admin','limiter-admin','limiter-admin@test.invalid','unused');
             INSERT INTO ip_restrictions(id,ip_range,restriction_type,created_by,created_at)
             VALUES ('deny-loopback','127.0.0.1','deny','limiter-admin','2000-01-01T00:00:00Z');",
        )
    })
    .await
    .unwrap();
    let absent_asset = format!("/assets/{}.js", uuid::Uuid::new_v4());
    for path in ["/api/v1/capabilities", "/robots.txt", absent_asset.as_str()] {
        for _ in 0..LIMIT {
            assert_eq!(
                server.get(path).await.status(),
                StatusCode::FORBIDDEN,
                "{path} must retain IP filtering within its edge budget"
            );
        }
        let before_rejected = db.state().checkouts;
        assert_eq!(
            server.get(path).await.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "edge rejection takes precedence after {path} exhausts its budget"
        );
        assert_eq!(db.state().checkouts, before_rejected);
    }
    server.shutdown().await;
}

#[tokio::test]
async fn api_pages_and_assets_fail_closed_when_ip_rules_cannot_be_read() {
    let server = LimitedServer::new(false).await;
    let db = server.db.as_ref().unwrap();
    db.run(|conn| conn.batch_execute("DROP TABLE ip_restrictions"))
        .await
        .unwrap();
    let absent_asset = format!("/assets/{}.js", uuid::Uuid::new_v4());
    for path in ["/api/v1/capabilities", "/robots.txt", absent_asset.as_str()] {
        for _ in 0..LIMIT {
            assert_eq!(
                server.get(path).await.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{path} must fail closed while its IP lookup is unavailable"
            );
        }
        let before_rejected = db.state().checkouts;
        assert_eq!(
            server.get(path).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            db.state().checkouts,
            before_rejected,
            "excess {path} requests must not amplify an existing database failure"
        );
    }
    server.shutdown().await;
}
