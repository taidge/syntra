#[allow(dead_code)]
mod common;

use common::TestServer;
use diesel::RunQueryDsl;
use diesel::sql_types::{BigInt, Text};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn create(server: &TestServer, token: &str, path: &str, body: Value) -> Value {
    let response = server
        .client
        .post(server.url(path))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert!(status.is_success(), "{path}: {status}: {body}");
    body["data"].clone()
}

/// 100 Continue is emitted only when Hyper's incoming body is polled. This
/// barrier proves authentication finished before the credential is changed;
/// no timing sleep can accidentally turn this into an initial-auth test.
async fn begin_body(
    server: &TestServer,
    method: &str,
    path: &str,
    token: &str,
    content_type: &str,
    body: &[u8],
) -> TcpStream {
    let address = server.base_url.strip_prefix("http://").unwrap();
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket.write_all(format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",body.len()).as_bytes()).await.unwrap();
    let mut interim = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !interim.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).await.unwrap();
            interim.push(byte[0]);
            assert!(interim.len() < 16 * 1024, "oversized interim response");
        }
    })
    .await
    .expect("server must reach the body read");
    let interim = String::from_utf8(interim).unwrap();
    assert!(interim.starts_with("HTTP/1.1 100"), "{interim}");
    socket.write_all(&body[..body.len() / 2]).await.unwrap();
    socket
}

async fn finish_body(mut socket: TcpStream, body: &[u8]) -> (u16, Value, String) {
    socket.write_all(&body[body.len() / 2..]).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        socket.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    let response = String::from_utf8(response).unwrap();
    let (headers, text) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("invalid HTTP response: {response:?}"));
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    let value = serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("non-JSON response {response:?}: {error}"));
    (status, value, response)
}

