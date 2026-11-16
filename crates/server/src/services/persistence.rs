//! Atomic database commits for immutable blobs and durable deferred cleanup.
use super::StorageBackend;
use crate::database::telemetry::{
    GcObservation, GcPhase, TransactionStage,
    immediate_transaction as observed_immediate_transaction,
};
use crate::middleware::request_observability::{
    with_database_stage, with_request_database_observation,
};
use crate::state::DbPool;
use chrono::Utc;
use contrix_core::error::AppError;
use contrix_core::models::{
    Document, File, FileLock, NewFileLock, NewOperationLogEntry, NewVersion,
};
use contrix_core::schema::{
    documents::dsl as d, file_locks::dsl as fl, files::dsl as f, room_members::dsl as rm,
    versions::dsl as v,
};
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Integer, Text};
use futures_util::StreamExt;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, oneshot};
use uuid::Uuid;

pub fn authorize(
    conn: &mut SqliteConnection,
    file: &File,
    user: &str,
    role: &str,
) -> Result<(), AppError> {
    if file.is_trashed {
        return Err(AppError::NotFound("Document not found".into()));
    }
    authorize_including_trashed(conn, file, user, role)
}

/// Trash restoration and purge still obey current room membership.
pub fn authorize_including_trashed(
    conn: &mut SqliteConnection,
    file: &File,
    user: &str,
    role: &str,
) -> Result<(), AppError> {
    crate::services::tenancy::ensure_same_tenant(conn, user, &file.owner_id)?;
    if let Some(room) = &file.room_id {
        authorize_room(conn, room, user, role)?;
    } else if file.owner_id != user {
        return Err(AppError::NotFound("Document not found".into()));
    }
    Ok(())
}

pub fn authorize_room(
    conn: &mut SqliteConnection,
    room: &str,
    user: &str,
    role: &str,
) -> Result<(), AppError> {
    use contrix_core::schema::rooms::dsl as r;
    let owner = r::rooms
        .find(room)
        .select(r::owner_id)
        .first::<String>(conn)
        .optional()?
        .ok_or_else(|| AppError::NotFound("Room not found".into()))?;
    crate::services::tenancy::ensure_same_tenant(conn, user, &owner)?;
    let actual = rm::room_members
        .filter(rm::room_id.eq(room))
        .filter(rm::user_id.eq(user))
        .select(rm::role)
        .first::<String>(conn)
        .optional()?;
    if !actual.is_some_and(|actual| match role {
        "viewer" => true,
        "editor" => matches!(actual.as_str(), "owner" | "admin" | "editor"),
        _ => matches!(actual.as_str(), "owner" | "admin"),
    }) {
        return Err(AppError::Forbidden("Insufficient room permissions".into()));
    }
    Ok(())
}

pub fn check_file_lock(
    conn: &mut SqliteConnection,
    file_id: &str,
    user: &str,
    token: Option<&str>,
) -> Result<(), AppError> {
    let lock = fl::file_locks
        .filter(fl::file_id.eq(file_id))
        .first::<FileLock>(conn)
        .optional()?;
    if let Some(lock) = lock {
        let active = chrono::DateTime::parse_from_rfc3339(&lock.expires_at)
            .is_ok_and(|expires| expires.with_timezone(&Utc) > Utc::now());
        if active && (lock.locked_by != user || token.is_some_and(|token| token != lock.lock_token))
        {
            return Err(AppError::Conflict(
                "File is locked by another editor".into(),
            ));
        }
    }
    Ok(())
}

pub async fn ensure_writable_lock(
    db: &DbPool,
    file_id: &str,
    user: &str,
    token: Option<&str>,
) -> Result<(), AppError> {
    let (file_id, user, token) = (
        file_id.to_owned(),
        user.to_owned(),
        token.map(str::to_owned),
    );
    db.run(move |conn| check_file_lock(conn, &file_id, &user, token.as_deref()))
        .await
}

/// Must run inside the same write transaction as the corresponding file update.
pub fn adjust_quota(conn: &mut SqliteConnection, owner: &str, delta: i64) -> Result<(), AppError> {
    if delta > 0 {
        check_quota_with_reservations(conn, owner, delta, false)?;
    }
    let changed = diesel::sql_query("UPDATE users SET storage_used = MAX(0, storage_used + ?) WHERE id = ? AND (? <= 0 OR storage_quota IS NULL OR storage_used + ? <= storage_quota)")
        .bind::<BigInt, _>(delta).bind::<Text, _>(owner).bind::<BigInt, _>(delta).bind::<BigInt, _>(delta).execute(conn)?;
    if changed != 1 {
        return Err(AppError::QuotaExceeded("Storage quota exceeded".into()));
    }
    Ok(())
}

/// Bound pending multipart/chunk staging before the session INSERT. The caller
/// must hold the same IMMEDIATE transaction for this check and that INSERT.
pub fn reserve_upload_session(
    conn: &mut SqliteConnection,
    owner: &str,
    new_size: i64,
) -> Result<(), AppError> {
    if new_size < 0 {
        return Err(AppError::Validation(
            "Upload size cannot be negative".into(),
        ));
    }
    check_quota_with_reservations(conn, owner, new_size, true)
}

/// Preflight before staging; the metadata transaction still repeats quota
/// validation atomically with its actual usage update.
pub fn check_staging_quota(
    conn: &mut SqliteConnection,
    owner: &str,
    delta: i64,
) -> Result<(), AppError> {
    if delta < 0 {
        return Err(AppError::Validation(
            "Staging size cannot be negative".into(),
        ));
    }
    check_quota_with_reservations(conn, owner, delta, false)
}

fn check_quota_with_reservations(
    conn: &mut SqliteConnection,
    owner: &str,
    delta: i64,
    new_session: bool,
) -> Result<(), AppError> {
    #[derive(diesel::QueryableByName)]
    struct Account {
        #[diesel(sql_type=BigInt)]
        used: i64,
        #[diesel(sql_type=diesel::sql_types::Nullable<BigInt>)]
        quota: Option<i64>,
    }
    #[derive(diesel::QueryableByName)]
    struct Reservation {
        #[diesel(sql_type=BigInt)]
        size: i64,
    }
    let account = diesel::sql_query(
        "SELECT storage_used AS used,storage_quota AS quota FROM users WHERE id=?",
    )
    .bind::<Text, _>(owner)
    .get_result::<Account>(conn)?;
    // LIMIT is independent of the total session table. Legacy 'active' is
    // included until its final old session has expired or been migrated.
    let reservations=diesel::sql_query("SELECT total_size AS size FROM upload_sessions WHERE user_id=? AND status IN ('pending','active') AND expires_at>? LIMIT 33")
        .bind::<Text,_>(owner).bind::<Text,_>(Utc::now().to_rfc3339()).load::<Reservation>(conn)?;
    if reservations.len() > 32 || new_session && reservations.len() >= 32 {
        return Err(AppError::RateLimit(
            "At most 32 active upload sessions are allowed".into(),
        ));
    }
    let total = reservations
        .iter()
        .try_fold(account.used, |sum, row| sum.checked_add(row.size.max(0)))
        .and_then(|sum| sum.checked_add(delta))
        .ok_or_else(|| AppError::QuotaExceeded("Upload reservation accounting overflow".into()))?;
    if account.quota.is_some_and(|quota| total > quota) {
        return Err(AppError::QuotaExceeded(
            "Storage quota includes pending upload reservations".into(),
        ));
    }
    Ok(())
}

