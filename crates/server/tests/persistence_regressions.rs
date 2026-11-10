#[allow(dead_code)]
mod common;
use common::TestServer;
use contrix_server::services::{StorageBackend, StorageService, persistence};
use serde_json::{Value, json};

async fn setup() -> (TestServer, String, String) {
    let server = TestServer::new().await;
    let (token, body) = server
        .register("persist", "persist@example.test", "Persist!2026")
        .await;
    let user = body["data"]["user"]["id"].as_str().unwrap().to_owned();
    (server, token, user)
}
async fn sql(server: &TestServer, statement: String) {
    server
        .db
        .run(move |conn| {
            use diesel::connection::SimpleConnection;
            conn.batch_execute(&statement)
        })
        .await
        .unwrap();
}
async fn document(server: &TestServer, token: &str, id: &str) -> Value {
    let response = server
        .client
        .get(server.url(&format!("/api/v1/documents/{id}")))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json::<Value>().await.unwrap()["data"].clone()
}
async fn quota(server: &TestServer, token: &str) -> i64 {
    server
        .client
        .get(server.url("/api/v1/quota"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["data"]["storage_used"]
        .as_i64()
        .unwrap()
}

#[tokio::test]
async fn concurrent_document_saves_cannot_lose_updates_or_split_download_content() {
    let (server, token, _) = setup().await;
    let (id, file, _) = server.create_document(&token, "CAS", "writer", None).await;
    let save = |content: Value| {
        server
            .client
            .put(server.url(&format!("/api/v1/documents/{id}")))
            .bearer_auth(&token)
            .json(&json!({"content":content,"expected_version":1}))
            .send()
    };
    let (a, b) = tokio::join!(save(json!({"text":"one"})), save(json!({"text":"two"})));
    let mut statuses = [a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 409]);
    let current = document(&server, &token, &id).await;
    assert_eq!(current["version"], 2);
    let download = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/download")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200);
    let bytes = download.bytes().await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        current["content"]
    );
    assert_eq!(quota(&server, &token).await, bytes.len() as i64);
}

