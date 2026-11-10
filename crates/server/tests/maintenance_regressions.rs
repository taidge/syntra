//! Real S3 acceptance is opt-in and uses the isolated test service supplied by CI.
#[allow(dead_code)]
mod common;
use common::TestServer;
use contrix_core::config::MaintenanceConfig;
use contrix_server::services::{StorageBackend, maintenance, persistence, s3_storage::S3Storage};
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use std::collections::HashSet;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires the isolated OFFICE_S3_* S3-compatible acceptance service"]
async fn live_s3_maintenance() {
    let required = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("missing {name}"));
    let storage = StorageBackend::S3(S3Storage::new(
        &required("OFFICE_S3_BUCKET"),
        &required("OFFICE_S3_REGION"),
        &required("OFFICE_S3_ACCESS_KEY"),
        &required("OFFICE_S3_SECRET_KEY"),
        &required("OFFICE_S3_ENDPOINT"),
    ));
    let server = TestServer::new().await;
    storage.attach_database(server.db.clone()).unwrap();
    let (token, user) = server
        .register("maintenance", "maintenance@s3.test", "Password!12345")
        .await;
    let owner = user["data"]["user"]["id"].as_str().unwrap().to_owned();
    let (_, file, _) = server
        .create_document(&token, "protected", "writer", None)
        .await;
    let mut paths = Vec::new();
    for index in 0..24 {
        paths.push(
            storage
                .save_file(
                    &owner.parse().unwrap(),
                    "live object 世界",
                    format!("blob-{index}").as_bytes(),
                )
                .await
                .unwrap(),
        );
    }
    // Reproduce a crash after S3 PUT and before any database registration.
    let crash = format!("{owner}/{}/crash-before-sql", Uuid::new_v4());
    if let StorageBackend::S3(s3) = &storage {
        s3.put_object(&crash, b"crash-window", "text/plain")
            .await
            .unwrap();
    }
    paths.push(crash.clone());
    let own: HashSet<_> = paths.iter().cloned().collect();
    let mut listed = HashSet::new();
    let mut cursor = storage.begin_scan().await.unwrap();
    let mut pages = 0;
    while !cursor.finished() {
        let page = storage.scan_page(&mut cursor, 2).await.unwrap();
        assert!(page.len() <= 2);
        pages += 1;
        assert!(pages < 2000);
        for entry in page {
            if own.contains(&entry.path) {
                listed.insert(entry.path);
            }
        }
    }
    assert!(pages > 1);
    assert_eq!(listed, own);
    let (objects, actor, file_id) = (paths.clone(), owner.clone(), file.clone());
    server.db.run(move|conn|conn.immediate_transaction::<_,contrix_core::error::AppError,_>(|conn|{
        diesel::sql_query("UPDATE files SET path=?,thumbnail_path=? WHERE id=?").bind::<Text,_>(&objects[0]).bind::<Text,_>(&objects[5]).bind::<Text,_>(&file_id).execute(conn)?;
        diesel::sql_query("INSERT INTO file_versions(id,file_id,version_number,path,size,created_by,created_at) VALUES ('original',?,0,?,6,?,'2000-01-01T00:00:00Z')")
            .bind::<Text,_>(&file_id).bind::<Text,_>(&objects[1]).bind::<Text,_>(&actor).execute(conn)?;
        diesel::sql_query("INSERT INTO upload_chunks(session_id,chunk_index,path,size,digest) VALUES ('active-upload',1,?,6,'test')").bind::<Text,_>(&objects[2]).execute(conn)?;
        diesel::sql_query("INSERT INTO conversion_jobs(id,user_id,source_file_id,target_format,status,output_path,created_at) VALUES ('output',?,?,'pdf','completed',?,'2000-01-01T00:00:00Z')")
            .bind::<Text,_>(&actor).bind::<Text,_>(&file_id).bind::<Text,_>(&objects[3]).execute(conn)?;
        diesel::sql_query("INSERT INTO retained_blobs(owner_id,path,size) VALUES (?,?,6)").bind::<Text,_>(&actor).bind::<Text,_>(&objects[4]).execute(conn)?;
        Ok(())
    })).await.unwrap();
    let config = MaintenanceConfig {
        orphan_grace_seconds: 300,
        reconcile_legacy_dedicated_namespace: true,
        scan_page_size: 2,
        ..Default::default()
    };
    let now = chrono::Utc::now().timestamp();
    for clock in [now, now + 301] {
        let mut cursor = storage.begin_scan().await.unwrap();
        let mut pages = 0;
        while !cursor.finished() {
            maintenance::reconcile_page(&server.db, &storage, &mut cursor, &config, clock)
                .await
                .unwrap();
            pages += 1;
            assert!(pages < 2000);
        }
        if clock == now {
            assert_eq!(storage.read_file(&crash).await.unwrap(), b"crash-window");
        }
    }
    persistence::drain_storage_gc(&server.db, &storage)
        .await
        .unwrap();
    for (index, path) in paths.iter().enumerate() {
        if index < 6 {
            assert_eq!(
                storage.read_file(path).await.unwrap(),
                format!("blob-{index}").as_bytes()
            );
        } else {
            assert!(
                storage.read_file(path).await.is_err(),
                "unreferenced S3 key survived reconciliation"
            );
        }
    }
    assert!(
        storage
            .write_file(&crash, b"stale writer", "text/plain")
            .await
            .is_err()
    );
    #[derive(QueryableByName)]
    struct Tombstone {
        #[diesel(sql_type=BigInt)]
        value: i64,
    }
    let deleted = server
        .db
        .run(move |conn| {
            Ok::<_, contrix_core::error::AppError>(
                diesel::sql_query(
                    "SELECT COUNT(*) AS value FROM blob_inventory WHERE path=? AND state='deleted'",
                )
                .bind::<Text, _>(crash)
                .get_result::<Tombstone>(conn)?
                .value,
            )
        })
        .await
        .unwrap();
    assert_eq!(deleted, 1);
}