/// Replace one current blob while charging retained immutable snapshots once.
/// Call this in the same transaction as the file pointer update.
pub fn replace_file_quota(
    conn: &mut SqliteConnection,
    owner: &str,
    old_path: &str,
    old_size: i64,
    new_size: i64,
) -> Result<(), AppError> {
    if old_size < 0 || new_size < 0 {
        return Err(AppError::Validation("Negative file size".into()));
    }
    let charged = diesel::sql_query("INSERT OR IGNORE INTO retained_blobs(owner_id, path, size) SELECT ?, ?, ? WHERE EXISTS (SELECT 1 FROM file_versions JOIN files ON files.id = file_versions.file_id WHERE files.owner_id = ? AND file_versions.path = ?)")
        .bind::<Text, _>(owner).bind::<Text, _>(old_path).bind::<BigInt, _>(old_size)
        .bind::<Text, _>(owner).bind::<Text, _>(old_path).execute(conn)?;
    let delta = new_size
        .checked_sub(old_size)
        .and_then(|delta| delta.checked_add(if charged == 1 { old_size } else { 0 }))
        .ok_or_else(|| AppError::QuotaExceeded("Storage accounting overflow".into()))?;
    adjust_quota(conn, owner, delta)
}

#[derive(diesel::QueryableByName)]
struct RetainedBlob {
    #[diesel(sql_type = Text)]
    path: String,
    #[diesel(sql_type = BigInt)]
    size: i64,
}

pub(crate) fn release_retained_path(
    conn: &mut SqliteConnection,
    owner: &str,
    path: &str,
) -> Result<(), AppError> {
    let retired = diesel::sql_query("SELECT path,size FROM retained_blobs WHERE owner_id=? AND path=? AND NOT EXISTS(SELECT 1 FROM file_versions JOIN files ON files.id=file_versions.file_id WHERE files.owner_id=retained_blobs.owner_id AND file_versions.path=retained_blobs.path)")
        .bind::<Text,_>(owner).bind::<Text,_>(path).get_result::<RetainedBlob>(conn).optional()?;
    if let Some(blob) = retired {
        adjust_quota(conn, owner, -blob.size)?;
        diesel::sql_query("DELETE FROM retained_blobs WHERE owner_id=? AND path=?")
            .bind::<Text, _>(owner)
            .bind::<Text, _>(path)
            .execute(conn)?;
    }
    Ok(())
}

fn release_unreferenced_retained_quota(
    conn: &mut SqliteConnection,
    owner: &str,
) -> Result<(), AppError> {
    let retired = diesel::sql_query("SELECT path, size FROM retained_blobs WHERE owner_id = ? AND NOT EXISTS (SELECT 1 FROM file_versions JOIN files ON files.id = file_versions.file_id WHERE files.owner_id = retained_blobs.owner_id AND file_versions.path = retained_blobs.path)")
        .bind::<Text, _>(owner).load::<RetainedBlob>(conn)?;
    for blob in retired {
        adjust_quota(conn, owner, -blob.size)?;
        diesel::sql_query("DELETE FROM retained_blobs WHERE owner_id = ? AND path = ?")
            .bind::<Text, _>(owner)
            .bind::<Text, _>(blob.path)
            .execute(conn)?;
    }
    Ok(())
}

pub fn enqueue_gc(conn: &mut SqliteConnection, path: &str) -> Result<(), AppError> {
    if path.is_empty() {
        return Ok(());
    }
    diesel::sql_query("INSERT OR IGNORE INTO storage_gc(path, created_at) VALUES (?, ?)")
        .bind::<Text, _>(path)
        .bind::<Text, _>(Utc::now().to_rfc3339())
        .execute(conn)?;
    Ok(())
}

#[derive(diesel::QueryableByName)]
struct GcPath {
    #[diesel(sql_type = Text)]
    path: String,
}

// Walk the FIFO index directly: the claim-deadline index cannot also satisfy
// this ordering and otherwise sorts the eligible backlog before applying LIMIT.
const SELECT_STORAGE_GC_PATHS_SQL: &str =
    "SELECT g.path FROM storage_gc g INDEXED BY idx_storage_gc_order WHERE g.claim_until<=?
     AND NOT EXISTS(SELECT 1 FROM blob_inventory b WHERE b.path=g.path AND b.state='writing' AND b.lease_until>?)
     ORDER BY g.created_at,g.path LIMIT ?";

async fn queue_unreferenced_candidate(db: DbPool, candidate: String) {
    loop {
        let path = candidate.clone();
        match db.run(move |conn| {
            diesel::sql_query("INSERT OR IGNORE INTO storage_gc(path, created_at) SELECT ?, ? WHERE NOT EXISTS (SELECT 1 FROM files WHERE path = ? OR thumbnail_path = ?) AND NOT EXISTS (SELECT 1 FROM upload_chunks WHERE path = ?) AND NOT EXISTS (SELECT 1 FROM file_versions WHERE path = ?) AND NOT EXISTS (SELECT 1 FROM conversion_jobs WHERE output_path = ?)")
                .bind::<Text, _>(&path).bind::<Text, _>(Utc::now().to_rfc3339()).bind::<Text, _>(&path)
                .bind::<Text, _>(&path).bind::<Text, _>(&path).bind::<Text, _>(&path).bind::<Text, _>(&path).execute(conn)?;
            Ok::<_, AppError>(())
        }).await {
            Ok(()) => return,
            Err(error) => { tracing::warn!(%error, "Candidate cleanup enqueue will retry"); tokio::time::sleep(std::time::Duration::from_secs(1)).await; }
        }
    }
}

/// An immutable upload owned until a metadata transaction takes responsibility.
/// Dropping an unpolled commit future still cleans the staged object.
pub struct StagedBlob {
    pub path: String,
    db: DbPool,
    handed_off: bool,
}

impl Drop for StagedBlob {
    fn drop(&mut self) {
        if self.handed_off {
            return;
        }
        let (db, path) = (self.db.clone(), self.path.clone());
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(queue_unreferenced_candidate(db, path));
            }
            Err(error) => {
                tracing::error!(%error, path = %self.path, "Staged blob dropped after runtime shutdown; cleanup requires recovery")
            }
        }
    }
}

impl StagedBlob {
    pub async fn commit<R, F>(mut self, callback: F) -> Result<R, AppError>
    where
        R: Send + 'static,
        F: FnOnce(&mut SqliteConnection) -> Result<R, AppError> + Send + 'static,
    {
        // No await separates this handoff from polling commit_blob, whose first
        // poll installs an owner before it can yield to database admission.
        self.handed_off = true;
        commit_blob(&self.db, self.path.clone(), callback).await
    }
}

