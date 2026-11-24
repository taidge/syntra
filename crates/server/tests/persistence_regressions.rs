#[allow(dead_code)]
mod common;
use common::TestServer;
use serde_json::{Value, json};
use syntra_server::services::{StorageBackend, StorageService, persistence};

/// A loopback S3 transport, not an alternate persistence implementation. Holding
/// PUT responses fixes the window between the real save preflight and its CAS.
struct ControlledObjects {
    endpoint: String,
    objects: std::sync::Arc<tokio::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>>,
    gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    arrivals: tokio::sync::mpsc::UnboundedReceiver<String>,
    release: std::sync::Arc<tokio::sync::Semaphore>,
    listener: tokio::task::JoinHandle<()>,
}

impl ControlledObjects {
    async fn new() -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let objects = std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::BTreeMap::<
            String,
            Vec<u8>,
        >::new()));
        let gate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
        let (arrived, arrivals) = tokio::sync::mpsc::unbounded_channel();
        let (store, held, permits) = (objects.clone(), gate.clone(), release.clone());
        let listener = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let (store, held, permits, arrived) = (
                    store.clone(),
                    held.clone(),
                    permits.clone(),
                    arrived.clone(),
                );
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let (method, path, body) = loop {
                        let mut bytes = [0; 4096];
                        let count = socket.read(&mut bytes).await.unwrap();
                        assert!(count > 0, "complete S3 request required");
                        request.extend_from_slice(&bytes[..count]);
                        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&request[..end]);
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    line.split_once(':')
                                        .filter(|(key, _)| {
                                            key.eq_ignore_ascii_case("content-length")
                                        })
                                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                            if request.len() < end + 4 + length {
                                continue;
                            }
                            let mut first = headers.lines().next().unwrap().split_whitespace();
                            let method = first.next().unwrap().to_owned();
                            let path = first
                                .next()
                                .unwrap()
                                .strip_prefix("/test/")
                                .unwrap()
                                .to_owned();
                            break (method, path, request[end + 4..end + 4 + length].to_vec());
                        }
                    };
                    let (status, data) = match method.as_str() {
                        "PUT" => {
                            store.lock().await.insert(path.clone(), body);
                            if held.load(std::sync::atomic::Ordering::SeqCst) {
                                arrived.send(path).unwrap();
                                permits.acquire().await.unwrap().forget();
                            }
                            (200, Vec::new())
                        }
                        "GET" => match store.lock().await.get(&path).cloned() {
                            Some(data) => (200, data),
                            None => (404, Vec::new()),
                        },
                        "DELETE" => {
                            store.lock().await.remove(&path);
                            (204, Vec::new())
                        }
                        _ => panic!("unexpected S3 method {method}"),
                    };
                    socket.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len()).as_bytes()).await.unwrap();
                    socket.write_all(&data).await.unwrap();
                });
            }
        });
        Self {
            endpoint,
            objects,
            gate,
            arrivals,
            release,
            listener,
        }
    }
}

impl Drop for ControlledObjects {
    fn drop(&mut self) {
        self.release.close();
        self.listener.abort();
    }
}

async fn persistence_snapshot(server: &TestServer, id: &str) -> Value {
    let id = id.to_owned();
    server
        .db
        .run(move |conn| {
            use diesel::prelude::*;
            use syntra_core::{
                models::{Document, File, Version},
                schema::{documents, files, users, versions},
            };
            conn.transaction::<_, syntra_core::error::AppError, _>(|conn| {
                let document = documents::table.find(id).first::<Document>(conn)?;
                let file = files::table.find(&document.file_id).first::<File>(conn)?;
                let history = versions::table
                    .filter(versions::document_id.eq(&document.id))
                    .order((versions::version_number, versions::id))
                    .load::<Version>(conn)?;
                let used = users::table
                    .find(&file.owner_id)
                    .select(users::storage_used)
                    .first::<i64>(conn)?;
                Ok(json!({"document":document,"file":file,"history":history,"quota":used}))
            })
        })
        .await
        .unwrap()
}

