//! Bounded retention and recovery. SQL fences prevent late references racing unlink.
use crate::database::telemetry::{
    TransactionStage, immediate_transaction as observed_immediate_transaction,
};
use crate::state::DbPool;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Integer, Text};
use syntra_core::{config::MaintenanceConfig, error::AppError};
use uuid::Uuid;

const WRITE_LEASE_SECONDS: i64 = 120;
const MAX_GC_BACKLOG: i32 = 1024;

#[derive(QueryableByName)]
struct Number {
    #[diesel(sql_type = Integer)]
    value: i32,
}
#[derive(QueryableByName)]
struct Inventory {
    #[diesel(sql_type = Text)]
    state: String,
    #[diesel(sql_type = BigInt)]
    first_seen: i64,
    #[diesel(sql_type = BigInt)]
    lease_until: i64,
}

fn gc_backlog(conn: &mut SqliteConnection) -> QueryResult<i32> {
    diesel::sql_query("SELECT COUNT(*) AS value FROM (SELECT 1 FROM storage_gc LIMIT ?)")
        .bind::<Integer, _>(MAX_GC_BACKLOG)
        .get_result::<Number>(conn)
        .map(|count| count.value)
}

pub(crate) fn referenced(conn: &mut SqliteConnection, path: &str) -> Result<bool, AppError> {
    let result = diesel::sql_query("SELECT (EXISTS(SELECT 1 FROM files WHERE path=?1 OR thumbnail_path=?1) OR EXISTS(SELECT 1 FROM file_versions WHERE path=?1) OR EXISTS(SELECT 1 FROM upload_chunks WHERE path=?1) OR EXISTS(SELECT 1 FROM conversion_jobs WHERE output_path=?1) OR EXISTS(SELECT 1 FROM retained_blobs WHERE path=?1)) AS value")
        .bind::<Text,_>(path).get_result::<Number>(conn)?;
    Ok(result.value != 0)
}

/// The heartbeat owns its database handle until the write future retires.
/// Expired writers are fenced: their completion and later metadata attach fail.
pub(crate) struct WriteLease {
    db: DbPool,
    path: String,
    token: String,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Drop for WriteLease {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}
impl WriteLease {
    pub async fn finish(self) -> Result<(), AppError> {
        let (path, token) = (self.path.clone(), self.token.clone());
        self.db.run(move |conn| {
            let changed=diesel::sql_query("UPDATE blob_inventory SET state='ready',lease_until=0,token=NULL WHERE path=? AND state='writing' AND token=?")
                .bind::<Text,_>(path).bind::<Text,_>(token).execute(conn)?;
            if changed != 1 { return Err(AppError::Conflict("Blob write lease was fenced; retry with a new object".into())); }
            Ok(())
        }).await
    }
}

pub(crate) async fn begin_write(db: &DbPool, path: &str) -> Result<WriteLease, AppError> {
    let token = Uuid::new_v4().to_string();
    let (key, claim) = (path.to_owned(), token.clone());
    db.run(move |conn| observed_immediate_transaction::<_,AppError>(conn, TransactionStage::BlobReservation, |conn| {
        let now=chrono::Utc::now().timestamp();
        // Bounded prefix count: all backend writes, not only multipart uploads,
        // stop admission when durable cleanup cannot keep pace.
        if gc_backlog(conn)? >= MAX_GC_BACKLOG {
            return Err(AppError::RateLimit("Deferred object cleanup is backlogged; retry after cleanup".into()));
        }
        let reserved = diesel::sql_query("INSERT INTO blob_inventory(path,state,first_seen,lease_until,token) VALUES (?,'writing',?,?,?) ON CONFLICT(path) DO NOTHING")
            .bind::<Text,_>(key).bind::<BigInt,_>(now).bind::<BigInt,_>(now+WRITE_LEASE_SECONDS).bind::<Text,_>(claim).execute(conn)?;
        if reserved != 1 {
            return Err(AppError::Conflict("Object key already exists or is retired; use a new key".into()));
        }
        Ok(())
    })).await?;
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let (renew_db, renew_path, renew_token) = (db.clone(), path.to_owned(), token.clone());
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(30));
        timer.tick().await;
        loop {
            tokio::select! {
                _=&mut stopped=>break,
                _=timer.tick()=>{
                    let (path,token)=(renew_path.clone(),renew_token.clone());
                    match renew_db.run(move |conn| {
                        Ok::<_,AppError>(diesel::sql_query("UPDATE blob_inventory SET lease_until=? WHERE path=? AND state='writing' AND token=?")
                            .bind::<BigInt,_>(chrono::Utc::now().timestamp()+WRITE_LEASE_SECONDS).bind::<Text,_>(path).bind::<Text,_>(token).execute(conn)?)
                    }).await {
                        Ok(0)=>break,
                        Ok(_)=>{},
                        Err(error)=>tracing::warn!(%error,"Blob write lease renewal failed; publication remains fenced"),
                    }
                }
            }
        }
    });
    Ok(WriteLease {
        db: db.clone(),
        path: path.to_owned(),
        token,
        stop: Some(stop),
    })
}

/// Preflight for a caller-owned metadata transaction. Reference triggers remain
/// authoritative if GC claims the key after this check. Compatibility callers
/// may supply an unregistered object, but cannot resurrect a tombstoned key.
pub(crate) fn prepare_attachment(conn: &mut SqliteConnection, path: &str) -> Result<(), AppError> {
    fn inventory(conn: &mut SqliteConnection, path: &str) -> QueryResult<Option<Inventory>> {
        diesel::sql_query("SELECT state,first_seen,lease_until FROM blob_inventory WHERE path=?")
            .bind::<Text, _>(path)
            .get_result::<Inventory>(conn)
            .optional()
    }
    // Normal uploads already registered a ready key. An INSERT OR IGNORE
    // would still acquire SQLite's writer lock even when the key exists.
    let row = if let Some(row) = inventory(conn, path)? {
        row
    } else {
        diesel::sql_query(
            "INSERT OR IGNORE INTO blob_inventory(path,state,first_seen) VALUES (?,'ready',?)",
        )
        .bind::<Text, _>(path)
        .bind::<BigInt, _>(chrono::Utc::now().timestamp())
        .execute(conn)?;
        inventory(conn, path)?.ok_or(diesel::result::Error::NotFound)?
    };
    if !matches!(row.state.as_str(), "ready" | "observed") {
        return Err(AppError::Conflict(
            "Object is not attachable; retry with a new object key".into(),
        ));
    }
    Ok(())
}

/// The persistent claim and every business reference write serialize in SQLite.
/// The caller must hold an IMMEDIATE transaction.
/// Network deletion happens after this transaction, while triggers fence inserts.
pub(crate) fn claim_gc_in_transaction(
    conn: &mut SqliteConnection,
    path: &str,
    now: i64,
) -> Result<Option<String>, AppError> {
    if referenced(conn, path)? {
        // This is not pending physical deletion. Last-reference retirement
        // re-enqueues it, with the owned inventory scan as a recovery path.
        diesel::sql_query("DELETE FROM storage_gc WHERE path=?")
            .bind::<Text, _>(path)
            .execute(conn)?;
        return Ok(None);
    }
    if let Some(row) =
        diesel::sql_query("SELECT state,first_seen,lease_until FROM blob_inventory WHERE path=?")
            .bind::<Text, _>(path)
            .get_result::<Inventory>(conn)
            .optional()?
        && row.state == "writing"
        && row.lease_until > now
    {
        return Ok(None);
    }
    let token = Uuid::new_v4().to_string();
    let claimed = diesel::sql_query(
        "UPDATE storage_gc SET claim_token=?,claim_until=? WHERE path=? AND claim_until<=?",
    )
    .bind::<Text, _>(&token)
    .bind::<BigInt, _>(now + 120)
    .bind::<Text, _>(path)
    .bind::<BigInt, _>(now)
    .execute(conn)?;
    if claimed == 0 {
        return Ok(None);
    }
    diesel::sql_query("INSERT INTO blob_inventory(path,state,first_seen) VALUES (?,'deleting',?) ON CONFLICT(path) DO UPDATE SET state='deleting',lease_until=0,token=NULL")
        .bind::<Text,_>(path).bind::<BigInt,_>(now).execute(conn)?;
    Ok(Some(token))
}

/// SQLite's lease index orders entries with equal expiry by rowid. A cursor
/// lets each tick inspect a bounded page even if early rows are referenced.
#[derive(Default)]
pub struct ExpiredWriteCursor(Option<(i64, i64)>);

/// Recover expired database reservations, including a reservation whose worker
/// returned late and never started object I/O. Fence the writer and enqueue
/// deletion in the same transaction so a late completion cannot make the
/// queued key attachable. Explicitly registered writes are recoverable at
/// lease expiry, as in `claim_gc_in_transaction`. Discovery grace applies to storage
/// scans, including objects written by a late PUT after the first deletion.
pub async fn expire_blob_writes_page(
    db: &DbPool,
    cursor: &mut ExpiredWriteCursor,
    config: &MaintenanceConfig,
    now: i64,
) -> Result<usize, AppError> {
    #[derive(QueryableByName)]
    struct ExpiredWrite {
        #[diesel(sql_type = BigInt)]
        rowid: i64,
        #[diesel(sql_type = BigInt)]
        lease_until: i64,
        #[diesel(sql_type = Text)]
        path: String,
    }

    let after = cursor.0;
    let batch = config.scan_page_size.clamp(1, 1000) as i32;
    let (queued, next) = db
        .run(move |conn| {
            let (lease_until, rowid) = after.unwrap_or((i64::MIN, i64::MIN));
            let page = diesel::sql_query(
                "SELECT rowid,lease_until,path FROM blob_inventory
                 WHERE state='writing' AND lease_until<=? AND (lease_until,rowid)>(?,?)
                 ORDER BY lease_until,rowid LIMIT ?",
            )
            .bind::<BigInt, _>(now)
            .bind::<BigInt, _>(lease_until)
            .bind::<BigInt, _>(rowid)
            .bind::<Integer, _>(batch)
            .load::<ExpiredWrite>(conn)?;
            if page.is_empty() {
                return Ok((0, None));
            }
            observed_immediate_transaction::<_, AppError>(conn, TransactionStage::BlobRecovery, |conn| {
                let limit = batch.min(MAX_GC_BACKLOG - gc_backlog(conn)?);
                if limit == 0 {
                    return Ok((0, after));
                }
                let next = page
                    .get(limit as usize - 1)
                    .map(|row| (row.lease_until, row.rowid));
                let mut queued = 0;
                for row in page.into_iter().take(limit as usize) {
                    if referenced(conn, &row.path)? {
                        continue;
                    }
                    let fenced = diesel::sql_query(
                        "UPDATE blob_inventory SET state='deleting',lease_until=0,token=NULL
                         WHERE rowid=? AND path=? AND state='writing' AND lease_until=? AND lease_until<=?",
                    )
                    .bind::<BigInt, _>(row.rowid)
                    .bind::<Text, _>(&row.path)
                    .bind::<BigInt, _>(row.lease_until)
                    .bind::<BigInt, _>(now)
                    .execute(conn)?;
                    if fenced == 1 {
                        super::persistence::enqueue_gc(conn, &row.path)?;
                        queued += 1;
                    }
                }
                Ok((queued, next))
            })
        })
        .await?;
    cursor.0 = next;
    Ok(queued)
}

