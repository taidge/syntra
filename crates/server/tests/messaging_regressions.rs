#[allow(dead_code)]
mod common;

use common::TestServer;
use contrix_collab::{CollabMessage, OpType, Operation};
use contrix_core::schema::{
    documents::dsl as d, files::dsl as f, operation_log::dsl as ol, users::dsl as u,
    versions::dsl as v,
};
use diesel::prelude::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{self, Message, client::IntoClientRequest},
};
use uuid::Uuid;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn handshake(server: &TestServer, document: &str, token: &str) -> tungstenite::http::Request<()> {
    let url = server
        .url(&format!("/api/v1/ws/collab/{document}"))
        .replacen("http://", "ws://", 1);
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

async fn connect(server: &TestServer, document: &str, token: &str) -> Socket {
    tokio_tungstenite::connect_async(handshake(server, document, token))
        .await
        .unwrap()
        .0
}

async fn next(socket: &mut Socket) -> CollabMessage {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let message = socket
                .next()
                .await
                .expect("socket closed")
                .expect("websocket failure");
            if let Message::Text(text) = message {
                return serde_json::from_str(&text).expect("invalid collaboration response");
            }
        }
    })
    .await
    .expect("collaboration response timed out")
}

async fn snapshot(socket: &mut Socket) -> (Value, u64) {
    loop {
        match next(socket).await {
            CollabMessage::DocumentState { content, version } => return (content, version),
            CollabMessage::UserJoined { .. }
            | CollabMessage::UserLeft { .. }
            | CollabMessage::CursorUpdate(_) => {}
            other => panic!("expected snapshot, got {other:?}"),
        }
    }
}

fn replacement(content: &Value, version: u64, text: &str) -> Operation {
    // The server must replace this deliberately forged author with the actual
    // credential owner, while preserving the client's operation id.
    Operation::new(
        Uuid::nil(),
        Uuid::new_v4(),
        1,
        OpType::Replace {
            path: vec![],
            old_value: content.clone(),
            new_value: contrix_doc_engine::writer::writer_content_with_text(content, text),
        },
    )
    .with_base_version(version)
}

