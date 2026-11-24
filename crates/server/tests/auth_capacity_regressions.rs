#[allow(dead_code)]
mod common;

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use common::{OccupiedPool, TestServer};
use diesel::RunQueryDsl;
use diesel::connection::SimpleConnection;
use serde_json::{Value, json};

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl LogCapture {
    fn subscriber(&self) -> impl tracing::Subscriber + Send + Sync + 'static {
        let writer = self.clone();
        tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_env_filter("off,syntra_server::middleware::request_observability=trace")
            .with_writer(move || writer.clone())
            .finish()
    }

    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

async fn api_key(server: &TestServer, jwt: &str) -> String {
    let response = server
        .client
        .post(server.url("/api/v1/api-keys"))
        .bearer_auth(jwt)
        .json(&json!({"name":"Document reader", "permissions":"read"}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 201, "{body}");
    body["data"]["key"].as_str().unwrap().to_owned()
}

fn request_id(response: &reqwest::Response) -> String {
    let id = response.headers()["x-request-id"].to_str().unwrap();
    uuid::Uuid::parse_str(id).expect("server request ID must be a UUID");
    id.to_owned()
}

#[tokio::test(flavor = "current_thread")]
async fn owned_document_reads_use_five_leases_and_report_exact_sql_counts() {
    let logs = LogCapture::default();
    let _subscriber = tracing::subscriber::set_default(logs.subscriber());
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("doc_read", "doc_read@test.invalid", "Password123!")
        .await;
    let key = api_key(&server, &jwt).await;
    let (document, _, _) = server.create_document(&jwt, "Owned", "writer", None).await;
    let url = server.url(&format!("/api/v1/documents/{document}"));

    // Warm the API key's first-use display timestamp, then measure the
    // repeated read path where both credentials' activity is recent.
    assert_eq!(
        server
            .client
            .get(&url)
            .bearer_auth(&key)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for token in [&jwt, &key] {
        let before = server.db.state().checkouts;
        let response = server
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        let id = request_id(&response);
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.json::<Value>().await.unwrap()["data"]["name"],
            "Owned"
        );
        assert_eq!(server.db.state().checkouts - before, 5);
        let output = logs.text();
        let line = output.lines().find(|line| line.contains(&id)).unwrap();
        assert!(line.contains("checkout_attempts=5"), "{line}");
        assert!(line.contains("checkouts_acquired=5"), "{line}");
        assert!(line.contains("statements_started=5"), "{line}");
        assert!(line.contains("statements_completed=5"), "{line}");
        assert!(line.contains("checkout_wait_us="), "{line}");
        assert!(line.contains("sql_run_us="), "{line}");
    }
}

#[derive(diesel::QueryableByName)]
struct ActivityCounts {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    session_writes: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    api_key_writes: i64,
}

#[tokio::test]
async fn activity_refreshes_are_throttled_without_caching_tenant_or_credential_status() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("fresh_auth", "fresh_auth@test.invalid", "Password123!")
        .await;
    let key = api_key(&server, &jwt).await;
    server
        .db
        .run(|conn| {
            conn.batch_execute(
                "UPDATE sessions SET last_activity_at = '2000-01-01T00:00:00+00:00';
                 CREATE TABLE activity_write_audit (kind TEXT NOT NULL);
                 CREATE TRIGGER session_activity_audit AFTER UPDATE OF last_activity_at ON sessions
                 BEGIN INSERT INTO activity_write_audit VALUES ('session'); END;
                 CREATE TRIGGER key_activity_audit AFTER UPDATE OF last_used_at ON api_keys
                 BEGIN INSERT INTO activity_write_audit VALUES ('key'); END;
                 INSERT INTO tenants (id,name,is_active,created_at,updated_at)
                 VALUES ('auth-tenant','Auth tenant',1,'2026-01-01','2026-01-01');
                 UPDATE users SET tenant_id = 'auth-tenant';",
            )
        })
        .await
        .unwrap();

    let request = |token: &str| {
        server
            .client
            .get(server.url("/api/v1/auth/me"))
            .bearer_auth(token)
    };
    for token in [&jwt, &key] {
        // A concurrent first-use burst may sample the same stale timestamp.
        // The write predicate still allows only one stored timestamp update.
        let (first, second) = tokio::join!(request(token).send(), request(token).send());
        assert_eq!(first.unwrap().status(), 200);
        assert_eq!(second.unwrap().status(), 200);
        assert_eq!(request(token).send().await.unwrap().status(), 200);
    }
    let activity = server
        .db
        .run(|conn| {
            diesel::sql_query(
                "SELECT (SELECT count(*) FROM activity_write_audit WHERE kind='session') AS session_writes,
                        (SELECT count(*) FROM activity_write_audit WHERE kind='key') AS api_key_writes",
            )
            .get_result::<ActivityCounts>(conn)
        })
        .await
        .unwrap();
    assert_eq!(activity.session_writes, 1);
    assert_eq!(activity.api_key_writes, 1);

    server
        .db
        .run(|conn| conn.batch_execute("UPDATE tenants SET is_active = 0"))
        .await
        .unwrap();
    for token in [&jwt, &key] {
        let response = request(token).send().await.unwrap();
        assert_eq!(response.status(), 403);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"]["message"],
            "Permission denied: Tenant is inactive or missing"
        );
    }
    server
        .db
        .run(|conn| {
            conn.batch_execute(
                "UPDATE tenants SET is_active = 1;
                 UPDATE api_keys SET permissions = 'write';
                 UPDATE sessions SET is_active = 0;",
            )
        })
        .await
        .unwrap();
    assert_eq!(request(&jwt).send().await.unwrap().status(), 401);
    let downgraded = request(&key).send().await.unwrap();
    assert_eq!(downgraded.status(), 403);
    assert_eq!(
        downgraded.json::<Value>().await.unwrap()["error"]["message"],
        "API key does not permit this operation"
    );
    server
        .db
        .run(|conn| conn.batch_execute("UPDATE api_keys SET is_active = 0"))
        .await
        .unwrap();
    assert_eq!(request(&key).send().await.unwrap().status(), 401);
}

