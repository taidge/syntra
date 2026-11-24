#[allow(dead_code)]
mod common;

use common::TestServer;
use serde_json::Value;

async fn file_snapshot(server: &TestServer, id: &str) -> Value {
    let id = id.to_owned();
    server
        .db
        .run(move |conn| {
            use diesel::prelude::*;
            use syntra_core::{models::File, schema::files};
            files::table
                .find(id)
                .first::<File>(conn)
                .map(|file| serde_json::to_value(file).unwrap())
        })
        .await
        .unwrap()
}

async fn trash_folder(server: &TestServer, token: &str, id: &str) {
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/folders/{id}")))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

async fn restore(server: &TestServer, token: &str, id: &str) -> reqwest::Response {
    server
        .client
        .post(server.url(&format!("/api/v1/trash/{id}/restore")))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
}

async fn execute_sql(server: &TestServer, sql: &str) {
    let sql = sql.to_owned();
    server
        .db
        .run(move |conn| {
            use diesel::connection::SimpleConnection;
            conn.batch_execute(&sql)
        })
        .await
        .unwrap();
}

async fn create_data(server: &TestServer, token: &str, path: &str, body: Value) -> Value {
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

async fn deletion_batch_id(server: &TestServer, file_id: &str) -> Option<String> {
    #[derive(diesel::QueryableByName)]
    struct Batch {
        #[diesel(sql_type = diesel::sql_types::Text)]
        batch_id: String,
    }
    let file_id = file_id.to_owned();
    server
        .db
        .run(move |conn| {
            use diesel::{OptionalExtension, RunQueryDsl};
            diesel::sql_query("SELECT batch_id FROM trash_batch_members WHERE file_id = ?")
                .bind::<diesel::sql_types::Text, _>(file_id)
                .get_result::<Batch>(conn)
                .optional()
                .map(|row| row.map(|batch| batch.batch_id))
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn restore_name_conflict_keeps_the_original_trashed_until_the_name_is_available() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("trash_names", "trash_names@test.invalid", "Password123!")
        .await;
    let (parent, _) = server.create_folder(&jwt, "parent", None).await;
    let (original, _) = server.create_folder(&jwt, "same name", Some(&parent)).await;
    trash_folder(&server, &jwt, &original).await;
    let (replacement, _) = server.create_folder(&jwt, "same name", Some(&parent)).await;
    let before_original = file_snapshot(&server, &original).await;
    let before_replacement = file_snapshot(&server, &replacement).await;
    let path = format!("/api/v1/trash/{original}/restore");
    let rejected = server
        .client
        .post(server.url(&path))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 409);
    let rejected: Value = rejected.json().await.unwrap();
    assert_eq!(rejected["success"], false);
    assert_eq!(file_snapshot(&server, &original).await, before_original);
    assert_eq!(
        file_snapshot(&server, &replacement).await,
        before_replacement
    );

    let renamed = server
        .client
        .put(server.url(&format!("/api/v1/folders/{replacement}")))
        .bearer_auth(&jwt)
        .json(&serde_json::json!({"name":"replacement renamed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status(), 200);
    let restored = server
        .client
        .post(server.url(&path))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(restored.status(), 200);
    let restored = file_snapshot(&server, &original).await;
    assert_eq!(restored["is_trashed"], false);
    assert_eq!(restored["trashed_at"], Value::Null);
    assert_eq!(restored["name"], "same name");
    assert_eq!(restored["parent_id"], parent);
    let children: Value = server
        .client
        .get(server.url(&format!("/api/v1/folders/{parent}/children")))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(children["data"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn another_users_private_root_name_does_not_block_restoration() {
    let server = TestServer::new().await;
    let (owner, _) = server
        .register("trash_owner", "trash_owner@test.invalid", "Password123!")
        .await;
    let (other, _) = server
        .register("trash_other", "trash_other@test.invalid", "Password123!")
        .await;
    let (original, _) = server.create_folder(&owner, "private name", None).await;
    trash_folder(&server, &owner, &original).await;
    let (unrelated, _) = server.create_folder(&other, "private name", None).await;
    let before_other = file_snapshot(&server, &unrelated).await;
    let restored = server
        .client
        .post(server.url(&format!("/api/v1/trash/{original}/restore")))
        .bearer_auth(&owner)
        .send()
        .await
        .unwrap();
    assert_eq!(restored.status(), 200);
    assert_eq!(file_snapshot(&server, &original).await["is_trashed"], false);
    assert_eq!(file_snapshot(&server, &unrelated).await, before_other);
}

#[tokio::test]
async fn restoring_a_folder_recovers_its_batch_but_preserves_earlier_trash() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("batch_tree", "batch_tree@test.invalid", "Password123!")
        .await;
    let (root, _) = server.create_folder(&jwt, "root", None).await;
    let (child, _) = server.create_folder(&jwt, "child", Some(&root)).await;
    let (document, leaf, _) = server
        .create_document(&jwt, "leaf", "writer", Some(&child))
        .await;
    let (old, _) = server
        .create_folder(&jwt, "earlier trash", Some(&root))
        .await;
    let (_, old_leaf, _) = server
        .create_document(&jwt, "old leaf", "writer", Some(&old))
        .await;
    trash_folder(&server, &jwt, &old).await;
    trash_folder(&server, &jwt, &root).await;
    // Deliberately equal timestamps cannot make earlier deletions join this batch.
    execute_sql(
        &server,
        "UPDATE files SET trashed_at = '2026-01-01T00:00:00Z' WHERE is_trashed = 1",
    )
    .await;
    let old_before = file_snapshot(&server, &old).await;
    let old_leaf_before = file_snapshot(&server, &old_leaf).await;
    let response = restore(&server, &jwt, &root).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["restored_count"],
        3
    );
    for id in [&root, &child, &leaf] {
        let file = file_snapshot(&server, id).await;
        assert_eq!(file["is_trashed"], false);
        assert_eq!(file["trashed_at"], Value::Null);
    }
    assert_eq!(file_snapshot(&server, &old).await, old_before);
    assert_eq!(file_snapshot(&server, &old_leaf).await, old_leaf_before);
    let opened = server
        .client
        .get(server.url(&format!("/api/v1/documents/{document}")))
        .bearer_auth(&jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(opened.status(), 200);
    // A later deletion gets a fresh batch and still excludes the old trash.
    trash_folder(&server, &jwt, &root).await;
    let response = restore(&server, &jwt, &root).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["restored_count"],
        3
    );
    assert_eq!(file_snapshot(&server, &old).await, old_before);
}

#[tokio::test]
async fn restoring_a_child_requires_its_parent_and_does_not_restore_siblings() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("batch_parent", "batch_parent@test.invalid", "Password123!")
        .await;
    let (root, _) = server.create_folder(&jwt, "root", None).await;
    let (child, _) = server.create_folder(&jwt, "child", Some(&root)).await;
    trash_folder(&server, &jwt, &root).await;
    let before_root = file_snapshot(&server, &root).await;
    let before_child = file_snapshot(&server, &child).await;
    let rejected = restore(&server, &jwt, &child).await;
    assert_eq!(rejected.status(), 409);
    assert_eq!(file_snapshot(&server, &root).await, before_root);
    assert_eq!(file_snapshot(&server, &child).await, before_child);
    let restored = restore(&server, &jwt, &root).await;
    assert_eq!(restored.status(), 200);
    assert_eq!(
        restored.json::<Value>().await.unwrap()["data"]["restored_count"],
        2
    );
}

#[tokio::test]
async fn descendant_name_conflict_rolls_back_the_entire_recovery() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register(
            "batch_conflict",
            "batch_conflict@test.invalid",
            "Password123!",
        )
        .await;
    let (root, _) = server.create_folder(&jwt, "root", None).await;
    let (child, _) = server.create_folder(&jwt, "same", Some(&root)).await;
    let (replacement, _) = server.create_folder(&jwt, "replacement", None).await;
    trash_folder(&server, &jwt, &root).await;
    // Emulate a legacy active row in the trashed folder, where normal create
    // correctly refuses to create new children.
    execute_sql(
        &server,
        &format!("UPDATE files SET parent_id = '{root}', name = 'same' WHERE id = '{replacement}'"),
    )
    .await;
    let before_root = file_snapshot(&server, &root).await;
    let before_child = file_snapshot(&server, &child).await;
    let before_replacement = file_snapshot(&server, &replacement).await;
    assert_eq!(restore(&server, &jwt, &root).await.status(), 409);
    assert_eq!(file_snapshot(&server, &root).await, before_root);
    assert_eq!(file_snapshot(&server, &child).await, before_child);
    assert_eq!(
        file_snapshot(&server, &replacement).await,
        before_replacement
    );
    execute_sql(
        &server,
        &format!("UPDATE files SET name = 'available' WHERE id = '{replacement}'"),
    )
    .await;
    let response = restore(&server, &jwt, &root).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["restored_count"],
        2
    );
}

#[tokio::test]
async fn legacy_trash_without_batch_restores_only_the_requested_row() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("legacy_trash", "legacy_trash@test.invalid", "Password123!")
        .await;
    let (root, _) = server.create_folder(&jwt, "root", None).await;
    let (child, _) = server.create_folder(&jwt, "child", Some(&root)).await;
    trash_folder(&server, &jwt, &root).await;
    execute_sql(&server, "DELETE FROM trash_batch_members").await;
    let before_child = file_snapshot(&server, &child).await;
    let response = restore(&server, &jwt, &root).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["restored_count"],
        1
    );
    assert_eq!(file_snapshot(&server, &child).await, before_child);
    let response = restore(&server, &jwt, &child).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["restored_count"],
        1
    );
}

#[tokio::test]
async fn legacy_trash_barriers_allow_the_root_then_each_old_branch_to_be_recovered() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("legacy_mixed", "legacy_mixed@test.invalid", "Password123!")
        .await;
    let (root, _) = server.create_folder(&jwt, "root", None).await;
    let (old, _) = server.create_folder(&jwt, "old trash", Some(&root)).await;
    let (_, leaf, _) = server
        .create_document(&jwt, "legacy active leaf", "writer", Some(&old))
        .await;
    // The old API could trash only this parent while leaving its leaf active.
    execute_sql(&server, &format!(
        "UPDATE files SET is_trashed = 1, trashed_at = '2026-01-01T00:00:00Z' WHERE id = '{old}'"
    )).await;
    let old_before = file_snapshot(&server, &old).await;
    trash_folder(&server, &jwt, &root).await;
    let leaf_before = file_snapshot(&server, &leaf).await;
    assert_eq!(leaf_before["is_trashed"], true);
    let batch = deletion_batch_id(&server, &leaf).await;
    assert!(batch.is_some());
    assert_eq!(deletion_batch_id(&server, &root).await, batch);
    assert_eq!(deletion_batch_id(&server, &old).await, None);

    let response = restore(&server, &jwt, &root).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["restored_count"],
        1
    );
    let restored_root = file_snapshot(&server, &root).await;
    assert_eq!(restored_root["is_trashed"], false);
    assert_eq!(file_snapshot(&server, &old).await, old_before);
    assert_eq!(file_snapshot(&server, &leaf).await, leaf_before);
    assert_eq!(deletion_batch_id(&server, &root).await, None);
    assert_eq!(deletion_batch_id(&server, &leaf).await, batch);
    assert_eq!(restore(&server, &jwt, &leaf).await.status(), 409);
    assert_eq!(file_snapshot(&server, &leaf).await, leaf_before);

    let response = restore(&server, &jwt, &old).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["restored_count"],
        1
    );
    let restored_old = file_snapshot(&server, &old).await;
    assert_eq!(restored_old["is_trashed"], false);
    assert_eq!(file_snapshot(&server, &root).await, restored_root);
    assert_eq!(file_snapshot(&server, &leaf).await, leaf_before);
    assert_eq!(deletion_batch_id(&server, &leaf).await, batch);

    let response = restore(&server, &jwt, &leaf).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["restored_count"],
        1
    );
    assert_eq!(file_snapshot(&server, &leaf).await["is_trashed"], false);
    assert_eq!(deletion_batch_id(&server, &leaf).await, None);
    assert_eq!(file_snapshot(&server, &root).await, restored_root);
    assert_eq!(file_snapshot(&server, &old).await, restored_old);
}