async fn revoke(server: &TestServer, token: &str) {
    let session: Value = server
        .client
        .get(server.url("/api/v1/sessions/current"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = session["data"]["id"].as_str().unwrap();
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/sessions/{id}")))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

fn multipart(field: &str) -> Vec<u8> {
    format!("--intake\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"safe.txt\"\r\nContent-Type: text/plain\r\n\r\ndata\r\n--intake--\r\n").into_bytes()
}

#[derive(diesel::QueryableByName)]
struct Count {
    #[diesel(sql_type=BigInt)]
    value: i64,
}

async fn count(server: &TestServer, sql: String) -> i64 {
    server
        .db
        .run(move |conn| diesel::sql_query(sql).get_result::<Count>(conn))
        .await
        .unwrap()
        .value
}

#[tokio::test]
async fn a_key_downgraded_during_a_json_body_cannot_commit_document_content() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("slow_json", "slow_json@test.invalid", "Password123!")
        .await;
    let document = create(
        &server,
        &jwt,
        "/api/v1/documents",
        json!({"name":"original","document_type":"writer"}),
    )
    .await;
    let key = create(
        &server,
        &jwt,
        "/api/v1/api-keys",
        json!({"name":"writer","permissions":"documents:write"}),
    )
    .await;
    let id = document["id"].as_str().unwrap();
    let mut content = document["content"].clone();
    content["title"] = json!("must not commit");
    let body = serde_json::to_vec(&json!({"content":content,"expected_version":1})).unwrap();
    let before = count(
        &server,
        "SELECT SUM(storage_used) AS value FROM users".into(),
    )
    .await;
    let socket = begin_body(
        &server,
        "PUT",
        &format!("/api/v1/documents/{id}"),
        key["key"].as_str().unwrap(),
        "application/json",
        &body,
    )
    .await;
    let key_id = key["id"].as_str().unwrap().to_owned();
    server
        .db
        .run(move |conn| {
            diesel::sql_query("UPDATE api_keys SET permissions='documents:read' WHERE id=?")
                .bind::<Text, _>(key_id)
                .execute(conn)
        })
        .await
        .unwrap();
    let (status, error, wire) = finish_body(socket, &body).await;
    assert_eq!(status, 403, "{wire}");
    assert_eq!(error["success"], false);
    let current: Value = server
        .client
        .get(server.url(&format!("/api/v1/documents/{id}")))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(current["data"]["version"], 1);
    assert_eq!(current["data"]["content"], document["content"]);
    assert_eq!(
        count(&server, "SELECT COUNT(*) AS value FROM versions".into()).await,
        0
    );
    assert_eq!(
        count(
            &server,
            "SELECT SUM(storage_used) AS value FROM users".into()
        )
        .await,
        before
    );
}

#[tokio::test]
async fn jwt_and_api_key_revocation_during_chunk_intake_cannot_publish_a_chunk() {
    for use_key in [false, true] {
        let server = TestServer::new().await;
        let (jwt, _) = server
            .register("slow_chunk", "slow_chunk@test.invalid", "Password123!")
            .await;
        let session = create(
            &server,
            &jwt,
            "/api/v1/upload/sessions",
            json!({"file_name":"safe.txt","total_size":4}),
        )
        .await;
        let key = if use_key {
            Some(
                create(
                    &server,
                    &jwt,
                    "/api/v1/api-keys",
                    json!({"name":"chunk","permissions":"upload:write"}),
                )
                .await,
            )
        } else {
            None
        };
        let bearer = key
            .as_ref()
            .map(|key| key["key"].as_str().unwrap())
            .unwrap_or(&jwt);
        let body = multipart("chunk");
        let socket = begin_body(
            &server,
            "POST",
            &format!(
                "/api/v1/upload/sessions/{}/chunks?chunk_index=1",
                session["id"].as_str().unwrap()
            ),
            bearer,
            "multipart/form-data; boundary=intake",
            &body,
        )
        .await;
        if let Some(key) = &key {
            let response = server
                .client
                .delete(server.url(&format!("/api/v1/api-keys/{}", key["id"].as_str().unwrap())))
                .bearer_auth(&jwt)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
        } else {
            revoke(&server, &jwt).await;
        }
        let (status, error, wire) = finish_body(socket, &body).await;
        assert_eq!(status, 401, "{wire}");
        assert_eq!(error["success"], false);
        assert_eq!(
            count(
                &server,
                "SELECT COUNT(*) AS value FROM upload_chunks".into()
            )
            .await,
            0
        );
        assert_eq!(
            count(
                &server,
                "SELECT SUM(uploaded_size) AS value FROM upload_sessions".into()
            )
            .await,
            0
        );
        assert_eq!(
            count(&server, "SELECT COUNT(*) AS value FROM files".into()).await,
            0
        );
        assert_eq!(
            count(
                &server,
                "SELECT SUM(storage_used) AS value FROM users".into()
            )
            .await,
            0
        );
    }
}

#[tokio::test]
async fn revoked_jwt_during_ordinary_upload_cannot_create_a_file_or_charge_quota() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("slow_file", "slow_file@test.invalid", "Password123!")
        .await;
    let body = multipart("file");
    let socket = begin_body(
        &server,
        "POST",
        "/api/v1/files/upload",
        &jwt,
        "multipart/form-data; boundary=intake",
        &body,
    )
    .await;
    revoke(&server, &jwt).await;
    let (status, error, wire) = finish_body(socket, &body).await;
    assert_eq!(status, 401, "{wire}");
    assert_eq!(error["success"], false);
    assert_eq!(
        count(&server, "SELECT COUNT(*) AS value FROM files".into()).await,
        0
    );
    assert_eq!(
        count(
            &server,
            "SELECT SUM(storage_used) AS value FROM users".into()
        )
        .await,
        0
    );
}

#[tokio::test]
async fn removed_room_uploader_cannot_replay_a_completion_receipt_for_current_metadata() {
    let server = TestServer::new().await;
    let (owner, _) = server
        .register("intake_owner", "intake_owner@test.invalid", "Password123!")
        .await;
    let (uploader, user) = server
        .register(
            "intake_member",
            "intake_member@test.invalid",
            "Password123!",
        )
        .await;
    let user = user["data"]["user"]["id"].as_str().unwrap();
    let room = create(
        &server,
        &owner,
        "/api/v1/rooms",
        json!({"name":"private room"}),
    )
    .await;
    let room = room["id"].as_str().unwrap();
    create(
        &server,
        &owner,
        &format!("/api/v1/rooms/{room}/members"),
        json!({"user_id":user,"role":"editor"}),
    )
    .await;
    let session = create(
        &server,
        &uploader,
        "/api/v1/upload/sessions",
        json!({"file_name":"safe.txt","total_size":4,"room_id":room}),
    )
    .await;
    let session = session["id"].as_str().unwrap();
    let chunk = server
        .client
        .post(server.url(&format!(
            "/api/v1/upload/sessions/{session}/chunks?chunk_index=1"
        )))
        .bearer_auth(&uploader)
        .multipart(
            reqwest::multipart::Form::new().part(
                "chunk",
                reqwest::multipart::Part::bytes(b"data".to_vec())
                    .file_name("safe.txt")
                    .mime_str("application/octet-stream")
                    .unwrap(),
            ),
        )
        .send()
        .await
        .unwrap();
    let chunk_status = chunk.status();
    let chunk_headers = chunk.headers().clone();
    let chunk_text = chunk.text().await.unwrap();
    assert_eq!(
        chunk_status, 200,
        "headers={chunk_headers:?}, body={chunk_text}"
    );
    let chunk_body: Value = serde_json::from_str(&chunk_text).unwrap();
    assert_eq!(chunk_body["success"], true);
    let completed = create(
        &server,
        &uploader,
        &format!("/api/v1/upload/sessions/{session}/complete"),
        json!({}),
    )
    .await;
    let file = completed["id"].as_str().unwrap();
    let removed = server
        .client
        .delete(server.url(&format!("/api/v1/rooms/{room}/members/{user}")))
        .bearer_auth(&owner)
        .send()
        .await
        .unwrap();
    assert_eq!(removed.status(), 200);
    let renamed = server
        .client
        .put(server.url(&format!("/api/v1/files/{file}")))
        .bearer_auth(&owner)
        .json(&json!({"name":"confidential current name"}))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200);
    let replay = server
        .client
        .post(server.url(&format!("/api/v1/upload/sessions/{session}/complete")))
        .bearer_auth(&uploader)
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 403);
    let text = replay.text().await.unwrap();
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["success"], false);
    assert!(!text.contains("confidential current name"));
}