fn legacy_namespace_exists(conn: &mut SqliteConnection, path: &str) -> Result<bool, AppError> {
    let parts: Vec<_> = path.split('/').collect();
    let (table, key) = if Uuid::parse_str(parts[0]).is_ok() {
        ("users", parts[0])
    } else {
        match parts[0] {
            "uploads" => ("users", parts.get(1).copied().unwrap_or("")),
            "chunks" => ("upload_sessions", parts.get(1).copied().unwrap_or("")),
            "conversions" => {
                let name = parts.last().copied().unwrap_or("");
                (
                    "conversion_jobs",
                    name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(""),
                )
            }
            "thumbnails" => {
                let key = parts.get(1).copied().unwrap_or("");
                (
                    "files",
                    key.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(key),
                )
            }
            _ => return Ok(false),
        }
    };
    Ok(diesel::sql_query(format!(
        "SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?) AS value"
    ))
    .bind::<Text, _>(key)
    .get_result::<Number>(conn)?
    .value
        != 0)
}

/// Discover one bounded page, without acquiring ownership of another database's
/// objects. Only pre-registered keys are eligible by default. The explicit
/// legacy option asserts exclusive ownership of the entire bucket/base_path,
/// additionally requires a recognized database namespace, and observes a full
/// first-discovery grace before enqueueing an unknown legacy object.
pub async fn reconcile_page(
    db: &DbPool,
    storage: &super::StorageBackend,
    cursor: &mut super::storage::ScanCursor,
    config: &MaintenanceConfig,
    now: i64,
) -> Result<usize, AppError> {
    let entries = storage
        .scan_page(cursor, config.scan_page_size.clamp(1, 1000) as usize)
        .await?;
    let cutoff = now.saturating_sub(i64::try_from(config.orphan_grace_seconds).unwrap_or(i64::MAX));
    let legacy = config.reconcile_legacy_dedicated_namespace;
    db.run(move |conn| {
        observed_immediate_transaction::<_, AppError>(
            conn,
            TransactionStage::StorageReconcile,
            |conn| {
                let mut queued = 0;
                for entry in entries {
                    if !super::storage::managed_blob(&entry.path) {
                        continue;
                    }
                    let row = diesel::sql_query(
                        "SELECT state,first_seen,lease_until FROM blob_inventory WHERE path=?",
                    )
                    .bind::<Text, _>(&entry.path)
                    .get_result::<Inventory>(conn)
                    .optional()?;
                    let row = if let Some(row) = row {
                        row
                    } else {
                        if !legacy || !legacy_namespace_exists(conn, &entry.path)? {
                            continue;
                        }
                        diesel::sql_query(
                        "INSERT INTO blob_inventory(path,state,first_seen) VALUES (?,'observed',?)",
                    )
                    .bind::<Text, _>(&entry.path)
                    .bind::<BigInt, _>(now)
                    .execute(conn)?;
                        Inventory {
                            state: "observed".into(),
                            first_seen: now,
                            lease_until: 0,
                        }
                    };
                    if entry.modified_at > cutoff
                        || row.first_seen > cutoff
                        || row.state == "writing" && row.lease_until > now
                        || referenced(conn, &entry.path)?
                    {
                        continue;
                    }
                    super::persistence::enqueue_gc(conn, &entry.path)?;
                    queued += 1;
                }
                Ok(queued)
            },
        )
    })
    .await
}

#[derive(Clone, Default)]
struct EntityCursor {
    last_entity: String,
    entity: Option<String>,
    after: Option<(i32, String, String)>,
}
#[derive(Default)]
pub struct HistoryCursor {
    native: EntityCursor,
    files: EntityCursor,
}

