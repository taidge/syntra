#[allow(dead_code)]
mod common;

use common::TestServer;
use diesel::RunQueryDsl;
use diesel::sql_types::Text;
use salvo::conn::Acceptor;
use salvo::prelude::*;
use serde_json::{Value, json};
use std::sync::Arc;
use syntra_server::middleware::auth::{hash_token, jwt_auth_middleware};
use syntra_server::state::AppState;
use tokio::sync::Notify;

#[derive(Clone)]
struct InjectState(Arc<AppState>);

#[handler]
impl InjectState {
    async fn handle(&self, depot: &mut Depot) {
        depot.inject(self.0.clone());
    }
}

/// Pause after the real authentication middleware, before the real handler.
/// No production hook or timing sleep is needed for this no-body DELETE race.
#[derive(Clone)]
struct AuthenticatedGate {
    arrived: Arc<Notify>,
    release: Arc<Notify>,
}

#[handler]
impl AuthenticatedGate {
    async fn handle(&self) {
        self.arrived.notify_one();
        self.release.notified().await;
    }
}

async fn snapshot(server: &TestServer) -> Value {
    #[derive(diesel::QueryableByName)]
    struct Snapshot {
        #[diesel(sql_type = Text)]
        value: String,
    }
    let value = server.db.run(|conn| {
        diesel::sql_query("SELECT json_object('files',(SELECT json_group_array(json_array(id,is_trashed,path,size)) FROM (SELECT * FROM files ORDER BY id)), 'documents',(SELECT json_group_array(json_array(id,file_id,version,content)) FROM (SELECT * FROM documents ORDER BY id)), 'versions',(SELECT COUNT(*) FROM versions), 'batches',(SELECT COUNT(*) FROM trash_batch_members), 'quota',(SELECT SUM(storage_used) FROM users), 'gc',(SELECT json_group_array(path) FROM (SELECT path FROM storage_gc ORDER BY path))) AS value")
            .get_result::<Snapshot>(conn)
    }).await.unwrap().value;
    serde_json::from_str(&value).unwrap()
}

async fn assert_revocation_during_trash_delete(empty: bool) {
    for kind in ["jwt", "api-key"] {
        let server = TestServer::new().await;
        let (jwt, _) = server
            .register("trash_owner", "trash_owner@test.invalid", "Password123!")
            .await;
        let (_, file, _) = server
            .create_document(&jwt, "retained trash", "writer", None)
            .await;
        assert_eq!(
            server
                .client
                .delete(server.url(&format!("/api/v1/files/{file}")))
                .bearer_auth(&jwt)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let (credential, key_id) = if kind == "api-key" {
            let response = server
                .client
                .post(server.url("/api/v1/api-keys"))
                .bearer_auth(&jwt)
                .json(&json!({"name":"trash writer","permissions":"write"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 201);
            let body: Value = response.json().await.unwrap();
            (
                body["data"]["key"].as_str().unwrap().to_owned(),
                Some(body["data"]["id"].as_str().unwrap().to_owned()),
            )
        } else {
            (jwt.clone(), None)
        };
        let before = snapshot(&server).await;
        let gate = AuthenticatedGate {
            arrived: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        };
        let path = if empty {
            "/api/v1/trash".to_owned()
        } else {
            format!("/api/v1/trash/{file}")
        };
        let route = Router::with_path(if empty {
            "api/v1/trash"
        } else {
            "api/v1/trash/{id}"
        })
        .hoop(InjectState(server.state.clone()))
        .hoop(jwt_auth_middleware)
        .hoop(gate.clone());
        let route = if empty {
            route.delete(syntra_server::handlers::trash::empty_trash)
        } else {
            route.delete(syntra_server::handlers::trash::permanently_delete)
        };
        let acceptor = TcpListener::new("127.0.0.1:0").bind().await;
        let address = acceptor.holdings()[0]
            .local_addr
            .clone()
            .into_std()
            .unwrap();
        let listener = tokio::spawn(async move {
            Server::new(acceptor).serve(route).await;
        });
        let request = server
            .client
            .delete(format!("http://{address}{path}"))
            .bearer_auth(&credential);
        let request = tokio::spawn(async move { request.send().await.unwrap() });
        tokio::time::timeout(std::time::Duration::from_secs(5), gate.arrived.notified())
            .await
            .expect("real authentication must reach the gate");
        let jwt_hash = hash_token(&jwt);
        server
            .db
            .run(move |conn| {
                if let Some(id) = key_id {
                    diesel::sql_query("UPDATE api_keys SET permissions='read' WHERE id=?")
                        .bind::<Text, _>(id)
                        .execute(conn)
                } else {
                    diesel::sql_query("UPDATE sessions SET is_active=0 WHERE token_hash=?")
                        .bind::<Text, _>(jwt_hash)
                        .execute(conn)
                }
            })
            .await
            .unwrap();
        gate.release.notify_one();
        let response = request.await.unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        listener.abort();
        let _ = listener.await;
        assert_eq!(
            status.as_u16(),
            if kind == "api-key" { 403 } else { 401 },
            "{kind}, {path}: {body}"
        );
        assert_eq!(
            snapshot(&server).await,
            before,
            "revoked {kind} changed trash, versions, quota or GC"
        );

        // A fresh legitimate credential can still perform the same operation.
        let (fresh, _) = server
            .login("trash_owner@test.invalid", "Password123!")
            .await;
        assert_eq!(
            server
                .client
                .delete(server.url(&path))
                .bearer_auth(&fresh)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        assert_ne!(snapshot(&server).await, before);
    }
}

#[tokio::test]
async fn permanent_delete_revalidates_revoked_jwt_and_downgraded_key_after_authentication() {
    assert_revocation_during_trash_delete(false).await;
}

#[tokio::test]
async fn empty_trash_revalidates_revoked_jwt_and_downgraded_key_after_authentication() {
    assert_revocation_during_trash_delete(true).await;
}