pub async fn stage_blob(
    db: &DbPool,
    storage: Arc<StorageBackend>,
    owner: Uuid,
    name: String,
    data: Arc<Vec<u8>>,
    capacity: Arc<OwnedSemaphorePermit>,
) -> Result<StagedBlob, AppError> {
    storage.attach_database(db.clone())?;
    let db = db.clone();
    tokio::spawn(with_request_database_observation(async move {
        let _capacity = capacity;
        let path = format!(
            "{owner}/{}/{}",
            Uuid::new_v4(),
            StorageBackend::sanitize_file_name(&name)
        );
        let mut staged = StagedBlob {
            path,
            db,
            handed_off: false,
        };
        if let Err(error) = storage
            .write_file(&staged.path, data.as_slice(), "application/octet-stream")
            .await
        {
            // The write gate returns RateLimit before object I/O. Rejecting
            // pressure must not manufacture additional fake GC queue entries.
            if matches!(error, AppError::RateLimit(_)) {
                staged.handed_off = true;
            }
            return Err(error);
        }
        Ok::<_, AppError>(staged)
    }))
    .await
    .map_err(|error| AppError::Storage(format!("Staged upload worker failed: {error}")))?
}

/// The cleanup owner outlives cancellation and database deadlines. It only
/// considers a candidate after its blocking transaction really retired, so a
/// timed-out caller can never unlink a blob that a late commit still needs.
pub async fn commit_blob<R, F>(db: &DbPool, candidate: String, callback: F) -> Result<R, AppError>
where
    R: Send + 'static,
    F: FnOnce(&mut SqliteConnection) -> Result<R, AppError> + Send + 'static,
{
    commit_blob_observed(db, candidate, callback, |_| {}).await
}

async fn commit_blob_observed<R, F, Notify>(
    db: &DbPool,
    candidate: String,
    callback: F,
    notify: Notify,
) -> Result<R, AppError>
where
    R: Send + 'static,
    F: FnOnce(&mut SqliteConnection) -> Result<R, AppError> + Send + 'static,
    Notify: FnOnce(&R) + Send + 'static,
{
    let (finished, completion) = oneshot::channel::<bool>();
    let cleanup_db = db.clone();
    let attachment_candidate = candidate.clone();
    tokio::spawn(async move {
        if matches!(completion.await, Ok(true)) {
            return;
        }
        queue_unreferenced_candidate(cleanup_db, candidate).await;
    });
    db.run(move |conn| {
        // Callers own their IMMEDIATE transaction. The BEFORE-reference
        // triggers also fence a GC claim occurring after this preflight.
        let result = super::maintenance::prepare_attachment(conn, &attachment_candidate)
            .and_then(|_| callback(conn));
        let _ = finished.send(result.is_ok());
        if let Ok(committed) = &result {
            notify(committed);
        }
        result
    })
    .await
}

/// Coalesce post-commit hints into at most one process-local worker. Durable
/// outbox rows survive skipped hints and are retriggered by the periodic job.
/// No network I/O or database admission is awaited on the request path.
pub fn request_gc(db: &DbPool, storage: &StorageBackend) -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RUNNING: AtomicBool = AtomicBool::new(false);
    static PENDING: AtomicBool = AtomicBool::new(false);
    struct RunningGuard;
    impl Drop for RunningGuard {
        fn drop(&mut self) {
            RUNNING.store(false, Ordering::Release);
        }
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return false;
    };
    PENDING.store(true, Ordering::Release);
    if RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    let (db, storage) = (db.clone(), storage.clone());
    let guard = RunningGuard;
    runtime.spawn(async move {
        let (_gc_observation_owner, observation) = db.observe_gc();
        PENDING.store(false, Ordering::Release);
        let mut failed = HashSet::new();
        // Coalesced hints may arrive during cleanup. Catch up in bounded pages
        // before retiring so a burst does not depend on a later HTTP request.
        for _ in 0..16 {
            match drain_storage_gc_page_observed(&db, &storage, &mut failed, &observation).await {
                Ok(page) => {
                    if let Some(error) = page.error {
                        observation.failure(&error);
                    }
                    if page.visited < 100 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                Err(error) => {
                    observation.failure(&error);
                    break;
                }
            }
        }
        drop(guard);
        // Rearm only on normal retirement. A cancelled task merely releases
        // RUNNING, which also avoids spawning work while Tokio shuts down.
        if PENDING.swap(false, Ordering::AcqRel) {
            request_gc(&db, &storage);
        }
    });
    true
}