async fn converted(server: &TestServer, token: &str, id: &str) -> serde_json::Value {
    for _ in 0..100 {
        let response = server
            .client
            .get(server.url(&format!("/api/v1/convert/{id}")))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        if body["data"]["status"] == "completed" {
            return body["data"].clone();
        }
        assert_ne!(
            body["data"]["status"], "failed",
            "conversion failed: {body}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("conversion did not finish");
}
#[tokio::test]
async fn repeated_exports_and_retry_publish_distinct_immutable_objects() {
    let server = TestServer::new().await;
    let (token, _) = server
        .register("exporter", "exporter@maintenance.test", "Password!12345")
        .await;
    let (document, file, created) = server
        .create_document(&token, "Original export", "writer", None)
        .await;
    let mut jobs = Vec::new();
    for _ in 0..2 {
        let response = server
            .client
            .post(server.url("/api/v1/convert"))
            .bearer_auth(&token)
            .json(&serde_json::json!({"file_id":file,"target_format":"html"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        let body: serde_json::Value = response.json().await.unwrap();
        let id = body["data"]["id"].as_str().unwrap().to_owned();
        let result = converted(&server, &token, &id).await;
        jobs.push((id, result["output_path"].as_str().unwrap().to_owned()));
    }
    assert_ne!(jobs[0].1, jobs[1].1);
    let root = server._temp_dir.path().join("files");
    let storage = StorageBackend::Local(contrix_server::services::StorageService::new(
        root.to_str().unwrap(),
    ));
    storage.attach_database(server.db.clone()).unwrap();
    let original = storage.read_file(&jobs[0].1).await.unwrap();
    let mut content = created["data"]["content"].clone();
    content["title"] = serde_json::json!("New export after retry");
    let saved = server
        .client
        .put(server.url(&format!("/api/v1/documents/{document}")))
        .bearer_auth(&token)
        .json(&serde_json::json!({"expected_version":1,"content":content}))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    let first = jobs[0].0.clone();
    server
        .db
        .run(move |conn| {
            diesel::sql_query("UPDATE conversion_jobs SET status='failed' WHERE id=?")
                .bind::<Text, _>(first)
                .execute(conn)
        })
        .await
        .unwrap();
    // A failed job still references its old output until retry clears it.
    assert_eq!(storage.read_file(&jobs[0].1).await.unwrap(), original);
    let retried = server
        .client
        .post(server.url(&format!("/api/v1/convert/{}/retry", jobs[0].0)))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(retried.status(), 202);
    let completed = converted(&server, &token, &jobs[0].0).await;
    let replacement = completed["output_path"].as_str().unwrap();
    assert_ne!(replacement, jobs[0].1);
    assert_ne!(replacement, jobs[1].1);
    assert_ne!(storage.read_file(replacement).await.unwrap(), original);
    // Clearing output_path queues retirement automatically; the test must not
    // manufacture an outbox entry that a production path forgot to persist.
    persistence::drain_storage_gc(&server.db, &storage)
        .await
        .unwrap();
    assert!(storage.read_file(&jobs[0].1).await.is_err());
    assert!(storage.read_file(&jobs[1].1).await.is_ok());
    assert!(storage.read_file(replacement).await.is_ok());
}

#[tokio::test]
#[ignore = "requires the isolated OFFICE_S3_* S3-compatible acceptance service"]
async fn live_s3_maintenance_shared_bucket_isolation() {
    let required = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("missing {name}"));
    let remote = || {
        StorageBackend::S3(S3Storage::new(
            &required("OFFICE_S3_BUCKET"),
            &required("OFFICE_S3_REGION"),
            &required("OFFICE_S3_ACCESS_KEY"),
            &required("OFFICE_S3_SECRET_KEY"),
            &required("OFFICE_S3_ENDPOINT"),
        ))
    };
    let first = TestServer::new().await;
    let second = TestServer::new().await;
    let (_token, user) = first
        .register("primary", "primary@s3-isolation.test", "Password!12345")
        .await;
    let owner = user["data"]["user"]["id"].as_str().unwrap().to_owned();
    let copy = owner.clone();
    // A restored backup can legitimately have exactly the same owner UUID.
    second.db.run(move|conn|{
        diesel::sql_query("INSERT INTO users(id,username,email,password_hash) VALUES (?,'backup','backup@s3-isolation.test','test')").bind::<Text,_>(copy).execute(conn)?;Ok::<_,contrix_core::error::AppError>(())
    }).await.unwrap();
    let storage_a = remote();
    let storage_b = remote();
    storage_a.attach_database(first.db.clone()).unwrap();
    storage_b.attach_database(second.db.clone()).unwrap();
    let referenced = storage_b
        .save_file(
            &owner.parse().unwrap(),
            "other DB referenced",
            b"other database",
        )
        .await
        .unwrap();
    let active = format!("{owner}/{}/active-other-db", Uuid::new_v4());
    let active_key = active.clone();
    let referenced_key = referenced.clone();
    let actor = owner.clone();
    second.db.run(move|conn|{
        diesel::sql_query("INSERT INTO files(id,name,path,size,owner_id) VALUES (?,'protected',?,14,?)").bind::<Text,_>(Uuid::new_v4().to_string()).bind::<Text,_>(referenced_key).bind::<Text,_>(actor).execute(conn)?;
        diesel::sql_query("INSERT INTO blob_inventory(path,state,first_seen,lease_until,token) VALUES (?,'writing',0,9223372036854775807,'active-writer')").bind::<Text,_>(active_key).execute(conn)?;
        Ok::<_,contrix_core::error::AppError>(())
    }).await.unwrap();
    if let StorageBackend::S3(s3) = &storage_b {
        s3.put_object(&active, b"active write owned by backup", "text/plain")
            .await
            .unwrap();
    }
    let orphan = storage_a
        .save_file(&owner.parse().unwrap(), "my orphan", b"my orphan")
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    let config = MaintenanceConfig {
        orphan_grace_seconds: 300,
        scan_page_size: 2,
        ..Default::default()
    };
    for clock in [now + 301, now + 100000] {
        let mut cursor = storage_a.begin_scan().await.unwrap();
        let mut pages = 0;
        while !cursor.finished() {
            maintenance::reconcile_page(&first.db, &storage_a, &mut cursor, &config, clock)
                .await
                .unwrap();
            pages += 1;
            assert!(pages < 2000);
        }
        persistence::drain_storage_gc(&first.db, &storage_a)
            .await
            .unwrap();
    }
    assert!(storage_a.read_file(&orphan).await.is_err());
    assert_eq!(
        storage_b.read_file(&referenced).await.unwrap(),
        b"other database"
    );
    assert_eq!(
        storage_b.read_file(&active).await.unwrap(),
        b"active write owned by backup"
    );
    #[derive(QueryableByName)]
    struct Count {
        #[diesel(sql_type=BigInt)]
        value: i64,
    }
    let adopted = first
        .db
        .run(move |conn| {
            Ok::<_, contrix_core::error::AppError>(
                diesel::sql_query(
                    "SELECT COUNT(*) AS value FROM blob_inventory WHERE path IN (?,?)",
                )
                .bind::<Text, _>(referenced)
                .bind::<Text, _>(active)
                .get_result::<Count>(conn)?
                .value,
            )
        })
        .await
        .unwrap();
    assert_eq!(
        adopted, 0,
        "LIST must never acquire another database's ownership"
    );
}