async fn send_operation(socket: &mut Socket, operation: &Operation) {
    socket
        .send(Message::Text(
            serde_json::to_string(&CollabMessage::Operation(operation.clone()))
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
}

async fn ack(socket: &mut Socket, operation: &Operation) {
    loop {
        match next(socket).await {
            CollabMessage::Ack { op_id } => {
                assert_eq!(op_id, operation.id);
                return;
            }
            CollabMessage::DocumentState { .. }
            | CollabMessage::UserJoined { .. }
            | CollabMessage::UserLeft { .. } => {}
            other => panic!("operation did not receive an ACK: {other:?}"),
        }
    }
}

async fn create(server: &TestServer, jwt: &str, path: &str, value: Value) -> Value {
    let response = server
        .client
        .post(server.url(path))
        .headers(server.auth_header(jwt))
        .json(&value)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let value: Value = response.json().await.unwrap();
    assert_eq!(status, 201, "{path}: {value}");
    value["data"].clone()
}

async fn api_key(server: &TestServer, jwt: &str, scope: &str) -> (String, String) {
    let data = create(
        server,
        jwt,
        "/api/v1/api-keys",
        json!({"name":format!("{scope}-key"),"permissions":scope}),
    )
    .await;
    (
        data["id"].as_str().unwrap().into(),
        data["key"].as_str().unwrap().into(),
    )
}

async fn get_document(server: &TestServer, jwt: &str, document: &str) -> Value {
    let response = server
        .client
        .get(server.url(&format!("/api/v1/documents/{document}")))
        .headers(server.auth_header(jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json::<Value>().await.unwrap()["data"].clone()
}

async fn rest_save(
    server: &TestServer,
    credential: &str,
    document: &str,
    content: &Value,
    version: u64,
) -> u64 {
    let response = server
        .client
        .put(server.url(&format!("/api/v1/documents/{document}")))
        .headers(server.auth_header(credential))
        .json(&json!({"content":content,"expected_version":version}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "REST save: {body}");
    body["data"]["version"].as_u64().unwrap()
}

#[derive(Debug, PartialEq)]
struct DurableState {
    content: Option<String>,
    version: i32,
    path: String,
    size: i64,
    storage_used: i64,
    snapshots: i64,
    operations: i64,
}

async fn durable(server: &TestServer, document: &str) -> DurableState {
    let document = document.to_owned();
    server
        .db
        .run(move |conn| {
            let (content, version, file_id) = d::documents
                .find(&document)
                .select((d::content, d::version, d::file_id))
                .first::<(Option<String>, i32, String)>(conn)?;
            let (path, size, owner) = f::files
                .find(file_id)
                .select((f::path, f::size, f::owner_id))
                .first::<(String, i64, String)>(conn)?;
            Ok::<_, diesel::result::Error>(DurableState {
                content,
                version,
                path,
                size,
                storage_used: u::users.find(owner).select(u::storage_used).first(conn)?,
                snapshots: v::versions
                    .filter(v::document_id.eq(&document))
                    .count()
                    .get_result(conn)?,
                operations: ol::operation_log
                    .filter(ol::document_id.eq(&document))
                    .count()
                    .get_result(conn)?,
            })
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn read_only_api_keys_cannot_upgrade_to_writable_collaboration() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("reader", "reader@test.com", "password123")
        .await;
    let (document, _, _) = server
        .create_document(&jwt, "read-key", "writer", None)
        .await;
    for scope in ["read", "ws:read"] {
        let (_, key) = api_key(&server, &jwt, scope).await;
        match tokio_tungstenite::connect_async(handshake(&server, &document, &key)).await {
            Err(tungstenite::Error::Http(response)) => assert_eq!(response.status(), 403),
            other => panic!("read-only websocket handshake must be forbidden: {other:?}"),
        }
    }
}

#[tokio::test]
async fn encoded_and_duplicate_slash_routes_cannot_bypass_jwt_only_key_management() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("keyboundary", "keyboundary@test.com", "password123")
        .await;
    let (_, key) = api_key(&server, &jwt, "write").await;
    for path in [
        "/api/v1/%61pi-keys",
        "/api//v1/api-keys",
        "/api/v1//api-keys/",
    ] {
        let response = server
            .client
            .post(server.url(path))
            .headers(server.auth_header(&key))
            .json(&json!({"name":"must-not-mint-wildcard"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{path}");
    }
    let response = server
        .client
        .get(server.url("/api/v1/api-keys"))
        .headers(server.auth_header(&jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn a_reused_operation_id_cannot_ack_a_different_payload() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("opidentity", "opidentity@test.com", "password123")
        .await;
    let (document, _, _) = server
        .create_document(&jwt, "identity", "writer", None)
        .await;
    let mut socket = connect(&server, &document, &jwt).await;
    let (initial, version) = snapshot(&mut socket).await;
    let operation = replacement(&initial, version, "original accepted request");
    send_operation(&mut socket, &operation).await;
    ack(&mut socket, &operation).await;
    let before = durable(&server, &document).await;
    let mut changed = operation.clone();
    changed.op_type = OpType::Replace {
        path: vec![],
        old_value: initial.clone(),
        new_value: contrix_doc_engine::writer::writer_content_with_text(
            &initial,
            "different request with stolen id",
        ),
    };
    send_operation(&mut socket, &changed).await;
    loop {
        match next(&mut socket).await {
            CollabMessage::Error { .. } => break,
            CollabMessage::UserJoined { .. } | CollabMessage::UserLeft { .. } => {}
            other => panic!(
                "a different request reusing an operation id must not be acknowledged: {other:?}"
            ),
        }
    }
    assert_eq!(durable(&server, &document).await, before);
}

#[tokio::test]
async fn transformed_requests_retry_by_original_identity_and_compacted_ids_cannot_reapply() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("otidentity", "otidentity@test.com", "password123")
        .await;
    let (document, _, _) = server.create_document(&jwt, "OT", "writer", None).await;
    let mut socket = connect(&server, &document, &jwt).await;
    let (initial, version) = snapshot(&mut socket).await;
    let first = Operation::new(
        Uuid::nil(),
        Uuid::new_v4(),
        1,
        OpType::Insert {
            path: vec!["writer".into(), "title".into()],
            index: 0,
            content: "A".into(),
        },
    )
    .with_base_version(version);
    send_operation(&mut socket, &first).await;
    ack(&mut socket, &first).await;
    let second = Operation::new(
        Uuid::nil(),
        Uuid::new_v4(),
        1,
        OpType::Insert {
            path: vec!["title".into()],
            index: 1,
            content: "B".into(),
        },
    )
    .with_base_version(version);
    send_operation(&mut socket, &second).await;
    ack(&mut socket, &second).await;
    let before = durable(&server, &document).await;
    assert_eq!(before.version as u64, version + 2);
    let content: Value = serde_json::from_str(before.content.as_ref().unwrap()).unwrap();
    let original_title = initial["title"].as_str().unwrap();
    let mut expected_title = original_title.to_owned();
    expected_title.insert(1, 'B');
    expected_title.insert(0, 'A');
    assert_eq!(content["title"], expected_title);
    send_operation(&mut socket, &second).await;
    ack(&mut socket, &second).await;
    assert_eq!(
        durable(&server, &document).await,
        before,
        "the original untransformed request is a valid retry"
    );

    let log_id = format!("{document}:{}:{}", second.id.client_id, second.id.counter);
    // Simulate independent history/receipt retention without 2049 expensive
    // document commits. The durable client high-water mark must survive both.
    server
        .db
        .run(move |conn| {
            diesel::sql_query("DELETE FROM operation_receipts WHERE id = ?")
                .bind::<diesel::sql_types::Text, _>(&log_id)
                .execute(conn)?;
            diesel::delete(ol::operation_log.find(log_id)).execute(conn)?;
            Ok::<_, diesel::result::Error>(())
        })
        .await
        .unwrap();
    let after_compaction = durable(&server, &document).await;
    let replay = second.with_base_version(after_compaction.version as u64);
    send_operation(&mut socket, &replay).await;
    loop {
        match next(&mut socket).await {
            CollabMessage::Error { .. } => break,
            CollabMessage::UserJoined { .. } | CollabMessage::UserLeft { .. } => {}
            other => panic!("compacted operation id must require synchronization: {other:?}"),
        }
    }
    assert_eq!(durable(&server, &document).await, after_compaction);
}

#[tokio::test]
async fn concurrent_insert_and_delete_keep_inserted_text_in_both_durable_orders() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("splitdelete", "splitdelete@test.com", "password123")
        .await;
    for insert_first in [true, false] {
        let name = if insert_first {
            "insert-first"
        } else {
            "delete-first"
        };
        let (document, _, response) = server.create_document(&jwt, name, "writer", None).await;
        let mut content = response["data"]["content"].clone();
        content["title"] = json!("abcd");
        let version = rest_save(&server, &jwt, &document, &content, 1).await;
        let mut a = connect(&server, &document, &jwt).await;
        let (initial, observed) = snapshot(&mut a).await;
        assert_eq!(observed, version);
        let mut b = connect(&server, &document, &jwt).await;
        let _ = snapshot(&mut b).await;
        let insertion = Operation::new(
            Uuid::nil(),
            Uuid::new_v4(),
            1,
            OpType::Insert {
                path: vec!["title".into()],
                index: 2,
                content: "X".into(),
            },
        )
        .with_base_version(version);
        let deletion = Operation::new(
            Uuid::nil(),
            Uuid::new_v4(),
            1,
            OpType::Delete {
                path: vec!["title".into()],
                index: 1,
                length: 2,
                removed_content: Some("bc".into()),
            },
        )
        .with_base_version(version);
        let (first, second) = if insert_first {
            (&insertion, &deletion)
        } else {
            (&deletion, &insertion)
        };
        send_operation(&mut a, first).await;
        ack(&mut a, first).await;
        // Ignore the queued first commit on B; second was produced against the
        // same original snapshot, exactly like two concurrent browser edits.
        let _ = snapshot(&mut b).await;
        send_operation(&mut b, second).await;
        ack(&mut b, second).await;
        let stored = durable(&server, &document).await;
        assert_eq!(stored.version as u64, version + 2);
        assert_eq!(
            serde_json::from_str::<Value>(stored.content.as_ref().unwrap()).unwrap()["title"],
            "aXd"
        );
        assert_eq!(stored.operations, 2);
        assert_eq!(initial["title"], "abcd");
        let mut reconnect = connect(&server, &document, &jwt).await;
        let (current, current_version) = snapshot(&mut reconnect).await;
        assert_eq!(current["title"], "aXd");
        assert_eq!(current_version, version + 2);
        // A split, transformed delete remains an idempotent retry of the
        // original contiguous selection, not of just its first physical step.
        send_operation(&mut reconnect, &deletion).await;
        ack(&mut reconnect, &deletion).await;
        assert_eq!(durable(&server, &document).await, stored);
    }
}

#[tokio::test]
async fn websocket_ack_follows_durable_commit_and_retries_snapshots_and_rest_stay_consistent() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("contrix_server::handlers::ws=debug")
        .with_test_writer()
        .try_init();
    let server = TestServer::new().await;
    let (jwt, registration) = server
        .register("writer", "writer@test.com", "password123")
        .await;
    let user = registration["data"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (document, file, initial_response) = server
        .create_document(&jwt, "durable-collab", "writer", None)
        .await;
    let (_, key) = api_key(&server, &jwt, "ws:write").await;
    let mut sender = connect(&server, &document, &key).await;
    eprintln!("durability phase: sender initial snapshot");
    let (initial, initial_version) = snapshot(&mut sender).await;
    assert_eq!(initial, initial_response["data"]["content"]);
    assert_eq!(
        initial_version,
        initial_response["data"]["version"].as_u64().unwrap()
    );
    let mut observer = connect(&server, &document, &jwt).await;
    eprintln!("durability phase: observer initial snapshot");
    assert_eq!(
        snapshot(&mut observer).await,
        (initial.clone(), initial_version)
    );

    let operation = replacement(&initial, initial_version, "Persisted before the ACK. 中文");
    let expected = match &operation.op_type {
        OpType::Replace { new_value, .. } => new_value.clone(),
        _ => unreachable!(),
    };
    send_operation(&mut sender, &operation).await;
    eprintln!("durability phase: initial operation ACK");
    ack(&mut sender, &operation).await;
    // No polling or sleeps: state must already be durable when ACK arrives.
    let committed = durable(&server, &document).await;
    assert_eq!(committed.version as u64, initial_version + 1);
    assert_eq!(committed.operations, 1);
    assert_eq!(
        serde_json::from_str::<Value>(committed.content.as_ref().unwrap()).unwrap(),
        expected
    );
    let doc = get_document(&server, &jwt, &document).await;
    assert_eq!(doc["content"], expected);
    assert_eq!(doc["version"], initial_version + 1);
    let download = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/download")))
        .headers(server.auth_header(&jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200);
    assert_eq!(download.json::<Value>().await.unwrap(), expected);
    let op_document = document.clone();
    let author = server
        .db
        .run(move |conn| {
            ol::operation_log
                .filter(ol::document_id.eq(op_document))
                .select(ol::user_id)
                .first::<String>(conn)
        })
        .await
        .unwrap();
    assert_eq!(author, user, "client-supplied author must not be trusted");
    eprintln!("durability phase: observer sees initial commit");
    assert_eq!(
        snapshot(&mut observer).await,
        (expected.clone(), initial_version + 1)
    );

    send_operation(&mut sender, &operation).await;
    eprintln!("durability phase: idempotent retry ACK");
    ack(&mut sender, &operation).await;
    assert_eq!(
        durable(&server, &document).await,
        committed,
        "retry must not change version, blob, quota or snapshots"
    );
    let mut reconnected = connect(&server, &document, &key).await;
    eprintln!("durability phase: reconnected initial snapshot");
    assert_eq!(
        snapshot(&mut reconnected).await,
        (expected.clone(), initial_version + 1)
    );
    let rest_content =
        contrix_doc_engine::writer::writer_content_with_text(&expected, "Saved over REST");
    let rest_version =
        rest_save(&server, &jwt, &document, &rest_content, initial_version + 1).await;
    assert_eq!(rest_version, initial_version + 2);
    eprintln!("durability phase: observer sees REST commit");
    assert_eq!(
        snapshot(&mut observer).await,
        (rest_content.clone(), rest_version)
    );
    assert_eq!(
        snapshot(&mut reconnected).await,
        (rest_content, rest_version)
    );
}

#[tokio::test]
async fn operation_log_failure_does_not_ack_publish_or_commit_document_or_quota() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("rollbackwriter", "rollbackwriter@test.com", "password123")
        .await;
    let (document, _, _) = server
        .create_document(&jwt, "rollback-collab", "writer", None)
        .await;
    let mut sender = connect(&server, &document, &jwt).await;
    let (initial, version) = snapshot(&mut sender).await;
    let mut observer = connect(&server, &document, &jwt).await;
    let _ = snapshot(&mut observer).await;
    let before = durable(&server, &document).await;
    server.db.run(|conn| diesel::sql_query(
        "CREATE TRIGGER refuse_operation_log BEFORE INSERT ON operation_log BEGIN SELECT RAISE(ABORT, 'injected-operation-log-failure'); END"
    ).execute(conn)).await.unwrap();
    let operation = replacement(&initial, version, "Must never be committed or broadcast");
    send_operation(&mut sender, &operation).await;
    loop {
        match next(&mut sender).await {
            CollabMessage::Error { .. } => break,
            CollabMessage::UserJoined { .. } | CollabMessage::UserLeft { .. } => {}
            other => panic!("failed commit must not ACK or publish state: {other:?}"),
        }
    }
    assert_eq!(durable(&server, &document).await, before);
    assert_eq!(
        get_document(&server, &jwt, &document).await["content"],
        initial
    );
    // A sync request after failure must return the unchanged durable snapshot.
    observer
        .send(Message::Text(
            serde_json::to_string(&CollabMessage::SyncRequest {
                from_version: version,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    assert_eq!(snapshot(&mut observer).await, (initial, version));
}

async fn assert_revoked(socket: &mut Socket, baseline: u64) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(message) = socket.next().await {
            match message {
                Ok(Message::Close(_)) | Err(_) => return,
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<CollabMessage>(&text).unwrap() {
                        CollabMessage::Ack { .. } => panic!("revoked connection ACKed a write"),
                        CollabMessage::DocumentState { version, .. } => assert!(
                            version <= baseline,
                            "revoked connection received new document content"
                        ),
                        CollabMessage::Operation(_) => {
                            panic!("revoked connection received an operation")
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    })
    .await
    .expect("revoked websocket was not closed within its revalidation budget");
}

#[tokio::test]
async fn database_backpressure_pauses_a_socket_without_losing_its_durable_operation() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("busy_socket", "busy_socket@test.invalid", "Password123!")
        .await;
    let (document, _, _) = server
        .create_document(&jwt, "busy document", "writer", None)
        .await;
    let mut socket = connect(&server, &document, &jwt).await;
    let (initial, version) = snapshot(&mut socket).await;
    let busy = common::OccupiedPool::hold(&server).await;
    let operation = replacement(&initial, version, "preserved across contention");
    send_operation(&mut socket, &operation).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), socket.next())
            .await
            .is_err(),
        "a database admission failure must neither reset the socket nor acknowledge/send content before revalidation"
    );
    busy.release().await;
    ack(&mut socket, &operation).await;
    let committed = durable(&server, &document).await;
    assert_eq!(committed.version as u64, version + 1);
    assert_eq!(committed.operations, 1);
}

#[tokio::test]
async fn revocation_during_database_backpressure_never_uses_cached_authorization() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("busy_revoke", "busy_revoke@test.invalid", "Password123!")
        .await;
    let (document, _, _) = server
        .create_document(&jwt, "revoked busy document", "writer", None)
        .await;
    let mut socket = connect(&server, &document, &jwt).await;
    let (initial, version) = snapshot(&mut socket).await;
    let busy = common::OccupiedPool::hold(&server).await;
    let operation = replacement(&initial, version, "must not be committed");
    send_operation(&mut socket, &operation).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), socket.next())
            .await
            .is_err()
    );
    // An independent administrative connection commits revocation while every
    // application lease is occupied. Waiting must rerun the actual check.
    let db_path = server._temp_dir.path().join("test.db");
    let mut admin = diesel::SqliteConnection::establish(db_path.to_str().unwrap()).unwrap();
    assert_eq!(
        diesel::sql_query("UPDATE sessions SET is_active=0 WHERE token_hash=?")
            .bind::<diesel::sql_types::Text, _>(contrix_server::middleware::auth::hash_token(&jwt))
            .execute(&mut admin)
            .unwrap(),
        1,
        "the independent revocation actually changed the active session"
    );
    drop(admin);
    busy.release().await;
    assert_revoked(&mut socket, version).await;
    let committed = durable(&server, &document).await;
    assert_eq!(committed.version as u64, version);
    assert_eq!(committed.operations, 0);
}

#[tokio::test]
async fn revoked_jwt_and_api_key_connections_cannot_write_or_receive_later_commits() {
    for revoke_kind in ["jwt", "api-key"] {
        let server = TestServer::new().await;
        let (jwt, _) = server
            .register("revokedwriter", "revokedwriter@test.com", "password123")
            .await;
        let (document, _, _) = server
            .create_document(&jwt, "revoked-collab", "writer", None)
            .await;
        let (key_id, key) = api_key(&server, &jwt, "read,write").await;
        let (credential, actor) = if revoke_kind == "jwt" {
            (&jwt, &key)
        } else {
            (&key, &jwt)
        };
        let mut sending = connect(&server, &document, credential).await;
        let (initial, version) = snapshot(&mut sending).await;
        let mut passive = connect(&server, &document, credential).await;
        let _ = snapshot(&mut passive).await;
        let path = if revoke_kind == "jwt" {
            let current = server
                .client
                .get(server.url("/api/v1/sessions/current"))
                .headers(server.auth_header(&jwt))
                .send()
                .await
                .unwrap();
            assert_eq!(current.status(), 200);
            let current: Value = current.json().await.unwrap();
            format!(
                "/api/v1/sessions/{}",
                current["data"]["id"].as_str().unwrap()
            )
        } else {
            format!("/api/v1/api-keys/{key_id}")
        };
        let response = server
            .client
            .delete(server.url(&path))
            .headers(server.auth_header(&jwt))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let forbidden = replacement(&initial, version, "Revoked write must not persist");
        let _ = sending
            .send(Message::Text(
                serde_json::to_string(&CollabMessage::Operation(forbidden))
                    .unwrap()
                    .into(),
            ))
            .await;
        let after_revoke = contrix_doc_engine::writer::writer_content_with_text(
            &initial,
            "Confidential after revocation",
        );
        rest_save(&server, actor, &document, &after_revoke, version).await;
        tokio::join!(
            assert_revoked(&mut sending, version),
            assert_revoked(&mut passive, version)
        );
        let stored = durable(&server, &document).await;
        assert_eq!(stored.version as u64, version + 1);
        assert_eq!(stored.operations, 0);
        assert_eq!(
            serde_json::from_str::<Value>(stored.content.as_ref().unwrap()).unwrap(),
            after_revoke
        );
    }
}

#[tokio::test]
async fn removed_room_member_connections_cannot_write_or_receive_new_body() {
    let server = TestServer::new().await;
    let (owner, _) = server
        .register("roomleader", "roomleader@test.com", "password123")
        .await;
    let (member, registration) = server
        .register("roomeditor", "roomeditor@test.com", "password123")
        .await;
    let user = registration["data"]["user"]["id"].as_str().unwrap();
    let room = create(
        &server,
        &owner,
        "/api/v1/rooms",
        json!({"name":"messaging-team"}),
    )
    .await;
    let room_id = room["id"].as_str().unwrap();
    create(
        &server,
        &owner,
        &format!("/api/v1/rooms/{room_id}/members"),
        json!({"user_id":user,"role":"editor"}),
    )
    .await;
    let doc = create(
        &server,
        &owner,
        "/api/v1/documents",
        json!({"name":"private-room-doc","document_type":"writer","room_id":room_id}),
    )
    .await;
    let document = doc["id"].as_str().unwrap();
    let mut sending = connect(&server, document, &member).await;
    let (initial, version) = snapshot(&mut sending).await;
    let mut passive = connect(&server, document, &member).await;
    let _ = snapshot(&mut passive).await;
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/rooms/{room_id}/members/{user}")))
        .headers(server.auth_header(&owner))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let forbidden = replacement(&initial, version, "Removed editor write");
    let _ = sending
        .send(Message::Text(
            serde_json::to_string(&CollabMessage::Operation(forbidden))
                .unwrap()
                .into(),
        ))
        .await;
    let private = contrix_doc_engine::writer::writer_content_with_text(
        &initial,
        "Only remaining members can see this",
    );
    rest_save(&server, &owner, document, &private, version).await;
    tokio::join!(
        assert_revoked(&mut sending, version),
        assert_revoked(&mut passive, version)
    );
    let stored = durable(&server, document).await;
    assert_eq!(stored.version as u64, version + 1);
    assert_eq!(stored.operations, 0);
}

#[tokio::test]
async fn idle_api_key_connection_is_closed_after_revocation_without_client_frames() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("idlewriter", "idlewriter@test.com", "password123")
        .await;
    let (document, _, _) = server
        .create_document(&jwt, "idle-collab", "writer", None)
        .await;
    let (id, key) = api_key(&server, &jwt, "ws:write").await;
    let mut socket = connect(&server, &document, &key).await;
    let (_, version) = snapshot(&mut socket).await;
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/api-keys/{id}")))
        .headers(server.auth_header(&jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    // Neither a client frame nor a new broadcast prompts the server here: this
    // specifically exercises the periodic credential revalidation path.
    assert_revoked(&mut socket, version).await;
}

#[tokio::test]
async fn downgrading_an_existing_write_key_closes_its_open_editor() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("scopeowner", "scopeowner@test.com", "password123")
        .await;
    let (document, _, _) = server
        .create_document(&jwt, "scope-downgrade", "writer", None)
        .await;
    let (id, key) = api_key(&server, &jwt, "ws:write").await;
    let mut socket = connect(&server, &document, &key).await;
    let (initial, version) = snapshot(&mut socket).await;
    let response = server
        .client
        .put(server.url(&format!("/api/v1/api-keys/{id}")))
        .headers(server.auth_header(&jwt))
        .json(&json!({"permissions":"ws:read"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let operation = replacement(&initial, version, "read scope must not write");
    let _ = socket
        .send(Message::Text(
            serde_json::to_string(&CollabMessage::Operation(operation))
                .unwrap()
                .into(),
        ))
        .await;
    assert_revoked(&mut socket, version).await;
    assert_eq!(durable(&server, &document).await.version as u64, version);
}

#[tokio::test]
async fn notification_socket_accepts_read_scope_but_stops_delivering_after_revocation() {
    let server = TestServer::new().await;
    let (jwt, registration) = server
        .register("notifyowner", "notifyowner@test.com", "password123")
        .await;
    let user = registration["data"]["user"]["id"].as_str().unwrap();
    let (id, key) = api_key(&server, &jwt, "ws:read").await;
    let url = server
        .url("/api/v1/ws/notifications")
        .replacen("http://", "ws://", 1);
    let mut request = url.into_client_request().unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {key}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    contrix_server::services::NotificationService::create(
        &server.db,
        &server.notifications,
        user,
        "comment",
        "before revocation",
        "allowed notification",
        None,
    )
    .await;
    let message = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
                break serde_json::from_str::<Value>(&text).unwrap();
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(message["title"], "before revocation");
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/api-keys/{id}")))
        .headers(server.auth_header(&jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    contrix_server::services::NotificationService::create(
        &server.db,
        &server.notifications,
        user,
        "comment",
        "confidential after revocation",
        "must not cross revoked socket",
        None,
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(message) = socket.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    panic!("revoked notification socket received content: {text}")
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
    })
    .await
    .expect("revoked notification socket remained open");
}

#[tokio::test]
async fn revocation_during_slow_operation_storage_write_prevents_the_final_transaction() {
    use contrix_server::middleware::auth::{AuthenticatedCredential, hash_token};
    use contrix_server::services::{StorageBackend, persistence, s3_storage::S3Storage};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let server = TestServer::new().await;
    let (jwt, registered) = server
        .register("slow_ws_write", "slow_ws@test.invalid", "Password123!")
        .await;
    let user = registered["data"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (document, _, _) = server
        .create_document(&jwt, "slow storage", "writer", None)
        .await;
    let before = durable(&server, &document).await;
    let initial: Value = serde_json::from_str(before.content.as_deref().unwrap()).unwrap();
    let (key_id, key) = api_key(&server, &jwt, "ws:write").await;
    let credential = AuthenticatedCredential {
        user_id: user.clone(),
        token_hash: hash_token(&key),
        api_key_id: Some(key_id.clone()),
        expires_at: None,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (started, observed) = tokio::sync::oneshot::channel();
    let (release, allowed) = tokio::sync::oneshot::channel();
    // This controlled PUT endpoint supplies an I/O barrier, not S3 signature
    // acceptance (which is independently tested against the real Garage fixture).
    let storage_server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut bytes = [0; 1024];
            let count = socket.read(&mut bytes).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&bytes[..count]);
            assert!(
                request.len() < 65536,
                "the controlled test request stays bounded"
            );
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                assert!(headers.starts_with("PUT "));
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .map(|(_, v)| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        started.send(()).unwrap();
        allowed.await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let storage = StorageBackend::S3(S3Storage::new(
        "bucket",
        "us-east-1",
        "test",
        "test",
        &format!("http://{address}"),
    ));
    let db = server.db.clone();
    let document_id = document.clone();
    let mut operation = replacement(&initial, before.version as u64, "revoked during storage");
    operation.user_id = user.parse().unwrap();
    let content = match &operation.op_type {
        OpType::Replace { new_value, .. } => serde_json::to_string(new_value).unwrap(),
        _ => unreachable!(),
    };
    let write = tokio::spawn(async move {
        let hash = persistence::operation_request_hash(&operation).unwrap();
        persistence::commit_document_operation_with_effects_and_credential(
            &db,
            &storage,
            &document_id,
            &user,
            before.version,
            content,
            &operation,
            &hash,
            std::slice::from_ref(&operation.op_type),
            &operation,
            Some(&credential),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), observed)
        .await
        .unwrap()
        .unwrap();
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/api-keys/{key_id}")))
        .headers(server.auth_header(&jwt))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    release.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), write)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            result,
            Err(contrix_core::error::AppError::Auth(_))
                | Err(contrix_core::error::AppError::Forbidden(_))
        ),
        "{result:?}"
    );
    storage_server.await.unwrap();
    assert_eq!(
        durable(&server, &document).await,
        before,
        "revocation must block document, quota, history, receipt and operation mutation after the storage wait"
    );
    let document_id = document.clone();
    assert_eq!(
        server
            .db
            .run(move |conn| {
                diesel::sql_query(
                    "SELECT COUNT(*) AS count FROM operation_receipts WHERE document_id=?",
                )
                .bind::<diesel::sql_types::Text, _>(&document_id)
                .get_result::<ReceiptCount>(conn)
            })
            .await
            .unwrap()
            .count,
        0
    );
}

#[derive(diesel::QueryableByName)]
struct ReceiptCount {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}