async fn inventory_paths(server: &TestServer) -> std::collections::BTreeSet<String> {
    #[derive(diesel::QueryableByName)]
    struct Path {
        #[diesel(sql_type = diesel::sql_types::Text)]
        path: String,
    }
    server
        .db
        .run(|conn| {
            use diesel::RunQueryDsl;
            Ok::<_, syntra_core::error::AppError>(
                diesel::sql_query("SELECT path FROM blob_inventory")
                    .load::<Path>(conn)?
                    .into_iter()
                    .map(|row| row.path)
                    .collect(),
            )
        })
        .await
        .unwrap()
}

async fn assert_candidate_collected(server: &TestServer, path: &str) {
    #[derive(diesel::QueryableByName)]
    struct Cleanup {
        #[diesel(sql_type = diesel::sql_types::Text)]
        state: String,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        pending: i64,
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            persistence::drain_storage_gc(&server.db, &server.state.storage)
                .await
                .unwrap();
            let key = path.to_owned();
            let cleanup = server.db.run(move |conn| {
                use diesel::RunQueryDsl;
                diesel::sql_query("SELECT state, (SELECT COUNT(*) FROM storage_gc WHERE path=?) AS pending FROM blob_inventory WHERE path=?")
                    .bind::<diesel::sql_types::Text, _>(&key)
                    .bind::<diesel::sql_types::Text, _>(&key)
                    .get_result::<Cleanup>(conn)
            }).await.unwrap();
            if cleanup.state == "deleted" && cleanup.pending == 0 {
                assert!(server.state.storage.read_file(path).await.is_err());
                if matches!(&*server.state.storage, StorageBackend::Local(_)) {
                    assert!(!server._temp_dir.path().join("files").join(path).exists());
                }
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("unreferenced candidate must be physically collected");
}

async fn setup() -> (TestServer, String, String) {
    let server = TestServer::new().await;
    let (token, body) = server
        .register("persist", "persist@example.test", "Persist!2026")
        .await;
    let user = body["data"]["user"]["id"].as_str().unwrap().to_owned();
    (server, token, user)
}

/// A valid legacy no-op log entry can carry unknown JSON metadata. This lets
/// retention tests hit the byte boundary exactly without sending an invalid
/// oversized websocket message or allocating a large current document.
fn operation_log_with_size(operation: &syntra_collab::Operation, size: usize) -> String {
    let mut value = serde_json::to_value(operation).unwrap();
    value["legacy_fixture_padding"] = json!("");
    let overhead = serde_json::to_string(&value).unwrap().len();
    assert!(size >= overhead);
    value["legacy_fixture_padding"] = json!("x".repeat(size - overhead));
    let value = serde_json::to_string(&value).unwrap();
    assert_eq!(value.len(), size);
    value
}

async fn seed_two_legacy_operations(
    server: &TestServer,
    document: &str,
    user: &str,
    first_size: usize,
    last_size: usize,
) {
    let (document, user) = (document.to_owned(), user.to_owned());
    server.db.run(move |conn| {
        use syntra_core::{error::AppError, models::{Document, NewOperationLogEntry, NewVersion}, schema::{documents::dsl as d, operation_log::dsl as ol, versions::dsl as v}};
        use diesel::{prelude::*, sql_types::{BigInt, Integer, Text}};
        conn.immediate_transaction::<_, AppError, _>(|conn| {
            let row = d::documents.find(&document).first::<Document>(conn)?;
            assert_eq!(row.version, 1);
            let content: Value = serde_json::from_str(row.content.as_ref().unwrap()).unwrap();
            let client = uuid::Uuid::new_v4();
            let now = chrono::Utc::now().to_rfc3339();
            for (counter, size) in [(1_u64, first_size), (2, last_size)] {
                let operation = syntra_collab::Operation::new(uuid::Uuid::parse_str(&user).unwrap(), client, counter, syntra_collab::OpType::Replace {
                    path: vec![], old_value: content.clone(), new_value: content.clone(),
                }).with_base_version(counter);
                let id = format!("{document}:{client}:{counter}");
                diesel::insert_into(v::versions).values(NewVersion {
                    id: uuid::Uuid::new_v4().to_string(), document_id: document.clone(),
                    version_number: i32::try_from(counter).unwrap(), content: row.content.clone(),
                    created_by: user.clone(), label: None, created_at: now.clone(),
                }).execute(conn)?;
                diesel::insert_into(ol::operation_log).values(NewOperationLogEntry {
                    id: id.clone(), document_id: document.clone(), user_id: user.clone(),
                    op_type: serde_json::to_string(&operation.op_type).unwrap(),
                    op_data: operation_log_with_size(&operation, size),
                    version: i32::try_from(counter+1).unwrap(), created_at: now.clone(),
                }).execute(conn)?;
                diesel::sql_query("INSERT INTO operation_receipts(id,document_id,client_id,counter,user_id,request_hash,version) VALUES (?,?,?,?,?,?,?)")
                    .bind::<Text,_>(&id).bind::<Text,_>(&document).bind::<Text,_>(client.to_string())
                    .bind::<BigInt,_>(i64::try_from(counter).unwrap()).bind::<Text,_>(&user)
                    .bind::<Text,_>(persistence::operation_request_hash(&operation)?)
                    .bind::<Integer,_>(i32::try_from(counter+1).unwrap()).execute(conn)?;
            }
            diesel::sql_query("INSERT INTO operation_clients(document_id,client_id,user_id,max_counter) VALUES (?,?,?,2)")
                .bind::<Text,_>(&document).bind::<Text,_>(client.to_string()).bind::<Text,_>(&user).execute(conn)?;
            diesel::update(d::documents.find(&document)).set(d::version.eq(3)).execute(conn)?;
            Ok(())
        })
    }).await.unwrap();
}

#[tokio::test]
async fn operation_history_commit_enforces_exact_32_mib_newest_suffix() {
    const BUDGET: usize = 32 * 1024 * 1024;
    for excess in [0_usize, 1] {
        let (server, token, user) = setup().await;
        let (id, _, _) = server
            .create_document(&token, "byte boundary", "writer", None)
            .await;
        let initial = document(&server, &token, &id).await["content"].clone();
        let content = syntra_doc_engine::writer::writer_content_with_text(
            &initial,
            "byte retention committed",
        );
        let operation = syntra_collab::Operation::new(
            uuid::Uuid::parse_str(&user).unwrap(),
            uuid::Uuid::new_v4(),
            1,
            syntra_collab::OpType::Replace {
                path: vec![],
                old_value: initial,
                new_value: content.clone(),
            },
        )
        .with_base_version(3);
        // commit_document_operation serializes through serde_json::Value.
        let new_log_size = serde_json::to_string(&serde_json::to_value(&operation).unwrap())
            .unwrap()
            .len();
        seed_two_legacy_operations(&server, &id, &user, 2048, BUDGET - new_log_size + excess).await;
        let version = persistence::commit_document_operation(
            &server.db,
            &server.state.storage,
            &id,
            &user,
            3,
            serde_json::to_string(&content).unwrap(),
            &operation,
        )
        .await
        .unwrap();
        assert_eq!(version, 4);
        #[derive(diesel::QueryableByName)]
        struct Length {
            #[diesel(sql_type = diesel::sql_types::Integer)]
            version: i32,
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            bytes: i64,
        }
        let key = id.clone();
        let lengths = server.db.run(move |conn| {
            use diesel::RunQueryDsl;
            diesel::sql_query("SELECT version,LENGTH(CAST(op_data AS BLOB)) AS bytes FROM operation_log WHERE document_id=? ORDER BY version")
                .bind::<diesel::sql_types::Text,_>(key).load::<Length>(conn)
        }).await.unwrap();
        assert_eq!(
            lengths.iter().map(|r| r.version).collect::<Vec<_>>(),
            if excess == 0 { vec![3, 4] } else { vec![4] }
        );
        assert_eq!(
            lengths.iter().map(|r| r.bytes).sum::<i64>(),
            if excess == 0 {
                BUDGET as i64
            } else {
                new_log_size as i64
            }
        );
        let after = persistence_snapshot(&server, &id).await;
        assert_eq!(after["document"]["version"], 4);
        assert_eq!(
            serde_json::from_str::<Value>(after["document"]["content"].as_str().unwrap()).unwrap(),
            content
        );
        assert_eq!(after["history"].as_array().unwrap().len(), 3);
        let blob = server
            .state
            .storage
            .read_file(after["file"]["path"].as_str().unwrap())
            .await
            .unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&blob).unwrap(), content);
        // Same operation identity remains a receipt hit after its audit suffix is
        // compacted: replay must not add a version or alter the charged bytes.
        assert_eq!(
            persistence::commit_document_operation(
                &server.db,
                &server.state.storage,
                &id,
                &user,
                3,
                serde_json::to_string(&content).unwrap(),
                &operation
            )
            .await
            .unwrap(),
            4
        );
        assert_eq!(persistence_snapshot(&server, &id).await, after);
    }
}