/// Release expired staging metadata in a bounded transaction. Reservations are
/// derived from unexpired pending sessions, so expiration never deducts usage
/// charged to real files. A large session resumes on the next cleanup page.
pub async fn expire_uploads_page(
    db: &DbPool,
    config: &MaintenanceConfig,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<usize, AppError> {
    let batch = config.history_batch_size.clamp(1, 1000) as i64;
    let cutoff = now.to_rfc3339();
    db.run(move|conn|observed_immediate_transaction::<_,AppError>(conn, TransactionStage::UploadExpiration, |conn|{
        #[derive(QueryableByName)]struct Chunk {#[diesel(sql_type=Integer)] chunk_index:i32,#[diesel(sql_type=Text)]path:String}
        let mut budget=batch;let mut removed=0;
        for status in ["pending","active"] {
            let sessions=diesel::sql_query("SELECT id FROM upload_sessions WHERE status=? AND expires_at<=? ORDER BY expires_at,id LIMIT ?")
                .bind::<Text,_>(status).bind::<Text,_>(&cutoff).bind::<BigInt,_>(batch).load::<Identity>(conn)?;
            for session in sessions {
                if budget==0 {break;}
                let chunks=diesel::sql_query("SELECT chunk_index,path FROM upload_chunks WHERE session_id=? ORDER BY chunk_index LIMIT ?")
                    .bind::<Text,_>(&session.id).bind::<BigInt,_>(budget).load::<Chunk>(conn)?;
                for chunk in chunks {
                    super::persistence::enqueue_gc(conn,&chunk.path)?;
                    diesel::sql_query("DELETE FROM upload_chunks WHERE session_id=? AND chunk_index=?").bind::<Text,_>(&session.id).bind::<Integer,_>(chunk.chunk_index).execute(conn)?;
                    budget-=1;removed+=1;
                }
                if budget>0 {
                    let remaining=diesel::sql_query("SELECT EXISTS(SELECT 1 FROM upload_chunks WHERE session_id=?) AS value").bind::<Text,_>(&session.id).get_result::<Number>(conn)?;
                    if remaining.value==0 {
                        removed+=diesel::sql_query("DELETE FROM upload_sessions WHERE id=? AND status=? AND expires_at<=?")
                            .bind::<Text,_>(session.id).bind::<Text,_>(status).bind::<Text,_>(&cutoff).execute(conn)?;
                        budget-=1;
                    }
                }
            }
        }
        Ok(removed)
    })).await
}
#[derive(QueryableByName)]
struct Identity {
    #[diesel(sql_type=Text)]
    id: String,
}
#[derive(QueryableByName)]
struct HistoryRow {
    #[diesel(sql_type=Text)]
    id: String,
    #[diesel(sql_type=Integer)]
    version_number: i32,
    #[diesel(sql_type=Text)]
    created_at: String,
}

#[derive(Default)]
pub struct NativeHistoryCountCursor(String);

pub struct NativeHistoryCountPage {
    pub removed: usize,
    pub sweep_finished: bool,
}

fn native_history_count_candidates(
    conn: &mut SqliteConnection,
    document: &str,
    current: i32,
    keep: i64,
    limit: i64,
) -> QueryResult<Vec<Identity>> {
    let protected_from =
        current.saturating_sub(super::collaboration::MAX_HISTORY_OPERATIONS as i32);
    if protected_from <= 0 {
        return Ok(Vec::new());
    }
    // Stable documents normally contain only the protected suffix. Check its
    // indexed lower range before walking even the bounded newest-N offset.
    if diesel::sql_query(
        "SELECT id FROM versions WHERE document_id=? AND version_number>=0
         AND version_number<? ORDER BY version_number,created_at,id LIMIT 1",
    )
    .bind::<Text, _>(document)
    .bind::<Integer, _>(protected_from)
    .get_result::<Identity>(conn)
    .optional()?
    .is_none()
    {
        return Ok(Vec::new());
    }
    let threshold = diesel::sql_query(
        "SELECT id,version_number,created_at FROM versions
         WHERE document_id=? AND version_number>=0
         ORDER BY version_number DESC,created_at DESC,id DESC LIMIT 1 OFFSET ?",
    )
    .bind::<Text, _>(document)
    .bind::<BigInt, _>(keep - 1)
    .get_result::<HistoryRow>(conn)
    .optional()?;
    let Some(threshold) = threshold else {
        return Ok(Vec::new());
    };
    let last_candidate_version = (protected_from - 1).min(threshold.version_number);
    diesel::sql_query(
        "SELECT id FROM versions WHERE document_id=? AND version_number>=0
         AND version_number<=? AND (version_number,created_at,id)<(?,?,?)
         ORDER BY version_number,created_at,id LIMIT ?",
    )
    .bind::<Text, _>(document)
    .bind::<Integer, _>(last_candidate_version)
    .bind::<Integer, _>(threshold.version_number)
    .bind::<Text, _>(threshold.created_at)
    .bind::<Text, _>(threshold.id)
    .bind::<BigInt, _>(limit)
    .load(conn)
}

/// Catch up count retention for old native snapshots without scanning the
/// current/OT suffix. The exact maintenance pass still applies age, duplicate,
/// file history and quota policy. Only one entity may delete rows per turn.
pub async fn retain_native_history_count_page(
    db: &DbPool,
    cursor: &mut NativeHistoryCountCursor,
    config: &MaintenanceConfig,
) -> Result<NativeHistoryCountPage, AppError> {
    const ENTITY_PAGE: i64 = 16;
    #[derive(QueryableByName)]
    struct Entity {
        #[diesel(sql_type = Text)]
        id: String,
        #[diesel(sql_type = Integer)]
        version: i32,
    }

    let after = cursor.0.clone();
    let keep = config.history_max_versions.clamp(1, 10000) as i64;
    let batch = config.history_batch_size.clamp(1, 1000) as i64;
    let (removed, next, sweep_finished) = db
        .run(move |conn| {
            let entities = diesel::sql_query(
                "SELECT id,version FROM documents WHERE id>? ORDER BY id LIMIT ?",
            )
            .bind::<Text, _>(after)
            .bind::<BigInt, _>(ENTITY_PAGE)
            .load::<Entity>(conn)?;
            let last_page = entities.len() < ENTITY_PAGE as usize;
            for (index, entity) in entities.iter().enumerate() {
                if native_history_count_candidates(conn, &entity.id, entity.version, keep, 1)?
                    .is_empty()
                {
                    continue;
                }
                let count = observed_immediate_transaction::<_, AppError>(
                    conn,
                    TransactionStage::NativeHistoryRetention,
                    |conn| {
                        use syntra_core::schema::documents::dsl as d;
                        use syntra_core::schema::versions::dsl as v;
                        let current = d::documents
                            .find(&entity.id)
                            .select(d::version)
                            .first::<i32>(conn)
                            .optional()?;
                        let Some(current) = current else { return Ok(0) };
                        let candidates = native_history_count_candidates(
                            conn, &entity.id, current, keep, batch,
                        )?;
                        if candidates.is_empty() {
                            return Ok(0);
                        }
                        let ids: Vec<_> = candidates.into_iter().map(|row| row.id).collect();
                        diesel::delete(v::versions.filter(v::id.eq_any(ids)))
                            .execute(conn)
                            .map_err(AppError::from)
                    },
                )?;
                let finished = last_page && index + 1 == entities.len();
                let next = if finished {
                    String::new()
                } else {
                    entity.id.clone()
                };
                return Ok::<_, AppError>((count, next, finished));
            }
            let next = if last_page {
                String::new()
            } else {
                entities.last().expect("full entity page").id.clone()
            };
            Ok((0, next, last_page))
        })
        .await?;
    cursor.0 = next;
    Ok(NativeHistoryCountPage {
        removed,
        sweep_finished,
    })
}

/// Ordinary history is kept only while within both the newest N and max age
/// (zero age disables expiry). Extra protection is bounded to one canonical
/// current snapshot and at most 2048 canonical bases from the contiguous OT
/// suffix ending at the current version. File original version zero is exempt.
/// Candidate reads use indexed entity/keyset pages, with no whole-table rank.
pub async fn retain_history_page(
    db: &DbPool,
    cursor: &mut HistoryCursor,
    config: &MaintenanceConfig,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<usize, AppError> {
    let mut removed = 0;
    for native in [true, false] {
        let source = if native {
            &mut cursor.native
        } else {
            &mut cursor.files
        };
        let mut next = source.clone();
        let keep = config.history_max_versions.clamp(1, 10000) as i64;
        let batch = config.history_batch_size.clamp(1, 1000) as i64;
        let cutoff = (now - chrono::Duration::days(config.history_max_age_days.min(36500) as i64))
            .timestamp();
        let expire_age = config.history_max_age_days != 0;
        let (count,updated)=db.run(move |conn|observed_immediate_transaction::<_,AppError>(conn, TransactionStage::HistoryRetention, |conn| {
            let (entities,table,foreign)=if native {("documents","versions","document_id")}else{("files","file_versions","file_id")};
            let lower=if native {-1}else{0};
            if next.entity.is_none() {
                let query=format!("SELECT id FROM {entities} WHERE id>? ORDER BY id LIMIT 1");
                let entity=diesel::sql_query(query).bind::<Text,_>(&next.last_entity).get_result::<Identity>(conn).optional()?;
                let Some(entity)=entity else {next.last_entity.clear();return Ok((0,next.clone()));};
                next.entity=Some(entity.id);next.after=None;
            }
            let entity=next.entity.as_ref().unwrap().clone();
            // Offset is bounded by configured N, never by total history length.
            let threshold=diesel::sql_query(format!("SELECT id,version_number,created_at FROM {table} WHERE {foreign}=? AND version_number>{lower} ORDER BY version_number DESC,created_at DESC,id DESC LIMIT 1 OFFSET ?"))
                .bind::<Text,_>(&entity).bind::<BigInt,_>(keep-1).get_result::<HistoryRow>(conn).optional()?;
            let page=if let Some((version,date,id))=&next.after {
                diesel::sql_query(format!("SELECT id,version_number,created_at FROM {table} WHERE {foreign}=? AND version_number>{lower} AND (version_number,created_at,id)<(?,?,?) ORDER BY version_number DESC,created_at DESC,id DESC LIMIT ?"))
                    .bind::<Text,_>(&entity).bind::<Integer,_>(*version).bind::<Text,_>(date).bind::<Text,_>(id).bind::<BigInt,_>(batch).load::<HistoryRow>(conn)?
            } else {
                diesel::sql_query(format!("SELECT id,version_number,created_at FROM {table} WHERE {foreign}=? AND version_number>{lower} ORDER BY version_number DESC,created_at DESC,id DESC LIMIT ?"))
                    .bind::<Text,_>(&entity).bind::<BigInt,_>(batch).load::<HistoryRow>(conn)?
            };
            let current=if native {
                diesel::sql_query("SELECT version AS value FROM documents WHERE id=?").bind::<Text,_>(&entity).get_result::<Number>(conn).optional()?.map(|n|n.value)
            } else {None};
            let floor=if let Some(current)=current {
                let log=diesel::sql_query("SELECT version AS value FROM operation_log WHERE document_id=? AND version<=? AND version>0 ORDER BY version DESC LIMIT ?")
                    .bind::<Text,_>(&entity).bind::<Integer,_>(current)
                    .bind::<BigInt,_>(super::collaboration::MAX_HISTORY_OPERATIONS as i64).load::<Number>(conn)?;
                let mut expected=current;let mut floor=None;
                for operation in log {if operation.value!=expected {break;}expected=expected.saturating_sub(1);floor=Some(expected);}
                floor
            }else {None};
            let mut count=0;
            for row in &page {
                let age=chrono::DateTime::parse_from_rfc3339(&row.created_at).map(|date|date.timestamp()).ok().or_else(||chrono::NaiveDateTime::parse_from_str(&row.created_at,"%Y-%m-%d %H:%M:%S").ok().map(|date|date.and_utc().timestamp()));
                let old=expire_age && age.is_some_and(|age|age<cutoff);
                let latest=threshold.as_ref().is_none_or(|bound|(row.version_number,&row.created_at,&row.id)>=(bound.version_number,&bound.created_at,&bound.id));
                if !old && latest {continue;}
                let replay_base=current.is_some_and(|version| row.version_number==version || floor.is_some_and(|floor|row.version_number>=floor && row.version_number<version));
                if replay_base {
                    let canonical=diesel::sql_query("SELECT id FROM versions WHERE document_id=? AND version_number=? ORDER BY created_at DESC,id DESC LIMIT 1")
                        .bind::<Text,_>(&entity).bind::<Integer,_>(row.version_number).get_result::<Identity>(conn).optional()?;
                    if canonical.is_some_and(|entry|entry.id==row.id) {continue;}
                }
                if !native {
                    #[derive(QueryableByName)] struct Retired {#[diesel(sql_type=Text)] path:String,#[diesel(sql_type=Text)] owner_id:String}
                    let retired=diesel::sql_query("SELECT v.path,f.owner_id FROM file_versions v JOIN files f ON f.id=v.file_id WHERE v.id=?")
                        .bind::<Text,_>(&row.id).get_result::<Retired>(conn)?;
                    diesel::sql_query("DELETE FROM file_versions WHERE id=?").bind::<Text,_>(&row.id).execute(conn)?;
                    super::persistence::enqueue_gc(conn,&retired.path)?;
                    super::persistence::release_retained_path(conn,&retired.owner_id,&retired.path)?;
                }else {diesel::sql_query("DELETE FROM versions WHERE id=?").bind::<Text,_>(&row.id).execute(conn)?;}
                count+=1;
            }
            if let Some(last)=page.last(){next.after=Some((last.version_number,last.created_at.clone(),last.id.clone()));}
            if page.len()<batch as usize {next.last_entity=entity;next.entity=None;next.after=None;}
            Ok((count,next.clone()))
        })).await?;
        *source = updated;
        removed += count;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{StorageBackend, StorageService, persistence};
    use diesel::connection::SimpleConnection;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn fixture() -> (tempfile::TempDir, DbPool, Arc<StorageBackend>, String) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("test.db");
        let db = DbPool::with_timeouts(
            path.to_str().unwrap(),
            4,
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(3),
        )
        .unwrap();
        let owner = Uuid::new_v4().to_string();
        let user = owner.clone();
        db.run(move |conn| {
            crate::migrations::run_directory(conn,&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations")).unwrap();
            diesel::sql_query("INSERT INTO users(id,username,email,password_hash) VALUES (?,'maintainer','maintenance@test.invalid','test')").bind::<Text,_>(user).execute(conn)?;
            Ok::<_,AppError>(())
        }).await.unwrap();
        let root = temp.path().join("files");
        std::fs::create_dir(&root).unwrap();
        let storage = Arc::new(StorageBackend::Local(StorageService::new(
            root.to_str().unwrap(),
        )));
        storage.attach_database(db.clone()).unwrap();
        (temp, db, storage, owner)
    }
    async fn exec(db: &DbPool, sql: String) {
        db.run(move |conn| conn.batch_execute(&sql)).await.unwrap();
    }
    async fn count(db: &DbPool, sql: impl Into<String>) -> i32 {
        let sql = sql.into();
        db.run(move |conn| {
            Ok::<_, AppError>(diesel::sql_query(sql).get_result::<Number>(conn)?.value)
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn ready_attachment_preflight_can_read_while_another_connection_holds_the_writer() {
        let (temp, db, storage, owner) = fixture().await;
        let key = storage
            .save_file(&owner.parse().unwrap(), "ready", b"object")
            .await
            .unwrap();
        let path = temp.path().join("test.db");
        let mut writer = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
        let mut reader = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
        reader.batch_execute("PRAGMA busy_timeout=0").unwrap();
        writer.batch_execute("BEGIN IMMEDIATE").unwrap();
        prepare_attachment(&mut reader, &key).unwrap();
        writer.batch_execute("ROLLBACK").unwrap();

        let reused = begin_write(&db, &key).await;
        assert!(matches!(reused, Err(AppError::Conflict(_))));
        assert_eq!(storage.read_file(&key).await.unwrap(), b"object");
    }

    #[tokio::test]
    async fn expired_reservations_without_objects_are_fenced_and_drained_in_pages() {
        let (temp, db, storage, owner) = fixture().await;
        let (_, referenced_path, _) = file(&db, &storage, &owner).await;
        let abandoned = format!("{owner}/{}/abandoned", Uuid::new_v4());
        let late = format!("{owner}/{}/late", Uuid::new_v4());
        let live = format!("{owner}/{}/live", Uuid::new_v4());
        drop(begin_write(&db, &abandoned).await.unwrap());
        let late_writer = begin_write(&db, &late).await.unwrap();
        let _live_writer = begin_write(&db, &live).await.unwrap();
        assert!(!temp.path().join("files").join(&abandoned).exists());
        assert!(!temp.path().join("files").join(&late).exists());

        let mut cursor = ExpiredWriteCursor::default();
        let config = MaintenanceConfig {
            reconcile_enabled: false,
            scan_page_size: 1,
            ..MaintenanceConfig::default()
        };
        let now = chrono::Utc::now().timestamp();
        let db_path = temp.path().join("test.db");
        let mut external = SqliteConnection::establish(db_path.to_str().unwrap()).unwrap();
        external.batch_execute("BEGIN IMMEDIATE").unwrap();
        assert_eq!(
            expire_blob_writes_page(&db, &mut cursor, &config, now)
                .await
                .unwrap(),
            0,
            "an empty expired page does not need the writer lock"
        );
        external.batch_execute("ROLLBACK").unwrap();
        drop(external);

        // Even an inconsistent inventory state must never authorize deleting
        // an object that is still referenced. Put it on the first page.
        exec(
            &db,
            format!(
                "UPDATE blob_inventory SET state='writing',lease_until=0 WHERE path='{referenced_path}';
                 UPDATE blob_inventory SET lease_until=1 WHERE path IN ('{abandoned}','{late}');
                 WITH RECURSIVE backlog(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM backlog WHERE n < {MAX_GC_BACKLOG})
                 INSERT INTO storage_gc(path,created_at) SELECT 'test-backlog-' || n,'2000-01-01T00:00:00Z' FROM backlog;"
            ),
        )
        .await;
        assert_eq!(
            expire_blob_writes_page(&db, &mut cursor, &config, now)
                .await
                .unwrap(),
            0,
            "recovery must leave room in the shared outbox"
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM storage_gc").await,
            MAX_GC_BACKLOG
        );
        exec(
            &db,
            "DELETE FROM storage_gc WHERE path LIKE 'test-backlog-%'".into(),
        )
        .await;
        for queued in [0, 1, 1, 0] {
            assert_eq!(
                expire_blob_writes_page(&db, &mut cursor, &config, now)
                    .await
                    .unwrap(),
                queued
            );
        }
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM storage_gc").await,
            2
        );
        assert!(matches!(
            late_writer.finish().await,
            Err(AppError::Conflict(_))
        ));

        // A fresh pool can drain the durable outbox without an object scan or
        // a live writer handle. Both absent local objects delete idempotently.
        let second = DbPool::with_timeouts(
            db_path.to_str().unwrap(),
            1,
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(3),
        )
        .unwrap();
        persistence::drain_storage_gc(&second, &storage)
            .await
            .unwrap();
        assert_eq!(
            count(&second, "SELECT COUNT(*) AS value FROM storage_gc").await,
            0
        );
        assert_eq!(
            count(
                &second,
                "SELECT COUNT(*) AS value FROM blob_inventory WHERE state='deleted'"
            )
            .await,
            2
        );
        assert_eq!(
            count(
                &second,
                "SELECT COUNT(*) AS value FROM blob_inventory WHERE state='writing'"
            )
            .await,
            2,
            "live and referenced keys are retained"
        );
        assert_eq!(
            storage.read_file(&referenced_path).await.unwrap(),
            b"current object"
        );
        assert!(matches!(
            begin_write(&second, &abandoned).await,
            Err(AppError::Conflict(_))
        ));
    }

    async fn file(db: &DbPool, storage: &StorageBackend, owner: &str) -> (String, String, i64) {
        let data = b"current object";
        let path = storage
            .save_file(&owner.parse().unwrap(), "current", data)
            .await
            .unwrap();
        let id = Uuid::new_v4().to_string();
        let (key, actor, file) = (path.clone(), owner.to_owned(), id.clone());
        db.run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                diesel::sql_query(
                    "INSERT INTO files(id,name,path,size,owner_id) VALUES (?,'current',?,?,?)",
                )
                .bind::<Text, _>(file)
                .bind::<Text, _>(key)
                .bind::<BigInt, _>(data.len() as i64)
                .bind::<Text, _>(&actor)
                .execute(conn)?;
                persistence::adjust_quota(conn, &actor, data.len() as i64)?;
                Ok(())
            })
        })
        .await
        .unwrap();
        (id, path, data.len() as i64)
    }
    fn age_object(temp: &tempfile::TempDir, path: &str, now: i64) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(temp.path().join("files").join(path))
            .unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs((now - 10000) as u64),
        ))
        .unwrap();
    }
    async fn scan(
        db: &DbPool,
        storage: &StorageBackend,
        config: &MaintenanceConfig,
        now: i64,
    ) -> usize {
        let mut cursor = storage.begin_scan().await.unwrap();
        let mut queued = 0;
        for _ in 0..1000 {
            queued += reconcile_page(db, storage, &mut cursor, config, now)
                .await
                .unwrap();
            if cursor.finished() {
                return queued;
            }
        }
        panic!("bounded scan failed to finish");
    }

    async fn append_native_snapshots(
        db: &DbPool,
        document: &str,
        owner: &str,
        first: i32,
        last: i32,
    ) {
        let (document, owner) = (document.to_owned(), owner.to_owned());
        db.run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                if first == 1 {
                    diesel::sql_query("INSERT INTO files(id,name,path,size,owner_id) VALUES ('file-' || ?,'native',? || '.json',0,?)")
                        .bind::<Text, _>(&document).bind::<Text, _>(&document)
                        .bind::<Text, _>(&owner).execute(conn)?;
                    diesel::sql_query("INSERT INTO documents(id,file_id,doc_type,content) VALUES (?,'file-' || ?,'writer','{}')")
                        .bind::<Text, _>(&document).bind::<Text, _>(&document).execute(conn)?;
                }
                diesel::sql_query(
                    "WITH RECURSIVE series(n) AS
                     (SELECT ? UNION ALL SELECT n+1 FROM series WHERE n<?)
                     INSERT INTO versions(id,document_id,version_number,content,created_by,created_at)
                     SELECT ? || '-' || n,?,n,'{}',?,'2000-01-01T00:00:00Z' FROM series",
                )
                .bind::<Integer, _>(first)
                .bind::<Integer, _>(last)
                .bind::<Text, _>(&document)
                .bind::<Text, _>(&document)
                .bind::<Text, _>(&owner)
                .execute(conn)?;
                diesel::sql_query("UPDATE documents SET version=? WHERE id=?")
                    .bind::<Integer, _>(last)
                    .bind::<Text, _>(&document)
                    .execute(conn)?;
                Ok(())
            })
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn native_history_count_catchup_is_bounded_and_fair_during_active_writes() {
        let (_temp, db, _storage, owner) = fixture().await;
        let protected = super::super::collaboration::MAX_HISTORY_OPERATIONS as i32;
        append_native_snapshots(&db, "a-hot", &owner, 1, 2400).await;
        append_native_snapshots(&db, "b-sibling", &owner, 1, 2400).await;
        let config = MaintenanceConfig {
            history_max_versions: 100,
            history_batch_size: 100,
            history_max_age_days: 0,
            ..Default::default()
        };
        let mut cursor = NativeHistoryCountCursor::default();
        let first = retain_native_history_count_page(&db, &mut cursor, &config)
            .await
            .unwrap();
        assert_eq!(first.removed, 100);
        assert!(!first.sweep_finished);
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS value FROM versions WHERE document_id='b-sibling'"
            )
            .await,
            2400
        );
        let second = retain_native_history_count_page(&db, &mut cursor, &config)
            .await
            .unwrap();
        assert_eq!(second.removed, 100);
        assert!(second.sweep_finished);
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS value FROM versions WHERE document_id='a-hot'"
            )
            .await,
            2300,
            "the hot document cannot monopolize a sibling's turn"
        );

        // Continue appending while the two bounded turns make progress.
        // This rate is below the configured deletion budget.
        for round in 0..8 {
            let first = 2401 + round * 25;
            append_native_snapshots(&db, "a-hot", &owner, first, first + 24).await;
            for _ in 0..2 {
                let page = retain_native_history_count_page(&db, &mut cursor, &config)
                    .await
                    .unwrap();
                assert!(page.removed <= config.history_batch_size as usize);
            }
        }
        for document in ["a-hot", "b-sibling"] {
            assert_eq!(
                count(
                    &db,
                    format!(
                        "SELECT COUNT(*) AS value FROM versions WHERE document_id='{document}'"
                    )
                )
                .await,
                protected + 1,
                "current and every possible canonical OT base survive"
            );
        }
        assert_eq!(
            count(&db, format!("SELECT COUNT(*) AS value FROM versions v JOIN documents d ON d.id=v.document_id WHERE v.version_number < d.version-{protected}")).await,
            0,
            "the backlog converges while snapshots are still being appended"
        );
        let quiet = retain_native_history_count_page(&db, &mut cursor, &config)
            .await
            .unwrap();
        assert_eq!(quiet.removed, 0);
        assert!(quiet.sweep_finished);
    }

    #[tokio::test]
    async fn native_history_count_catchup_advances_bounded_empty_entity_pages() {
        let (temp, db, _storage, owner) = fixture().await;
        exec(&db, format!(
            "WITH RECURSIVE series(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM series WHERE n<17)
             INSERT INTO files(id,name,path,size,owner_id)
             SELECT printf('empty-%02d',n),'native',printf('empty-%02d.json',n),0,'{owner}' FROM series;
             INSERT INTO documents(id,file_id,doc_type,content)
             SELECT id,id,'writer','{{}}' FROM files;"
        )).await;
        append_native_snapshots(&db, "z-backlog", &owner, 1, 2400).await;
        let config = MaintenanceConfig {
            history_max_versions: 100,
            history_batch_size: 100,
            ..Default::default()
        };
        let mut cursor = NativeHistoryCountCursor::default();
        let mut writer =
            SqliteConnection::establish(temp.path().join("test.db").to_str().unwrap()).unwrap();
        writer.batch_execute("BEGIN IMMEDIATE").unwrap();
        let empty = retain_native_history_count_page(&db, &mut cursor, &config)
            .await
            .unwrap();
        assert_eq!(empty.removed, 0);
        assert!(
            !empty.sweep_finished,
            "the entity scan is bounded to sixteen documents"
        );
        writer.batch_execute("ROLLBACK").unwrap();
        let backlog = retain_native_history_count_page(&db, &mut cursor, &config)
            .await
            .unwrap();
        assert_eq!(backlog.removed, 100);
        assert!(backlog.sweep_finished);
    }

    #[tokio::test]
    async fn native_history_count_catchup_preserves_newest_tuple_and_reads_empty_pages_without_writer_lock()
     {
        let (temp, db, _storage, owner) = fixture().await;
        append_native_snapshots(&db, "tuple", &owner, 1, 2300).await;
        exec(
            &db,
            format!(
                "INSERT INTO versions(id,document_id,version_number,content,created_by,created_at)
             VALUES ('tie-old','tuple',101,'{{}}','{owner}','1999-01-01T00:00:00Z'),
                    ('tie-same','tuple',101,'{{}}','{owner}','2000-01-01T00:00:00Z')"
            ),
        )
        .await;
        let config = MaintenanceConfig {
            history_max_versions: 2200,
            history_batch_size: 70,
            history_max_age_days: 0,
            ..Default::default()
        };
        let mut cursor = NativeHistoryCountCursor::default();
        assert_eq!(
            retain_native_history_count_page(&db, &mut cursor, &config)
                .await
                .unwrap()
                .removed,
            70
        );
        assert_eq!(
            retain_native_history_count_page(&db, &mut cursor, &config)
                .await
                .unwrap()
                .removed,
            32
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS value FROM versions WHERE document_id='tuple'"
            )
            .await,
            2200
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS value FROM versions WHERE id='tuple-101'"
            )
            .await,
            1,
            "the newest-N tuple boundary survives equal-version timestamp/id ties"
        );
        let mut writer =
            SqliteConnection::establish(temp.path().join("test.db").to_str().unwrap()).unwrap();
        writer.batch_execute("BEGIN IMMEDIATE").unwrap();
        let empty = retain_native_history_count_page(&db, &mut cursor, &config)
            .await
            .unwrap();
        assert_eq!(empty.removed, 0);
        assert!(empty.sweep_finished);
        writer.batch_execute("ROLLBACK").unwrap();
    }

    #[tokio::test]
    async fn history_retention_is_paged_and_preserves_original_current_ot_and_charged_quota() {
        let (_temp, db, storage, owner) = fixture().await;
        let (file_id, current, base_size) = file(&db, &storage, &owner).await;
        let document = Uuid::new_v4().to_string();
        let original =
            serde_json::to_value(syntra_doc_engine::WriterDocument::new("current".into())).unwrap();
        let current_json = serde_json::to_string(&original).unwrap();
        let (doc, fileid, actor) = (document.clone(), file_id.clone(), owner.clone());
        db.run(move |conn| {
            diesel::sql_query("INSERT INTO documents(id,file_id,doc_type,content,version) VALUES (?,?,'writer',?,12)").bind::<Text,_>(&doc).bind::<Text,_>(fileid).bind::<Text,_>(&current_json).execute(conn)?;
            for version in 0..=12 {
                let mut snapshot=original.clone();snapshot["title"]=serde_json::json!(format!("v{version}"));
                let created=if version==4{chrono::Utc::now().to_rfc3339()}else{"2000-01-01T00:00:00Z".into()};
                diesel::sql_query("INSERT INTO versions(id,document_id,version_number,content,created_by,created_at) VALUES (?,?,?,?,?,?)")
                    .bind::<Text,_>(format!("snapshot-{version}")).bind::<Text,_>(&doc).bind::<Integer,_>(version).bind::<Text,_>(snapshot.to_string()).bind::<Text,_>(&actor).bind::<Text,_>(created).execute(conn)?;
            }
            for version in 9..=12 {
                diesel::sql_query("INSERT INTO operation_log(id,document_id,user_id,op_type,op_data,version,created_at) VALUES (?,?,?,'{}','{}',?,'2000-01-01T00:00:00Z')")
                    .bind::<Text,_>(format!("retained-op-{version}")).bind::<Text,_>(&doc).bind::<Text,_>(&actor).bind::<Integer,_>(version).execute(conn)?;
            }
            Ok::<_,AppError>(())
        }).await.unwrap();
        let mut snapshots = Vec::new();
        for version in 0..=6 {
            let size = version as i64 + 10;
            let path = storage
                .save_file(
                    &owner.parse().unwrap(),
                    "snapshot",
                    &vec![version as u8; size as usize],
                )
                .await
                .unwrap();
            let (key, fileid, actor) = (path.clone(), file_id.clone(), owner.clone());
            db.run(move|conn|conn.immediate_transaction::<_,AppError,_>(|conn| {
                let date=if version==2{chrono::Utc::now().to_rfc3339()}else{"2000-01-01T00:00:00Z".into()};
                diesel::sql_query("INSERT INTO file_versions(id,file_id,version_number,path,size,created_by,created_at) VALUES (?,?,?,?,?,?,?)")
                    .bind::<Text,_>(format!("file-snapshot-{version}")).bind::<Text,_>(fileid).bind::<Integer,_>(version).bind::<Text,_>(&key).bind::<BigInt,_>(size).bind::<Text,_>(&actor).bind::<Text,_>(date).execute(conn)?;
                // A legacy uncharged snapshot must not subtract another file's usage.
                if version!=3 {
                    diesel::sql_query("INSERT INTO retained_blobs(owner_id,path,size) VALUES (?,?,?)").bind::<Text,_>(&actor).bind::<Text,_>(key).bind::<BigInt,_>(size).execute(conn)?;
                    persistence::adjust_quota(conn,&actor,size)?;
                }
                Ok(())
            })).await.unwrap();
            snapshots.push(path);
        }
        exec(&db,format!("INSERT INTO file_versions(id,file_id,version_number,path,size,created_by,created_at) VALUES ('current-snapshot','{file_id}',99,'{current}',{base_size},'{owner}','2000-01-01T00:00:00Z')")).await;
        let config = MaintenanceConfig {
            history_max_versions: 2,
            history_batch_size: 2,
            history_max_age_days: 0,
            ..Default::default()
        };
        let mut cursor = HistoryCursor::default();
        let mut removed = 0;
        for _ in 0..32 {
            let batch = retain_history_page(&db, &mut cursor, &config, chrono::Utc::now())
                .await
                .unwrap();
            assert!(batch <= 4);
            removed += batch;
        }
        assert_eq!(removed, 13);
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS value FROM versions WHERE version_number IN (8,9,10,11,12)"
            )
            .await,
            5
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM versions").await,
            5
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS value FROM file_versions WHERE version_number IN (0,6,99)"
            )
            .await,
            3
        );
        assert_eq!(
            count(
                &db,
                format!("SELECT storage_used AS value FROM users WHERE id='{owner}'")
            )
            .await as i64,
            base_size + 10 + 16
        );
        persistence::drain_storage_gc(&db, &storage).await.unwrap();
        for (version, path) in snapshots.iter().enumerate() {
            assert_eq!(
                storage.read_file(path).await.is_ok(),
                matches!(version, 0 | 6)
            );
        }
        assert_eq!(
            storage.read_file(&current).await.unwrap(),
            b"current object"
        );
        // The retained OT base snapshot is still usable for an actual restore commit.
        #[derive(QueryableByName)]
        struct Content {
            #[diesel(sql_type=Text)]
            content: String,
        }
        let restored = db
            .run(|conn| {
                Ok::<_, AppError>(
                    diesel::sql_query("SELECT content FROM versions WHERE id='snapshot-8'")
                        .get_result::<Content>(conn)?
                        .content,
                )
            })
            .await
            .unwrap();
        assert_eq!(
            persistence::commit_document(&db, &storage, &document, &owner, 12, restored, None)
                .await
                .unwrap(),
            13
        );
        #[derive(QueryableByName)]
        struct Plan {
            #[diesel(sql_type=Text)]
            detail: String,
        }
        let plan=db.run(move|conn|Ok::<_,AppError>(diesel::sql_query("EXPLAIN QUERY PLAN SELECT id,version_number,created_at FROM versions WHERE document_id=? AND version_number>-1 AND (version_number,created_at,id)<(7,'2000','x') ORDER BY version_number DESC,created_at DESC,id DESC LIMIT 2").bind::<Text,_>(document).load::<Plan>(conn)?)).await.unwrap();
        assert!(
            plan.iter()
                .any(|row| row.detail.contains("idx_versions_retention"))
        );
    }

    #[tokio::test]
    async fn orphan_discovery_grace_protects_current_original_all_reference_tables_and_active_writes()
     {
        let (temp, db, storage, owner) = fixture().await;
        let now = chrono::Utc::now().timestamp();
        let (file_id, current, _) = file(&db, &storage, &owner).await;
        let mut protected = vec![current];
        for name in ["original", "chunk", "conversion", "retained", "thumbnail"] {
            let path = storage
                .save_file(&owner.parse().unwrap(), name, b"keep")
                .await
                .unwrap();
            protected.push(path);
        }
        exec(&db,format!("INSERT INTO file_versions(id,file_id,version_number,path,size,created_by,created_at) VALUES ('original','{file_id}',0,'{}',4,'{owner}','2000-01-01T00:00:00Z'); INSERT INTO upload_chunks(session_id,chunk_index,path,size,digest) VALUES ('pending',1,'{}',4,'digest'); INSERT INTO retained_blobs(owner_id,path,size) VALUES ('{owner}','{}',4); UPDATE files SET thumbnail_path='{}' WHERE id='{file_id}';",protected[1],protected[2],protected[4],protected[5])).await;
        // Conversion reference fixture uses the real table's required fields.
        exec(&db,format!("INSERT INTO conversion_jobs(id,source_file_id,target_format,status,output_path,user_id,created_at) VALUES ('conversion','{file_id}','pdf','completed','{}','{owner}','2000-01-01T00:00:00Z')",protected[3])).await;
        let ready = storage
            .save_file(&owner.parse().unwrap(), "abandoned", b"crashed before SQL")
            .await
            .unwrap();
        let unknown = format!("{owner}/{}/unknown", Uuid::new_v4());
        let unknown_file = temp.path().join("files").join(&unknown);
        std::fs::create_dir_all(unknown_file.parent().unwrap()).unwrap();
        std::fs::write(&unknown_file, b"pre-upgrade orphan").unwrap();
        let active = format!("{owner}/{}/active", Uuid::new_v4());
        let lease = begin_write(&db, &active).await.unwrap();
        let active_file = temp.path().join("files").join(&active);
        std::fs::create_dir_all(active_file.parent().unwrap()).unwrap();
        std::fs::write(&active_file, b"write in progress").unwrap();
        for path in protected.iter().chain([&ready, &unknown, &active]) {
            age_object(&temp, path, now);
        }
        exec(&db,format!("UPDATE blob_inventory SET first_seen={} WHERE path IN ('{ready}','{active}'); UPDATE blob_inventory SET lease_until={} WHERE path='{active}'",now-10000,now+10000)).await;
        let config = MaintenanceConfig {
            orphan_grace_seconds: 300,
            reconcile_legacy_dedicated_namespace: true,
            scan_page_size: 2,
            ..Default::default()
        };
        scan(&db, &storage, &config, now).await;
        persistence::drain_storage_gc(&db, &storage).await.unwrap();
        assert!(storage.read_file(&ready).await.is_err());
        assert!(storage.read_file(&unknown).await.is_ok());
        assert!(storage.read_file(&active).await.is_ok());
        scan(&db, &storage, &config, now + 301).await;
        persistence::drain_storage_gc(&db, &storage).await.unwrap();
        assert!(storage.read_file(&unknown).await.is_err());
        for path in protected {
            assert!(
                storage.read_file(&path).await.is_ok(),
                "reference deleted: {path}"
            );
        }
        assert_eq!(count(&db,format!("SELECT COUNT(*) AS value FROM blob_inventory WHERE path='{active}' AND state='writing'")).await,1);
        lease.finish().await.unwrap();
        assert!(
            storage
                .write_file(&unknown, b"late resurrection", "text/plain")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn cross_instance_gc_claim_fences_a_late_metadata_commit_before_network_delete() {
        let (temp, db, local, owner) = fixture().await;
        let (file_id, old, _) = file(&db, &local, &owner).await;
        let path = format!("{owner}/{}/candidate", Uuid::new_v4());
        exec(&db,format!("INSERT INTO blob_inventory(path,state,first_seen) VALUES ('{path}','ready',0); INSERT INTO storage_gc(path,created_at) VALUES ('{path}','2000-01-01T00:00:00Z')")).await;
        let second = DbPool::with_timeouts(
            temp.path().join("test.db").to_str().unwrap(),
            2,
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(3),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        async fn read_delete(socket: &mut tokio::net::TcpStream, path: &str) {
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut chunk = [0; 1024];
                let size = socket.read(&mut chunk).await.unwrap();
                assert!(size > 0);
                request.extend_from_slice(&chunk[..size]);
                assert!(request.len() <= 16 * 1024);
            }
            assert!(request.starts_with(format!("DELETE /bucket/{path} HTTP/1.1\r\n").as_bytes()));
        }
        async fn active_claim(db: &DbPool, path: &str) -> String {
            #[derive(QueryableByName)]
            struct Claim {
                #[diesel(sql_type = Text)]
                token: String,
                #[diesel(sql_type = Text)]
                state: String,
                #[diesel(sql_type = BigInt)]
                claim_until: i64,
            }
            let path = path.to_owned();
            let claim = db
                .run(move |conn| {
                    diesel::sql_query(
                        "SELECT g.claim_token AS token,b.state,g.claim_until \
                         FROM storage_gc g JOIN blob_inventory b ON b.path=g.path \
                         WHERE g.path=? AND g.claim_token IS NOT NULL",
                    )
                    .bind::<Text, _>(path)
                    .get_result::<Claim>(conn)
                })
                .await
                .unwrap();
            assert_eq!(claim.state, "deleting");
            assert!(claim.claim_until > chrono::Utc::now().timestamp());
            claim.token
        }
        let (arrived, barrier) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let (endpoint_db, endpoint_path) = (db.clone(), path.clone());
        let endpoint = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_delete(&mut socket, &endpoint_path).await;
            let first_claim = active_claim(&endpoint_db, &endpoint_path).await;
            arrived.send(first_claim.clone()).unwrap();
            tokio::select! {
                unexpected = listener.accept() => {
                    unexpected.unwrap();
                    panic!("another DELETE arrived while the original claim was held");
                }
                released = wait => released.unwrap(),
            }
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            drop(socket);
            // A failed metadata commit owns detached candidate cleanup. Its
            // enqueue may happen after finalization removed the first outbox
            // row. A later DELETE is legitimate only under a fresh SQL claim;
            // the mock must stay alive for the whole drain, not one request.
            let mut claims = std::collections::HashSet::from([first_claim]);
            tokio::pin!(stopped);
            loop {
                tokio::select! {
                    done = &mut stopped => {
                        done.unwrap();
                        break;
                    }
                    incoming = listener.accept() => {
                        let (mut socket, _) = incoming.unwrap();
                        read_delete(&mut socket, &endpoint_path).await;
                        assert!(claims.insert(active_claim(&endpoint_db, &endpoint_path).await),
                            "a repeated DELETE must have a new persistent claim");
                        socket.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await.unwrap();
                    }
                }
            }
            claims
        });
        let remote = Arc::new(StorageBackend::S3(
            super::super::s3_storage::S3Storage::new(
                "bucket",
                "us-east-1",
                "test",
                "secret",
                &format!("http://{address}"),
            )
            .without_proxy_for_tests(),
        ));
        remote.attach_database(db.clone()).unwrap();
        let (worker_db, worker_storage) = (db.clone(), remote.clone());
        let worker = tokio::spawn(async move {
            persistence::drain_storage_gc(&worker_db, &worker_storage).await
        });
        let first_claim = tokio::time::timeout(std::time::Duration::from_secs(5), barrier)
            .await
            .unwrap()
            .unwrap();
        // A second reaper cannot acquire the live claim or send another DELETE.
        persistence::drain_storage_gc(&second, &remote)
            .await
            .unwrap();
        assert_eq!(active_claim(&second, &path).await, first_claim);
        let target = path.clone();
        let file = file_id.clone();
        assert!(
            second
                .run(
                    move |conn| diesel::sql_query("UPDATE files SET path=? WHERE id=?")
                        .bind::<Text, _>(target)
                        .bind::<Text, _>(file)
                        .execute(conn)
                )
                .await
                .is_err()
        );
        let target = path.clone();
        let file = file_id.clone();
        let committed = persistence::commit_blob(&second, path.clone(), move |conn| {
            diesel::sql_query("UPDATE files SET path=? WHERE id=?")
                .bind::<Text, _>(target)
                .bind::<Text, _>(file)
                .execute(conn)?;
            Ok(())
        })
        .await;
        assert!(matches!(committed, Err(AppError::Conflict(_))));
        release.send(()).unwrap();
        worker.await.unwrap().unwrap();
        stop.send(()).unwrap();
        let claims = endpoint.await.unwrap();
        assert!(claims.contains(&first_claim));
        // This fixture has one original outbox row and one failed commit's
        // candidate cleanup, so a fresh claim is legitimate at most once.
        assert!(
            claims.len() <= 2,
            "unexpected DELETE claims: {}",
            claims.len()
        );
        eprintln!("observed {} distinct DELETE claims", claims.len());
        assert_eq!(count(&db,format!("SELECT COUNT(*) AS value FROM blob_inventory WHERE path='{path}' AND state='deleted'")).await,1);
        assert_eq!(
            count(
                &db,
                format!(
                    "SELECT COUNT(*) AS value FROM files WHERE id='{file_id}' AND path='{old}'"
                )
            )
            .await,
            1
        );
        assert!(begin_write(&second, &path).await.is_err());
        assert_eq!(local.read_file(&old).await.unwrap(), b"current object");
    }

    #[tokio::test]
    async fn scanner_and_unlink_never_follow_symlinks_or_windows_junctions() {
        let (temp, db, storage, _) = fixture().await;
        let outside = tempfile::tempdir().unwrap();
        let external = outside.path().join("victim");
        std::fs::write(&external, b"outside storage").unwrap();
        let name = Uuid::new_v4().to_string();
        let link = temp.path().join("files").join(&name);
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let status = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&link)
                .arg(outside.path())
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(status.status.success(), "could not create junction fixture");
        }
        let path = format!("{name}/victim");
        assert_eq!(
            scan(
                &db,
                &storage,
                &MaintenanceConfig::default(),
                chrono::Utc::now().timestamp() + 100000
            )
            .await,
            0
        );
        assert!(storage.read_file(&path).await.is_err());
        assert!(storage.delete_file(&path).await.is_err());
        assert_eq!(std::fs::read(external).unwrap(), b"outside storage");
    }
    #[tokio::test]
    async fn active_slow_write_is_protected_and_expired_writer_cannot_publish_after_gc_fence() {
        async fn read_head(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut chunk = [0; 1024];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
            }
            request
        }

        let (_temp, db, _local, owner) = fixture().await;
        let path = format!("{owner}/{}/slow", Uuid::new_v4());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (started, observed) = tokio::sync::oneshot::channel();
        let (release, allowed) = tokio::sync::oneshot::channel();
        let listed_path = path.clone();
        let modified = chrono::Utc::now();
        let last_modified = modified.to_rfc3339();
        let endpoint = tokio::spawn(async move {
            let (mut put, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            loop {
                let mut chunk = [0; 1024];
                let count = put.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                data.extend_from_slice(&chunk[..count]);
                if let Some(end) = data.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if data.len() >= end + 4 + length {
                        assert!(data.starts_with(b"PUT "));
                        break;
                    }
                }
            }
            started.send(()).unwrap();
            let (mut delete, _) = listener.accept().await.unwrap();
            let delete_line = format!("DELETE /bucket/{listed_path} ");
            assert!(
                read_head(&mut delete)
                    .await
                    .starts_with(delete_line.as_bytes())
            );
            delete
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            let _ = allowed.await;
            put.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();

            // The delayed PUT has recreated the physical object. The normal
            // scanner sees it, and the durable fence survives a second DELETE.
            let (mut list, _) = listener.accept().await.unwrap();
            assert!(
                read_head(&mut list)
                    .await
                    .starts_with(b"GET /bucket?list-type=2&")
            );
            let xml = format!(
                "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>{listed_path}</Key><LastModified>{last_modified}</LastModified></Contents></ListBucketResult>"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
                xml.len()
            );
            list.write_all(response.as_bytes()).await.unwrap();
            let (mut delete, _) = listener.accept().await.unwrap();
            assert!(
                read_head(&mut delete)
                    .await
                    .starts_with(delete_line.as_bytes())
            );
            delete
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });
        let storage = Arc::new(StorageBackend::S3(
            super::super::s3_storage::S3Storage::new(
                "bucket",
                "us-east-1",
                "test",
                "secret",
                &format!("http://{address}"),
            ),
        ));
        storage.attach_database(db.clone()).unwrap();
        let (writer_storage, key) = (storage.clone(), path.clone());
        let writer = tokio::spawn(async move {
            writer_storage
                .write_file(&key, b"late physical PUT", "text/plain")
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), observed)
            .await
            .unwrap()
            .unwrap();
        exec(&db,format!("UPDATE blob_inventory SET first_seen=0 WHERE path='{path}'; INSERT INTO storage_gc(path,created_at) VALUES ('{path}','2000-01-01T00:00:00Z')")).await;
        persistence::drain_storage_gc(&db, &storage).await.unwrap();
        assert_eq!(count(&db,format!("SELECT COUNT(*) AS value FROM blob_inventory WHERE path='{path}' AND state='writing'")).await,1);
        // Simulate a lost process/heartbeat; recovery owns the key from now on.
        exec(
            &db,
            format!("UPDATE blob_inventory SET lease_until=0 WHERE path='{path}'; DELETE FROM storage_gc WHERE path='{path}'"),
        )
        .await;
        assert_eq!(
            expire_blob_writes_page(
                &db,
                &mut ExpiredWriteCursor::default(),
                &MaintenanceConfig::default(),
                chrono::Utc::now().timestamp(),
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(count(&db,format!("SELECT COUNT(*) AS value FROM blob_inventory WHERE path='{path}' AND state='deleting'")).await,1);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            persistence::drain_storage_gc(&db, &storage),
        )
        .await
        .unwrap()
        .unwrap();
        release.send(()).unwrap();
        assert!(matches!(writer.await.unwrap(), Err(AppError::Conflict(_))));
        assert_eq!(count(&db,format!("SELECT COUNT(*) AS value FROM blob_inventory WHERE path='{path}' AND state='deleted'")).await,1);
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM storage_gc").await,
            0
        );
        let config = MaintenanceConfig {
            orphan_grace_seconds: 30,
            ..MaintenanceConfig::default()
        };
        let mut scan = storage.begin_scan().await.unwrap();
        assert_eq!(
            reconcile_page(&db, &storage, &mut scan, &config, modified.timestamp() + 31)
                .await
                .unwrap(),
            1
        );
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            persistence::drain_storage_gc(&db, &storage),
        )
        .await
        .unwrap()
        .unwrap();
        endpoint.await.unwrap();
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM storage_gc").await,
            0
        );
        assert_eq!(count(&db,format!("SELECT COUNT(*) AS value FROM blob_inventory WHERE path='{path}' AND state='deleted'")).await,1);
        assert!(matches!(
            persistence::commit_blob(&db, path, |_| Ok(())).await,
            Err(AppError::Conflict(_))
        ));
    }
    #[tokio::test]
    async fn same_day_hard_cap_rest_gaps_and_canonical_live_ot_bases_are_bounded() {
        let (_temp, db, storage, owner) = fixture().await;
        let (file_id, _, _) = file(&db, &storage, &owner).await;
        let document = Uuid::new_v4().to_string();
        let mut content =
            serde_json::to_value(syntra_doc_engine::WriterDocument::new("base".into())).unwrap();
        let (doc, file, actor, base) = (document.clone(), file_id, owner.clone(), content.clone());
        db.run(move|conn|conn.immediate_transaction::<_,AppError,_>(|conn|{
            diesel::sql_query("INSERT INTO documents(id,file_id,doc_type,content,version) VALUES (?,?,'writer',?,300)").bind::<Text,_>(&doc).bind::<Text,_>(file).bind::<Text,_>(base.to_string()).execute(conn)?;
            let date=chrono::Utc::now().to_rfc3339();
            for version in 1..=300 {
                diesel::sql_query("INSERT INTO versions(id,document_id,version_number,content,created_by,created_at) VALUES (?,?,?,?,?,?)")
                    .bind::<Text,_>(format!("snapshot-{version}")).bind::<Text,_>(&doc).bind::<Integer,_>(version).bind::<Text,_>(base.to_string()).bind::<Text,_>(&actor).bind::<Text,_>(&date).execute(conn)?;
            }
            for duplicate in 0..150 {
                diesel::sql_query("INSERT INTO versions(id,document_id,version_number,content,created_by,created_at) VALUES (?,?,300,?,?,?)")
                    .bind::<Text,_>(format!("duplicate-{duplicate:03}")).bind::<Text,_>(&doc).bind::<Text,_>(base.to_string()).bind::<Text,_>(&actor).bind::<Text,_>(&date).execute(conn)?;
            }
            // Stale pre-REST logs cannot pin everything through version 300.
            diesel::sql_query("INSERT INTO operation_log(id,document_id,user_id,op_type,op_data,version,created_at) VALUES ('old-log',?,?,'{}','{}',1,?)")
                .bind::<Text,_>(doc).bind::<Text,_>(actor).bind::<Text,_>(date).execute(conn)?;
            Ok(())
        })).await.unwrap();
        let policy = MaintenanceConfig {
            history_max_versions: 100,
            history_max_age_days: 0,
            history_batch_size: 50,
            ..Default::default()
        };
        let mut cursor = HistoryCursor::default();
        for _ in 0..24 {
            assert!(
                retain_history_page(&db, &mut cursor, &policy, chrono::Utc::now())
                    .await
                    .unwrap()
                    <= 100
            );
        }
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM versions").await,
            100
        );
        // Three actual serialized insert operations form a contiguous live suffix.
        let (doc, actor, base) = (document.clone(), owner.clone(), content.clone());
        db.run(move|conn|conn.immediate_transaction::<_,AppError,_>(|conn|{
            let date=chrono::Utc::now().to_rfc3339();
            for version in 301..=303 {
                let operation=syntra_collab::Operation::new(actor.parse().unwrap(),Uuid::new_v4(),1,syntra_collab::OpType::Insert{path:vec!["title".into()],index:4+(version-301) as usize,content:char::from(b'A'+(version-301) as u8).to_string()}).with_base_version((version-1) as u64);
                diesel::sql_query("INSERT INTO operation_log(id,document_id,user_id,op_type,op_data,version,created_at) VALUES (?,?,?,?,?,?,?)")
                    .bind::<Text,_>(format!("live-{version}")).bind::<Text,_>(&doc).bind::<Text,_>(&actor).bind::<Text,_>(serde_json::to_string(&operation.op_type).unwrap()).bind::<Text,_>(serde_json::to_string(&operation).unwrap()).bind::<Integer,_>(version).bind::<Text,_>(&date).execute(conn)?;
                let mut snapshot=base.clone();snapshot["title"]=serde_json::json!(format!("base{}",&"ABC"[..(version-300) as usize]));
                diesel::sql_query("INSERT INTO versions(id,document_id,version_number,content,created_by,created_at) VALUES (?,?,?,?,?,?)")
                    .bind::<Text,_>(format!("live-base-{version}")).bind::<Text,_>(&doc).bind::<Integer,_>(version).bind::<Text,_>(snapshot.to_string()).bind::<Text,_>(&actor).bind::<Text,_>(&date).execute(conn)?;
            }
            let mut current=base;current["title"]=serde_json::json!("baseABC");
            diesel::sql_query("UPDATE documents SET version=303,content=? WHERE id=?").bind::<Text,_>(current.to_string()).bind::<Text,_>(doc).execute(conn)?;
            Ok(())
        })).await.unwrap();
        let tight = MaintenanceConfig {
            history_max_versions: 2,
            history_max_age_days: 0,
            history_batch_size: 25,
            ..Default::default()
        };
        let mut cursor = HistoryCursor::default();
        for _ in 0..20 {
            retain_history_page(&db, &mut cursor, &tight, chrono::Utc::now())
                .await
                .unwrap();
        }
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM versions").await,
            4
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS value FROM versions WHERE version_number=300"
            )
            .await,
            1
        );
        let config: syntra_core::config::AppConfig = toml::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config/default.toml"
        )))
        .unwrap();
        let state = crate::state::AppState {
            db: db.clone(),
            config,
            storage: storage.clone(),
            collab: syntra_collab::CollabManager::new(),
            started_at: chrono::Utc::now(),
            ai: None,
            thumbnail: None,
            notifications: Arc::new(super::super::NotificationBus::new()),
            wopi_sessions: Arc::new(super::super::WopiSessionStore::new()),
        };
        // A stale delete really loads the retained canonical base and preserves
        // the three concurrent inserts, proving this exception is operational.
        let operation = syntra_collab::Operation::new(
            owner.parse().unwrap(),
            Uuid::new_v4(),
            1,
            syntra_collab::OpType::Delete {
                path: vec!["title".into()],
                index: 0,
                length: 4,
                removed_content: Some("base".into()),
            },
        )
        .with_base_version(300);
        let committed = super::super::collaboration::commit_operation(
            &state,
            document.parse().unwrap(),
            operation,
        )
        .await
        .unwrap();
        assert_eq!(committed.content["title"], "ABC");
        assert_eq!(committed.version, 304);
        content["title"] = serde_json::json!("rest gap");
        assert_eq!(
            persistence::commit_document(
                &db,
                &storage,
                &document,
                &owner,
                304,
                content.to_string(),
                None
            )
            .await
            .unwrap(),
            305
        );
        let mut cursor = HistoryCursor::default();
        for _ in 0..20 {
            retain_history_page(&db, &mut cursor, &tight, chrono::Utc::now())
                .await
                .unwrap();
        }
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM versions").await,
            2
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS value FROM versions WHERE version_number<303"
            )
            .await,
            0
        );
        // Age is a second upper bound; the authoritative current document stays.
        exec(
            &db,
            "UPDATE versions SET created_at='2000-01-01T00:00:00Z'".into(),
        )
        .await;
        let aging = MaintenanceConfig {
            history_max_versions: 100,
            history_max_age_days: 30,
            ..Default::default()
        };
        let mut cursor = HistoryCursor::default();
        for _ in 0..4 {
            retain_history_page(&db, &mut cursor, &aging, chrono::Utc::now())
                .await
                .unwrap();
        }
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM versions").await,
            0
        );
        assert_eq!(
            count(&db, "SELECT version AS value FROM documents").await,
            305
        );
    }

    #[tokio::test]
    async fn regenerated_thumbnails_remain_immutable_until_reference_switch() {
        let (_temp, db, storage, owner) = fixture().await;
        let (file_id, _, _) = file(&db, &storage, &owner).await;
        let mut image = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(8, 8)
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        let source = storage
            .save_file(&owner.parse().unwrap(), "image.png", image.get_ref())
            .await
            .unwrap();
        let thumbnails = super::super::ThumbnailService::new();
        let first = thumbnails
            .generate_thumbnail(&storage, &source, &file_id)
            .await
            .unwrap();
        exec(
            &db,
            format!("UPDATE files SET thumbnail_path='{first}' WHERE id='{file_id}'"),
        )
        .await;
        let second = thumbnails
            .generate_thumbnail(&storage, &source, &file_id)
            .await
            .unwrap();
        assert_ne!(first, second);
        let old = first.clone();
        db.run(move |conn| persistence::enqueue_gc(conn, &old))
            .await
            .unwrap();
        persistence::drain_storage_gc(&db, &storage).await.unwrap();
        assert!(storage.read_file(&first).await.is_ok());
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM storage_gc").await,
            0
        );
        exec(
            &db,
            format!("UPDATE files SET thumbnail_path='{second}' WHERE id='{file_id}'"),
        )
        .await;
        assert_eq!(
            count(
                &db,
                format!("SELECT COUNT(*) AS value FROM storage_gc WHERE path='{first}'")
            )
            .await,
            1
        );
        persistence::drain_storage_gc(&db, &storage).await.unwrap();
        assert!(storage.read_file(&first).await.is_err());
        assert!(storage.read_file(&second).await.is_ok());
    }
    #[tokio::test]
    async fn a_reference_committed_after_discovery_wins_before_gc_claim() {
        let (temp, db, storage, owner) = fixture().await;
        let (file_id, _, _) = file(&db, &storage, &owner).await;
        let candidate = storage
            .save_file(&owner.parse().unwrap(), "late commit", b"must survive")
            .await
            .unwrap();
        let key = candidate.clone();
        db.run(move |conn| persistence::enqueue_gc(conn, &key))
            .await
            .unwrap();
        let second = DbPool::with_timeouts(
            temp.path().join("test.db").to_str().unwrap(),
            2,
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(3),
        )
        .unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let (reaper_barrier, reaper_db, reaper_storage) =
            (barrier.clone(), db.clone(), storage.clone());
        let reaper = tokio::spawn(async move {
            reaper_barrier.wait().await;
            persistence::drain_storage_gc(&reaper_db, &reaper_storage).await
        });
        let key = candidate.clone();
        second
            .run(move |conn| {
                diesel::sql_query("UPDATE files SET path=? WHERE id=?")
                    .bind::<Text, _>(key)
                    .bind::<Text, _>(file_id)
                    .execute(conn)
            })
            .await
            .unwrap();
        barrier.wait().await;
        reaper.await.unwrap().unwrap();
        assert_eq!(
            storage.read_file(&candidate).await.unwrap(),
            b"must survive"
        );
        assert_eq!(count(&db,format!("SELECT COUNT(*) AS value FROM blob_inventory WHERE path='{candidate}' AND state='ready'")).await,1);
    }

    #[tokio::test]
    async fn repeated_real_migrations_preserve_recovery_tombstones_and_live_references() {
        let (_temp, db, storage, owner) = fixture().await;
        let (_, current, _) = file(&db, &storage, &owner).await;
        let retired = storage
            .save_file(&owner.parse().unwrap(), "retired", b"gone")
            .await
            .unwrap();
        let key = retired.clone();
        db.run(move |conn| persistence::enqueue_gc(conn, &key))
            .await
            .unwrap();
        persistence::drain_storage_gc(&db, &storage).await.unwrap();
        for _ in 0..2 {
            db.run(|conn| {
                crate::migrations::run_directory(
                    conn,
                    &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations"),
                )
            })
            .await
            .unwrap();
        }
        assert_eq!(
            storage.read_file(&current).await.unwrap(),
            b"current object"
        );
        assert!(begin_write(&db, &retired).await.is_err());
        assert_eq!(count(&db,format!("SELECT COUNT(*) AS value FROM blob_inventory WHERE path='{retired}' AND state='deleted'")).await,1);
    }
    #[tokio::test]
    async fn independent_databases_with_the_same_owner_never_adopt_shared_root_objects() {
        let (temp, first, storage_a, owner) = fixture().await;
        let now = chrono::Utc::now().timestamp();
        let second = DbPool::with_timeouts(
            temp.path().join("backup.db").to_str().unwrap(),
            2,
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(3),
        )
        .unwrap();
        let actor = owner.clone();
        second.run(move|conn|{
            crate::migrations::run_directory(conn,&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations")).unwrap();
            diesel::sql_query("INSERT INTO users(id,username,email,password_hash) VALUES (?,'backup','backup@test.invalid','test')").bind::<Text,_>(actor).execute(conn)?;Ok::<_,AppError>(())
        }).await.unwrap();
        let root = temp.path().join("files");
        let storage_b = StorageBackend::Local(StorageService::new(root.to_str().unwrap()));
        storage_b.attach_database(second.clone()).unwrap();
        let (_, referenced_b, _) = file(&second, &storage_b, &owner).await;
        let active_b = format!("{owner}/{}/in-progress", Uuid::new_v4());
        let lease = begin_write(&second, &active_b).await.unwrap();
        let active_file = root.join(&active_b);
        std::fs::create_dir_all(active_file.parent().unwrap()).unwrap();
        std::fs::write(active_file, b"other database's active writer").unwrap();
        let abandoned_a = storage_a
            .save_file(&owner.parse().unwrap(), "my orphan", b"mine")
            .await
            .unwrap();
        exec(
            &first,
            format!("UPDATE blob_inventory SET first_seen=0 WHERE path='{abandoned_a}'"),
        )
        .await;
        let unknown = format!("{owner}/{}/unknown-ownership", Uuid::new_v4());
        let unknown_file = root.join(&unknown);
        std::fs::create_dir_all(unknown_file.parent().unwrap()).unwrap();
        std::fs::write(unknown_file, b"unowned legacy").unwrap();
        std::fs::write(root.join("foreign-application.txt"), b"foreign application").unwrap();
        for path in [&referenced_b, &active_b, &abandoned_a, &unknown] {
            age_object(&temp, path, now);
        }
        let policy = MaintenanceConfig {
            orphan_grace_seconds: 300,
            scan_page_size: 2,
            ..Default::default()
        };
        for clock in [now + 301, now + 100000, now + 200000] {
            scan(&first, &storage_a, &policy, clock).await;
            persistence::drain_storage_gc(&first, &storage_a)
                .await
                .unwrap();
        }
        assert!(storage_a.read_file(&abandoned_a).await.is_err());
        assert_eq!(
            storage_b.read_file(&referenced_b).await.unwrap(),
            b"current object"
        );
        assert_eq!(
            storage_b.read_file(&active_b).await.unwrap(),
            b"other database's active writer"
        );
        assert_eq!(
            storage_a.read_file(&unknown).await.unwrap(),
            b"unowned legacy"
        );
        assert_eq!(
            std::fs::read(root.join("foreign-application.txt")).unwrap(),
            b"foreign application"
        );
        assert_eq!(count(&first,format!("SELECT COUNT(*) AS value FROM blob_inventory WHERE path IN ('{referenced_b}','{active_b}','{unknown}')")).await,0);
        lease.finish().await.unwrap();
    }
    #[tokio::test]
    async fn upload_reservations_serialize_quota_enforce_session_cap_and_expire_in_bounded_pages() {
        let (temp, db, storage, owner) = fixture().await;
        exec(
            &db,
            format!("UPDATE users SET storage_quota=100 WHERE id='{owner}'"),
        )
        .await;
        let second = DbPool::with_timeouts(
            temp.path().join("test.db").to_str().unwrap(),
            2,
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(3),
        )
        .unwrap();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for provider in [db.clone(), second] {
            let (gate, actor) = (barrier.clone(), owner.clone());
            workers.push(tokio::spawn(async move{
                gate.wait().await;
                provider.run(move|conn|conn.immediate_transaction::<_,AppError,_>(|conn|{
                    persistence::reserve_upload_session(conn,&actor,60)?;
                    diesel::sql_query("INSERT INTO upload_sessions(id,user_id,file_name,total_size,status,created_at,expires_at) VALUES (?,?,'reserved',60,'pending',?,?)")
                        .bind::<Text,_>(Uuid::new_v4().to_string()).bind::<Text,_>(actor).bind::<Text,_>(chrono::Utc::now().to_rfc3339()).bind::<Text,_>((chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339()).execute(conn)?;
                    Ok(())
                })).await
            }));
        }
        barrier.wait().await;
        let mut successes = 0;
        let mut exhausted = 0;
        for worker in workers {
            match worker.await.unwrap() {
                Ok(()) => successes += 1,
                Err(AppError::QuotaExceeded(_)) => exhausted += 1,
                Err(error) => panic!("unexpected reservation error: {error}"),
            }
        }
        assert_eq!((successes, exhausted), (1, 1));
        let actor = owner.clone();
        assert!(matches!(
            db.run(
                move |conn| conn.immediate_transaction::<_, AppError, _>(|conn| {
                    persistence::check_staging_quota(conn, &actor, 41)
                })
            )
            .await,
            Err(AppError::QuotaExceeded(_))
        ));
        let actor = owner.clone();
        db.run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                persistence::adjust_quota(conn, &actor, 40)?;
                persistence::adjust_quota(conn, &actor, -40)
            })
        })
        .await
        .unwrap();
        let committed = storage
            .save_file(&owner.parse().unwrap(), "complete", &[0; 60])
            .await
            .unwrap();
        let actor = owner.clone();
        db.run(move|conn|conn.immediate_transaction::<_,AppError,_>(|conn|{
            diesel::sql_query("UPDATE upload_sessions SET status='processing' WHERE user_id=? AND status='pending'").bind::<Text,_>(&actor).execute(conn)?;
            persistence::adjust_quota(conn,&actor,60)?;
            diesel::sql_query("INSERT INTO files(id,name,path,size,owner_id) VALUES (?,'completed',?,60,?)").bind::<Text,_>(Uuid::new_v4().to_string()).bind::<Text,_>(committed).bind::<Text,_>(&actor).execute(conn)?;
            diesel::sql_query("UPDATE upload_sessions SET status='completed' WHERE user_id=? AND status='processing'").bind::<Text,_>(&actor).execute(conn)?;Ok(())
        })).await.unwrap();
        let actor = owner.clone();
        db.run(move|conn|conn.immediate_transaction::<_,AppError,_>(|conn|{
            diesel::sql_query("UPDATE users SET storage_quota=NULL WHERE id=?").bind::<Text,_>(&actor).execute(conn)?;
            for index in 0..32 {
                persistence::reserve_upload_session(conn,&actor,0)?;
                diesel::sql_query("INSERT INTO upload_sessions(id,user_id,file_name,total_size,status,created_at,expires_at) VALUES (?,?,'zero',0,'pending',?,?)")
                    .bind::<Text,_>(format!("zero-{index}")).bind::<Text,_>(&actor).bind::<Text,_>(chrono::Utc::now().to_rfc3339()).bind::<Text,_>((chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339()).execute(conn)?;
            }
            assert!(matches!(persistence::reserve_upload_session(conn,&actor,0),Err(AppError::RateLimit(_))));
            Ok(())
        })).await.unwrap();
        exec(
            &db,
            "UPDATE upload_sessions SET expires_at='2000-01-01T00:00:00Z' WHERE status='pending'"
                .into(),
        )
        .await;
        let policy = MaintenanceConfig {
            history_batch_size: 2,
            ..Default::default()
        };
        for _ in 0..20 {
            assert!(
                expire_uploads_page(&db, &policy, chrono::Utc::now())
                    .await
                    .unwrap()
                    <= 2
            );
        }
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS value FROM upload_sessions WHERE status='pending'"
            )
            .await,
            0
        );
        let expired = Uuid::new_v4().to_string();
        let active = Uuid::new_v4().to_string();
        exec(&db,format!("INSERT INTO upload_sessions(id,user_id,file_name,total_size,status,created_at,expires_at) VALUES ('{expired}','{owner}','old',50,'pending','2000-01-01T00:00:00Z','2000-01-02T00:00:00Z'),('{active}','{owner}','active',50,'pending','2000-01-01T00:00:00Z','2999-01-01T00:00:00Z')")).await;
        let mut old_paths = Vec::new();
        for index in 1..=5 {
            let path = storage
                .save_file(&owner.parse().unwrap(), "expired chunk", b"old chunk")
                .await
                .unwrap();
            let key = path.clone();
            let session = expired.clone();
            db.run(move|conn|diesel::sql_query("INSERT INTO upload_chunks(session_id,chunk_index,path,size,digest) VALUES (?,?,?,9,'test')").bind::<Text,_>(session).bind::<Integer,_>(index).bind::<Text,_>(key).execute(conn)).await.unwrap();
            old_paths.push(path);
        }
        for _ in 0..5 {
            assert!(
                expire_uploads_page(&db, &policy, chrono::Utc::now())
                    .await
                    .unwrap()
                    <= 2
            );
        }
        assert_eq!(
            count(
                &db,
                format!("SELECT COUNT(*) AS value FROM upload_sessions WHERE id='{expired}'")
            )
            .await,
            0
        );
        assert_eq!(
            count(
                &db,
                format!("SELECT COUNT(*) AS value FROM upload_sessions WHERE id='{active}'")
            )
            .await,
            1
        );
        assert_eq!(
            count(
                &db,
                format!("SELECT storage_used AS value FROM users WHERE id='{owner}'")
            )
            .await,
            60
        );
        persistence::drain_storage_gc(&db, &storage).await.unwrap();
        for path in old_paths {
            assert!(storage.read_file(&path).await.is_err());
        }
    }
    #[tokio::test]
    async fn gc_backlog_rejects_all_staging_without_creating_fake_objects_or_outbox_rows() {
        let (_temp, db, storage, owner) = fixture().await;
        let (_, current, _) = file(&db, &storage, &owner).await;
        let actor = owner.clone();
        db.run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                for index in 0..1024 {
                    persistence::enqueue_gc(
                        conn,
                        &format!("{actor}/{}/retired-{index}", Uuid::new_v4()),
                    )?;
                }
                Ok(())
            })
        })
        .await
        .unwrap();
        assert!(matches!(
            storage
                .save_file(&owner.parse().unwrap(), "blocked", b"not written")
                .await,
            Err(AppError::RateLimit(_))
        ));
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = Arc::new(capacity.clone().acquire_owned().await.unwrap());
        assert!(matches!(
            persistence::stage_blob(
                &db,
                storage.clone(),
                owner.parse().unwrap(),
                "blocked staged".into(),
                Arc::new(b"not written".to_vec()),
                permit
            )
            .await,
            Err(AppError::RateLimit(_))
        ));
        assert_eq!(capacity.available_permits(), 1);
        tokio::task::yield_now().await;
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM storage_gc").await,
            1024
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM blob_inventory").await,
            1
        );
        // Cleanup is independent of write admission, so the system can recover.
        persistence::drain_storage_gc(&db, &storage).await.unwrap();
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM storage_gc").await,
            924
        );
        let admitted = storage
            .save_file(&owner.parse().unwrap(), "admitted", b"after cleanup")
            .await
            .unwrap();
        assert_eq!(
            storage.read_file(&admitted).await.unwrap(),
            b"after cleanup"
        );
        assert_eq!(
            storage.read_file(&current).await.unwrap(),
            b"current object"
        );
    }

    #[tokio::test]
    async fn protected_snapshot_hints_retire_without_permanently_filling_the_gc_write_gate() {
        let (_temp, db, storage, owner) = fixture().await;
        let (file_id, current, _) = file(&db, &storage, &owner).await;
        let (actor, file, key) = (owner.clone(), file_id, current.clone());
        db.run(move|conn|conn.immediate_transaction::<_,AppError,_>(|conn|{
            for index in 0..1024 {
                let path=if index==0{key.clone()}else{format!("{actor}/{}/retained",Uuid::new_v4())};
                diesel::sql_query("INSERT INTO file_versions(id,file_id,version_number,path,size,created_by,created_at) VALUES (?,?,?,?,0,?,'2000-01-01T00:00:00Z')")
                    .bind::<Text,_>(Uuid::new_v4().to_string()).bind::<Text,_>(&file).bind::<Integer,_>(index+1).bind::<Text,_>(&path).bind::<Text,_>(&actor).execute(conn)?;
                persistence::enqueue_gc(conn,&path)?;
            }Ok(())
        })).await.unwrap();
        assert!(matches!(
            storage
                .save_file(&owner.parse().unwrap(), "full", b"blocked")
                .await,
            Err(AppError::RateLimit(_))
        ));
        persistence::drain_storage_gc(&db, &storage).await.unwrap();
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM storage_gc").await,
            924
        );
        let admitted = storage
            .save_file(&owner.parse().unwrap(), "healthy", b"admitted")
            .await
            .unwrap();
        for _ in 0..10 {
            persistence::drain_storage_gc(&db, &storage).await.unwrap();
        }
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM storage_gc").await,
            0
        );
        assert_eq!(
            count(&db, "SELECT COUNT(*) AS value FROM file_versions").await,
            1024
        );
        assert_eq!(
            count(
                &db,
                "SELECT COUNT(*) AS value FROM blob_inventory WHERE state IN ('deleting','deleted')"
            )
            .await,
            0
        );
        assert_eq!(
            storage.read_file(&current).await.unwrap(),
            b"current object"
        );
        assert_eq!(storage.read_file(&admitted).await.unwrap(), b"admitted");
    }
}
