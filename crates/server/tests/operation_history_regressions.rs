//! Shared policy and the real idle owner; no HTTP traffic is needed to retry.
//! Synthetic equal-version/non-JSON rows exercise catalog retention metadata,
//! not production-valid WebSocket operations (covered by messaging tests).
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Duration;

use contrix_core::error::AppError;
use contrix_server::services::operation_history::{
    MAX_HISTORY_BYTES, MAX_HISTORY_ROWS, OperationHistoryCursor, catch_up_operation_history_page,
    has_future_history, prune_operation_history_page, run_operation_history_retention,
};
use contrix_server::state::DbPool;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Integer, Text};

#[derive(Clone, Copy, Default)]
struct AllocationCount {
    enabled: bool,
    calls: usize,
    bytes: usize,
}

thread_local! {
    // Count only this synchronous test thread, excluding Tokio/pool workers and
    // other tests. SQLite's C allocations are intentionally outside this probe.
    static ALLOCATIONS: Cell<AllocationCount> = const { Cell::new(AllocationCount {
        enabled: false, calls: 0, bytes: 0,
    }) };
}

struct CountingAllocator;

fn record_allocation(bytes: usize) {
    let _ = ALLOCATIONS.try_with(|state| {
        let mut count = state.get();
        if count.enabled {
            count.calls += 1;
            count.bytes += bytes;
            state.set(count);
        }
    });
}