#[tokio::test]
async fn failed_restore_rolls_back_history_metadata_and_preserves_old_blob() {
    let (server, token, _) = setup().await;
    let (id, file, _) = server
        .create_document(&token, "rollback", "writer", None)
        .await;
    let response = server
        .client
        .put(server.url(&format!("/api/v1/documents/{id}")))
        .bearer_auth(&token)
        .json(&json!({"expected_version":1,"content":{"text":"new"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let versions = server
        .client
        .get(server.url(&format!("/api/v1/documents/{id}/versions")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let version_id = versions["data"][0]["id"].as_str().unwrap();
    let before = document(&server, &token, &id).await;
    let used = quota(&server, &token).await;
    sql(&server, "CREATE TRIGGER reject_file_update BEFORE UPDATE ON files BEGIN SELECT RAISE(ABORT, 'injected failure'); END;".into()).await;
    let response = server
        .client
        .post(server.url(&format!("/api/v1/documents/{id}/restore/{version_id}")))
        .bearer_auth(&token)
        .json(&json!({"expected_version":2}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    assert_eq!(document(&server, &token, &id).await, before);
    assert_eq!(quota(&server, &token).await, used);
    let download = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/download")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200);
    assert_eq!(download.json::<Value>().await.unwrap(), before["content"]);
    let after = server
        .client
        .get(server.url(&format!("/api/v1/documents/{id}/versions")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(after, versions);
}

#[tokio::test]
async fn same_named_concurrent_blobs_remain_independently_readable_and_deletable() {
    let dir = tempfile::tempdir().unwrap();
    let storage = StorageBackend::Local(StorageService::new(dir.path().to_str().unwrap()));
    let owner = uuid::Uuid::new_v4();
    let (a, b) = tokio::join!(
        storage.save_file(&owner, "same.txt", b"one"),
        storage.save_file(&owner, "same.txt", b"two")
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a, b);
    assert_eq!(storage.read_file(&a).await.unwrap(), b"one");
    assert_eq!(storage.read_file(&b).await.unwrap(), b"two");
    storage.delete_file(&a).await.unwrap();
    assert_eq!(storage.read_file(&b).await.unwrap(), b"two");
}

async fn upload_session(server: &TestServer, token: &str, size: i64) -> String {
    let response = server
        .client
        .post(server.url("/api/v1/upload/sessions"))
        .bearer_auth(token)
        .json(&json!({"file_name":"chunks.txt","total_size":size}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    response.json::<Value>().await.unwrap()["data"]["id"]
        .as_str()
        .unwrap()
        .into()
}
async fn chunk(
    server: &TestServer,
    token: &str,
    id: &str,
    index: i32,
    content: &str,
) -> reqwest::Response {
    server
        .client
        .post(server.url(&format!(
            "/api/v1/upload/sessions/{id}/chunks?chunk_index={index}"
        )))
        .bearer_auth(token)
        .multipart(
            reqwest::multipart::Form::new().part(
                "chunk",
                reqwest::multipart::Part::bytes(content.as_bytes().to_vec())
                    .file_name("chunk")
                    .mime_str("application/octet-stream")
                    .unwrap(),
            ),
        )
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn parallel_chunks_and_completion_are_idempotent_and_charge_once() {
    let (server, token, _) = setup().await;
    let id = upload_session(&server, &token, 6).await;
    let (a, b) = tokio::join!(
        chunk(&server, &token, &id, 1, "abc"),
        chunk(&server, &token, &id, 2, "def")
    );
    assert_eq!(a.status(), 200);
    assert_eq!(b.status(), 200);
    assert_eq!(chunk(&server, &token, &id, 1, "abc").await.status(), 200);
    assert_eq!(chunk(&server, &token, &id, 1, "xyz").await.status(), 409);
    let complete = || {
        server
            .client
            .post(server.url(&format!("/api/v1/upload/sessions/{id}/complete")))
            .bearer_auth(&token)
            .send()
    };
    let (a, b) = tokio::join!(complete(), complete());
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(a.status().is_success());
    assert!(b.status().is_success());
    let (a, b) = (
        a.json::<Value>().await.unwrap(),
        b.json::<Value>().await.unwrap(),
    );
    assert_eq!(a["data"]["id"], b["data"]["id"]);
    assert_eq!(quota(&server, &token).await, 6);
    let file = a["data"]["id"].as_str().unwrap();
    let bytes = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/download")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), b"abcdef");
}

#[tokio::test]
async fn completion_database_failure_keeps_chunks_for_retry() {
    let (server, token, _) = setup().await;
    let id = upload_session(&server, &token, 3).await;
    assert_eq!(chunk(&server, &token, &id, 1, "abc").await.status(), 200);
    sql(&server,"CREATE TRIGGER reject_file_insert BEFORE INSERT ON files BEGIN SELECT RAISE(ABORT, 'injected failure'); END;".into()).await;
    let complete = || {
        server
            .client
            .post(server.url(&format!("/api/v1/upload/sessions/{id}/complete")))
            .bearer_auth(&token)
            .send()
    };
    assert_eq!(complete().await.unwrap().status(), 500);
    assert_eq!(quota(&server, &token).await, 0);
    sql(&server, "DROP TRIGGER reject_file_insert".into()).await;
    assert_eq!(complete().await.unwrap().status(), 201);
    assert_eq!(quota(&server, &token).await, 3);
}

#[tokio::test]
async fn exclusive_locks_and_revoked_creator_acl_apply_to_document_writes() {
    let (server, token, owner) = setup().await;
    let (peer_token, peer_body) = server
        .register("peer", "peer@example.test", "PeerStrong!2026")
        .await;
    let peer = peer_body["data"]["user"]["id"].as_str().unwrap();
    let (id, file, _) = server.create_document(&token, "room", "writer", None).await;
    let room = uuid::Uuid::new_v4();
    sql(&server,format!("INSERT INTO rooms(id,name,owner_id,created_at,updated_at) VALUES ('{room}','room','{owner}','now','now'); INSERT INTO room_members(id,room_id,user_id,role,joined_at) VALUES ('a','{room}','{owner}','owner','now'),('b','{room}','{peer}','editor','now'); UPDATE files SET room_id='{room}' WHERE id='{file}';")).await;
    let a = persistence::acquire_file_lock(&server.db, &file, &owner, "a");
    let b = persistence::acquire_file_lock(&server.db, &file, peer, "b");
    let (a, b) = tokio::join!(a, b);
    assert_ne!(a.is_ok(), b.is_ok());
    sql(&server,format!("DELETE FROM file_locks WHERE file_id='{file}'; DELETE FROM room_members WHERE user_id='{owner}';")).await;
    assert_eq!(
        server
            .client
            .get(server.url(&format!("/api/v1/documents/{id}")))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let denied = server
        .client
        .put(server.url(&format!("/api/v1/documents/{id}")))
        .bearer_auth(&token)
        .json(&json!({"expected_version":1,"content":{}}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    for suffix in ["versions", "properties"] {
        let denied = server
            .client
            .get(server.url(&format!("/api/v1/files/{file}/{suffix}")))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), 403);
    }
    let property = server
        .client
        .put(server.url(&format!("/api/v1/files/{file}/properties/label")))
        .bearer_auth(&peer_token)
        .json(&json!({"value":"room editor"}))
        .send()
        .await
        .unwrap();
    assert_eq!(property.status(), 200);
    let version = server
        .client
        .post(server.url(&format!("/api/v1/files/{file}/versions")))
        .bearer_auth(&peer_token)
        .send()
        .await
        .unwrap();
    assert_eq!(version.status(), 201);
    let allowed = server
        .client
        .put(server.url(&format!("/api/v1/documents/{id}")))
        .bearer_auth(&peer_token)
        .json(&json!({"expected_version":1,"content":{"text":"allowed"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);
}

#[tokio::test]
async fn shared_purge_releases_quota_removes_versions_and_does_not_delete_restored_file() {
    let (server, token, owner) = setup().await;
    let (id, file, _) = server
        .create_document(&token, "trash", "writer", None)
        .await;
    assert_eq!(
        server
            .client
            .put(server.url(&format!("/api/v1/documents/{id}")))
            .bearer_auth(&token)
            .json(&json!({"expected_version":1,"content":{"text":"v2"}}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert!(quota(&server, &token).await > 0);
    sql(
        &server,
        format!(
            "UPDATE files SET is_trashed=1,trashed_at='2000-01-01T00:00:00Z' WHERE id='{file}'"
        ),
    )
    .await;
    assert_eq!(
        persistence::purge_trashed(
            &server.db,
            &file,
            Some(&owner),
            Some("2020-01-01T00:00:00Z")
        )
        .await
        .unwrap(),
        1
    );
    assert_eq!(quota(&server, &token).await, 0);
    assert!(
        persistence::purge_trashed(&server.db, &file, Some(&owner), None)
            .await
            .is_err()
    );
    let (_, active, _) = server
        .create_document(&token, "restored", "writer", None)
        .await;
    assert!(
        persistence::purge_trashed(&server.db, &active, Some(&owner), None)
            .await
            .is_err()
    );
    assert!(quota(&server, &token).await > 0);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_blob_commit_never_cleans_a_candidate_while_its_worker_can_commit() {
    let (server, token, owner) = setup().await;
    let (_, file, _) = server
        .create_document(&token, "cancel", "writer", None)
        .await;
    let storage = std::sync::Arc::new(StorageBackend::Local(StorageService::new(
        server._temp_dir.path().join("files").to_str().unwrap(),
    )));
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let capacity = std::sync::Arc::new(permits.acquire_owned().await.unwrap());
    let staged = persistence::stage_blob(
        &server.db,
        storage.clone(),
        owner.parse().unwrap(),
        "late.json".into(),
        std::sync::Arc::new(b"{}0".to_vec()),
        capacity,
    )
    .await
    .unwrap();
    let candidate = staged.path.clone();
    let (started, running) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let (finished, retired) = tokio::sync::oneshot::channel::<()>();
    let path = candidate.clone();
    let task = tokio::spawn(async move {
        staged
            .commit(move |conn| {
                use diesel::RunQueryDsl;
                use diesel::sql_types::Text;
                let _retired = finished;
                let _ = started.send(());
                blocked
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
                diesel::sql_query("UPDATE files SET path = ?, size = 3 WHERE id = ?")
                    .bind::<Text, _>(path)
                    .bind::<Text, _>(file)
                    .execute(conn)?;
                Ok(())
            })
            .await
    });
    running.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    persistence::drain_storage_gc(&server.db, &storage)
        .await
        .unwrap();
    assert_eq!(storage.read_file(&candidate).await.unwrap(), b"{}0");
    release.send(()).unwrap();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), retired)
        .await
        .unwrap();
    persistence::drain_storage_gc(&server.db, &storage)
        .await
        .unwrap();
    assert_eq!(storage.read_file(&candidate).await.unwrap(), b"{}0");
}

#[tokio::test]
async fn local_readiness_rejects_missing_or_non_directory_storage() {
    let dir = tempfile::tempdir().unwrap();
    let good = StorageBackend::Local(StorageService::new(dir.path().to_str().unwrap()));
    good.check_ready().await.unwrap();
    let missing = StorageBackend::Local(StorageService::new(
        dir.path().join("missing").to_str().unwrap(),
    ));
    assert!(missing.check_ready().await.is_err());
    let file = dir.path().join("file");
    tokio::fs::write(&file, b"not a directory").await.unwrap();
    let invalid = StorageBackend::Local(StorageService::new(file.to_str().unwrap()));
    assert!(invalid.check_ready().await.is_err());
}

#[tokio::test]
async fn operation_commit_is_idempotent_but_rejects_id_collisions_across_authors_and_payloads() {
    let (server, token, owner) = setup().await;
    let (_, peer_body) = server
        .register(
            "other_editor",
            "other_editor@example.test",
            "PeerStrong!2026",
        )
        .await;
    let peer = peer_body["data"]["user"]["id"].as_str().unwrap();
    let (id, file, _) = server
        .create_document(&token, "operation", "writer", None)
        .await;
    let room = uuid::Uuid::new_v4();
    sql(&server,format!("INSERT INTO rooms(id,name,owner_id,created_at,updated_at) VALUES ('{room}','room','{owner}','now','now'); INSERT INTO room_members(id,room_id,user_id,role,joined_at) VALUES ('a','{room}','{owner}','owner','now'),('b','{room}','{peer}','editor','now'); UPDATE files SET room_id='{room}' WHERE id='{file}';")).await;
    let storage = StorageBackend::Local(StorageService::new(
        server._temp_dir.path().join("files").to_str().unwrap(),
    ));
    let operation = contrix_collab::Operation::new(
        owner.parse().unwrap(),
        uuid::Uuid::new_v4(),
        1,
        contrix_collab::operation::OpType::Replace {
            path: vec!["writer".into()],
            old_value: json!({}),
            new_value: json!({"text":"operation"}),
        },
    )
    .with_base_version(1);
    assert_eq!(
        persistence::commit_document_operation(
            &server.db,
            &storage,
            &id,
            &owner,
            1,
            "{\"text\":\"operation\"}".into(),
            &operation
        )
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        persistence::commit_document_operation(
            &server.db,
            &storage,
            &id,
            &owner,
            1,
            "{\"text\":\"operation\"}".into(),
            &operation
        )
        .await
        .unwrap(),
        2
    );
    let mut forged_author = operation.clone();
    forged_author.user_id = peer.parse().unwrap();
    assert!(
        persistence::commit_document_operation(
            &server.db,
            &storage,
            &id,
            peer,
            1,
            "{}".into(),
            &forged_author
        )
        .await
        .is_err()
    );
    let mut forged_payload = operation.clone();
    forged_payload.op_type = contrix_collab::operation::OpType::Replace {
        path: vec!["writer".into()],
        old_value: json!({}),
        new_value: json!({"text":"forged"}),
    };
    assert!(
        persistence::commit_document_operation(
            &server.db,
            &storage,
            &id,
            &owner,
            1,
            "{}".into(),
            &forged_payload
        )
        .await
        .is_err()
    );
    // The original request hash, not a transformed payload hash, identifies
    // a legitimate retry after OT and after transformed history compaction.
    let mut original = operation.clone();
    original.id.counter = 2;
    original.op_type = contrix_collab::OpType::Insert {
        path: vec!["text".into()],
        index: 0,
        content: "x".into(),
    };
    let original_hash = persistence::operation_request_hash(&original).unwrap();
    let mut transformed = original.clone();
    transformed.base_version = 2;
    transformed.op_type = contrix_collab::OpType::Insert {
        path: vec!["text".into()],
        index: 1,
        content: "x".into(),
    };
    assert_eq!(
        persistence::commit_document_operation_with_request(
            &server.db,
            &storage,
            &id,
            &owner,
            2,
            "{\"text\":\"oxperation\"}".into(),
            &transformed,
            &original_hash
        )
        .await
        .unwrap(),
        3
    );
    sql(
        &server,
        format!("DELETE FROM operation_log WHERE document_id='{id}'"),
    )
    .await;
    assert_eq!(
        persistence::operation_receipt(&server.db, &id, &owner, &original, &original_hash)
            .await
            .unwrap(),
        Some(3)
    );
    assert_eq!(
        persistence::commit_document_operation_with_request(
            &server.db,
            &storage,
            &id,
            &owner,
            2,
            "{}".into(),
            &original,
            &original_hash
        )
        .await
        .unwrap(),
        3
    );
    sql(
        &server,
        format!("DELETE FROM operation_receipts WHERE document_id='{id}'"),
    )
    .await;
    let mut replay = original.clone();
    replay.base_version = 3;
    assert!(
        persistence::commit_document_operation(
            &server.db,
            &storage,
            &id,
            &owner,
            3,
            "{}".into(),
            &replay
        )
        .await
        .is_err()
    );
    let current = document(&server, &token, &id).await;
    assert_eq!(current["version"], 3);
    assert_eq!(current["content"]["text"], "oxperation");
}

#[tokio::test]
async fn imported_original_survives_document_saves_and_is_collected_only_after_purge() {
    let (server, token, owner) = setup().await;
    let mut writer = contrix_doc_engine::WriterDocument::new("Original".into());
    writer.add_paragraph("Keep original OOXML bytes");
    let original = contrix_export::docx::writer_to_docx(&writer).unwrap();
    let response = server
        .client
        .post(server.url("/api/v1/files/upload"))
        .bearer_auth(&token)
        .multipart(
            reqwest::multipart::Form::new().part(
                "file",
                reqwest::multipart::Part::bytes(original.clone())
                    .file_name("original.docx")
                    .mime_str(
                        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                    )
                    .unwrap(),
            ),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let uploaded = response.json::<Value>().await.unwrap();
    let file = uploaded["data"]["id"].as_str().unwrap();
    let id = uploaded["data"]["document_id"].as_str().unwrap();
    let stored_versions = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/versions")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let original_path = stored_versions["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["version_number"] == 0)
        .unwrap()["path"]
        .as_str()
        .unwrap();
    let mut current = document(&server, &token, id).await;
    current["content"]["title"] = json!("Edited");
    // Retained original bytes still consume quota after the editable JSON
    // becomes the current blob; a tiny replacement must not erase that charge.
    sql(
        &server,
        format!(
            "UPDATE users SET storage_quota={} WHERE id='{owner}'",
            original.len()
        ),
    )
    .await;
    let rejected = server
        .client
        .put(server.url(&format!("/api/v1/documents/{id}")))
        .bearer_auth(&token)
        .json(&json!({"expected_version":1,"content":current["content"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 413);
    assert_eq!(document(&server, &token, id).await["version"], 1);
    assert_eq!(quota(&server, &token).await, original.len() as i64);
    sql(
        &server,
        format!("UPDATE users SET storage_quota=104857600 WHERE id='{owner}'"),
    )
    .await;
    let save = server
        .client
        .put(server.url(&format!("/api/v1/documents/{id}")))
        .bearer_auth(&token)
        .json(&json!({"expected_version":1,"content":current["content"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(save.status(), 200);
    assert_eq!(
        quota(&server, &token).await,
        original.len() as i64 + current["content"].to_string().len() as i64
    );

    let storage = StorageBackend::Local(StorageService::new(
        server._temp_dir.path().join("files").to_str().unwrap(),
    ));
    persistence::drain_storage_gc(&server.db, &storage)
        .await
        .unwrap();
    let download = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/download?original=true")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), 200);
    assert_eq!(
        download.bytes().await.unwrap().as_ref(),
        original.as_slice()
    );
    assert_eq!(storage.read_file(original_path).await.unwrap(), original);
    assert_eq!(
        server
            .client
            .delete(server.url(&format!("/api/v1/files/{file}")))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        persistence::purge_trashed(&server.db, file, Some(&owner), None)
            .await
            .unwrap(),
        1
    );
    persistence::drain_storage_gc(&server.db, &storage)
        .await
        .unwrap();
    assert!(storage.read_file(original_path).await.is_err());
    assert_eq!(quota(&server, &token).await, 0);
}

#[tokio::test]
async fn duplicate_file_snapshots_charge_one_retained_object_and_release_only_its_charge() {
    let (server, token, owner) = setup().await;
    let (id, file, _) = server
        .create_document(&token, "archive quota", "writer", None)
        .await;
    let original_size = quota(&server, &token).await;
    for _ in 0..2 {
        assert_eq!(
            server
                .client
                .post(server.url(&format!("/api/v1/files/{file}/versions")))
                .bearer_auth(&token)
                .send()
                .await
                .unwrap()
                .status(),
            201
        );
    }
    let content = json!({"text":"small"});
    assert_eq!(
        server
            .client
            .put(server.url(&format!("/api/v1/documents/{id}")))
            .bearer_auth(&token)
            .json(&json!({"expected_version":1,"content":content}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        quota(&server, &token).await,
        original_size + content.to_string().len() as i64
    );
    let (_, other, _) = server
        .create_document(&token, "unrelated current file", "writer", None)
        .await;
    let total = quota(&server, &token).await;
    sql(
        &server,
        format!("UPDATE files SET is_trashed=1 WHERE id='{file}'"),
    )
    .await;
    assert_eq!(
        persistence::purge_trashed(&server.db, &file, Some(&owner), None)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        quota(&server, &token).await,
        total - original_size - content.to_string().len() as i64
    );
    // A legacy uncharged snapshot must not release the unrelated file's quota.
    let before_sentinel = quota(&server, &token).await;
    let (_, _, _) = server
        .create_document(&token, "sentinel survives legacy purge", "writer", None)
        .await;
    let sentinel_size = quota(&server, &token).await - before_sentinel;
    assert!(sentinel_size > 0);
    sql(&server,format!("INSERT INTO file_versions(id,file_id,version_number,path,size,created_by,created_at) VALUES ('legacy','{other}',1,'missing-legacy-blob',999999,'{owner}','now'); UPDATE files SET is_trashed=1 WHERE id='{other}'")).await;
    assert_eq!(
        persistence::purge_trashed(&server.db, &other, Some(&owner), None)
            .await
            .unwrap(),
        1
    );
    assert_eq!(quota(&server, &token).await, sentinel_size);
}

async fn wait_for_gc(server: &TestServer, path: &str) {
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type=diesel::sql_types::BigInt)]
        total: i64,
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let path = path.to_owned();
            let queued = server
                .db
                .run(move |conn| {
                    Ok::<_, contrix_core::error::AppError>(
                        diesel::RunQueryDsl::get_result::<Count>(
                            diesel::sql_query(
                                "SELECT COUNT(*) AS total FROM storage_gc WHERE path = ?",
                            )
                            .bind::<diesel::sql_types::Text, _>(path),
                            conn,
                        )?
                        .total,
                    )
                })
                .await
                .unwrap();
            if queued == 1 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("staged object cleanup must become durable");
}

#[tokio::test]
async fn dropping_an_unpolled_staged_commit_cleans_the_object_without_sql() {
    let (server, _, owner) = setup().await;
    let storage = std::sync::Arc::new(StorageBackend::Local(StorageService::new(
        server._temp_dir.path().join("files").to_str().unwrap(),
    )));
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let permit = std::sync::Arc::new(semaphore.clone().acquire_owned().await.unwrap());
    let staged = persistence::stage_blob(
        &server.db,
        storage.clone(),
        owner.parse().unwrap(),
        "staged.txt".into(),
        std::sync::Arc::new(b"staged".to_vec()),
        permit,
    )
    .await
    .unwrap();
    let path = staged.path.clone();
    assert_eq!(storage.read_file(&path).await.unwrap(), b"staged");
    let never_polled = staged.commit(|_| -> Result<(), contrix_core::error::AppError> {
        panic!("unpolled transaction must never execute")
    });
    drop(never_polled);
    wait_for_gc(&server, &path).await;
    persistence::drain_storage_gc(&server.db, &storage)
        .await
        .unwrap();
    assert!(storage.read_file(&path).await.is_err());
    assert_eq!(semaphore.available_permits(), 1);
}

#[tokio::test]
async fn cancelling_a_staged_s3_write_retains_capacity_until_the_real_worker_retires() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (server, _, owner) = setup().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (started, observed) = tokio::sync::oneshot::channel::<String>();
    let (release, allowed) = tokio::sync::oneshot::channel::<()>();
    let mock = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut bytes = [0; 1024];
            let count = socket.read(&mut bytes).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&bytes[..count]);
            if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() < header_end + 4 + length {
                    continue;
                }
                let raw_path = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let path = raw_path.strip_prefix("/bucket/").unwrap().to_owned();
                let _ = started.send(path);
                break;
            }
        }
        let _ = allowed.await;
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let storage = std::sync::Arc::new(StorageBackend::S3(
        contrix_server::services::s3_storage::S3Storage::new(
            "bucket",
            "us-east-1",
            "test-access",
            "test-secret",
            &format!("http://{address}"),
        ),
    ));
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
    let permit = std::sync::Arc::new(semaphore.clone().acquire_owned().await.unwrap());
    let db = server.db.clone();
    let owner = owner.parse().unwrap();
    let upload = tokio::spawn(async move {
        persistence::stage_blob(
            &db,
            storage,
            owner,
            "cancel.txt".into(),
            std::sync::Arc::new(b"retained".to_vec()),
            permit,
        )
        .await
    });
    let path = tokio::time::timeout(std::time::Duration::from_secs(5), observed)
        .await
        .unwrap()
        .unwrap();
    upload.abort();
    match upload.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("cancelled upload unexpectedly returned"),
    }
    assert_eq!(
        semaphore.available_permits(),
        0,
        "cancelled caller cannot release its active write worker"
    );
    release.send(()).unwrap();
    mock.await.unwrap();
    wait_for_gc(&server, &path).await;
    assert_eq!(semaphore.available_permits(), 1);
}
