//! Shared bounded operation-log retention for commits and idle catch-up.
//!
//! Call the synchronous helper inside the caller's transaction. The newest
//! eligible rows define a contiguous suffix by (version DESC, id DESC); only
//! metadata for at most 2049 rows is read; payloads are not materialized into Rust values.
use std::time::Duration;

use contrix_core::error::AppError;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Integer, Text};

use crate::state::DbPool;

pub const MAX_HISTORY_ROWS: usize = 2048;
pub const MAX_HISTORY_BYTES: i64 = 32 * 1024 * 1024;
pub const MAX_PRUNE_PAGE: usize = 1000;
const ENTITY_PAGE: i64 = 16;
const ACTIVE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct OperationHistoryPage {
    pub removed: usize,
    pub has_excess: bool,
    /// Future records are never deleted; a foreground caller must reject them.
    pub has_future: bool,
}

#[derive(QueryableByName)]
struct Entry {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Integer)]
    version: i32,
    #[diesel(sql_type = BigInt)]
    bytes: i64,
}
#[derive(QueryableByName)]
struct Number {
    #[diesel(sql_type = Integer)]
    value: i32,
}

/// Check against the durable version before a foreground caller writes next.
/// Otherwise a preexisting current+1 row could be absorbed by that new version.
pub fn has_future_history(
    conn: &mut SqliteConnection,
    document: &str,
    current_version: i32,
) -> Result<bool, AppError> {
    Ok(diesel::sql_query(
        "SELECT EXISTS(SELECT 1 FROM operation_log WHERE document_id=? AND version>?) AS value",
    )
    .bind::<Text, _>(document)
    .bind::<Integer, _>(current_version)
    .get_result::<Number>(conn)?
    .value
        != 0)
}

struct Analysis {
    cutoff: Option<(i32, String)>,
    has_future: bool,
}

fn analyze_history(
    conn: &mut SqliteConnection,
    document: &str,
    current_version: i32,
) -> Result<Analysis, AppError> {
    let has_future = has_future_history(conn, document, current_version)?;
    let entries = diesel::sql_query(
        "SELECT id,version,LENGTH(CAST(op_data AS BLOB)) AS bytes FROM operation_log
         WHERE document_id=? AND version<=? ORDER BY version DESC,id DESC LIMIT 2049",
    )
    .bind::<Text, _>(document)
    .bind::<Integer, _>(current_version)
    .load::<Entry>(conn)?;
    let mut bytes = 0_i64;
    let cutoff = entries.iter().enumerate().find_map(|(index, entry)| {
        let fits = entry.bytes >= 0
            && bytes
                .checked_add(entry.bytes)
                .is_some_and(|sum| sum <= MAX_HISTORY_BYTES);
        if index == MAX_HISTORY_ROWS || !fits {
            Some((entry.version, entry.id.clone()))
        } else {
            bytes += entry.bytes;
            None
        }
    });
    Ok(Analysis { cutoff, has_future })
}

