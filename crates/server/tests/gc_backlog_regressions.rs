#[allow(dead_code)]
mod common;

use std::time::Duration;

use common::TestServer;
use contrix_core::error::AppError;
use contrix_server::services::persistence;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    value: i64,
}

async fn count(server: &TestServer, query: &'static str) -> i64 {
    server
        .db
        .run(move |connection| {
            diesel::sql_query(query)
                .get_result::<Count>(connection)
                .map(|row| row.value)
        })
        .await
        .unwrap()
}

async fn enqueue(server: &TestServer, paths: Vec<String>) {
    server
        .db
        .run(move |connection| {
            connection.immediate_transaction::<_, diesel::result::Error, _>(|connection| {
                for path in paths {
                    diesel::sql_query(
                        "INSERT INTO blob_inventory(path,state,first_seen) VALUES (?,'ready',0)",
                    )
                    .bind::<Text, _>(&path)
                    .execute(connection)?;
                    diesel::sql_query(
                        "INSERT INTO storage_gc(path,created_at) VALUES (?,'2000-01-01T00:00:00Z')",
                    )
                    .bind::<Text, _>(path)
                    .execute(connection)?;
                }
                Ok(())
            })
        })
        .await
        .unwrap();
}

async fn objects(server: &TestServer, prefix: &str, total: usize) -> Vec<String> {
    let directory = server._temp_dir.path().join("files").join(prefix);
    tokio::fs::create_dir_all(&directory).await.unwrap();
    let paths: Vec<_> = (0..total)
        .map(|index| format!("{prefix}/{index:04}"))
        .collect();
    for path in &paths {
        tokio::fs::write(server._temp_dir.path().join("files").join(path), b"retired")
            .await
            .unwrap();
    }
    enqueue(server, paths.clone()).await;
    paths
}

