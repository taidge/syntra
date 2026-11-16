#[allow(dead_code)]
mod common;

use common::TestServer;
use contrix_core::models::{Document, File, FileLock, Version};
use contrix_core::schema::{documents, file_locks, files, sessions, users, versions};
use diesel::prelude::*;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn wopi_token(server: &TestServer, jwt: &str, file: &str) -> String {
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

async fn upload(server: &TestServer, jwt: &str, name: &str) -> String {
    let response = server
        .client
        .post(server.url("/api/v1/files/upload"))
        .bearer_auth(jwt)
        .multipart(
            reqwest::multipart::Form::new().part(
                "file",
                reqwest::multipart::Part::bytes(b"original WOPI bytes".to_vec())
                    .file_name(name.to_owned())
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
}

/// Hyper sends 100 only when the handler actually polls the body, after the
/// WOPI token, source session, file ACL and initial lock checks have completed.
async fn begin_body(server: &TestServer, path: &str, extra: &str, body: &[u8]) -> TcpStream {
    let address = server.base_url.strip_prefix("http://").unwrap();
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n{extra}\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
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
    .expect("WOPI handler did not reach its body read");
    let interim = String::from_utf8(interim).unwrap();
    assert!(interim.starts_with("HTTP/1.1 100"), "{interim}");
    socket.write_all(&body[..body.len() / 2]).await.unwrap();
    socket
}

async fn finish_body(mut socket: TcpStream, body: &[u8]) -> (u16, String) {
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
    let status = response.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, response)
}

async fn snapshot(server: &TestServer) -> Value {
    server
        .db
        .run(|conn| {
            conn.transaction::<_, contrix_core::error::AppError, _>(|conn| {
                Ok(json!({
                    "files": files::table.order(files::id).load::<File>(conn)?,
                    "documents": documents::table.order(documents::id).load::<Document>(conn)?,
                    "versions": versions::table.order(versions::id).load::<Version>(conn)?,
                    "locks": file_locks::table.order(file_locks::id).load::<FileLock>(conn)?,
                    "quota": users::table.order(users::id).select((users::id, users::storage_used)).load::<(String, i64)>(conn)?,
                }))
            })
        })
        .await
        .unwrap()
}

async fn revoke_source(server: &TestServer, user: String, kind: &'static str) {
    server
        .db
        .run(move |conn| {
            match kind {
                "revoked" => {
                    diesel::update(sessions::table.filter(sessions::user_id.eq(&user)))
                        .set(sessions::is_active.eq(false))
                        .execute(conn)?;
                }
                "disabled" => {
                    diesel::update(users::table.find(&user))
                        .set(users::role.eq("disabled"))
                        .execute(conn)?;
                }
                "expired" => {
                    diesel::update(sessions::table.filter(sessions::user_id.eq(&user)))
                        .set(sessions::expires_at.eq("2000-01-01T00:00:00Z"))
                        .execute(conn)?;
                }
                _ => unreachable!(),
            }
            Ok::<_, diesel::result::Error>(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn source_changed_during_wopi_contents_body_cannot_commit_linked_or_plain_files() {
    for linked in [true, false] {
        for kind in ["revoked", "disabled", "expired"] {
            let server = TestServer::new().await;
            let (jwt, registration) = server
                .register("wopiintake", "wopiintake@test.com", "password123")
                .await;
            let user = registration["data"]["user"]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let file = if linked {
                server
                    .create_document(&jwt, "linked", "writer", None)
                    .await
                    .1
            } else {
                upload(&server, &jwt, "plain.txt").await
            };
            let token = wopi_token(&server, &jwt, &file).await;
            let path = format!("/api/v1/wopi/files/{file}/contents?access_token={token}");
            let before = snapshot(&server).await;
            let original_path = before["files"][0]["path"].as_str().unwrap();
            let original_bytes = server.state.storage.read_file(original_path).await.ok();
            let body = if linked {
                br#"{"type":"writer","children":[],"marker":"must not commit"}"#.to_vec()
            } else {
                b"revoked source must not replace bytes".to_vec()
            };
            let socket = begin_body(&server, &path, "", &body).await;
            revoke_source(&server, user, kind).await;
            let (status, response) = finish_body(socket, &body).await;
            assert_eq!(status, 401, "linked={linked} kind={kind}: {response}");
            assert_eq!(snapshot(&server).await, before);
            if let Some(original) = original_bytes {
                assert_eq!(
                    server.state.storage.read_file(original_path).await.unwrap(),
                    original
                );
            }
        }
    }
}

#[tokio::test]
async fn revoked_source_during_put_relative_cannot_create_or_overwrite_siblings() {
    for overwrite in [false, true] {
        let server = TestServer::new().await;
        let (jwt, registration) = server
            .register("wopirelative", "wopirelative@test.com", "password123")
            .await;
        let user = registration["data"]["user"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let file = upload(&server, &jwt, "source.txt").await;
        if overwrite {
            upload(&server, &jwt, "target.txt").await;
        }
        let token = wopi_token(&server, &jwt, &file).await;
        let before = snapshot(&server).await;
        let path = format!("/api/v1/wopi/files/{file}?access_token={token}");
        let body = b"revoked relative bytes";
        let extra = format!(
            "X-WOPI-Override: PUT_RELATIVE\r\nX-WOPI-RelativeTarget: target.txt\r\nX-WOPI-OverwriteRelativeTarget: {overwrite}\r\n"
        );
        let socket = begin_body(&server, &path, &extra, body).await;
        revoke_source(&server, user, "revoked").await;
        let (status, response) = finish_body(socket, body).await;
        assert_eq!(status, 401, "overwrite={overwrite}: {response}");
        assert_eq!(snapshot(&server).await, before);
        for item in before["files"].as_array().unwrap() {
            assert_eq!(
                server
                    .state
                    .storage
                    .read_file(item["path"].as_str().unwrap())
                    .await
                    .unwrap(),
                b"original WOPI bytes"
            );
        }
    }
}

#[tokio::test]
async fn put_relative_rechecks_its_bound_source_even_when_the_destination_is_writable() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("wopisource", "wopisource@test.com", "password123")
        .await;
    let source = upload(&server, &jwt, "source.txt").await;
    let target = upload(&server, &jwt, "target.txt").await;
    let token = wopi_token(&server, &jwt, &source).await;
    let path = format!("/api/v1/wopi/files/{source}?access_token={token}");
    let body = b"must not overwrite a writable target via a trashed source";
    let extra = "X-WOPI-Override: PUT_RELATIVE\r\nX-WOPI-RelativeTarget: target.txt\r\nX-WOPI-OverwriteRelativeTarget: true\r\n";
    let socket = begin_body(&server, &path, extra, body).await;
    server
        .db
        .run(move |conn| {
            diesel::update(files::table.find(source))
                .set((
                    files::is_trashed.eq(true),
                    files::trashed_at.eq(Some(chrono::Utc::now().to_rfc3339())),
                ))
                .execute(conn)
        })
        .await
        .unwrap();
    let before = snapshot(&server).await;
    let (status, response) = finish_body(socket, body).await;
    assert_eq!(status, 404, "{response}");
    assert_eq!(snapshot(&server).await, before);
    let target_path = before["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["id"] == target)
        .unwrap()["path"]
        .as_str()
        .unwrap();
    assert_eq!(
        server.state.storage.read_file(target_path).await.unwrap(),
        b"original WOPI bytes"
    );
}
