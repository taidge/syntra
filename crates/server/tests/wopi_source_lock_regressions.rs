#[allow(dead_code)]
mod common;

use common::TestServer;
use diesel::prelude::*;
use serde_json::{Value, json};
use syntra_core::models::{Document, File, FileLock, Version};
use syntra_core::schema::{documents, file_locks, files, users, versions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn create(server: &TestServer, jwt: &str, path: &str, body: Value) -> Value {
    let response = server
        .client
        .post(server.url(path))
        .bearer_auth(jwt)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 201, "{path}: {body}");
    body["data"].clone()
}

async fn mint(server: &TestServer, jwt: &str, file: &str) -> String {
    let response = server
        .client
        .get(server.url("/api/v1/wopi/host/writer/edit"))
        .query(&[
            ("access_token", jwt),
            (
                "WOPISrc",
                &server.url(&format!("/api/v1/wopi/files/{file}")),
            ),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let html = response.text().await.unwrap();
    let start = html.find("const wopiSessionToken = ").unwrap() + "const wopiSessionToken = ".len();
    serde_json::from_str(html[start..].split(';').next().unwrap()).unwrap()
}

async fn snapshot(server: &TestServer) -> Value {
    server.db.run(|conn| conn.transaction::<_, syntra_core::error::AppError, _>(|conn| Ok(json!({
        "files": files::table.order(files::id).load::<File>(conn)?,
        "documents": documents::table.order(documents::id).load::<Document>(conn)?,
        "versions": versions::table.order(versions::id).load::<Version>(conn)?,
        "locks": file_locks::table.order(file_locks::id).load::<FileLock>(conn)?,
        "quota": users::table.order(users::id).select((users::id, users::storage_used)).load::<(String,i64)>(conn)?,
    })))).await.unwrap()
}

#[tokio::test]
async fn put_relative_cannot_overwrite_after_another_editor_locks_its_source() {
    changed_source_cannot_overwrite(true).await;
}

#[tokio::test]
async fn put_relative_cannot_overwrite_an_old_sibling_after_its_source_moves() {
    changed_source_cannot_overwrite(false).await;
}

async fn changed_source_cannot_overwrite(lock_source: bool) {
    for linked_target in [true, false] {
        let server = TestServer::new().await;
        let (jwt, _) = server
            .register("sourcelockowner", "sourcelockowner@test.com", "password123")
            .await;
        let (editor, registration) = server
            .register(
                "sourcelockeditor",
                "sourcelockeditor@test.com",
                "password123",
            )
            .await;
        let room = create(
            &server,
            &jwt,
            "/api/v1/rooms",
            json!({"name":"source lock team"}),
        )
        .await;
        let room = room["id"].as_str().unwrap();
        create(
            &server,
            &jwt,
            &format!("/api/v1/rooms/{room}/members"),
            json!({"user_id":registration["data"]["user"]["id"],"role":"editor"}),
        )
        .await;
        let source = create(
            &server,
            &jwt,
            "/api/v1/documents",
            json!({"name":"source.txt","document_type":"writer","room_id":room}),
        )
        .await;
        let source = source["file_id"].as_str().unwrap();
        let target = if linked_target {
            create(
                &server,
                &jwt,
                "/api/v1/documents",
                json!({"name":"target.txt","document_type":"writer","room_id":room}),
            )
            .await["file_id"]
                .as_str()
                .unwrap()
                .to_owned()
        } else {
            let response = server
                .client
                .post(server.url("/api/v1/files/upload"))
                .bearer_auth(&jwt)
                .multipart(
                    reqwest::multipart::Form::new()
                        .text("room_id", room.to_owned())
                        .part(
                            "file",
                            reqwest::multipart::Part::bytes(b"original target".to_vec())
                                .file_name("target.txt")
                                .mime_str("text/plain")
                                .unwrap(),
                        ),
                )
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 201);
            response.json::<Value>().await.unwrap()["data"]["id"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let token = mint(&server, &jwt, source).await;
        let editor_token = mint(&server, &editor, source).await;
        let address = server.base_url.strip_prefix("http://").unwrap();
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let body = br#"{"type":"writer","children":[],"marker":"blocked relative write"}"#;
        socket.write_all(format!("POST /api/v1/wopi/files/{source}?access_token={token} HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\nX-WOPI-Override: PUT_RELATIVE\r\nX-WOPI-RelativeTarget: target.txt\r\nX-WOPI-OverwriteRelativeTarget: true\r\n\r\n",body.len()).as_bytes()).await.unwrap();
        let mut interim = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !interim.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).await.unwrap();
                interim.push(byte[0]);
                assert!(interim.len() < 16 * 1024);
            }
        })
        .await
        .unwrap();
        assert!(
            String::from_utf8(interim)
                .unwrap()
                .starts_with("HTTP/1.1 100")
        );
        socket.write_all(&body[..body.len() / 2]).await.unwrap();

        // The real mutation completes after the initial source checks and
        // before this blocked relative request can reach its final commit.
        if lock_source {
            let lock = server
                .client
                .post(server.url(&format!("/api/v1/wopi/files/{source}")))
                .query(&[("access_token", &editor_token)])
                .header("X-WOPI-Override", "LOCK")
                .header("X-WOPI-Lock", "another-editor-source-lock")
                .send()
                .await
                .unwrap();
            assert_eq!(lock.status(), 200);
        } else {
            let destination = create(
                &server,
                &jwt,
                "/api/v1/folders",
                json!({"name":"moved source", "room_id":room}),
            )
            .await;
            let moved = server
                .client
                .post(server.url(&format!("/api/v1/files/{source}/move")))
                .bearer_auth(&jwt)
                .json(&json!({"target_folder_id":destination["id"]}))
                .send()
                .await
                .unwrap();
            assert_eq!(moved.status(), 200);
        }
        let before = snapshot(&server).await;
        assert_eq!(
            before["locks"].as_array().unwrap().len(),
            usize::from(lock_source)
        );
        if lock_source {
            assert_eq!(before["locks"][0]["file_id"], source);
        }
        let target_path = before["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["id"] == target)
            .unwrap()["path"]
            .as_str()
            .unwrap();
        let target_bytes = server.state.storage.read_file(target_path).await.ok();
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
        assert_eq!(
            response.split_whitespace().nth(1),
            Some("409"),
            "linked={linked_target}: {response}"
        );
        assert_eq!(snapshot(&server).await, before);
        if let Some(bytes) = target_bytes {
            assert_eq!(
                server.state.storage.read_file(target_path).await.unwrap(),
                bytes
            );
        }
    }
}