#[tokio::test]
async fn private_trash_cannot_be_probed_or_restored_by_another_owner() {
    let server = TestServer::new().await;
    let (owner, _) = server
        .register("batch_owner", "batch_owner@test.invalid", "Password123!")
        .await;
    let (other, _) = server
        .register("batch_other", "batch_other@test.invalid", "Password123!")
        .await;
    let (root, _) = server.create_folder(&owner, "root", None).await;
    let (child, _) = server.create_folder(&owner, "child", Some(&root)).await;
    trash_folder(&server, &owner, &root).await;
    let before_root = file_snapshot(&server, &root).await;
    let before_child = file_snapshot(&server, &child).await;
    let rejected = restore(&server, &other, &root).await;
    let missing = restore(&server, &other, &uuid::Uuid::new_v4().to_string()).await;
    assert_eq!(rejected.status(), 404);
    assert_eq!(missing.status(), 404);
    assert_eq!(
        rejected.json::<Value>().await.unwrap(),
        missing.json::<Value>().await.unwrap()
    );
    assert_eq!(file_snapshot(&server, &root).await, before_root);
    assert_eq!(file_snapshot(&server, &child).await, before_child);
    let response = restore(&server, &owner, &root).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["restored_count"],
        2
    );
}

#[tokio::test]
async fn room_admin_restores_other_owners_batch_members_but_editor_cannot() {
    let server = TestServer::new().await;
    let (admin, _) = server
        .register("trash_admin", "trash_admin@test.invalid", "Password123!")
        .await;
    let (editor, registered) = server
        .register("trash_editor", "trash_editor@test.invalid", "Password123!")
        .await;
    let user = registered["data"]["user"]["id"].as_str().unwrap();
    let room = create_data(
        &server,
        &admin,
        "/api/v1/rooms",
        serde_json::json!({"name":"shared trash"}),
    )
    .await;
    let room = room["id"].as_str().unwrap();
    create_data(
        &server,
        &admin,
        &format!("/api/v1/rooms/{room}/members"),
        serde_json::json!({"user_id":user,"role":"editor"}),
    )
    .await;
    let root = create_data(
        &server,
        &admin,
        "/api/v1/folders",
        serde_json::json!({"name":"root","room_id":room}),
    )
    .await;
    let root = root["id"].as_str().unwrap();
    let (child, _) = server
        .create_folder(&editor, "editor child", Some(root))
        .await;
    assert_ne!(
        file_snapshot(&server, root).await["owner_id"],
        file_snapshot(&server, &child).await["owner_id"]
    );
    trash_folder(&server, &admin, root).await;
    let before_root = file_snapshot(&server, root).await;
    let before_child = file_snapshot(&server, &child).await;
    assert_eq!(restore(&server, &editor, root).await.status(), 403);
    assert_eq!(file_snapshot(&server, root).await, before_root);
    assert_eq!(file_snapshot(&server, &child).await, before_child);
    let response = restore(&server, &admin, root).await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["restored_count"],
        2
    );
    assert_eq!(file_snapshot(&server, root).await["is_trashed"], false);
    assert_eq!(file_snapshot(&server, &child).await["is_trashed"], false);
}