/// Enforce one bounded page of the shared row/byte policy. The caller owns the
/// transaction and must roll back if foreground publication sees has_excess or
/// has_future. Limit zero only inspects; positive limits are capped at1000.
/// An individually oversized newest record leaves no retained log;
/// authoritative document snapshots and receipts are independent of this log.
pub fn prune_operation_history_page(
    conn: &mut SqliteConnection,
    document: &str,
    current_version: i32,
    limit: usize,
) -> Result<OperationHistoryPage, AppError> {
    let analysis = analyze_history(conn, document, current_version)?;
    let Some((version, id)) = analysis.cutoff else {
        return Ok(OperationHistoryPage {
            has_future: analysis.has_future,
            ..Default::default()
        });
    };
    if limit == 0 {
        return Ok(OperationHistoryPage {
            removed: 0,
            has_excess: true,
            has_future: analysis.has_future,
        });
    }
    // cutoff.version is already <= current_version. The tuple range both
    // preserves future records and lets the composite index seek directly.
    let removed = diesel::sql_query(
        "DELETE FROM operation_log WHERE id IN (
           SELECT id FROM operation_log WHERE document_id=?
           AND (version,id)<=(?,?) ORDER BY version DESC,id DESC LIMIT ?)",
    )
    .bind::<Text, _>(document)
    .bind::<Integer, _>(version)
    .bind::<Text, _>(&id)
    .bind::<BigInt, _>(limit.min(MAX_PRUNE_PAGE) as i64)
    .execute(conn)?;
    let has_excess = diesel::sql_query(
        "SELECT EXISTS(SELECT 1 FROM operation_log WHERE document_id=?
         AND (version,id)<=(?,?)) AS value",
    )
    .bind::<Text, _>(document)
    .bind::<Integer, _>(version)
    .bind::<Text, _>(&id)
    .get_result::<Number>(conn)?
    .value
        != 0;
    Ok(OperationHistoryPage {
        removed,
        has_excess,
        has_future: analysis.has_future,
    })
}

/// A single retry identity is bounded regardless of the number of broken docs.
/// Other failures are revisited by the cyclic keyset sweep, never forgotten as
/// successfully cleaned. A retry cannot consume every subsequent healthy turn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OperationHistoryCursor {
    after: String,
    retry_document: Option<String>,
}

#[derive(Debug, Default)]
pub struct OperationHistoryCatchUpPage {
    pub removed: usize,
    pub visited_entities: usize,
    pub failed_entities: usize,
    pub has_excess: bool,
    pub has_future: bool,
    pub sweep_finished: bool,
    pub first_error: Option<AppError>,
}

#[derive(Clone, QueryableByName)]
struct Entity {
    #[diesel(sql_type = Text)]
    id: String,
    #[diesel(sql_type = Integer)]
    version: i32,
}

fn prune_entity(
    conn: &mut SqliteConnection,
    entity: &Entity,
    limit: usize,
) -> Result<OperationHistoryPage, AppError> {
    // Pair the version and projection in a deferred read transaction. Stable
    // documents never acquire SQLite's writer lock just to discover no work.
    let analysis = conn.transaction::<_, AppError, _>(|conn| {
        let current = diesel::sql_query("SELECT id,version FROM documents WHERE id=?")
            .bind::<Text, _>(&entity.id)
            .get_result::<Entity>(conn)
            .optional()?;
        current
            .map(|current| analyze_history(conn, &current.id, current.version))
            .transpose()
    })?;
    let Some(analysis) = analysis else {
        return Ok(OperationHistoryPage::default());
    };
    if analysis.cutoff.is_none() || limit == 0 {
        return Ok(OperationHistoryPage {
            removed: 0,
            has_excess: analysis.cutoff.is_some(),
            has_future: analysis.has_future,
        });
    }
    conn.immediate_transaction::<_, AppError, _>(|conn| {
        // The read transaction has ended. Recheck under the write transaction;
        // another process may have committed or removed the document meanwhile.
        let current = diesel::sql_query("SELECT id,version FROM documents WHERE id=?")
            .bind::<Text, _>(&entity.id)
            .get_result::<Entity>(conn)
            .optional()?;
        match current {
            Some(current) => {
                prune_operation_history_page(conn, &current.id, current.version, limit)
            }
            None => Ok(OperationHistoryPage::default()),
        }
    })
}

fn record_result(
    page: &mut OperationHistoryCatchUpPage,
    next: &mut OperationHistoryCursor,
    entity: &Entity,
    result: Result<OperationHistoryPage, AppError>,
) -> Result<(), AppError> {
    page.visited_entities += 1;
    match result {
        Ok(result) => {
            page.removed += result.removed;
            page.has_excess |= result.has_excess;
            page.has_future |= result.has_future;
        }
        Err(error) if error.is_transient_database() => return Err(error),
        Err(error) => {
            page.failed_entities += 1;
            next.retry_document.get_or_insert_with(|| entity.id.clone());
            if page.first_error.is_none() {
                page.first_error = Some(error);
            }
        }
    }
    Ok(())
}