#[tokio::test]
async fn operation_history_future_row_conflicts_without_partial_metadata_commit() {
    use diesel::{prelude::*, sql_types::Text};
    let (server, token, user) = setup().await;
    let (id, _, _) = server
        .create_document(&token, "future log guard", "writer", None)
        .await;
    seed_two_legacy_operations(&server, &id, &user, 2048, 2048).await;
    // Deliberately inject an inconsistent legacy future log. A normal commit
    // must neither erase evidence of it nor ACK past the history resource guard.
    let key = id.clone();
    server
        .db
        .run(move |conn| {
            diesel::sql_query(
                "UPDATE operation_log SET version=4 WHERE document_id=? AND version=3",
            )
            .bind::<Text, _>(key)
            .execute(conn)
        })
        .await
        .unwrap();
    let before = persistence_snapshot(&server, &id).await;
    let inventory = inventory_paths(&server).await;
    let old_path = before["file"]["path"].as_str().unwrap().to_owned();
    let old_blob = server.state.storage.read_file(&old_path).await.unwrap();
    let initial: Value =
        serde_json::from_str(before["document"]["content"].as_str().unwrap()).unwrap();
    let content = syntra_doc_engine::writer::writer_content_with_text(
        &initial,
        "Must not commit past future log",
    );
    let operation = syntra_collab::Operation::new(
        uuid::Uuid::parse_str(&user).unwrap(),
        uuid::Uuid::new_v4(),
        1,
        syntra_collab::OpType::Replace {
            path: vec![],
            old_value: initial,
            new_value: content.clone(),
        },
    )
    .with_base_version(3);
    let error = persistence::commit_document_operation(
        &server.db,
        &server.state.storage,
        &id,
        &user,
        3,
        serde_json::to_string(&content).unwrap(),
        &operation,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, syntra_core::error::AppError::Conflict(_)));
    assert_eq!(persistence_snapshot(&server, &id).await, before);
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        value: i64,
    }
    let key = id.clone();
    let count = server.db.run(move |conn| diesel::sql_query("SELECT COUNT(*) AS value FROM operation_log WHERE document_id=? AND version IN (2,4)")
        .bind::<Text,_>(key).get_result::<Count>(conn)).await.unwrap();
    assert_eq!(
        count.value, 2,
        "future evidence and old suffix must survive rollback"
    );
    let added = inventory_paths(&server)
        .await
        .difference(&inventory)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(added.len(), 1);
    assert_candidate_collected(&server, &added[0]).await;
    assert_eq!(
        server.state.storage.read_file(&old_path).await.unwrap(),
        old_blob
    );
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
    let mut objects = ControlledObjects::new().await;
    let server = TestServer::new_with_s3_endpoint(&objects.endpoint).await;
    let (token, _) = server
        .register("persist", "persist@example.test", "Persist!2026")
        .await;
    let (id, file, _) = server.create_document(&token, "CAS", "writer", None).await;
    let original = persistence_snapshot(&server, &id).await;
    objects
        .gate
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let save = |content: Value| {
        server
            .client
            .put(server.url(&format!("/api/v1/documents/{id}")))
            .bearer_auth(&token)
            .json(&json!({"content":content,"expected_version":1}))
            .send()
    };
    let saves = async { tokio::join!(save(json!({"text":"one"})), save(json!({"text":"two"}))) };
    tokio::pin!(saves);
    let candidates = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::select! {
            _ = &mut saves => panic!("save returned before both PUT gates were released"),
            paths = async {
                let first = objects.arrivals.recv().await.unwrap();
                let second = objects.arrivals.recv().await.unwrap();
                [first, second]
            } => paths,
        }
    })
    .await
    .expect("both requests must finish the old-v1 preflight and reach PUT");
    assert_ne!(candidates[0], candidates[1]);
    assert_eq!(persistence_snapshot(&server, &id).await, original);
    objects
        .gate
        .store(false, std::sync::atomic::Ordering::SeqCst);
    objects.release.add_permits(2);
    let (a, b) = saves.await;
    let (a, b) = (a.unwrap(), b.unwrap());
    let loser_content = if a.status() == 409 {
        json!({"text":"one"})
    } else {
        json!({"text":"two"})
    };
    let mut statuses = [a.status().as_u16(), b.status().as_u16()];
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
    let committed = persistence_snapshot(&server, &id).await;
    assert_eq!(committed["history"].as_array().unwrap().len(), 1);
    assert_eq!(committed["history"][0]["version_number"], 1);
    assert_eq!(
        committed["history"][0]["content"],
        original["document"]["content"]
    );
    assert_eq!(committed["file"]["size"], bytes.len() as i64);
    let live = committed["file"]["path"].as_str().unwrap();
    assert!(candidates.iter().any(|path| path == live));
    let abandoned = candidates.iter().find(|path| *path != live).unwrap();
    assert_candidate_collected(&server, abandoned).await;
    assert!(!objects.objects.lock().await.contains_key(abandoned));
    assert_eq!(
        objects.objects.lock().await.get(live),
        Some(&bytes.to_vec())
    );
    // The losing editor can explicitly rebase its preserved content on v2.
    let retry = server
        .client
        .put(server.url(&format!("/api/v1/documents/{id}")))
        .bearer_auth(&token)
        .json(&json!({"expected_version":2,"content":loser_content}))
        .send()
        .await
        .unwrap();
    assert_eq!(retry.status(), 200);
    let retried = persistence_snapshot(&server, &id).await;
    assert_eq!(retried["document"]["version"], 3);
    assert_eq!(retried["history"].as_array().unwrap().len(), 2);
    assert_eq!(
        retried["history"][1]["content"],
        committed["document"]["content"]
    );
    let downloaded = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/download")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(downloaded.status(), 200);
    let bytes = downloaded.bytes().await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        loser_content
    );
    assert_eq!(quota(&server, &token).await, bytes.len() as i64);
}

