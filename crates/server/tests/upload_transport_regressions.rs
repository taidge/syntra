//! Exercise real HTTP framing and realistic document/chunk sizes.
#[allow(dead_code)]
mod common;

use common::TestServer;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn json_response(response: reqwest::Response, status: u16) -> Value {
    let actual = response.status().as_u16();
    let text = response.text().await.unwrap();
    assert_eq!(
        actual,
        status,
        "unexpected HTTP status; response bytes={}",
        text.len()
    );
    serde_json::from_str(&text).unwrap()
}

async fn get_document(server: &TestServer, token: &str, id: &str) -> Value {
    json_response(
        server
            .client
            .get(server.url(&format!("/api/v1/documents/{id}")))
            .headers(server.auth_header(token))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await["data"]
        .clone()
}

async fn inventory_count(server: &TestServer) -> i64 {
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }
    server
        .db
        .run(|conn| {
            diesel::RunQueryDsl::get_result::<Count>(
                diesel::sql_query("SELECT COUNT(*) AS count FROM blob_inventory"),
                conn,
            )
        })
        .await
        .unwrap()
        .count
}

async fn raw_chunked(
    server: &TestServer,
    token: &str,
    path: &str,
    content_type: &str,
    prefix: &[u8],
    data_size: usize,
    finish: bool,
) -> String {
    let address = server.base_url.strip_prefix("http://").unwrap();
    let socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let (mut reader, mut writer) = socket.into_split();
    let headers = format!(
        "POST {path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    let prefix = prefix.to_vec();
    let (finished, hold) = tokio::sync::oneshot::channel::<()>();
    let sending = tokio::spawn(async move {
        writer.write_all(headers.as_bytes()).await?;
        if !prefix.is_empty() {
            writer
                .write_all(format!("{:x}\r\n", prefix.len()).as_bytes())
                .await?;
            writer.write_all(&prefix).await?;
            writer.write_all(b"\r\n").await?;
        }
        let chunk = [b'z'; 16 * 1024];
        let mut remaining = data_size;
        while remaining > 0 {
            let count = remaining.min(chunk.len());
            writer
                .write_all(format!("{count:x}\r\n").as_bytes())
                .await?;
            writer.write_all(&chunk[..count]).await?;
            writer.write_all(b"\r\n").await?;
            remaining -= count;
        }
        if finish {
            // The multipart boundary is part of the decoded body.
            let suffix = b"\r\n--transport--\r\n";
            writer
                .write_all(format!("{:x}\r\n", suffix.len()).as_bytes())
                .await?;
            writer.write_all(suffix).await?;
            writer.write_all(b"\r\n0\r\n\r\n").await?;
        }
        // A normal HTTP client keeps its write half open while waiting for
        // the response. Preserve that both for complete and stalled bodies;
        // an accidental TCP FIN can make Hyper cancel the handler itself.
        let _ = hold.await;
        Ok::<_, std::io::Error>(())
    });
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(8),
        reader.read_to_end(&mut response),
    )
    .await
    .expect("body framing response exceeded its deadline")
    .unwrap();
    let _ = finished.send(());
    sending
        .await
        .unwrap()
        .expect("upload sender failed before its response");
    String::from_utf8(response).unwrap()
}

fn raw_json(response: &str, status: u16) -> Value {
    let (headers, body) = response.split_once("\r\n\r\n").unwrap_or_else(|| {
        panic!(
            "missing HTTP response headers for expected {status}; response bytes={}",
            response.len()
        )
    });
    assert!(
        headers.starts_with(&format!("HTTP/1.1 {status}")),
        "{headers}"
    );
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    serde_json::from_str(body).expect("complete JSON response body")
}