#[tokio::test]
async fn committed_revocation_or_scope_downgrade_prevents_a_later_write_transaction() {
    use contrix_core::error::AppError;
    use contrix_server::middleware::auth::{
        AuthenticatedCredential, hash_token, validate_credential_in_transaction,
    };
    for use_key in [false, true] {
        let server = TestServer::new().await;
        let (jwt, registration) = server
            .register(
                "transaction_guard",
                "transaction_guard@test.invalid",
                "Password123!",
            )
            .await;
        let user = registration["data"]["user"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let key = if use_key {
            Some(
                create(
                    &server,
                    &jwt,
                    "/api/v1/api-keys",
                    json!({"name":"transaction","permissions":"files:write"}),
                )
                .await,
            )
        } else {
            None
        };
        let credential = AuthenticatedCredential {
            user_id: user.clone(),
            token_hash: hash_token(
                key.as_ref()
                    .map(|key| key["key"].as_str().unwrap())
                    .unwrap_or(&jwt),
            ),
            api_key_id: key
                .as_ref()
                .map(|key| key["id"].as_str().unwrap().to_owned()),
            expires_at: None,
        };
        let original = credential.clone();
        server
            .db
            .run(move |conn| {
                conn.immediate_transaction::<_, AppError, _>(|conn| {
                    validate_credential_in_transaction(
                        conn,
                        &original,
                        &salvo::http::Method::POST,
                        "/api/v1/files/upload",
                    )
                })
            })
            .await
            .expect("credential is initially allowed to mutate");
        if let Some(key) = &key {
            let id = key["id"].as_str().unwrap().to_owned();
            server
                .db
                .run(move |conn| {
                    diesel::sql_query("UPDATE api_keys SET permissions='files:read' WHERE id=?")
                        .bind::<Text, _>(id)
                        .execute(conn)
                })
                .await
                .unwrap();
        } else {
            revoke(&server, &jwt).await;
        }
        // The revocation/downgrade transaction above has committed before this
        // write transaction begins. No async credential precheck is involved.
        let result = server
            .db
            .run(move |conn| {
                conn.immediate_transaction::<_, AppError, _>(|conn| {
                    validate_credential_in_transaction(
                        conn,
                        &credential,
                        &salvo::http::Method::POST,
                        "/api/v1/files/upload",
                    )?;
                    diesel::sql_query("UPDATE users SET storage_used=storage_used+1 WHERE id=?")
                        .bind::<Text, _>(user)
                        .execute(conn)?;
                    Ok(())
                })
            })
            .await;
        if use_key {
            assert!(matches!(result, Err(AppError::Forbidden(_))), "{result:?}");
        } else {
            assert!(matches!(result, Err(AppError::Auth(_))), "{result:?}");
        }
        assert_eq!(
            count(
                &server,
                "SELECT SUM(storage_used) AS value FROM users".into()
            )
            .await,
            0
        );
    }
}