#[tokio::test(flavor = "current_thread")]
async fn coalesced_gc_catches_up_without_new_traffic_or_failed_object_starvation() {
    let server = TestServer::new().await;
    let paths = objects(&server, "gc", 1024).await;
    let storage = &server.state.storage;
    assert!(matches!(
        storage
            .write_file("after-drain", b"new object", "application/octet-stream")
            .await,
        Err(AppError::RateLimit(message)) if message.contains("cleanup is backlogged")
    ));
    assert!(!server._temp_dir.path().join("files/after-drain").exists());
    assert_eq!(
        count(
            &server,
            "SELECT COUNT(*) AS value FROM blob_inventory WHERE path='after-drain'"
        )
        .await,
        0,
        "the full-backlog rejection must happen before reserving or writing an object"
    );

    // The spawned worker cannot run between these synchronous calls on this
    // current-thread runtime. The second hint therefore exercises coalescing.
    assert!(persistence::request_gc(&server.db, storage));
    assert!(!persistence::request_gc(&server.db, storage));
    tokio::time::timeout(Duration::from_secs(30), async {
        while count(&server, "SELECT COUNT(*) AS value FROM storage_gc").await != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("one coalesced wakeup must clear more than one 100-object page");
    assert_eq!(
        count(
            &server,
            "SELECT COUNT(*) AS value FROM blob_inventory WHERE state='deleted'"
        )
        .await,
        1024
    );
    for path in paths {
        assert!(!server._temp_dir.path().join("files").join(path).exists());
    }
    storage
        .write_file("after-drain", b"new object", "application/octet-stream")
        .await
        .unwrap();
    assert_eq!(
        storage.read_file("after-drain").await.unwrap(),
        b"new object"
    );

    // A separate prefix keeps the first phase's durable tombstones intact.
    // One failed object must not stop the worker at its first claim batch.
    let paths = objects(&server, "partial", 131).await;
    let bad = server._temp_dir.path().join("files").join(&paths[0]);
    tokio::fs::remove_file(&bad).await.unwrap();
    tokio::fs::create_dir(&bad).await.unwrap();
    persistence::request_gc(&server.db, storage);
    tokio::time::timeout(Duration::from_secs(30), async {
        while count(&server, "SELECT COUNT(*) AS value FROM storage_gc").await != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("one bad object must not stop other pages without more hints");
    assert!(bad.is_dir());
    assert_eq!(
        count(
            &server,
            "SELECT COUNT(*) AS value FROM storage_gc g JOIN blob_inventory i ON i.path=g.path \
             WHERE g.path='partial/0000' AND g.claim_token IS NULL AND g.claim_until=0 AND i.state='deleting'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &server,
            "SELECT COUNT(*) AS value FROM blob_inventory WHERE path LIKE 'partial/%' AND state='deleted'"
        )
        .await,
        130
    );
    for path in &paths[1..] {
        assert!(!server._temp_dir.path().join("files").join(path).exists());
    }
}

#[tokio::test]
async fn failing_delete_preserves_successes_across_the_public_drain_page() {
    let server = TestServer::new().await;
    let paths = objects(&server, "gc", 131).await;
    let bad = server._temp_dir.path().join("files").join(&paths[0]);
    tokio::fs::remove_file(&bad).await.unwrap();
    tokio::fs::create_dir(&bad).await.unwrap();

    assert!(matches!(
        persistence::drain_storage_gc(&server.db, &server.state.storage).await,
        Err(AppError::Storage(_))
    ));
    assert!(bad.is_dir());
    assert_eq!(
        count(&server, "SELECT COUNT(*) AS value FROM storage_gc").await,
        32
    );
    assert_eq!(
        count(
            &server,
            "SELECT COUNT(*) AS value FROM storage_gc g JOIN blob_inventory i ON i.path=g.path \
             WHERE g.path='gc/0000' AND g.claim_token IS NULL AND g.claim_until=0 AND i.state='deleting'"
        )
        .await,
        1,
        "failed deletion stays queued and fenced, with its worker claim released"
    );
    assert_eq!(
        count(
            &server,
            "SELECT COUNT(*) AS value FROM blob_inventory WHERE state='deleted'"
        )
        .await,
        99
    );
    for path in &paths[1..100] {
        assert!(!server._temp_dir.path().join("files").join(path).exists());
    }
    for path in &paths[100..] {
        assert!(server._temp_dir.path().join("files").join(path).is_file());
    }

    // Once the fixture's directory obstruction is removed, the retained
    // outbox row can finalize an already-absent object idempotently.
    tokio::fs::remove_dir(&bad).await.unwrap();
    persistence::drain_storage_gc(&server.db, &server.state.storage)
        .await
        .unwrap();
    assert_eq!(
        count(&server, "SELECT COUNT(*) AS value FROM storage_gc").await,
        0
    );
    assert_eq!(
        count(
            &server,
            "SELECT COUNT(*) AS value FROM blob_inventory WHERE state='deleted'"
        )
        .await,
        131
    );
}

#[derive(Debug, PartialEq, QueryableByName)]
struct ProtectedObject {
    #[diesel(sql_type = Text)]
    path: String,
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = Nullable<Text>)]
    token: Option<String>,
    #[diesel(sql_type = BigInt)]
    lease_until: i64,
    #[diesel(sql_type = Nullable<Text>)]
    claim_token: Option<String>,
    #[diesel(sql_type = BigInt)]
    claim_until: i64,
}

async fn protected_objects(server: &TestServer) -> Vec<ProtectedObject> {
    server
        .db
        .run(|connection| {
            diesel::sql_query(
                "SELECT i.path,i.state,i.token,i.lease_until,g.claim_token,g.claim_until \
                 FROM blob_inventory i JOIN storage_gc g ON g.path=i.path ORDER BY i.path",
            )
            .load(connection)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn live_writer_and_another_workers_claim_are_preserved_while_ready_objects_progress() {
    let server = TestServer::new().await;
    let paths = objects(&server, "gc", 3).await;
    let deadline = chrono::Utc::now().timestamp() + 3600;
    server
        .db
        .run(move |connection| {
            connection.immediate_transaction::<_, diesel::result::Error, _>(|connection| {
                diesel::sql_query(
                    "UPDATE blob_inventory SET state='writing',token='live-writer',lease_until=? WHERE path='gc/0000'",
                )
                .bind::<BigInt, _>(deadline)
                .execute(connection)?;
                diesel::sql_query(
                    "UPDATE blob_inventory SET state='deleting' WHERE path='gc/0001'",
                )
                .execute(connection)?;
                diesel::sql_query(
                    "UPDATE storage_gc SET claim_token='another-worker',claim_until=? WHERE path='gc/0001'",
                )
                .bind::<BigInt, _>(deadline)
                .execute(connection)?;
                Ok(())
            })
        })
        .await
        .unwrap();
    let before = protected_objects(&server).await;

    persistence::drain_storage_gc(&server.db, &server.state.storage)
        .await
        .unwrap();
    assert_eq!(protected_objects(&server).await, before[..2]);
    assert_eq!(
        count(&server, "SELECT COUNT(*) AS value FROM storage_gc").await,
        2
    );
    for path in &paths[..2] {
        assert_eq!(
            server.state.storage.read_file(path).await.unwrap(),
            b"retired"
        );
    }
    assert!(
        !server
            ._temp_dir
            .path()
            .join("files")
            .join(&paths[2])
            .exists()
    );
    assert_eq!(
        count(
            &server,
            "SELECT COUNT(*) AS value FROM blob_inventory WHERE path='gc/0002' AND state='deleted'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn explicit_gc_drain_keeps_the_existing_hundred_object_page_bound() {
    let server = TestServer::new().await;
    let paths = objects(&server, "gc", 131).await;
    persistence::drain_storage_gc(&server.db, &server.state.storage)
        .await
        .unwrap();
    assert_eq!(
        count(&server, "SELECT COUNT(*) AS value FROM storage_gc").await,
        31
    );
    assert_eq!(
        count(
            &server,
            "SELECT COUNT(*) AS value FROM blob_inventory WHERE state='deleted'"
        )
        .await,
        100
    );
    for path in &paths[..100] {
        assert!(!server._temp_dir.path().join("files").join(path).exists());
    }
    for path in &paths[100..] {
        assert!(server._temp_dir.path().join("files").join(path).is_file());
    }
}