#[tokio::test]
async fn failed_restore_rolls_back_history_metadata_and_preserves_old_blob() {
    assert_failed_restore_rolls_back("files").await;
}

#[tokio::test]
async fn failed_restore_document_update_rolls_back_and_collects_new_blob() {
    assert_failed_restore_rolls_back("documents").await;
}

#[tokio::test]
async fn failed_restore_quota_update_rolls_back_and_collects_new_blob() {
    assert_failed_restore_rolls_back("users").await;
}

async fn assert_failed_restore_rolls_back(table: &str) {
    assert!(["documents", "files", "users"].contains(&table));
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
    let snapshot = persistence_snapshot(&server, &id).await;
    let old_path = snapshot["file"]["path"].as_str().unwrap();
    let old_blob = server.state.storage.read_file(old_path).await.unwrap();
    assert_ne!(
        snapshot["history"][0]["content"].as_str().unwrap().len(),
        old_blob.len(),
        "restoring the fixture must change the billed byte count"
    );
    let inventory = inventory_paths(&server).await;
    // Scope the quota fault to its billing column; authentication bookkeeping
    // must not be the operation that accidentally trips the injected failure.
    let column = if table == "users" {
        " OF storage_used"
    } else {
        ""
    };
    let marker = format!("injected restore {table} failure");
    sql(&server, format!("CREATE TRIGGER reject_restore_update BEFORE UPDATE{column} ON {table} BEGIN SELECT RAISE(ABORT, '{marker}'); END;")).await;
    let response = server
        .client
        .post(server.url(&format!("/api/v1/documents/{id}/restore/{version_id}")))
        .bearer_auth(&token)
        .json(&json!({"expected_version":2}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    let error = response.text().await.unwrap();
    assert!(
        error.contains(&marker),
        "restore must reach the intended {table} failure: {error}"
    );
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
    assert_eq!(persistence_snapshot(&server, &id).await, snapshot);
    assert_eq!(
        server.state.storage.read_file(old_path).await.unwrap(),
        old_blob
    );
    let after_inventory = inventory_paths(&server).await;
    let candidates: Vec<_> = after_inventory.difference(&inventory).cloned().collect();
    assert_eq!(
        candidates.len(),
        1,
        "restore must stage one independent blob before its {table} fault"
    );
    assert_ne!(candidates[0], old_path);
    assert_candidate_collected(&server, &candidates[0]).await;
    assert_eq!(
        server.state.storage.read_file(old_path).await.unwrap(),
        old_blob
    );
    assert_eq!(persistence_snapshot(&server, &id).await, snapshot);
    // A positive control distinguishes the injected SQL failure from an
    // unrelated restore error, and proves rollback left expected_version usable.
    sql(&server, "DROP TRIGGER reject_restore_update".into()).await;
    let restored = server
        .client
        .post(server.url(&format!("/api/v1/documents/{id}/restore/{version_id}")))
        .bearer_auth(&token)
        .json(&json!({"expected_version":2}))
        .send()
        .await
        .unwrap();
    assert_eq!(restored.status(), 200);
    let restored = persistence_snapshot(&server, &id).await;
    assert_eq!(restored["document"]["version"], 3);
    assert_eq!(restored["history"].as_array().unwrap().len(), 2);
    assert_eq!(
        restored["document"]["content"],
        snapshot["history"][0]["content"]
    );
    let downloaded = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/download")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(downloaded.status(), 200);
    let bytes = downloaded.bytes().await.unwrap();
    assert_eq!(restored["file"]["size"], bytes.len() as i64);
    assert_eq!(restored["quota"], bytes.len() as i64);
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        document(&server, &token, &id).await["content"]
    );
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
async fn expired_lock_takeover_contends_and_fences_the_previous_token() {
    use diesel::{connection::SimpleConnection, prelude::*};
    use syntra_core::{models::FileLock, schema::file_locks::dsl as fl};
    let (server, token, owner) = setup().await;
    let (_, peer_body) = server
        .register("takeover", "takeover@example.test", "Takeover!2026")
        .await;
    let peer = peer_body["data"]["user"]["id"].as_str().unwrap().to_owned();
    let (id, file, _) = server
        .create_document(&token, "expired lock", "writer", None)
        .await;
    let room = uuid::Uuid::new_v4();
    sql(&server, format!("INSERT INTO rooms(id,name,owner_id,created_at,updated_at) VALUES ('{room}','takeover','{owner}','now','now'); INSERT INTO room_members(id,room_id,user_id,role,joined_at) VALUES ('takeover-owner','{room}','{owner}','owner','now'),('takeover-peer','{room}','{peer}','editor','now'); UPDATE files SET room_id='{room}' WHERE id='{file}';")).await;
    let old = persistence::acquire_file_lock(&server.db, &file, &owner, "expired-token")
        .await
        .unwrap();
    let old_id = old.id.clone();
    let expired = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    server
        .db
        .run(move |conn| {
            diesel::update(fl::file_locks.find(old_id))
                .set(fl::expires_at.eq(expired))
                .execute(conn)
        })
        .await
        .unwrap();

    // Exchange credentials and warm HTTP bookkeeping before taking the SQL
    // writer gate. The contending tasks below call the real lock service, so
    // their held leases cannot be mistaken for authentication writes.
    let response = server
        .client
        .get(server.url("/api/v1/wopi/host/writer/edit"))
        .query(&[
            ("access_token", token.as_str()),
            (
                "WOPISrc",
                server.url(&format!("/api/v1/wopi/files/{file}")).as_str(),
            ),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let html = response.text().await.unwrap();
    let start = html.find("const wopiSessionToken = ").unwrap() + "const wopiSessionToken = ".len();
    let wopi: String = serde_json::from_str(html[start..].split(';').next().unwrap()).unwrap();
    let before = persistence_snapshot(&server, &id).await;

    // Initialize both pooled connections before imposing the external writer
    // lock. Two channel acknowledgements prove both leases exist; no sleep or
    // cold connection setup is allowed to stand in for lock contention.
    let warmup_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let (entered, mut arrivals) = tokio::sync::mpsc::unbounded_channel();
    let mut warmers = Vec::new();
    let mut releases = Vec::new();
    for _ in 0..2 {
        let (release, wait) = std::sync::mpsc::channel();
        releases.push(release);
        let (db, entered) = (server.db.clone(), entered.clone());
        warmers.push(tokio::spawn(async move {
            db.run(move |conn| {
                diesel::sql_query("SELECT 1").execute(conn)?;
                entered.send(()).unwrap();
                wait.recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                Ok::<_, syntra_core::error::AppError>(())
            })
            .await
            .unwrap();
        }));
    }
    for _ in 0..2 {
        tokio::time::timeout_at(warmup_deadline, arrivals.recv())
            .await
            .unwrap()
            .unwrap();
    }
    for release in releases {
        release.send(()).unwrap();
    }
    for warmer in warmers {
        tokio::time::timeout_at(warmup_deadline, warmer)
            .await
            .expect("both warmup SQL workers must finish within the original budget")
            .unwrap();
    }
    // run() returns the SQL result before detached lease cleanup returns the
    // Object to the pool. Observe that completion under the same warmup budget.
    tokio::time::timeout_at(warmup_deadline, async {
        loop {
            let state = server.db.state();
            if state.connections == 2
                && state.idle_connections == 2
                && state.admitted_operations == 0
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both prewarmed leases must finish cleanup before writer contention");
    assert_eq!(server.db.state().idle_connections, 2);
    let mut writer =
        diesel::SqliteConnection::establish(&server.state.config.database.url).unwrap();
    writer.batch_execute("BEGIN IMMEDIATE").unwrap();
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    let acquire = |user: String, token: &'static str| {
        let (db, file, start) = (server.db.clone(), file.clone(), start.clone());
        tokio::spawn(async move {
            start.wait().await;
            persistence::acquire_file_lock(&db, &file, &user, token).await
        })
    };
    let checkouts = server.db.state().checkouts;
    let a = acquire(owner.clone(), "takeover-a");
    let b = acquire(peer, "takeover-b");
    start.wait().await;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            assert!(
                !a.is_finished() && !b.is_finished(),
                "neither takeover may bypass the held writer"
            );
            let state = server.db.state();
            if state.idle_connections == 0
                && state.admitted_operations == 2
                && state.checkouts >= checkouts + 2
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both real lock-service SQL workers must contend behind the writer gate");
    writer.batch_execute("COMMIT").unwrap();
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    let winner = match (a, b) {
        (Ok(winner), Err(error)) | (Err(error), Ok(winner)) => {
            assert_eq!(error.status_code(), 409);
            winner
        }
        result => panic!("expired takeover must have one winner and one conflict: {result:?}"),
    };
    assert_ne!(winner.id, old.id);
    assert_ne!(winner.lock_token, old.lock_token);
    let load_lock = || {
        let (db, file) = (server.db.clone(), file.clone());
        async move {
            db.run(move |conn| {
                fl::file_locks
                    .filter(fl::file_id.eq(file))
                    .load::<FileLock>(conn)
            })
            .await
            .unwrap()
        }
    };
    let rows = load_lock().await;
    assert_eq!(rows.len(), 1);
    let winner_snapshot = serde_json::to_value(&rows[0]).unwrap();
    assert_eq!(winner_snapshot, serde_json::to_value(&winner).unwrap());
    assert!(chrono::DateTime::parse_from_rfc3339(&winner.expires_at).unwrap() > chrono::Utc::now());
    let url = server.url(&format!("/api/v1/wopi/files/{file}"));
    for operation in ["REFRESH_LOCK", "UNLOCK", "LOCK"] {
        let mut request = server
            .client
            .post(&url)
            .query(&[("access_token", &wopi)])
            .header("X-WOPI-Override", operation)
            .header("X-WOPI-Lock", &old.lock_token);
        if operation == "LOCK" {
            request = request
                .header("X-WOPI-Lock", "stale-replacement")
                .header("X-WOPI-OldLock", &old.lock_token);
        }
        let response = request.send().await.unwrap();
        assert_eq!(
            response.status(),
            409,
            "expired token must not {operation} the winner"
        );
        assert_eq!(
            response.headers()["X-WOPI-Lock"].to_str().unwrap(),
            winner.lock_token
        );
        let rows = load_lock().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(serde_json::to_value(&rows[0]).unwrap(), winner_snapshot);
    }
    let rejected = server
        .client
        .post(format!("{url}/contents"))
        .query(&[("access_token", &wopi)])
        .header("X-WOPI-Lock", &old.lock_token)
        .body(r#"{"text":"stale overwrite"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), 409);
    assert_eq!(
        rejected.headers()["X-WOPI-Lock"].to_str().unwrap(),
        winner.lock_token
    );
    assert_eq!(persistence_snapshot(&server, &id).await, before);
    let rows = load_lock().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(serde_json::to_value(&rows[0]).unwrap(), winner_snapshot);
    let downloaded = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}/download")))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(downloaded.status(), 200);
    assert_eq!(
        downloaded.json::<Value>().await.unwrap(),
        document(&server, &token, &id).await["content"]
    );
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
    let operation = syntra_collab::Operation::new(
        owner.parse().unwrap(),
        uuid::Uuid::new_v4(),
        1,
        syntra_collab::operation::OpType::Replace {
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
    forged_payload.op_type = syntra_collab::operation::OpType::Replace {
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
    original.op_type = syntra_collab::OpType::Insert {
        path: vec!["text".into()],
        index: 0,
        content: "x".into(),
    };
    let original_hash = persistence::operation_request_hash(&original).unwrap();
    let mut transformed = original.clone();
    transformed.base_version = 2;
    transformed.op_type = syntra_collab::OpType::Insert {
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
    let mut writer = syntra_doc_engine::WriterDocument::new("Original".into());
    writer.add_paragraph("Keep original OOXML bytes");
    let original = syntra_export::docx::writer_to_docx(&writer).unwrap();
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
                    Ok::<_, syntra_core::error::AppError>(
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
    let never_polled = staged.commit(|_| -> Result<(), syntra_core::error::AppError> {
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
        syntra_server::services::s3_storage::S3Storage::new(
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