/// Metadata is committed first; failed object deletion stays queued for retry.
pub async fn drain_storage_gc(db: &DbPool, storage: &StorageBackend) -> Result<(), AppError> {
    let page = drain_storage_gc_page(db, storage, &mut HashSet::new()).await?;
    match page.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct GcDrainPage {
    visited: usize,
    error: Option<AppError>,
}

async fn drain_storage_gc_page(
    db: &DbPool,
    storage: &StorageBackend,
    failed: &mut HashSet<String>,
) -> Result<GcDrainPage, AppError> {
    let (_owner, observation) = db.observe_gc();
    let result = drain_storage_gc_page_observed(db, storage, failed, &observation).await;
    match &result {
        Ok(page) => {
            if let Some(error) = &page.error {
                observation.failure(error);
            }
        }
        Err(error) => observation.failure(error),
    }
    result
}

async fn drain_storage_gc_page_observed(
    db: &DbPool,
    storage: &StorageBackend,
    failed: &mut HashSet<String>,
    observation: &GcObservation,
) -> Result<GcDrainPage, AppError> {
    let mut visited = 0;
    let mut first_error = None;
    while visited < 100 {
        // Two waves of at most eight 30-second deletes fit within the existing
        // 120-second ownership lease. Database work never waits on storage I/O.
        let limit = (100 - visited).min(16) as i64;
        let excluded = failed.clone();
        observation.phase(GcPhase::Claim);
        let claim_observation = observation.clone();
        let (selected, claims) = db
            .run(move |conn| {
                let now = Utc::now().timestamp();
                let paths = diesel::sql_query(SELECT_STORAGE_GC_PATHS_SQL)
                    .bind::<BigInt, _>(now)
                    .bind::<BigInt, _>(now)
                    .bind::<BigInt, _>(limit + excluded.len() as i64)
                    .load::<GcPath>(conn)?
                    .into_iter()
                    .filter(|entry| !excluded.contains(&entry.path))
                    .take(limit as usize)
                    .collect::<Vec<_>>();
                let selected = paths.len();
                claim_observation.selected(selected);
                if paths.is_empty() {
                    return Ok::<_, AppError>((0, Vec::new()));
                }
                let claims = observed_immediate_transaction::<_, AppError>(
                    conn,
                    TransactionStage::GcClaim,
                    |conn| {
                        let mut claims = Vec::new();
                        for entry in paths {
                            if let Some(token) = super::maintenance::claim_gc_in_transaction(
                                conn,
                                &entry.path,
                                Utc::now().timestamp(),
                            )? {
                                claims.push((entry.path, token));
                            }
                        }
                        Ok(claims)
                    },
                )?;
                claim_observation.claimed(claims.len());
                Ok((selected, claims))
            })
            .await?;
        if selected == 0 {
            break;
        }
        visited += selected;
        observation.phase(GcPhase::Unlink);
        let completed = futures_util::stream::iter(claims)
            .map(|(path, token)| {
                let observation = observation.clone();
                async move {
                    let in_flight = observation.unlink();
                    let result = tokio::time::timeout(
                        std::time::Duration::from_secs(30),
                        storage.delete_file(&path),
                    )
                    .await
                    .unwrap_or_else(|_| Err(AppError::Storage("Object deletion timed out".into())));
                    if let Err(error) = &result {
                        observation.failure(error);
                    }
                    in_flight.finish(result.is_ok());
                    (path, token, result)
                }
            })
            .buffer_unordered(8)
            .collect::<Vec<_>>()
            .await;
        if completed.is_empty() {
            continue;
        }
        observation.phase(GcPhase::Finalize);
        let finalize_observation = observation.clone();
        let (error, failed_paths) = db.run(move |conn| {
          let result = observed_immediate_transaction::<_, AppError>(conn, TransactionStage::GcFinalize, |conn| {
            let mut first_error = None;
            let mut failed_paths = Vec::new();
            let mut finalized = 0;
            for (path, token, result) in completed {
                match result {
                    Ok(()) => {
                        let removed = diesel::sql_query("DELETE FROM storage_gc WHERE path=? AND claim_token=?")
                            .bind::<Text, _>(&path).bind::<Text, _>(token).execute(conn)?;
                        if removed == 1 {
                            diesel::sql_query("UPDATE blob_inventory SET state='deleted',token=NULL,lease_until=0 WHERE path=?")
                                .bind::<Text, _>(path).execute(conn)?;
                            finalized += 1;
                        }
                    }
                    Err(error) => {
                        failed_paths.push(path.clone());
                        diesel::sql_query("UPDATE storage_gc SET created_at=?,claim_token=NULL,claim_until=0 WHERE path=? AND claim_token=?")
                            .bind::<Text, _>(Utc::now().to_rfc3339()).bind::<Text, _>(path).bind::<Text, _>(token).execute(conn)?;
                        first_error.get_or_insert(error);
                    }
                }
            }
            Ok((first_error, failed_paths, finalized))
          });
          result.map(|(error, paths, finalized)| {
            finalize_observation.finalized(finalized);
            (error, paths)
          })
        }).await?;
        if first_error.is_none() {
            first_error = error;
        }
        failed.extend(failed_paths);
    }
    Ok(GcDrainPage {
        visited,
        error: first_error,
    })
}

pub async fn commit_document(
    db: &DbPool,
    storage: &StorageBackend,
    document_id: &str,
    user: &str,
    expected_version: i32,
    content: String,
    label: Option<String>,
) -> Result<i32, AppError> {
    commit_document_inner(
        db,
        storage,
        document_id,
        user,
        expected_version,
        content,
        label,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn commit_document_with_credential(
    db: &DbPool,
    storage: &StorageBackend,
    document_id: &str,
    user: &str,
    expected_version: i32,
    content: String,
    credential: &crate::middleware::auth::AuthenticatedCredential,
    committed: oneshot::Sender<i32>,
) -> Result<i32, AppError> {
    commit_document_inner(
        db,
        storage,
        document_id,
        user,
        expected_version,
        content,
        None,
        None,
        None,
        None,
        None,
        Some(CommitCredential {
            credential: credential.clone(),
            method: salvo::http::Method::PUT,
            path: format!("/api/v1/documents/{document_id}"),
            wopi_authorization: None,
        }),
        Some(committed),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn restore_document_with_credential(
    db: &DbPool,
    storage: &StorageBackend,
    document_id: &str,
    version_id: &str,
    user: &str,
    expected_version: i32,
    content: String,
    credential: &crate::middleware::auth::AuthenticatedCredential,
) -> Result<i32, AppError> {
    commit_document_inner(
        db,
        storage,
        document_id,
        user,
        expected_version,
        content,
        Some("Before restore".into()),
        None,
        None,
        None,
        None,
        Some(CommitCredential {
            credential: credential.clone(),
            method: salvo::http::Method::POST,
            path: format!("/api/v1/documents/{document_id}/restore/{version_id}"),
            wopi_authorization: None,
        }),
        None,
    )
    .await
}

#[derive(Clone)]
struct CommitCredential {
    credential: crate::middleware::auth::AuthenticatedCredential,
    method: salvo::http::Method,
    path: String,
    wopi_authorization: Option<super::wopi_sessions::WopiWriteAuthorization>,
}

impl CommitCredential {
    fn validate(&self, conn: &mut SqliteConnection) -> Result<(), AppError> {
        if let Some(authorization) = &self.wopi_authorization {
            authorization.validate_in_transaction(conn)
        } else {
            crate::middleware::auth::validate_credential_in_transaction(
                conn,
                &self.credential,
                &self.method,
                &self.path,
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn commit_document_with_lock(
    db: &DbPool,
    storage: &StorageBackend,
    document_id: &str,
    user: &str,
    expected_version: i32,
    content: String,
    label: Option<String>,
    token: Option<String>,
) -> Result<i32, AppError> {
    commit_document_inner(
        db,
        storage,
        document_id,
        user,
        expected_version,
        content,
        label,
        token,
        None,
        None,
        None,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn commit_document_with_wopi_authorization(
    db: &DbPool,
    storage: &StorageBackend,
    document_id: &str,
    user: &str,
    expected_version: i32,
    content: String,
    label: Option<String>,
    token: Option<String>,
    authorization: &super::wopi_sessions::WopiWriteAuthorization,
) -> Result<i32, AppError> {
    commit_document_inner(
        db,
        storage,
        document_id,
        user,
        expected_version,
        content,
        label,
        token,
        None,
        None,
        None,
        Some(CommitCredential {
            credential: authorization.credential(),
            method: salvo::http::Method::POST,
            path: String::new(),
            wopi_authorization: Some(authorization.clone()),
        }),
        None,
    )
    .await
}

/// Hash the request before OT; timestamps are transport metadata, not identity.
pub fn operation_request_hash(operation: &contrix_collab::Operation) -> Result<String, AppError> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(operation.base_version, &operation.op_type))
        .map_err(|error| AppError::Validation(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[derive(diesel::QueryableByName)]
struct OperationReceipt {
    #[diesel(sql_type = Text)]
    user_id: String,
    #[diesel(sql_type = Text)]
    request_hash: String,
    #[diesel(sql_type = Integer)]
    version: i32,
}
#[derive(diesel::QueryableByName)]
struct OperationClient {
    #[diesel(sql_type = Text)]
    user_id: String,
    #[diesel(sql_type = BigInt)]
    max_counter: i64,
}

fn check_operation_receipt(
    conn: &mut SqliteConnection,
    document: &str,
    user: &str,
    client: &str,
    counter: i64,
    request_hash: &str,
) -> Result<Option<i32>, AppError> {
    let id = format!("{document}:{client}:{counter}");
    let receipt = diesel::sql_query(
        "SELECT user_id, request_hash, version FROM operation_receipts WHERE id = ?",
    )
    .bind::<Text, _>(&id)
    .get_result::<OperationReceipt>(conn)
    .optional()?;
    if let Some(receipt) = receipt {
        if receipt.user_id != user || receipt.request_hash != request_hash {
            return Err(AppError::Conflict(
                "Operation id already belongs to a different request".into(),
            ));
        }
        return Ok(Some(receipt.version));
    }
    let high_water = diesel::sql_query("SELECT user_id, max_counter FROM operation_clients WHERE document_id = ? AND client_id = ?")
        .bind::<Text, _>(document).bind::<Text, _>(client).get_result::<OperationClient>(conn).optional()?;
    if high_water.is_some_and(|entry| entry.user_id != user || counter <= entry.max_counter) {
        return Err(AppError::Conflict("Operation receipt expired or client id belongs to another author; synchronize with a new client id".into()));
    }
    Ok(None)
}

pub async fn operation_receipt(
    db: &DbPool,
    document: &str,
    user: &str,
    operation: &contrix_collab::Operation,
    request_hash: &str,
) -> Result<Option<i32>, AppError> {
    if operation.user_id.to_string() != user {
        return Err(AppError::Forbidden("Operation author mismatch".into()));
    }
    let counter = i64::try_from(operation.id.counter)
        .map_err(|_| AppError::Validation("Operation counter exceeds supported range".into()))?;
    let (document, user, client, request_hash) = (
        document.to_owned(),
        user.to_owned(),
        operation.id.client_id.to_string(),
        request_hash.to_owned(),
    );
    db.run(move |conn| {
        let stored = d::documents.find(&document).first::<Document>(conn)?;
        let file = f::files.find(&stored.file_id).first::<File>(conn)?;
        authorize(conn, &file, &user, "editor")?;
        check_operation_receipt(conn, &document, &user, &client, counter, &request_hash)
    })
    .await
}

pub async fn commit_document_operation(
    db: &DbPool,
    storage: &StorageBackend,
    document: &str,
    user: &str,
    expected_version: i32,
    content: String,
    operation: &contrix_collab::Operation,
) -> Result<i32, AppError> {
    let request_hash = operation_request_hash(operation)?;
    commit_document_operation_with_request(
        db,
        storage,
        document,
        user,
        expected_version,
        content,
        operation,
        &request_hash,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn commit_document_operation_with_request(
    db: &DbPool,
    storage: &StorageBackend,
    document: &str,
    user: &str,
    expected_version: i32,
    content: String,
    operation: &contrix_collab::Operation,
    request_hash: &str,
) -> Result<i32, AppError> {
    commit_document_inner(
        db,
        storage,
        document,
        user,
        expected_version,
        content,
        None,
        None,
        Some(operation.clone()),
        Some(request_hash.to_owned()),
        None,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn commit_document_operation_with_effects(
    db: &DbPool,
    storage: &StorageBackend,
    document: &str,
    user: &str,
    expected_version: i32,
    content: String,
    operation: &contrix_collab::Operation,
    request_hash: &str,
    effects: &[contrix_collab::OpType],
    original: &contrix_collab::Operation,
) -> Result<i32, AppError> {
    commit_document_operation_with_effects_and_credential(
        db,
        storage,
        document,
        user,
        expected_version,
        content,
        operation,
        request_hash,
        effects,
        original,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn commit_document_operation_with_effects_and_credential(
    db: &DbPool,
    storage: &StorageBackend,
    document: &str,
    user: &str,
    expected_version: i32,
    content: String,
    operation: &contrix_collab::Operation,
    request_hash: &str,
    effects: &[contrix_collab::OpType],
    original: &contrix_collab::Operation,
    credential: Option<&crate::middleware::auth::AuthenticatedCredential>,
) -> Result<i32, AppError> {
    if original.id != operation.id
        || original.user_id != operation.user_id
        || operation_request_hash(original)? != request_hash
    {
        return Err(AppError::Conflict(
            "Operation effects do not match the original request".into(),
        ));
    }
    commit_document_inner(
        db,
        storage,
        document,
        user,
        expected_version,
        content,
        None,
        None,
        Some(operation.clone()),
        Some(request_hash.to_owned()),
        Some((effects.to_vec(), original.clone())),
        credential.map(|credential| CommitCredential {
            credential: credential.clone(),
            method: salvo::http::Method::GET,
            path: format!("/api/v1/ws/collab/{document}"),
            wopi_authorization: None,
        }),
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn commit_document_inner(
    db: &DbPool,
    storage: &StorageBackend,
    document_id: &str,
    user: &str,
    expected_version: i32,
    content: String,
    label: Option<String>,
    token: Option<String>,
    operation: Option<contrix_collab::Operation>,
    request_hash: Option<String>,
    effects: Option<(Vec<contrix_collab::OpType>, contrix_collab::Operation)>,
    credential: Option<CommitCredential>,
    committed: Option<oneshot::Sender<i32>>,
) -> Result<i32, AppError> {
    if credential
        .as_ref()
        .is_some_and(|credential| credential.credential.user_id != user)
    {
        return Err(AppError::Forbidden("Credential owner mismatch".into()));
    }
    storage.attach_database(db.clone())?;
    if operation
        .as_ref()
        .is_some_and(|operation| operation.user_id.to_string() != user)
    {
        return Err(AppError::Forbidden(
            "Operation author does not match the authenticated user".into(),
        ));
    }
    let operation_id = operation.as_ref().map(|operation| {
        format!(
            "{}:{}:{}",
            document_id, operation.id.client_id, operation.id.counter
        )
    });
    let operation_client = operation
        .as_ref()
        .map(|operation| {
            let counter = i64::try_from(operation.id.counter).map_err(|_| {
                AppError::Validation("Operation counter exceeds supported range".into())
            })?;
            Ok::<_, AppError>((operation.id.client_id.to_string(), counter))
        })
        .transpose()?;
    let (id, user) = (document_id.to_owned(), user.to_owned());
    let read_credential = credential.clone();
    let (read_id, read_user, read_client, read_hash) = (
        id.clone(),
        user.clone(),
        operation_client.clone(),
        request_hash.clone(),
    );
    let preflight = db.run(move |conn| {
        conn.transaction::<_, AppError, _>(|conn| {
            if let Some(credential) = &read_credential {
                credential.validate(conn)?;
            }
            let (version, file) = d::documents
                .inner_join(f::files.on(d::file_id.eq(f::id)))
                .filter(d::id.eq(&read_id))
                .select((d::version, File::as_select()))
                .first::<(i32, File)>(conn)?;
            authorize(conn, &file, &read_user, "editor")?;
            if let (Some((client, counter)), Some(hash)) = (&read_client, &read_hash)
                && let Some(version) =
                    check_operation_receipt(conn, &read_id, &read_user, client, *counter, hash)?
            {
                return Ok((file, Some(version)));
            }
            if version != expected_version {
                return Err(AppError::Conflict(
                    "Document changed; reload before saving".into(),
                ));
            }
            Ok((file, None))
        })
    });
    let (original, replayed_version): (File, Option<i32>) =
        with_database_stage("document_preflight", preflight).await?;
    if let Some(version) = replayed_version {
        return Ok(version);
    }
    let owner = original
        .owner_id
        .parse()
        .map_err(|_| AppError::Internal("Invalid file owner".into()))?;
    let new_path = storage
        .save_file(
            &owner,
            &format!("{}.contrix.json", original.id),
            content.as_bytes(),
        )
        .await?;
    let committed_path = new_path.clone();
    let write = move |conn: &mut SqliteConnection| {
        observed_immediate_transaction::<_, AppError>(
            conn,
            TransactionStage::DocumentMetadata,
            |conn| {
                if let Some(credential) = &credential {
                    credential.validate(conn)?;
                }
                let current = d::documents.find(&id).first::<Document>(conn)?;
                let file = f::files.find(&current.file_id).first::<File>(conn)?;
                authorize(conn, &file, &user, "editor")?;
                check_file_lock(conn, &file.id, &user, token.as_deref())?;
                if let (Some((client, counter)), Some(hash)) = (&operation_client, &request_hash)
                    && let Some(version) =
                        check_operation_receipt(conn, &id, &user, client, *counter, hash)?
                {
                    enqueue_gc(conn, &committed_path)?;
                    return Ok(version);
                }

                if current.version != expected_version {
                    return Err(AppError::Conflict(
                        "Document changed; reload before saving".into(),
                    ));
                }
                // Check against the durable version before incrementing it: a
                // pre-existing current+1 log must not be legitimized by this write.
                if operation.is_some()
                    && super::operation_history::has_future_history(conn, &id, current.version)?
                {
                    return Err(AppError::Conflict(
                        "Operation history is ahead of the document; synchronize before saving"
                            .into(),
                    ));
                }
                let next = current
                    .version
                    .checked_add(1)
                    .ok_or_else(|| AppError::Conflict("Document version exhausted".into()))?;
                let now = Utc::now().to_rfc3339();
                diesel::insert_into(v::versions)
                    .values(NewVersion {
                        id: Uuid::new_v4().to_string(),
                        document_id: id.clone(),
                        version_number: current.version,
                        content: current.content,
                        created_by: user.clone(),
                        label,
                        created_at: now.clone(),
                    })
                    .execute(conn)?;
                let size = i64::try_from(content.len())
                    .map_err(|_| AppError::Validation("Document too large".into()))?;
                let changed = diesel::update(
                    d::documents
                        .filter(d::id.eq(&id))
                        .filter(d::version.eq(expected_version)),
                )
                .set((
                    d::content.eq(Some(content)),
                    d::version.eq(next),
                    d::updated_at.eq(&now),
                ))
                .execute(conn)?;
                if changed != 1 {
                    return Err(AppError::Conflict(
                        "Document changed; reload before saving".into(),
                    ));
                }
                if diesel::update(f::files.find(&file.id))
                    .set((
                        f::path.eq(committed_path),
                        f::size.eq(size),
                        f::updated_at.eq(&now),
                    ))
                    .execute(conn)?
                    != 1
                {
                    return Err(AppError::NotFound("File not found".into()));
                }
                replace_file_quota(conn, &file.owner_id, &file.path, file.size, size)?;
                enqueue_gc(conn, &file.path)?;
                if let (Some(operation), Some(op_id)) = (operation, operation_id) {
                    use contrix_core::schema::operation_log::dsl as ol;
                    let mut data = serde_json::to_value(&operation)
                        .map_err(|error| AppError::Validation(error.to_string()))?;
                    if let Some((effects, original)) = effects {
                        data["transform_format"] = serde_json::json!(1);
                        data["transformed_ops"] = serde_json::to_value(effects)
                            .map_err(|error| AppError::Validation(error.to_string()))?;
                        data["original_request"] = serde_json::to_value(original)
                            .map_err(|error| AppError::Validation(error.to_string()))?;
                    }
                    diesel::insert_into(ol::operation_log)
                        .values(NewOperationLogEntry {
                            id: op_id.clone(),
                            document_id: id.clone(),
                            user_id: user.clone(),
                            op_type: serde_json::to_string(&operation.op_type)
                                .map_err(|error| AppError::Validation(error.to_string()))?,
                            op_data: serde_json::to_string(&data)
                                .map_err(|error| AppError::Validation(error.to_string()))?,
                            version: next,
                            created_at: now,
                        })
                        .execute(conn)?;
                    let (client, counter) = operation_client
                        .ok_or_else(|| AppError::Internal("Missing operation client".into()))?;
                    let hash = request_hash.ok_or_else(|| {
                        AppError::Internal("Missing operation request hash".into())
                    })?;
                    diesel::sql_query("INSERT INTO operation_receipts(id, document_id, client_id, counter, user_id, request_hash, version) VALUES (?, ?, ?, ?, ?, ?, ?)")
                    .bind::<Text, _>(op_id).bind::<Text, _>(&id).bind::<Text, _>(&client).bind::<BigInt, _>(counter)
                    .bind::<Text, _>(&user).bind::<Text, _>(hash).bind::<Integer, _>(next).execute(conn)?;
                    diesel::sql_query("INSERT INTO operation_clients(document_id, client_id, user_id, max_counter) VALUES (?, ?, ?, ?) ON CONFLICT(document_id, client_id) DO UPDATE SET max_counter = excluded.max_counter")
                    .bind::<Text, _>(&id).bind::<Text, _>(client).bind::<Text, _>(user).bind::<BigInt, _>(counter).execute(conn)?;
                    diesel::sql_query("DELETE FROM operation_receipts WHERE document_id = ? AND id IN (SELECT id FROM operation_receipts WHERE document_id = ? ORDER BY version DESC LIMIT -1 OFFSET 2048)")
                    .bind::<Text, _>(&id).bind::<Text, _>(&id).execute(conn)?;
                    // Audit retention belongs to this same IMMEDIATE transaction.
                    // A failed or insufficient bounded pass cannot publish/ACK a
                    // new operation whose history exceeds the shared policy.
                    let history = super::operation_history::prune_operation_history_page(
                        conn,
                        &id,
                        next,
                        super::operation_history::MAX_PRUNE_PAGE,
                    )?;
                    if history.has_excess || history.has_future {
                        return Err(AppError::Conflict(
                            "Operation history cleanup is backlogged; retry after cleanup".into(),
                        ));
                    }
                }
                Ok(next)
            },
        )
    };
    let commit = commit_blob_observed(db, new_path, write, move |version| {
        if let Some(committed) = committed {
            // Notify after the transaction completes even if its async caller timed out.
            let _ = committed.send(*version);
        }
    });
    let result = with_database_stage("document_metadata_commit", commit).await;
    request_gc(db, storage);
    result
}

pub async fn acquire_file_lock(
    db: &DbPool,
    file_id: &str,
    user: &str,
    token: &str,
) -> Result<FileLock, AppError> {
    let (file_id, user, token) = (file_id.to_owned(), user.to_owned(), token.to_owned());
    db.run(move |conn| {
        conn.immediate_transaction::<_, AppError, _>(|conn| {
            let file = f::files.find(&file_id).first::<File>(conn)?;
            authorize(conn, &file, &user, "editor")?;
            if let Some(lock) = fl::file_locks
                .filter(fl::file_id.eq(&file_id))
                .first::<FileLock>(conn)
                .optional()?
            {
                let active = chrono::DateTime::parse_from_rfc3339(&lock.expires_at)
                    .is_ok_and(|expires| expires.with_timezone(&Utc) > Utc::now());
                if active {
                    if lock.locked_by == user {
                        return Ok(lock);
                    }
                    return Err(AppError::Conflict(
                        "File is already locked by another user".into(),
                    ));
                }
                diesel::delete(fl::file_locks.filter(fl::id.eq(lock.id))).execute(conn)?;
            }
            let now = Utc::now();
            let id = Uuid::new_v4().to_string();
            diesel::insert_into(fl::file_locks)
                .values(NewFileLock {
                    id: id.clone(),
                    file_id,
                    locked_by: user,
                    lock_token: token,
                    lock_type: "exclusive".into(),
                    expires_at: (now + chrono::Duration::minutes(30)).to_rfc3339(),
                    created_at: now.to_rfc3339(),
                })
                .execute(conn)?;
            Ok(fl::file_locks.find(id).first::<FileLock>(conn)?)
        })
    })
    .await
}

/// Delete a trashed subtree and all references in one transaction. Refuse active
/// descendants instead of silently erasing content restored concurrently.
pub async fn purge_trashed(
    db: &DbPool,
    file_id: &str,
    owner: Option<&str>,
    cutoff: Option<&str>,
) -> Result<usize, AppError> {
    purge_trashed_inner(db, file_id, owner, cutoff, None).await
}

#[allow(clippy::too_many_arguments)]
pub async fn purge_trashed_with_credential(
    db: &DbPool,
    file_id: &str,
    owner: Option<&str>,
    cutoff: Option<&str>,
    credential: &crate::middleware::auth::AuthenticatedCredential,
    method: &salvo::http::Method,
    path: &str,
) -> Result<usize, AppError> {
    if owner != Some(credential.user_id.as_str()) {
        return Err(AppError::Forbidden("Credential owner mismatch".into()));
    }
    purge_trashed_inner(
        db,
        file_id,
        owner,
        cutoff,
        Some(CommitCredential {
            credential: credential.clone(),
            method: method.clone(),
            path: path.to_owned(),
            wopi_authorization: None,
        }),
    )
    .await
}

async fn purge_trashed_inner(
    db: &DbPool,
    file_id: &str,
    owner: Option<&str>,
    cutoff: Option<&str>,
    credential: Option<CommitCredential>,
) -> Result<usize, AppError> {
    let (id, owner, cutoff) = (
        file_id.to_owned(),
        owner.map(str::to_owned),
        cutoff.map(str::to_owned),
    );
    db.run(move |conn| conn.immediate_transaction::<_, AppError, _>(|conn| {
        if let Some(credential) = &credential { credential.validate(conn)?; }
        let file = f::files.find(&id).first::<File>(conn).optional()?.ok_or_else(|| AppError::NotFound("Trashed file not found".into()))?;
        if let Some(user) = &owner { authorize_including_trashed(conn, &file, user, "admin")?; }
        if !file.is_trashed || cutoff.as_ref().is_some_and(|cutoff| file.trashed_at.as_ref().is_none_or(|date| date >= cutoff)) {
            return Err(AppError::NotFound("Trashed file not found".into()));
        }
        #[derive(diesel::QueryableByName)] struct Descendant { #[diesel(sql_type = Text)] id: String }
        let ids = diesel::sql_query("WITH RECURSIVE subtree(id) AS (SELECT ? UNION SELECT files.id FROM files JOIN subtree ON files.parent_id = subtree.id) SELECT id FROM subtree")
            .bind::<Text, _>(&id).load::<Descendant>(conn)?;
        for child in &ids {
            let row = f::files.find(&child.id).first::<File>(conn)?;
            if let Some(user) = &owner { authorize_including_trashed(conn, &row, user, "admin")?; }
            if !row.is_trashed || cutoff.as_ref().is_some_and(|cutoff| row.trashed_at.as_ref().is_none_or(|date| date >= cutoff)) { return Err(AppError::Conflict("Folder contains active or recently trashed files".into())); }
        }
        for child in &ids {
            let row = f::files.find(&child.id).first::<File>(conn)?;
            for path in diesel::sql_query("SELECT path FROM file_versions WHERE file_id = ?").bind::<Text, _>(&row.id).load::<GcPath>(conn)? { enqueue_gc(conn, &path.path)?; }
            if !row.is_folder { enqueue_gc(conn, &row.path)?; adjust_quota(conn, &row.owner_id, -row.size)?; }
            if let Some(path) = row.thumbnail_path { enqueue_gc(conn, &path)?; }
            for statement in [
                "DELETE FROM document_signatures WHERE document_id IN (SELECT id FROM documents WHERE file_id = ?)",
                "DELETE FROM operation_receipts WHERE document_id IN (SELECT id FROM documents WHERE file_id = ?)",
                "DELETE FROM operation_clients WHERE document_id IN (SELECT id FROM documents WHERE file_id = ?)",
                "DELETE FROM operation_log WHERE document_id IN (SELECT id FROM documents WHERE file_id = ?)",
                "DELETE FROM user_presence WHERE document_id IN (SELECT id FROM documents WHERE file_id = ?)",
                "DELETE FROM versions WHERE document_id IN (SELECT id FROM documents WHERE file_id = ?)",
                "UPDATE comments SET parent_id = NULL WHERE document_id IN (SELECT id FROM documents WHERE file_id = ?)",
                "DELETE FROM comments WHERE document_id IN (SELECT id FROM documents WHERE file_id = ?)",
                "DELETE FROM documents WHERE file_id = ?", "DELETE FROM shares WHERE file_id = ?",
                "DELETE FROM file_locks WHERE file_id = ?", "DELETE FROM file_versions WHERE file_id = ?",
                "DELETE FROM file_properties WHERE file_id = ?", "DELETE FROM file_read_status WHERE file_id = ?",
                "DELETE FROM file_tags WHERE file_id = ?", "DELETE FROM favorites WHERE file_id = ?",
                "DELETE FROM recent_files WHERE file_id = ?", "DELETE FROM conversion_jobs WHERE source_file_id = ?",
                "DELETE FROM upload_results WHERE file_id = ?",
                "DELETE FROM trash_batch_members WHERE file_id = ?",
            ] { diesel::sql_query(statement).bind::<Text, _>(&row.id).execute(conn)?; }
        }
        // Break parent references within the selected subtree before deleting.
        let all_ids: Vec<String> = ids.iter().map(|child| child.id.clone()).collect();
        let owners: Vec<String> = f::files.filter(f::id.eq_any(&all_ids)).select(f::owner_id).distinct().load(conn)?;
        diesel::update(f::files.filter(f::id.eq_any(&all_ids))).set(f::parent_id.eq(None::<String>)).execute(conn)?;
        let deleted = diesel::delete(f::files.filter(f::id.eq_any(&all_ids))).execute(conn)?;
        for owner in owners { release_unreferenced_retained_quota(conn, &owner)?; }
        Ok(deleted)
    })).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn gc_selection_database() -> DbPool {
        let db = DbPool::with_timeouts(
            ":memory:",
            1,
            Duration::from_secs(3),
            Duration::from_secs(15),
            Duration::from_secs(3),
        )
        .unwrap();
        db.run_startup_migrations(|conn| {
            crate::migrations::run_directory(
                conn,
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations"),
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn gc_selection_uses_fifo_index_without_sorting_the_backlog() {
        let db = gc_selection_database().await;
        db.run(|conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                // Reverse insertion and tied timestamps exercise both FIFO keys.
                for index in (0..1024).rev() {
                    diesel::sql_query("INSERT INTO storage_gc(path,created_at) VALUES (?,?)")
                        .bind::<Text, _>(format!("object-{index:04}"))
                        .bind::<Text, _>(format!("2000-01-01T00:{:02}:00Z", (1023 - index) / 32))
                        .execute(conn)?;
                }
                Ok(())
            })?;

            #[derive(QueryableByName)]
            struct QueryPlan {
                #[diesel(sql_type = Text)]
                detail: String,
            }
            let plan =
                diesel::sql_query(format!("EXPLAIN QUERY PLAN {SELECT_STORAGE_GC_PATHS_SQL}"))
                    .bind::<BigInt, _>(10_000_i64)
                    .bind::<BigInt, _>(10_000_i64)
                    .bind::<BigInt, _>(16_i64)
                    .load::<QueryPlan>(conn)?;
            assert!(
                plan.iter()
                    .any(|row| row.detail.contains("idx_storage_gc_order"))
            );
            assert!(
                plan.iter()
                    .all(|row| !row.detail.to_ascii_uppercase().contains("TEMP B-TREE")),
                "FIFO selection must apply LIMIT without a temporary sort: {:?}",
                plan.iter().map(|row| &row.detail).collect::<Vec<_>>()
            );

            // Compare the production query to its previous, unconstrained form.
            // Both must independently return the actual oldest sixteen paths.
            let original =
                SELECT_STORAGE_GC_PATHS_SQL.replace(" INDEXED BY idx_storage_gc_order", "");
            assert_ne!(original, SELECT_STORAGE_GC_PATHS_SQL);
            let expected = (992..1008)
                .map(|index| format!("object-{index:04}"))
                .collect::<Vec<_>>();
            for sql in [SELECT_STORAGE_GC_PATHS_SQL, original.as_str()] {
                let actual = diesel::sql_query(sql)
                    .bind::<BigInt, _>(10_000_i64)
                    .bind::<BigInt, _>(10_000_i64)
                    .bind::<BigInt, _>(16_i64)
                    .load::<GcPath>(conn)?
                    .into_iter()
                    .map(|row| row.path)
                    .collect::<Vec<_>>();
                assert_eq!(actual, expected);
            }
            Ok::<_, AppError>(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn gc_fifo_selection_preserves_claims_writers_and_excluded_failures() {
        let db = gc_selection_database().await;
        let temp = tempfile::tempdir().unwrap();
        let storage = StorageBackend::Local(super::super::StorageService::new(
            temp.path().to_str().unwrap(),
        ));
        for index in 0..48 {
            std::fs::write(temp.path().join(format!("object-{index:04}")), b"object").unwrap();
        }
        let now = Utc::now().timestamp();
        db.run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                for index in (0..48).rev() {
                    let path = format!("object-{index:04}");
                    diesel::sql_query("INSERT INTO storage_gc(path,created_at) VALUES (?,'2000-01-01T00:00:00Z')")
                        .bind::<Text, _>(&path).execute(conn)?;
                    diesel::sql_query("INSERT INTO blob_inventory(path,state,first_seen) VALUES (?,'ready',0)")
                        .bind::<Text, _>(&path).execute(conn)?;
                }
                diesel::sql_query("UPDATE storage_gc SET claim_token='active',claim_until=? WHERE path='object-0020'")
                    .bind::<BigInt, _>(now + 3600).execute(conn)?;
                diesel::sql_query("UPDATE blob_inventory SET state='deleting' WHERE path='object-0020'")
                    .execute(conn)?;
                diesel::sql_query("UPDATE blob_inventory SET state='writing',token='live',lease_until=? WHERE path='object-0021'")
                    .bind::<BigInt, _>(now + 3600).execute(conn)?;
                diesel::sql_query("UPDATE storage_gc SET claim_token='expired',claim_until=? WHERE path='object-0022'")
                    .bind::<BigInt, _>(now - 3600).execute(conn)?;
                diesel::sql_query("UPDATE blob_inventory SET state='writing',token='expired',lease_until=? WHERE path='object-0023'")
                    .bind::<BigInt, _>(now - 3600).execute(conn)?;
                Ok(())
            })
        }).await.unwrap();

        // More than one wave's failed prefix must not hide later eligible work:
        // production selection requests limit + excluded.len() before filtering.
        let mut failed = (0..20)
            .map(|index| format!("object-{index:04}"))
            .collect::<HashSet<_>>();
        let page = drain_storage_gc_page(&db, &storage, &mut failed)
            .await
            .unwrap();
        assert_eq!(page.visited, 26);
        assert!(page.error.is_none());
        assert_eq!(failed.len(), 20);
        db.run(|conn| {
            let queued = diesel::sql_query("SELECT path FROM storage_gc ORDER BY path")
                .load::<GcPath>(conn)?
                .into_iter()
                .map(|row| row.path)
                .collect::<Vec<_>>();
            assert_eq!(
                queued,
                (0..22)
                    .map(|index| format!("object-{index:04}"))
                    .collect::<Vec<_>>()
            );
            #[derive(QueryableByName)]
            struct InventoryState {
                #[diesel(sql_type = Text)]
                path: String,
                #[diesel(sql_type = Text)]
                state: String,
            }
            let inventory =
                diesel::sql_query("SELECT path,state FROM blob_inventory ORDER BY path")
                    .load::<InventoryState>(conn)?;
            assert_eq!(inventory.len(), 48);
            for (index, row) in inventory.iter().enumerate() {
                assert_eq!(row.path, format!("object-{index:04}"));
                let expected = match index {
                    0..20 => "ready",
                    20 => "deleting",
                    21 => "writing",
                    _ => "deleted",
                };
                assert_eq!(row.state, expected);
            }
            Ok::<_, AppError>(())
        })
        .await
        .unwrap();
        for index in 0..48 {
            assert_eq!(
                temp.path().join(format!("object-{index:04}")).exists(),
                index < 22
            );
        }
        drop(storage);
        temp.close().unwrap();
    }

    #[tokio::test]
    async fn commit_notification_reports_durable_success_after_the_caller_times_out() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("late-commit.db");
        let db = DbPool::with_timeouts(
            path.to_str().unwrap(),
            1,
            Duration::from_secs(3),
            Duration::from_secs(1),
            Duration::from_secs(3),
        )
        .unwrap();
        let candidate = "candidate";
        db.run_startup_migrations(move |conn| {
            crate::migrations::run_directory(
                conn,
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations"),
            )?;
            diesel::sql_query("CREATE TABLE commit_fixture(version INTEGER NOT NULL)")
                .execute(conn)?;
            super::super::maintenance::prepare_attachment(conn, candidate)?;
            Ok(())
        })
        .await
        .unwrap();

        let (entered, started) = oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let (notify, mut completion) = oneshot::channel();
        let commit = tokio::spawn(async move {
            commit_blob_observed(
                &db,
                candidate.to_owned(),
                move |conn| {
                    conn.immediate_transaction::<_, AppError, _>(|conn| {
                        diesel::sql_query("INSERT INTO commit_fixture(version) VALUES (2)")
                            .execute(conn)?;
                        entered.send(()).unwrap();
                        wait.recv_timeout(Duration::from_secs(5)).unwrap();
                        Ok(2)
                    })
                },
                move |version| {
                    notify.send(*version).unwrap();
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(commit.await.unwrap().unwrap_err().status_code(), 503);
        assert_eq!(
            completion.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        release.send(()).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), completion)
                .await
                .unwrap()
                .unwrap(),
            2
        );

        #[derive(QueryableByName)]
        struct Committed {
            #[diesel(sql_type = Integer)]
            version: i32,
        }
        let mut reader = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
        let row = diesel::sql_query("SELECT version FROM commit_fixture")
            .get_result::<Committed>(&mut reader)
            .unwrap();
        assert_eq!(row.version, 2);
    }
}
