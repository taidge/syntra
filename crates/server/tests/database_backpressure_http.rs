#[allow(dead_code)]
mod common;

use common::TestServer;
use salvo::conn::Acceptor;
use salvo::prelude::*;
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use syntra_core::error::AppError;
use syntra_server::handlers::{documents, files, upload};
use syntra_server::middleware::auth::jwt_auth_middleware;
use syntra_server::state::AppState;
use tokio::sync::Notify;

#[derive(Clone)]
struct StateInjector(Arc<AppState>);
#[handler]
impl StateInjector {
    async fn handle(&self, depot: &mut Depot) {
        depot.inject(self.0.clone());
    }
}

/// Real authentication completes before the test fills the real DB capacity.
/// This prevents a middleware 503 from masking a handler's incorrect 500.
#[derive(Clone)]
struct AuthenticatedBarrier {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
#[handler]
impl AuthenticatedBarrier {
    async fn handle(&self, depot: &mut Depot) {
        assert!(syntra_server::middleware::auth::get_credential(depot).is_some());
        self.entered.notify_one();
        self.release.notified().await;
    }
}

async fn assert_503(response: reqwest::Response) {
    let status = response.status();
    let headers = response.headers().clone();
    let text = response.text().await.unwrap();
    let body: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("status={status} headers={headers:?} text={text:?}: {e}"));
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["success"], false);
    assert_eq!(
        body["error"]["code"], 503,
        "wire and JSON status must agree: {body}"
    );
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("database admission capacity exceeded"),
        "the handler, rather than auth middleware, must report the actual DB pressure: {body}"
    );
}

#[tokio::test]
async fn authenticated_get_post_put_handlers_return_503_when_real_admission_is_full() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("db_pressure", "db_pressure@test.invalid", "Password123!")
        .await;
    let (document, file, created) = server
        .create_document(&jwt, "pressure", "writer", None)
        .await;
    let barrier = AuthenticatedBarrier {
        entered: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    };
    let router = Router::new()
        .hoop(StateInjector(server.state.clone()))
        .hoop(jwt_auth_middleware)
        .hoop(barrier.clone())
        .push(Router::with_path("api/v1/documents").post(documents::create_document))
        .push(
            Router::with_path("api/v1/documents/{id}")
                .get(documents::get_document)
                .put(documents::update_document),
        )
        .push(Router::with_path("api/v1/files").get(files::list_files))
        .push(Router::with_path("api/v1/files/{id}").get(files::get_file))
        .push(Router::with_path("api/v1/upload/sessions").post(upload::create_upload_session));
    let acceptor = TcpListener::new("127.0.0.1:0").bind().await;
    let address = acceptor.holdings()[0]
        .local_addr
        .clone()
        .into_std()
        .unwrap();
    let task = tokio::spawn(async move {
        Server::new(acceptor).serve(router).await;
    });
    let content = created["data"]["content"].clone();
    for (method, path, payload) in [
        (
            reqwest::Method::GET,
            format!("/api/v1/documents/{document}"),
            Value::Null,
        ),
        (reqwest::Method::GET, "/api/v1/files".into(), Value::Null),
        (
            reqwest::Method::GET,
            format!("/api/v1/files/{file}"),
            Value::Null,
        ),
        (
            reqwest::Method::POST,
            "/api/v1/documents".into(),
            json!({"name":"blocked","document_type":"writer"}),
        ),
        (
            reqwest::Method::POST,
            "/api/v1/upload/sessions".into(),
            json!({"file_name":"blocked.txt","total_size":4}),
        ),
        (
            reqwest::Method::PUT,
            format!("/api/v1/documents/{document}"),
            json!({"content":content,"expected_version":1}),
        ),
    ] {
        let mut request = server
            .client
            .request(method, format!("http://{address}{path}"))
            .bearer_auth(&jwt);
        if !payload.is_null() {
            request = request.json(&payload);
        }
        let response = tokio::spawn(async move { request.send().await.unwrap() });
        tokio::time::timeout(Duration::from_secs(5), barrier.entered.notified())
            .await
            .unwrap();
        let busy = common::OccupiedPool::hold(&server).await;
        barrier.release.notify_one();
        let result = tokio::time::timeout(Duration::from_secs(5), response)
            .await
            .unwrap()
            .unwrap();
        assert_503(result).await;
        busy.release().await;
    }
    task.abort();
    let current = server
        .client
        .get(server.url(&format!("/api/v1/documents/{document}")))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(current.status(), 200);
    assert_eq!(current.json::<Value>().await.unwrap()["data"]["version"], 1);
    assert_eq!(server.db.run(|conn| {
        use diesel::RunQueryDsl;
        #[derive(diesel::QueryableByName)]
        struct Count { #[diesel(sql_type = diesel::sql_types::BigInt)] count: i64 }
        diesel::sql_query("SELECT (SELECT COUNT(*) FROM documents)+(SELECT COUNT(*) FROM upload_sessions) AS count").get_result::<Count>(conn).map(|v| v.count)
    }).await.unwrap(), 1, "rejected POSTs leave no new document or session");
}

#[tokio::test]
async fn a_real_sqlite_write_lock_is_503_but_a_real_sql_error_stays_500() {
    use diesel::{Connection, RunQueryDsl};
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("db_locked", "db_locked@test.invalid", "Password123!")
        .await;
    let (document, _, created) = server.create_document(&jwt, "locked", "writer", None).await;
    let mut external = diesel::SqliteConnection::establish(
        server._temp_dir.path().join("test.db").to_str().unwrap(),
    )
    .unwrap();
    diesel::sql_query("BEGIN IMMEDIATE")
        .execute(&mut external)
        .unwrap();
    for request in [
        server
            .client
            .post(server.url("/api/v1/upload/sessions"))
            .json(&json!({"file_name":"locked.txt","total_size":4})),
        server
            .client
            .put(server.url(&format!("/api/v1/documents/{document}")))
            .json(&json!({"content":created["data"]["content"],"expected_version":1})),
    ] {
        let response =
            tokio::time::timeout(Duration::from_secs(20), request.bearer_auth(&jwt).send())
                .await
                .unwrap()
                .unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert_eq!(status, 503, "{body}");
        assert_eq!(body["error"]["code"], 503, "{body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("database is locked"),
            "the actual SQLite diagnostic must remain visible: {body}"
        );
    }
    diesel::sql_query("ROLLBACK")
        .execute(&mut external)
        .unwrap();
    let error = server
        .db
        .run(|conn| {
            diesel::sql_query("THIS IS NOT SQL")
                .execute(conn)
                .map_err(AppError::from)
        })
        .await
        .unwrap_err();
    assert!(!error.is_transient_database());
    assert_eq!(error.status_code(), 500);
    assert_eq!(error.to_response().code, 500);
}