// SAFETY: Every allocation/deallocation forwards the original pointer, layout
// and size to System. The observer only updates allocation-free thread locals.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        // SAFETY: Forward the caller's allocation contract unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        // SAFETY: Forward the caller's allocation contract unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation(size);
        // SAFETY: Forward the caller's allocation contract unchanged.
        unsafe { System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: System owns the same pointer and layout from allocation.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn measure_allocations<T>(operation: impl FnOnce() -> T) -> (T, AllocationCount) {
    struct StopCounting;
    impl Drop for StopCounting {
        fn drop(&mut self) {
            ALLOCATIONS.with(|state| {
                let mut count = state.get();
                count.enabled = false;
                state.set(count);
            });
        }
    }
    ALLOCATIONS.with(|state| {
        assert!(!state.get().enabled, "allocation measurements cannot nest");
        state.set(AllocationCount {
            enabled: true,
            ..Default::default()
        });
    });
    let guard = StopCounting;
    let result = operation();
    let count = ALLOCATIONS.with(Cell::get);
    drop(guard);
    (result, count)
}

#[derive(QueryableByName)]
struct Number {
    #[diesel(sql_type = BigInt)]
    value: i64,
}
#[derive(QueryableByName)]
struct Plan {
    #[diesel(sql_type = Text)]
    detail: String,
}

async fn fixture() -> (tempfile::TempDir, DbPool) {
    let directory = tempfile::tempdir().unwrap();
    let db = DbPool::with_timeouts(
        directory.path().join("history.sqlite").to_str().unwrap(),
        2,
        Duration::from_secs(3),
        Duration::from_secs(15),
        Duration::from_secs(3),
    )
    .unwrap();
    db.run_startup_migrations(|conn| {
        contrix_server::migrations::run_directory(
            conn,
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations"),
        )
    })
    .await
    .unwrap();
    db.run(|conn| {
        conn.batch_execute("INSERT INTO users(id,username,email,password_hash) VALUES ('owner','owner','owner@example.invalid','fixture-only')")
    }).await.unwrap();
    (directory, db)
}

async fn seed(db: &DbPool, doc: &str, count: usize, current: i32) {
    let doc = doc.to_owned();
    db.run(move |conn| conn.immediate_transaction::<_, AppError, _>(|conn| {
        diesel::sql_query("INSERT INTO files(id,name,path,owner_id) VALUES (?,?,?,'owner')")
            .bind::<Text,_>(&doc).bind::<Text,_>(&doc).bind::<Text,_>(format!("fixture/{doc}"))
            .execute(conn)?;
        diesel::sql_query("INSERT INTO documents(id,file_id,doc_type,content,version) VALUES (?,?,'writer','{}',?)")
            .bind::<Text,_>(&doc).bind::<Text,_>(&doc).bind::<Integer,_>(current).execute(conn)?;
        for index in 0..count {
            diesel::sql_query("INSERT INTO operation_log(id,document_id,user_id,op_type,op_data,version,created_at) VALUES (?,?,'owner','{}','x',1,'2000-01-01T00:00:00Z')")
                .bind::<Text,_>(format!("{doc}-{index:05}")).bind::<Text,_>(&doc).execute(conn)?;
        }
        Ok(())
    })).await.unwrap();
}

async fn count(db: &DbPool, doc: &str) -> i64 {
    let doc = doc.to_owned();
    db.run(move |conn| {
        diesel::sql_query("SELECT COUNT(*) AS value FROM operation_log WHERE document_id=?")
            .bind::<Text, _>(doc)
            .get_result::<Number>(conn)
            .map(|row| row.value)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn exact_row_boundary_and_ties_are_deterministic_and_keep_future_records() {
    let (_directory, db) = fixture().await;
    seed(&db, "doc", MAX_HISTORY_ROWS, 1).await;
    seed(&db, "other", 1, 1).await;
    db.run(|conn| {
        let exact = conn.immediate_transaction::<_,AppError,_>(|conn| prune_operation_history_page(conn,"doc",1,1000))?;
        assert_eq!(exact.removed,0);
        assert!(!exact.has_excess && !exact.has_future);
        conn.batch_execute("INSERT INTO operation_log(id,document_id,user_id,op_type,op_data,version,created_at) VALUES ('doc-02048','doc','owner','{}','x',1,'2000-01-01T00:00:00Z'),('future','doc','owner','{}','x',2,'2000-01-01T00:00:00Z')")?;
        let inspected = prune_operation_history_page(conn,"doc",1,0)?;
        assert_eq!(inspected.removed,0);
        assert!(inspected.has_excess && inspected.has_future);
        let page = conn.immediate_transaction::<_,AppError,_>(|conn| prune_operation_history_page(conn,"doc",1,1000))?;
        assert_eq!(page.removed,1);
        assert!(!page.has_excess && page.has_future);
        assert!(has_future_history(conn,"doc",1)?);
        let oldest = diesel::sql_query("SELECT COUNT(*) AS value FROM operation_log WHERE id='doc-00000'").get_result::<Number>(conn)?;
        assert_eq!(oldest.value,0,"lowest id in an equal-version tie is discarded");
        let future = diesel::sql_query("SELECT COUNT(*) AS value FROM operation_log WHERE id='future'").get_result::<Number>(conn)?;
        assert_eq!(future.value,1);
        Ok::<_,AppError>(())
    }).await.unwrap();
    assert_eq!(
        count(&db, "doc").await,
        2049,
        "2048 eligible plus untouched future"
    );
    assert_eq!(count(&db, "other").await, 1);
}

#[tokio::test]
async fn legacy_backlog_deletes_only_a_bounded_page_until_the_policy_is_met() {
    let (_directory, db) = fixture().await;
    seed(&db, "large", 5000, 1).await;
    let mut removed = Vec::new();
    loop {
        let page = db
            .run(|conn| {
                conn.immediate_transaction::<_, AppError, _>(|conn| {
                    prune_operation_history_page(conn, "large", 1, usize::MAX)
                })
            })
            .await
            .unwrap();
        removed.push(page.removed);
        assert!(page.removed <= 1000);
        if !page.has_excess {
            break;
        }
    }
    assert_eq!(removed, vec![1000, 1000, 952]);
    assert_eq!(count(&db, "large").await, MAX_HISTORY_ROWS as i64);
}

#[tokio::test]
async fn real_byte_boundary_and_an_oversized_newest_record_are_enforced() {
    let (_directory, db) = fixture().await;
    seed(&db, "bytes", 0, 4).await;
    db.run(|conn| {
        // SQLite supplies the real byte-sized value; no giant Rust fixture buffer.
        diesel::sql_query("INSERT INTO operation_log(id,document_id,user_id,op_type,op_data,version,created_at) VALUES ('exact','bytes','owner','{}',CAST(zeroblob(?) AS TEXT),3,'2000-01-01T00:00:00Z')")
            .bind::<BigInt,_>(MAX_HISTORY_BYTES).execute(conn)?;
        conn.batch_execute("INSERT INTO operation_log(id,document_id,user_id,op_type,op_data,version,created_at) VALUES ('old','bytes','owner','{}','é',2,'2000-01-01T00:00:00Z')")?;
        let page=conn.immediate_transaction::<_,AppError,_>(|conn| prune_operation_history_page(conn,"bytes",4,1000))?;
        assert_eq!(page.removed,1);
        assert!(!page.has_excess);
        let bytes=diesel::sql_query("SELECT SUM(LENGTH(CAST(op_data AS BLOB))) AS value FROM operation_log WHERE document_id='bytes'").get_result::<Number>(conn)?;
        assert_eq!(bytes.value,MAX_HISTORY_BYTES,"exactly32MiB remains legal");
        diesel::sql_query("INSERT INTO operation_log(id,document_id,user_id,op_type,op_data,version,created_at) VALUES ('oversized','bytes','owner','{}',CAST(zeroblob(?) AS TEXT),4,'2000-01-01T00:00:00Z')")
            .bind::<BigInt,_>(MAX_HISTORY_BYTES+1).execute(conn)?;
        let page=conn.immediate_transaction::<_,AppError,_>(|conn| prune_operation_history_page(conn,"bytes",4,1000))?;
        assert_eq!(page.removed,2,"oversized newest entry cannot create a budget exception");
        assert!(!page.has_excess);
        Ok::<_,AppError>(())
    }).await.unwrap();
    assert_eq!(count(&db, "bytes").await, 0);
}

#[tokio::test]
async fn migration_index_supports_metadata_and_delete_ranges_without_a_sort() {
    let (_directory, db) = fixture().await;
    db.run(|conn| {
        for sql in [
            "EXPLAIN QUERY PLAN SELECT LENGTH(CAST(op_data AS BLOB)) AS bytes FROM operation_log WHERE document_id='doc' AND version<=10 ORDER BY version DESC,id DESC LIMIT 2049",
            "EXPLAIN QUERY PLAN SELECT id,version FROM operation_log WHERE document_id='doc' AND version<=10 ORDER BY version DESC,id DESC LIMIT 1 OFFSET 2048",
            "EXPLAIN QUERY PLAN SELECT id FROM operation_log WHERE document_id='doc' AND (version,id)<=(9,'cutoff') ORDER BY version DESC,id DESC LIMIT 1000",
        ] {
            let plan=diesel::sql_query(sql).load::<Plan>(conn)?;
            assert!(plan.iter().any(|row|row.detail.contains("SEARCH")&&row.detail.contains("idx_operation_log_retention")),"actual migration index must be usable");
            assert!(plan.iter().all(|row|!row.detail.contains("TEMP B-TREE")),"equal-version ties must not require a sort");
        }
        Ok::<_,AppError>(())
    }).await.unwrap();
}

#[tokio::test]
async fn bad_document_and_large_backlog_do_not_starve_later_entities() {
    let (_directory, db) = fixture().await;
    seed(&db, "00-bad", 2049, 1).await;
    seed(&db, "01-hot", 5000, 1).await;
    for index in 2..23 {
        let rows = if index == 22 { 2049 } else { 1 };
        seed(&db, &format!("{index:02}-healthy"), rows, 1).await;
    }
    db.run(|conn|conn.batch_execute("CREATE TRIGGER refuse_one_history_delete BEFORE DELETE ON operation_log WHEN OLD.document_id='00-bad' BEGIN SELECT RAISE(ABORT,'injected document-local delete failure'); END")).await.unwrap();
    let mut cursor = OperationHistoryCursor::default();
    let first = catch_up_operation_history_page(&db, &mut cursor)
        .await
        .unwrap();
    assert_eq!(first.failed_entities, 1);
    assert_eq!(first.removed, 1000);
    assert!(first.first_error.is_some());
    for _ in 0..7 {
        let page = catch_up_operation_history_page(&db, &mut cursor)
            .await
            .unwrap();
        assert!(page.visited_entities <= 16 && page.removed <= 1000);
    }
    assert_eq!(
        count(&db, "22-healthy").await,
        2048,
        "later entities pass the permanently bad and hot documents"
    );
    assert_eq!(
        count(&db, "00-bad").await,
        2049,
        "failed deletion remains pending"
    );
    db.run(|conn| conn.batch_execute("DROP TRIGGER refuse_one_history_delete"))
        .await
        .unwrap();
    let retried = catch_up_operation_history_page(&db, &mut cursor)
        .await
        .unwrap();
    assert_eq!(retried.failed_entities, 0);
    assert_eq!(
        count(&db, "00-bad").await,
        2048,
        "priority retry is not lost by keyset progress"
    );
}

#[tokio::test]
async fn an_overall_scan_error_keeps_the_cursor_for_retry() {
    let (_directory, db) = fixture().await;
    seed(&db, "doc", 2049, 1).await;
    let mut cursor = OperationHistoryCursor::default();
    db.run(|conn| conn.batch_execute("ALTER TABLE documents RENAME TO unavailable_documents"))
        .await
        .unwrap();
    let previous = cursor.clone();
    assert!(
        catch_up_operation_history_page(&db, &mut cursor)
            .await
            .is_err()
    );
    assert_eq!(cursor, previous);
    db.run(|conn| conn.batch_execute("ALTER TABLE unavailable_documents RENAME TO documents"))
        .await
        .unwrap();
    let page = catch_up_operation_history_page(&db, &mut cursor)
        .await
        .unwrap();
    assert_eq!(page.removed, 1);
}

#[tokio::test]
async fn real_background_owner_retries_after_fault_removal_without_new_operations() {
    let (_directory, db) = fixture().await;
    seed(&db, "00-bad", 2049, 1).await;
    seed(&db, "01-good", 2049, 1).await;
    db.run(|conn|conn.batch_execute("CREATE TRIGGER refuse_one_history_delete BEFORE DELETE ON operation_log WHEN OLD.document_id='00-bad' BEGIN SELECT RAISE(ABORT,'injected document-local delete failure'); END")).await.unwrap();
    let worker = tokio::spawn(run_operation_history_retention(
        db.clone(),
        Duration::from_millis(10),
    ));
    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        while count(&db, "01-good").await != 2048 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(count(&db, "00-bad").await, 2049);
        db.run(|conn| conn.batch_execute("DROP TRIGGER refuse_one_history_delete"))
            .await
            .unwrap();
        // Read-only polling: no HTTP/WS operation, leave or other producer hint.
        while count(&db, "00-bad").await != 2048 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    observed.expect("independent owner must retry without a new operation");
}

#[tokio::test]
async fn clean_catch_up_is_read_only_while_an_external_writer_holds_its_lock() {
    let (directory, db) = fixture().await;
    seed(&db, "clean", MAX_HISTORY_ROWS, 1).await;
    let mut writer =
        SqliteConnection::establish(directory.path().join("history.sqlite").to_str().unwrap())
            .unwrap();
    writer.batch_execute("BEGIN IMMEDIATE").unwrap();
    let mut cursor = OperationHistoryCursor::default();
    let result = catch_up_operation_history_page(&db, &mut cursor).await;
    writer.batch_execute("ROLLBACK").unwrap();
    let page = result.expect("WAL read-only inspection must not wait for the writer reservation");
    assert_eq!(page.visited_entities, 1);
    assert_eq!(page.removed, 0);
    assert_eq!(page.failed_entities, 0);
    assert!(page.first_error.is_none());
}

#[test]
fn inspection_does_not_materialize_an_id_string_for_each_retained_row() {
    #[derive(QueryableByName)]
    struct MaterializedEntry {
        #[diesel(sql_type = Text)]
        id: String,
        #[diesel(sql_type = Integer)]
        version: i32,
        #[diesel(sql_type = BigInt)]
        bytes: i64,
    }

    let mut conn = SqliteConnection::establish(":memory:").unwrap();
    // Use the actual table and index, without pool scheduling in the measured
    // thread. These rows only exercise the synchronous catalog inspection.
    conn.batch_execute(include_str!("../../../migrations/014_operation_log.sql"))
        .unwrap();
    conn.batch_execute(include_str!(
        "../../../migrations/027_operation_history_retention.sql"
    ))
    .unwrap();
    conn.transaction::<_, diesel::result::Error, _>(|conn| {
        for (document, rows) in [("short", 16), ("large", MAX_HISTORY_ROWS + 1)] {
            for index in 0..rows {
                diesel::sql_query("INSERT INTO operation_log(id,document_id,user_id,op_type,op_data,version,created_at) VALUES (?,?,'owner','update','x',1,'2000')")
                    .bind::<Text, _>(format!("{document}-00000000-0000-4000-8000-{index:012}"))
                    .bind::<Text, _>(document)
                    .execute(conn)?;
            }
        }
        Ok(())
    })
    .unwrap();
    for document in ["short", "large"] {
        conn.transaction::<_, AppError, _>(|conn| {
            prune_operation_history_page(conn, document, 1, 0)
        })
        .unwrap();
    }

    let (short, short_allocations) = measure_allocations(|| {
        conn.transaction::<_, AppError, _>(|conn| prune_operation_history_page(conn, "short", 1, 0))
            .unwrap()
    });
    let (large, large_allocations) = measure_allocations(|| {
        conn.transaction::<_, AppError, _>(|conn| prune_operation_history_page(conn, "large", 1, 0))
            .unwrap()
    });
    // Positive control: this is the old eager metadata projection. It proves
    // the observer sees real Diesel String/Vec allocations on this platform.
    let (materialized, eager_allocations) = measure_allocations(|| {
        diesel::sql_query("SELECT id,version,LENGTH(CAST(op_data AS BLOB)) AS bytes FROM operation_log WHERE document_id='large' AND version<=1 ORDER BY version DESC,id DESC LIMIT 2049")
            .load::<MaterializedEntry>(&mut conn).unwrap()
    });
    assert!(!short.has_excess);
    assert!(large.has_excess);
    assert_eq!(large.removed, 0);
    assert_eq!(materialized.len(), MAX_HISTORY_ROWS + 1);
    assert!(
        materialized
            .iter()
            .all(|row| { row.id.starts_with("large-") && row.version == 1 && row.bytes == 1 })
    );
    assert!(eager_allocations.calls >= MAX_HISTORY_ROWS);
    // Allow bounded statement/bind/cutoff bookkeeping, without asserting an
    // allocator-specific exact count or measuring SQLite's own C heap/RSS.
    assert!(
        large_allocations.calls <= short_allocations.calls + 64,
        "2049 rows must not add per-row Rust allocations: short={}, large={}, eager={}",
        short_allocations.calls,
        large_allocations.calls,
        eager_allocations.calls
    );
    assert!(large_allocations.bytes <= short_allocations.bytes + 16 * 1024);
    assert!(large_allocations.calls * 16 < eager_allocations.calls);
    eprintln!(
        "history allocations short={}/{}B large={}/{}B eager={}/{}B",
        short_allocations.calls,
        short_allocations.bytes,
        large_allocations.calls,
        large_allocations.bytes,
        eager_allocations.calls,
        eager_allocations.bytes
    );
}

#[tokio::test]
async fn negative_version_ties_and_utf8_nul_bytes_keep_the_same_cutoff() {
    let (_directory, db) = fixture().await;
    seed(&db, "negative", MAX_HISTORY_ROWS + 1, -2).await;
    db.run(|conn| {
        conn.batch_execute("UPDATE operation_log SET version=-2 WHERE document_id='negative'; INSERT INTO operation_log(id,document_id,user_id,op_type,op_data,version,created_at) VALUES ('future-negative','negative','owner','{}','x',-1,'2000')")?;
        diesel::sql_query("UPDATE operation_log SET op_data=? WHERE id='negative-00000'")
            .bind::<Text, _>("é\0x")
            .execute(conn)?;
        let bytes = diesel::sql_query("SELECT LENGTH(CAST(op_data AS BLOB)) AS value FROM operation_log WHERE id='negative-00000'")
            .get_result::<Number>(conn)?;
        assert_eq!(bytes.value, 4, "the policy counts UTF8 and NUL bytes");
        let page = conn.immediate_transaction::<_, AppError, _>(|conn| {
            prune_operation_history_page(conn, "negative", -2, 1000)
        })?;
        assert_eq!(page.removed, 1);
        assert!(!page.has_excess && page.has_future);
        let oldest = diesel::sql_query("SELECT COUNT(*) AS value FROM operation_log WHERE id='negative-00000'")
            .get_result::<Number>(conn)?;
        assert_eq!(oldest.value, 0, "id ordering still breaks negative-version ties");
        let future = diesel::sql_query("SELECT COUNT(*) AS value FROM operation_log WHERE id='future-negative'")
            .get_result::<Number>(conn)?;
        assert_eq!(future.value, 1, "current+1 remains untouched");
        Ok::<_, AppError>(())
    })
    .await
    .unwrap();
    assert_eq!(count(&db, "negative").await, MAX_HISTORY_ROWS as i64 + 1);
}
