#[allow(dead_code)]
mod common;

use common::TestServer;
use serde_json::{Value, json};

const PDF: &[u8] = include_bytes!("../../export/tests/fixtures/pdf-selectable-text.pdf");

async fn upload(server: &TestServer, token: &str, parent: Option<&str>) -> String {
    let part = reqwest::multipart::Part::bytes(PDF.to_vec())
        .file_name("native.pdf")
        .mime_str("application/pdf")
        .unwrap();
    let mut form = reqwest::multipart::Form::new().part("file", part);
    if let Some(parent) = parent {
        form = form.text("parent_id", parent.to_owned());
    }
    let response = server
        .client
        .post(server.url("/api/v1/files/upload"))
        .headers(server.auth_header(token))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let body: Value = response.json().await.unwrap();
    assert!(body["data"]["document_id"].is_null());
    body["data"]["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn pdf_saves_use_content_cas_keep_original_history_and_convert_without_mutating_source() {
    let server = TestServer::new().await;
    let (token, _) = server
        .register("pdf_owner", "pdf_owner@regression.test", "Password!12345")
        .await;
    let (outsider, _) = server
        .register("pdf_other", "pdf_other@regression.test", "Password!12345")
        .await;
    let id = upload(&server, &token, None).await;
    let path = format!("/api/v1/files/{id}/pdf");
    let response = server
        .client
        .get(server.url(&path))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/pdf");
    assert_eq!(response.headers()["x-pdf-can-edit"], "true");
    let original_etag = response.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(response.bytes().await.unwrap().as_ref(), PDF);
    let denied = server
        .client
        .get(server.url(&path))
        .headers(server.auth_header(&outsider))
        .send()
        .await
        .unwrap();
    assert!(!denied.status().is_success());
    let invalid = server
        .client
        .put(server.url(&path))
        .headers(server.auth_header(&token))
        .header("if-match", &original_etag)
        .header("content-type", "application/pdf")
        .body("invalid PDF")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 400);
    let edited = String::from_utf8(PDF.to_vec())
        .unwrap()
        .replace("Original", "Modified")
        .into_bytes();
    assert_eq!(edited.len(), PDF.len(), "fixture offsets remain valid");
    let saved = server
        .client
        .put(server.url(&path))
        .headers(server.auth_header(&token))
        .header("if-match", &original_etag)
        .header("content-type", "application/pdf")
        .body(edited.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    let saved: Value = saved.json().await.unwrap();
    let edited_etag = saved["data"]["etag"].as_str().unwrap();
    assert_ne!(edited_etag, original_etag);
    assert_eq!(saved["data"]["version_number"], 1);
    let conflict = server
        .client
        .put(server.url(&path))
        .headers(server.auth_header(&token))
        .header("if-match", &original_etag)
        .header("content-type", "application/pdf")
        .body(PDF.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), 409);
    let original = server
        .client
        .get(server.url(&format!("{path}?version=0")))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(original.status(), 200);
    assert_eq!(original.bytes().await.unwrap().as_ref(), PDF);
    let current = server
        .client
        .get(server.url(&path))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(current.bytes().await.unwrap().as_ref(), edited.as_slice());
    let restored = server
        .client
        .post(server.url(&format!("{path}/restore")))
        .headers(server.auth_header(&token))
        .json(&json!({"version_number":0,"expected_etag":edited_etag}))
        .send()
        .await
        .unwrap();
    assert_eq!(restored.status(), 200);
    let restored: Value = restored.json().await.unwrap();
    assert_eq!(restored["data"]["version_number"], 2);
    assert_eq!(restored["data"]["etag"], original_etag);
    let historical = server
        .client
        .get(server.url(&format!("{path}?version=1")))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(
        historical.bytes().await.unwrap().as_ref(),
        edited.as_slice()
    );
    let converted = server
        .client
        .post(server.url(&format!("{path}/convert")))
        .headers(server.auth_header(&token))
        .json(&json!({"expected_etag":original_etag,"name":"Editable PDF text"}))
        .send()
        .await
        .unwrap();
    assert_eq!(converted.status(), 201);
    let converted: Value = converted.json().await.unwrap();
    assert_eq!(converted["data"]["page_count"], 1);
    let document_id = converted["data"]["document_id"].as_str().unwrap();
    let document = server
        .client
        .get(server.url(&format!("/api/v1/documents/{document_id}")))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    let document: Value = document.json().await.unwrap();
    assert!(
        document["data"]["content"]["body"]
            .to_string()
            .contains("Original selectable PDF text")
    );
    let ocr_invalid = server
        .client
        .post(server.url(&format!("{path}/convert")))
        .headers(server.auth_header(&token))
        .json(&json!({"expected_etag":original_etag,"ocr_pages":["one","extra page"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(ocr_invalid.status(), 400);
    let ocr = server
        .client
        .post(server.url(&format!("{path}/convert")))
        .headers(server.auth_header(&token))
        .json(&json!({"expected_etag":original_etag,"ocr_pages":["数学识别文字 1 + 1 = 2"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(ocr.status(), 201);
    let ocr: Value = ocr.json().await.unwrap();
    assert!(ocr["data"]["warnings"].to_string().contains("OCR"));
    let current = server
        .client
        .get(server.url(&path))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(current.headers()["etag"].to_str().unwrap(), original_etag);
    assert_eq!(current.headers()["x-pdf-version"], "2");
    assert_eq!(current.bytes().await.unwrap().as_ref(), PDF);
    let trash = server
        .client
        .delete(server.url(&format!("/api/v1/files/{id}")))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(trash.status(), 200);
    let missing = server
        .client
        .get(server.url(&path))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn room_viewers_can_read_pdf_but_cannot_replace_or_restore_its_bytes() {
    let server = TestServer::new().await;
    let (owner, _) = server
        .register(
            "pdf_room_owner",
            "pdf_room_owner@regression.test",
            "Password!12345",
        )
        .await;
    let (viewer, viewer_user) = server
        .register(
            "pdf_room_viewer",
            "pdf_room_viewer@regression.test",
            "Password!12345",
        )
        .await;
    let room = server
        .client
        .post(server.url("/api/v1/rooms"))
        .headers(server.auth_header(&owner))
        .json(&json!({"name":"PDF room"}))
        .send()
        .await
        .unwrap();
    assert_eq!(room.status(), 201);
    let room: Value = room.json().await.unwrap();
    let room_id = room["data"]["id"].as_str().unwrap();
    let member = server
        .client
        .post(server.url(&format!("/api/v1/rooms/{room_id}/members")))
        .headers(server.auth_header(&owner))
        .json(&json!({"user_id":viewer_user["data"]["user"]["id"],"role":"viewer"}))
        .send()
        .await
        .unwrap();
    assert_eq!(member.status(), 201);
    let folder = server
        .client
        .post(server.url("/api/v1/folders"))
        .headers(server.auth_header(&owner))
        .json(&json!({"name":"PDFs","room_id":room_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(folder.status(), 201);
    let folder: Value = folder.json().await.unwrap();
    let id = upload(&server, &owner, folder["data"]["id"].as_str()).await;
    let path = format!("/api/v1/files/{id}/pdf");
    let preview = server
        .client
        .get(server.url(&path))
        .headers(server.auth_header(&viewer))
        .send()
        .await
        .unwrap();
    assert_eq!(preview.status(), 200);
    assert_eq!(preview.headers()["x-pdf-can-edit"], "false");
    let etag = preview.headers()["etag"].to_str().unwrap().to_owned();
    let denied = server
        .client
        .put(server.url(&path))
        .headers(server.auth_header(&viewer))
        .header("if-match", &etag)
        .header("content-type", "application/pdf")
        .body(PDF.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    let denied = server
        .client
        .post(server.url(&format!("{path}/restore")))
        .headers(server.auth_header(&viewer))
        .json(&json!({"expected_etag":etag,"version_number":0}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
}