#[tokio::test(flavor = "current_thread")]
async fn ip_filter_admission_failure_is_correlated_and_never_bypassed() {
    let logs = LogCapture::default();
    let _subscriber = tracing::subscriber::set_default(logs.subscriber());
    let server = TestServer::new().await;
    let occupied = OccupiedPool::hold(&server).await;
    let response = server
        .client
        .get(server.url("/api/v1/health?token=private_query_marker"))
        .bearer_auth("private_header_marker")
        .header("x-request-id", "untrusted_correlation_marker")
        .send()
        .await
        .unwrap();
    let id = request_id(&response);
    assert_eq!(response.status(), 503);
    occupied.release().await;

    let output = logs.text();
    let line = output.lines().find(|line| line.contains(&id)).unwrap();
    assert!(line.contains("HTTP 503 response"), "{line}");
    assert!(line.contains("ip_filter_rules"), "{line}");
    assert!(line.contains("admission_exhausted"), "{line}");
    assert!(line.contains("checkout_attempts=1"), "{line}");
    assert!(line.contains("checkouts_acquired=0"), "{line}");
    assert!(line.contains("statements_started=0"), "{line}");
    for marker in [
        "private_query_marker",
        "private_header_marker",
        "untrusted_correlation_marker",
    ] {
        assert!(
            !output.contains(marker),
            "sensitive request input reached logs"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn timed_out_blob_reservation_reports_owned_save_stage_without_publishing() {
    use diesel::Connection;
    use std::time::Duration;

    let logs = LogCapture::default();
    let _subscriber = tracing::subscriber::set_default(logs.subscriber());
    let server = TestServer::new_with_database_deadlines(1_500, 10_000).await;
    let (jwt, _) = server
        .register(
            "save_deadline",
            "save_deadline@test.invalid",
            "Password123!",
        )
        .await;
    let (document, _, created) = server
        .create_document(&jwt, "Deadline", "writer", None)
        .await;
    let url = server.url(&format!("/api/v1/documents/{document}"));
    // Ensure the activity heartbeat stays a read during this request.
    server
        .db
        .run(|conn| {
            diesel::sql_query("UPDATE sessions SET last_activity_at = ?")
                .bind::<diesel::sql_types::Text, _>(chrono::Utc::now().to_rfc3339())
                .execute(conn)
        })
        .await
        .unwrap();

    let mut lock = diesel::SqliteConnection::establish(
        server._temp_dir.path().join("test.db").to_str().unwrap(),
    )
    .unwrap();
    lock.batch_execute("BEGIN IMMEDIATE").unwrap();
    let mut draft = created["data"]["content"].clone();
    draft["title"] = json!("private_draft_marker");
    let response = tokio::time::timeout(
        Duration::from_secs(8),
        server
            .client
            .put(&url)
            .bearer_auth(&jwt)
            .json(&json!({"expected_version": 1, "content": draft}))
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    let id = request_id(&response);
    assert_eq!(response.status(), 503);
    let error = response.json::<Value>().await.unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("database operation timed out"),
        "{error}"
    );
    lock.batch_execute("ROLLBACK").unwrap();

    let output = logs.text();
    let line = output
        .lines()
        .find(|line| line.contains(&id) && line.contains("HTTP 503 response"))
        .unwrap();
    assert!(
        line.contains("blob_write_reservation") && line.contains("operation_timeout"),
        "{line}"
    );
    assert!(line.contains("operation_phase=Some(\"running\")"), "{line}");
    assert!(!output.contains("private_draft_marker"));
    assert!(!output.contains(&jwt));

    // The SQLite worker can finish its reservation after HTTP has returned.
    // Wait for its cleanup before checking that the version stayed unchanged.
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.db.state().admitted_operations > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let current = server
        .client
        .get(&url)
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(current.status(), 200);
    let current = current.json::<Value>().await.unwrap();
    assert_eq!(current["data"]["version"], 1);
    assert_eq!(current["data"]["content"], created["data"]["content"]);
}
