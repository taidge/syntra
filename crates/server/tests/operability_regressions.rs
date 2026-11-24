#[allow(dead_code)]
mod common;
use common::TestServer;
use serde_json::{Value, json};

#[tokio::test]
async fn readiness_detects_storage_failure_while_liveness_remains_available() {
    let server = TestServer::new().await;
    assert_eq!(
        server
            .client
            .get(server.url("/api/v1/ready"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let files = server._temp_dir.path().join("files");
    std::fs::remove_dir(&files).unwrap();
    std::fs::write(&files, b"not a directory").unwrap();
    assert_eq!(
        server
            .client
            .get(server.url("/api/v1/ready"))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    let health = server
        .client
        .get(server.url("/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200);
    let health: Value = health.json().await.unwrap();
    assert_eq!(health["data"]["status"], "ok");
}

#[tokio::test]
async fn failed_metrics_collection_is_unavailable_instead_of_zero() {
    let server = TestServer::new().await;
    let (token, user) = server
        .register("monitor", "monitor@review.test", "password123")
        .await;
    server
        .make_admin(user["data"]["user"]["id"].as_str().unwrap())
        .await;
    server
        .db
        .run(|conn| {
            diesel::RunQueryDsl::execute(diesel::sql_query("DROP TABLE conversion_jobs"), conn)?;
            Ok::<_, diesel::result::Error>(())
        })
        .await
        .unwrap();
    let response = server
        .client
        .get(server.url("/api/v1/admin/monitoring/metrics"))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert!(
        !response
            .text()
            .await
            .unwrap()
            .contains("syntra_conversion_jobs{")
    );
}

fn external_pptx() -> Vec<u8> {
    use std::io::{Cursor, Write};
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, xml) in [
        (
            "[Content_Types].xml",
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#,
        ),
        (
            "ppt/presentation.xml",
            r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#,
        ),
        (
            "ppt/_rels/presentation.xml.rels",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#,
        ),
        (
            "ppt/slides/slide1.xml",
            r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Original external slide</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#,
        ),
    ] {
        zip.start_file(path, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(xml.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[tokio::test]
async fn external_import_warns_and_retains_original_after_document_save() {
    let server = TestServer::new().await;
    let (token, _) = server
        .register("importer", "importer@review.test", "password123")
        .await;
    let original = external_pptx();
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(original.clone())
            .file_name("external.pptx")
            .mime_str("application/vnd.openxmlformats-officedocument.presentationml.presentation")
            .unwrap(),
    );
    let response = server
        .client
        .post(server.url("/api/v1/files/upload"))
        .headers(server.auth_header(&token))
        .multipart(form)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let upload: Value = response.json().await.unwrap();
    assert_eq!(status, 201, "{upload}");
    assert!(
        !upload["data"]["import_warnings"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let doc = upload["data"]["document_id"]
        .as_str()
        .expect("external PPTX imported");
    let response = server
        .client
        .get(server.url(&format!("/api/v1/documents/{doc}")))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    let document: Value = response.json().await.unwrap();
    let mut content = document["data"]["content"].clone();
    content["title"] = json!("Edited title");
    let response = server
        .client
        .put(server.url(&format!("/api/v1/documents/{doc}")))
        .headers(server.auth_header(&token))
        .json(&json!({"content":content,"expected_version":document["data"]["version"]}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 200, "{body}");
    let current = server
        .client
        .get(server.url(&format!(
            "/api/v1/files/{}/download",
            upload["data"]["id"].as_str().unwrap()
        )))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(current.status(), 200);
    let current = current.bytes().await.unwrap();
    assert!(
        current.starts_with(b"PK"),
        "Current Office download must be an OOXML ZIP, not mislabeled native JSON"
    );
    let exported = syntra_export::pptx::pptx_to_presentation(&current).unwrap();
    assert_eq!(exported.title, "Edited title");
    let url = upload["data"]["original_download_url"].as_str().unwrap();
    let response = server
        .client
        .get(server.url(url))
        .headers(server.auth_header(&token))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.bytes().await.unwrap().as_ref(),
        original.as_slice()
    );
    let response = server.client.get(server.url(url)).send().await.unwrap();
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn tenant_membership_cannot_cross_resource_boundaries_or_grant_platform_admin() {
    let server = TestServer::new().await;
    let (owner, owner_body) = server
        .register("tenantowner", "tenantowner@review.test", "password123")
        .await;
    let (other, other_body) = server
        .register("tenantother", "tenantother@review.test", "password123")
        .await;
    let (doc, file, _) = server
        .create_document(&owner, "tenant sentinel", "writer", None)
        .await;
    let owner_id = owner_body["data"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let other_id = other_body["data"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server.make_admin(&owner_id).await;
    server.db.run(move |conn|{
        use diesel::sql_types::Text;
        for id in ["tenant-a","tenant-b"]{
            diesel::RunQueryDsl::execute(diesel::sql_query("INSERT INTO tenants(id,name,created_at,updated_at) VALUES (?,?, 'now','now')")
                .bind::<Text,_>(id).bind::<Text,_>(id),conn)?;
        }
        for (id,tenant) in [(&owner_id,"tenant-a"),(&other_id,"tenant-b")]{
            diesel::RunQueryDsl::execute(diesel::sql_query("UPDATE users SET tenant_id=? WHERE id=?")
                .bind::<Text,_>(tenant).bind::<Text,_>(id),conn)?;
        }
        diesel::RunQueryDsl::execute(diesel::sql_query("INSERT INTO rooms(id,name,owner_id,created_at,updated_at) VALUES ('crossroom','room',?,'now','now')")
            .bind::<Text,_>(&owner_id),conn)?;
        diesel::RunQueryDsl::execute(diesel::sql_query("UPDATE files SET room_id='crossroom' WHERE id=?").bind::<Text,_>(file),conn)?;
        diesel::RunQueryDsl::execute(diesel::sql_query("INSERT INTO room_members(id,room_id,user_id,role,joined_at) VALUES ('crossmember','crossroom',?,'editor','now')")
            .bind::<Text,_>(other_id),conn)?;
        Ok::<_,diesel::result::Error>(())
    }).await.unwrap();
    let response = server
        .client
        .get(server.url(&format!("/api/v1/documents/{doc}")))
        .headers(server.auth_header(&other))
        .send()
        .await
        .unwrap();
    assert!(matches!(response.status().as_u16(), 403 | 404));
    let response = server
        .client
        .get(server.url("/api/v1/search?q=sentinel"))
        .headers(server.auth_header(&other))
        .send()
        .await
        .unwrap();
    let search: Value = response.json().await.unwrap();
    assert_eq!(search["data"]["total"], 0);
    let response = server
        .client
        .get(server.url("/api/v1/admin/users"))
        .headers(server.auth_header(&owner))
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        403,
        "tenant-scoped admin must not become platform admin"
    );
    server
        .db
        .run(|conn| {
            diesel::RunQueryDsl::execute(
                diesel::sql_query("UPDATE tenants SET is_active=0 WHERE id='tenant-a'"),
                conn,
            )?;
            Ok::<_, diesel::result::Error>(())
        })
        .await
        .unwrap();
    let response = server
        .client
        .get(server.url("/api/v1/auth/me"))
        .headers(server.auth_header(&owner))
        .send()
        .await
        .unwrap();
    assert!(matches!(response.status().as_u16(), 401 | 403));
}