fn multipart_prefix(field: &str) -> String {
    format!(
        "--transport\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"transport.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
}

#[tokio::test]
async fn unknown_length_multipart_accepts_valid_data_and_returns_json_after_exceeding_limit() {
    let server = TestServer::new().await;
    let (token, _) = server
        .register("streamed", "streamed@review.test", "password123")
        .await;
    let prefix = multipart_prefix("file");
    let small = raw_chunked(
        &server,
        &token,
        "/api/v1/files/upload",
        "multipart/form-data; boundary=transport",
        prefix.as_bytes(),
        1024,
        true,
    )
    .await;
    let file = raw_json(&small, 201);
    let id = file["data"]["id"].as_str().unwrap();
    let downloaded = server
        .client
        .get(server.url(&format!("/api/v1/files/{id}/download")))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert!(downloaded.iter().all(|byte| *byte == b'z') && downloaded.len() == 1024);

    let too_large = raw_chunked(
        &server,
        &token,
        "/api/v1/files/upload",
        "multipart/form-data; boundary=transport",
        prefix.as_bytes(),
        11 * 1024 * 1024,
        true,
    )
    .await;
    let rejected = raw_json(&too_large, 413);
    assert_eq!(rejected["error"]["code"], 413);
    let files = json_response(
        server
            .client
            .get(server.url("/api/v1/files"))
            .headers(server.auth_header(&token))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(files["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_length_oversized_body_can_stall_without_holding_rejection_forever() {
    let server = TestServer::new().await;
    let (token, _) = server
        .register("stalled", "stalled@review.test", "password123")
        .await;
    let response = raw_chunked(
        &server,
        &token,
        "/api/v1/files/upload",
        "multipart/form-data; boundary=transport",
        multipart_prefix("file").as_bytes(),
        10 * 1024 * 1024 + 128 * 1024,
        false,
    )
    .await;
    assert_eq!(raw_json(&response, 413)["error"]["code"], 413);
}

#[tokio::test]
async fn an_incomplete_small_multipart_body_expires_with_408_and_no_file() {
    let server = TestServer::new_with_body_timeouts(1, 3).await;
    let (token, _) = server
        .register("idle", "idle@review.test", "password123")
        .await;
    let response = raw_chunked(
        &server,
        &token,
        "/api/v1/files/upload",
        "multipart/form-data; boundary=transport",
        multipart_prefix("file").as_bytes(),
        16,
        false,
    )
    .await;
    assert_eq!(raw_json(&response, 408)["error"]["code"], 408);
    let files = json_response(
        server
            .client
            .get(server.url("/api/v1/files"))
            .headers(server.auth_header(&token))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert!(files["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn multi_megabyte_document_saves_downloads_and_rejects_oversized_content_atomically() {
    let server = TestServer::new().await;
    let (token, _) = server
        .register("large_doc", "large_doc@review.test", "password123")
        .await;
    let (id, file, created) = server
        .create_document(&token, "Large document", "writer", None)
        .await;
    let version = created["data"]["version"].as_i64().unwrap();
    let content = syntra_doc_engine::writer::writer_content_with_text(
        &created["data"]["content"],
        &"x".repeat(2 * 1024 * 1024),
    );
    let saved = json_response(
        server
            .client
            .put(server.url(&format!("/api/v1/documents/{id}")))
            .headers(server.auth_header(&token))
            .json(&json!({"expected_version":version,"content":content}))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    let current_version = saved["data"]["version"].as_i64().unwrap();
    assert_eq!(current_version, version + 1);
    let persisted = get_document(&server, &token, &id).await;
    assert!(
        persisted["content"] == content,
        "large native content changed during save"
    );
    let downloaded: Value = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/download")))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        downloaded == content,
        "download differs from the committed document"
    );

    let excessive =
        json!({"padding":"x".repeat(syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES + 1)});
    let rejected = json_response(
        server
            .client
            .put(server.url(&format!("/api/v1/documents/{id}")))
            .headers(server.auth_header(&token))
            .json(&json!({"expected_version":current_version,"content":excessive}))
            .send()
            .await
            .unwrap(),
        413,
    )
    .await;
    assert_eq!(rejected["error"]["code"], 413);
    let after = get_document(&server, &token, &id).await;
    assert_eq!(after["version"], current_version);
    assert!(
        after["content"] == content,
        "rejected oversized save changed content"
    );
}

#[tokio::test]
async fn document_body_idle_timeout_does_not_change_the_document() {
    let server = TestServer::new_with_body_timeouts(1, 3).await;
    let (token, _) = server
        .register("idle_doc", "idle_doc@review.test", "password123")
        .await;
    let (id, _, created) = server
        .create_document(&token, "Idle document", "writer", None)
        .await;
    let address = server.base_url.strip_prefix("http://").unwrap();
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    let headers = format!(
        "PUT /api/v1/documents/{id} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 1024\r\n\r\n{{\"expected_version\":1,"
    );
    socket.write_all(headers.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        socket.read_to_end(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert_eq!(raw_json(&response, 408)["error"]["code"], 408);
    let after = get_document(&server, &token, &id).await;
    assert_eq!(after["version"], created["data"]["version"]);
    assert!(after["content"] == created["data"]["content"]);
}

#[tokio::test]
async fn realistic_megabyte_chunks_retry_and_complete_without_duplicate_billing() {
    let server = TestServer::new().await;
    let (token, registered) = server
        .register("chunks", "chunks@review.test", "password123")
        .await;
    let size = 1024 * 1024;
    let owner = registered["data"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .db
        .run(move |conn| {
            diesel::RunQueryDsl::execute(
                diesel::sql_query("UPDATE users SET storage_quota = ? WHERE id = ?")
                    .bind::<diesel::sql_types::BigInt, _>(size as i64)
                    .bind::<diesel::sql_types::Text, _>(owner),
                conn,
            )
        })
        .await
        .unwrap();
    let session = json_response(
        server
            .client
            .post(server.url("/api/v1/upload/sessions"))
            .headers(server.auth_header(&token))
            .json(&json!({"file_name":"chunk.bin","total_size":size}))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    assert_eq!(
        session["data"]["max_chunk_size"],
        syntra_core::dto::MAX_UPLOAD_CHUNK_BYTES
    );
    assert_eq!(
        session["data"]["max_chunk_count"],
        syntra_core::dto::MAX_UPLOAD_CHUNKS
    );
    let id = session["data"]["id"].as_str().unwrap();
    let bytes: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
    for _ in 0..2 {
        let part = reqwest::multipart::Part::bytes(bytes.clone())
            .file_name("chunk.bin")
            .mime_str("application/octet-stream")
            .unwrap();
        let chunk = json_response(
            server
                .client
                .post(server.url(&format!(
                    "/api/v1/upload/sessions/{id}/chunks?chunk_index=1"
                )))
                .headers(server.auth_header(&token))
                .multipart(reqwest::multipart::Form::new().part("chunk", part))
                .send()
                .await
                .unwrap(),
            200,
        )
        .await;
        assert_eq!(chunk["data"]["uploaded_size"], size);
        assert_eq!(
            inventory_count(&server).await,
            1,
            "a sequential retry must not stage another immutable object"
        );
    }
    let conflicting = reqwest::multipart::Part::bytes(vec![b'!'; size])
        .file_name("chunk.bin")
        .mime_str("application/octet-stream")
        .unwrap();
    json_response(
        server
            .client
            .post(server.url(&format!(
                "/api/v1/upload/sessions/{id}/chunks?chunk_index=1"
            )))
            .bearer_auth(&token)
            .multipart(reqwest::multipart::Form::new().part("chunk", conflicting))
            .send()
            .await
            .unwrap(),
        409,
    )
    .await;
    assert_eq!(
        inventory_count(&server).await,
        1,
        "a conflicting retry must not stage another immutable object"
    );
    let completed = json_response(
        server
            .client
            .post(server.url(&format!("/api/v1/upload/sessions/{id}/complete")))
            .headers(server.auth_header(&token))
            .send()
            .await
            .unwrap(),
        201,
    )
    .await;
    let again = json_response(
        server
            .client
            .post(server.url(&format!("/api/v1/upload/sessions/{id}/complete")))
            .headers(server.auth_header(&token))
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(completed["data"]["id"], again["data"]["id"]);
    let file = completed["data"]["id"].as_str().unwrap();
    let downloaded = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/download")))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert!(
        downloaded.as_ref() == bytes.as_slice(),
        "assembled chunk data changed"
    );
}

#[tokio::test]
async fn concurrent_session_reservations_limit_other_writes_and_cancellation_releases_capacity() {
    let server = TestServer::new().await;
    let (token, registered) = server
        .register("reserved", "reserved@review.test", "password123")
        .await;
    let owner = registered["data"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .db
        .run(move |conn| {
            diesel::RunQueryDsl::execute(
                diesel::sql_query("UPDATE users SET storage_quota = 100 WHERE id = ?")
                    .bind::<diesel::sql_types::Text, _>(owner),
                conn,
            )
        })
        .await
        .unwrap();
    let reserve = || {
        server
            .client
            .post(server.url("/api/v1/upload/sessions"))
            .bearer_auth(&token)
            .json(&json!({"file_name":"reserved.bin","total_size":60}))
            .send()
    };
    let (first, second) = tokio::join!(reserve(), reserve());
    let first = first.unwrap();
    let second = second.unwrap();
    let mut statuses = [first.status().as_u16(), second.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [201, 413]);
    let reserved = if first.status() == 201 { first } else { second };
    let reserved: Value = reserved.json().await.unwrap();
    let id = reserved["data"]["id"].as_str().unwrap();
    let upload = || {
        server
            .client
            .post(server.url("/api/v1/files/upload"))
            .bearer_auth(&token)
            .multipart(
                reqwest::multipart::Form::new().part(
                    "file",
                    reqwest::multipart::Part::bytes(vec![b'x'; 61])
                        .file_name("reserved.bin")
                        .mime_str("application/octet-stream")
                        .unwrap(),
                ),
            )
            .send()
    };
    json_response(upload().await.unwrap(), 413).await;
    json_response(
        server
            .client
            .delete(server.url(&format!("/api/v1/upload/sessions/{id}")))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    json_response(upload().await.unwrap(), 201).await;
    let quota = json_response(
        server
            .client
            .get(server.url("/api/v1/quota"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap(),
        200,
    )
    .await;
    assert_eq!(quota["data"]["storage_used"], 61);
}