/// Visit at most16 entities (including one priority retry), delete at most1000
/// rows total, and rotate past productive entities rather than monopolizing a
/// hot document. Ordinary per-document failures retain a retry identity while
/// healthy documents advance. Query/admission/transient database failure leaves
/// the cursor unchanged; no unbounded failed-document queue is introduced.
pub async fn catch_up_operation_history_page(
    db: &DbPool,
    cursor: &mut OperationHistoryCursor,
) -> Result<OperationHistoryCatchUpPage, AppError> {
    let previous = cursor.clone();
    let (page, next) = db
        .run(move |conn| {
            let entities = diesel::sql_query(
                "SELECT id,version FROM documents WHERE id>? ORDER BY id LIMIT 16",
            )
            .bind::<Text, _>(&previous.after)
            .load::<Entity>(conn)?;
            let retry = match previous.retry_document.as_deref() {
                Some(id) => diesel::sql_query("SELECT id,version FROM documents WHERE id=?")
                    .bind::<Text, _>(id)
                    .get_result::<Entity>(conn)
                    .optional()?,
                None => None,
            };
            let mut next = OperationHistoryCursor {
                after: previous.after,
                retry_document: None,
            };
            let mut page = OperationHistoryCatchUpPage::default();
            if let Some(entity) = &retry {
                let result = prune_entity(conn, entity, MAX_PRUNE_PAGE);
                record_result(&mut page, &mut next, entity, result)?;
            }
            let mut consumed = 0;
            for entity in &entities {
                if retry.as_ref().is_some_and(|retry| retry.id == entity.id) {
                    next.after = entity.id.clone();
                    consumed += 1;
                    continue;
                }
                if page.visited_entities == ENTITY_PAGE as usize || page.removed == MAX_PRUNE_PAGE {
                    break;
                }
                let result = prune_entity(conn, entity, MAX_PRUNE_PAGE - page.removed);
                record_result(&mut page, &mut next, entity, result)?;
                next.after = entity.id.clone();
                consumed += 1;
            }
            page.sweep_finished =
                entities.len() < ENTITY_PAGE as usize && consumed == entities.len();
            if page.sweep_finished {
                next.after.clear();
            }
            Ok::<_, AppError>((page, next))
        })
        .await?;
    *cursor = next;
    Ok(page)
}

/// Idle catch-up is independent of history_enabled and has no traffic trigger.
/// Bounded metadata projection still may read text overflow pages for LENGTH;
/// this is bounded work, not a claim of constant physical I/O cost.
pub async fn run_operation_history_retention(db: DbPool, idle_interval: Duration) {
    let idle = idle_interval.max(Duration::from_millis(1));
    let mut cursor = OperationHistoryCursor::default();
    let mut sweep_changed_or_failed = false;
    loop {
        let delay = match catch_up_operation_history_page(&db, &mut cursor).await {
            Ok(mut page) => {
                sweep_changed_or_failed |= page.removed != 0 || page.failed_entities != 0;
                if let Some(error) = page.first_error.take() {
                    tracing::warn!(failed_entities = page.failed_entities, %error, "Operation history cleanup will retry failed documents");
                }
                if page.has_future {
                    tracing::warn!(
                        "Operation history retains records ahead of the durable document version"
                    );
                }
                if page.sweep_finished && !std::mem::take(&mut sweep_changed_or_failed) {
                    idle
                } else {
                    ACTIVE_INTERVAL
                }
            }
            Err(error) => {
                tracing::warn!(%error, "Operation history cleanup page will retry without advancing");
                idle.min(Duration::from_secs(1))
            }
        };
        tokio::time::sleep(delay).await;
    }
}
