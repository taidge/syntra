use chrono::{Duration, Utc};
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{
    ApiResponse, CreateUploadSessionRequest, FileResponse, MAX_UPLOAD_CHUNK_BYTES,
    MAX_UPLOAD_CHUNKS, UploadSessionResponse,
};
use contrix_core::error::AppError;
use contrix_core::models::{File, NewFile, NewUploadSession, UploadSession};
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::upload_sessions::dsl as us;

use crate::middleware::auth::get_user_id;
use crate::services::StorageBackend;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn app_error_status(err: &AppError) -> StatusCode {
    StatusCode::from_u16(err.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

fn render_app_error(res: &mut Response, err: AppError) {
    res.status_code(app_error_status(&err));
    res.render(Json(ApiResponse::<()>::err(err.to_response())));
}

fn max_upload_bytes(max_upload_size_mb: u64) -> i64 {
    let bytes = (max_upload_size_mb as u128).saturating_mul(1024 * 1024);
    bytes.min(i64::MAX as u128) as i64
}

fn session_is_expired(session: &UploadSession) -> bool {
    chrono::DateTime::parse_from_rfc3339(&session.expires_at)
        .map(|expires_at| expires_at.with_timezone(&Utc) <= Utc::now())
        .unwrap_or(true)
}

async fn check_room_access(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    user_id: &str,
    room_id: &str,
    required_role: &str,
) -> Result<(), AppError> {
    crate::handlers::access::check_room_access(conn, user_id, room_id, required_role).await
}

async fn verify_upload_destination(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    user_id: &str,
    parent_id: Option<&str>,
    room_id: Option<&str>,
) -> Result<Option<String>, AppError> {
    if let Some(parent_id) = parent_id {
        let parent = f::files
            .filter(f::id.eq(parent_id))
            .filter(f::is_folder.eq(true))
            .filter(f::is_trashed.eq(false))
            .first::<File>(conn)
            .await
            .optional()
            .map_err(|e| AppError::Database(e.to_string()))?
            .ok_or_else(|| AppError::NotFound("Parent folder not found".into()))?;

        if let Some(parent_room_id) = parent.room_id.as_deref() {
            check_room_access(conn, user_id, parent_room_id, "editor").await?;
        } else if parent.owner_id != user_id {
            return Err(AppError::NotFound("Parent folder not found".into()));
        }

        if let Some(requested_room_id) = room_id
            && parent.room_id.as_deref() != Some(requested_room_id)
        {
            return Err(AppError::Validation(
                "room_id must match the parent folder room".into(),
            ));
        }

        Ok(parent.room_id)
    } else if let Some(room_id) = room_id {
        check_room_access(conn, user_id, room_id, "editor").await?;
        Ok(Some(room_id.to_string()))
    } else {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// POST /upload/sessions
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_upload_session(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let body: CreateUploadSessionRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let max_size = max_upload_bytes(state.config.storage.max_upload_size_mb)
        .min(MAX_UPLOAD_CHUNK_BYTES as i64 * MAX_UPLOAD_CHUNKS as i64);
    if body.total_size <= 0 {
        render_app_error(
            res,
            AppError::Validation("total_size must be greater than zero".into()),
        );
        return;
    }
    if body.total_size > max_size {
        render_app_error(
            res,
            AppError::QuotaExceeded(format!(
                "Upload exceeds configured limit of {} MB",
                state.config.storage.max_upload_size_mb
            )),
        );
        return;
    }

    let file_name = StorageBackend::sanitize_file_name(&body.file_name);
    let parent_id = body.parent_id.map(|pid| pid.to_string());
    let requested_room_id = body.room_id.map(|rid| rid.to_string());

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_app_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let room_id = match verify_upload_destination(
        &mut conn,
        &uid,
        parent_id.as_deref(),
        requested_room_id.as_deref(),
    )
    .await
    {
        Ok(room_id) => room_id,
        Err(e) => {
            render_app_error(res, e);
            return;
        }
    };

    let session_id = Uuid::new_v4();
    let now = Utc::now();
    let expires_at = now + Duration::hours(24);

    let new_session = NewUploadSession {
        id: session_id.to_string(),
        user_id: uid.clone(),
        file_name: file_name.clone(),
        mime_type: body.mime_type.clone(),
        total_size: body.total_size,
        uploaded_size: 0,
        chunk_count: 0,
        parent_id,
        room_id,
        status: "pending".to_string(),
        created_at: now.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };

    drop(conn);
    let reserved = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                crate::services::persistence::reserve_upload_session(
                    conn,
                    &new_session.user_id,
                    new_session.total_size,
                )?;
                if let Some(parent_id) = &new_session.parent_id {
                    let parent =
                        diesel::RunQueryDsl::first::<File>(f::files.find(parent_id), conn)?;
                    if !parent.is_folder || parent.room_id != new_session.room_id {
                        return Err(AppError::Conflict("Upload destination changed".into()));
                    }
                    crate::services::persistence::authorize(
                        conn,
                        &parent,
                        &new_session.user_id,
                        "editor",
                    )?;
                } else if let Some(room_id) = &new_session.room_id {
                    crate::services::persistence::authorize_room(
                        conn,
                        room_id,
                        &new_session.user_id,
                        "editor",
                    )?;
                }
                diesel::RunQueryDsl::execute(
                    diesel::insert_into(us::upload_sessions).values(&new_session),
                    conn,
                )?;
                Ok(())
            })
        })
        .await;
    if let Err(error) = reserved {
        render_app_error(res, error);
        return;
    }

    let resp = UploadSessionResponse {
        id: session_id,
        file_name,
        total_size: body.total_size,
        uploaded_size: 0,
        chunk_count: 0,
        max_chunk_size: (MAX_UPLOAD_CHUNK_BYTES as i64).min(max_size),
        max_chunk_count: MAX_UPLOAD_CHUNKS,
        status: "pending".to_string(),
        created_at: now,
        expires_at,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(resp)));
}

// ---------------------------------------------------------------------------
// POST /upload/sessions/:id/chunks
// ---------------------------------------------------------------------------

#[handler]
pub async fn upload_chunk(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let Some(uid) = get_user_id(depot) else {
        render_app_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_app_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let session_id: String = req.param("id").unwrap_or_default();
    let index = req.query::<i32>("chunk_index").filter(|index| *index > 0);
    let Some(index) = index else {
        render_app_error(
            res,
            AppError::Validation("A positive chunk_index is required for idempotent upload".into()),
        );
        return;
    };
    let maximum = max_upload_bytes(state.config.storage.max_upload_size_mb);
    let mut provider = state.db.clone();
    let session = match us::upload_sessions
        .filter(us::id.eq(&session_id))
        .filter(us::user_id.eq(&uid))
        .first::<UploadSession>(&mut provider)
        .await
        .optional()
    {
        Ok(Some(session)) if session.status == "pending" && !session_is_expired(&session) => {
            session
        }
        Ok(_) => {
            render_app_error(
                res,
                AppError::NotFound("Pending upload session not found".into()),
            );
            return;
        }
        Err(error) => {
            render_app_error(res, error.into());
            return;
        }
    };
    if session.total_size > maximum {
        render_app_error(
            res,
            AppError::QuotaExceeded("Upload exceeds configured limit".into()),
        );
        return;
    }
    if index > MAX_UPLOAD_CHUNKS || index as i64 > session.total_size {
        render_app_error(
            res,
            AppError::Validation("Chunk index exceeds the declared byte count".into()),
        );
        return;
    }
    static CAPACITY: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));
    let capacity = match CAPACITY.clone().try_acquire_owned() {
        Ok(permit) => std::sync::Arc::new(permit),
        Err(_) => {
            render_app_error(
                res,
                AppError::RateLimit("Chunk upload capacity is busy; retry later".into()),
            );
            return;
        }
    };
    let maximum_chunk = MAX_UPLOAD_CHUNK_BYTES
        .min(maximum as usize)
        .min(session.total_size as usize);
    let multipart_limit = maximum_chunk.saturating_add(64 * 1024);
    let retained = super::request_body::RetainedBody::install_with_timeouts(
        req,
        std::time::Duration::from_secs(state.config.storage.upload_idle_timeout_seconds),
        std::time::Duration::from_secs(state.config.storage.upload_total_timeout_seconds),
    );
    let declared_too_large = req
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|value| value > multipart_limit as u64);
    if declared_too_large {
        let drained = retained
            .drain(
                multipart_limit.saturating_add(1024 * 1024),
                std::time::Duration::from_secs(2),
            )
            .await;
        super::request_body::close_if_incomplete(req, res, drained);
        render_app_error(
            res,
            AppError::QuotaExceeded("Chunk request exceeds its advertised size budget".into()),
        );
        return;
    }
    req.set_secure_max_size(multipart_limit);
    if let Err(error) = req.form_data().await {
        let drained = retained
            .drain(
                multipart_limit.saturating_add(1024 * 1024),
                std::time::Duration::from_secs(2),
            )
            .await;
        super::request_body::close_if_incomplete(req, res, drained);
        let error = if retained.timed_out() {
            AppError::Timeout("Reading the chunk exceeded its configured deadline".into())
        } else if super::request_body::exceeds_limit(&error) {
            AppError::QuotaExceeded("Chunk request exceeds its advertised size budget".into())
        } else {
            AppError::Validation(format!("Invalid chunk upload: {error}"))
        };
        render_app_error(res, error);
        return;
    }
    if !retained
        .drain(multipart_limit, std::time::Duration::from_secs(2))
        .await
    {
        super::request_body::close_if_incomplete(req, res, false);
        render_app_error(
            res,
            AppError::Validation("Multipart transport did not finish within its budget".into()),
        );
        return;
    }
    drop(retained);
    let Some(part) = req.file("chunk").await else {
        render_app_error(res, AppError::Validation("No chunk data provided".into()));
        return;
    };
    let temporary_path = part.path().to_owned();
    let read_capacity = capacity.clone();
    let bytes = match tokio::task::spawn_blocking(move || {
        let _capacity = read_capacity;
        use std::io::Read;
        let mut file = std::fs::File::open(temporary_path)?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(maximum_chunk as u64 + 1)
            .read_to_end(&mut bytes)?;
        Ok::<_, std::io::Error>(bytes)
    })
    .await
    {
        Ok(Ok(bytes)) if !bytes.is_empty() && bytes.len() <= maximum_chunk => {
            std::sync::Arc::new(bytes)
        }
        Ok(Ok(bytes)) if bytes.is_empty() => {
            render_app_error(res, AppError::Validation("Chunk data is empty".into()));
            return;
        }
        Ok(Ok(_)) => {
            render_app_error(
                res,
                AppError::QuotaExceeded("Chunk data exceeds the size limit".into()),
            );
            return;
        }
        Ok(Err(error)) => {
            render_app_error(res, AppError::Storage(error.to_string()));
            return;
        }
        Err(_) => {
            render_app_error(res, AppError::Internal("Chunk read worker failed".into()));
            return;
        }
    };
    let size = bytes.len() as i64;
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(bytes.as_slice()));
    // Resolve a sequential retry before writing another immutable object. The
    // transaction below repeats the check for concurrent requests that miss it.
    let (retry_session, retry_user, retry_digest, retry_credential) = (
        session_id.clone(),
        uid.clone(),
        digest.clone(),
        credential.clone(),
    );
    let existing = state.db.run(move |conn| {
        use diesel::OptionalExtension;
        use diesel::sql_types::{Integer, Text};
        conn.transaction::<_, AppError, _>(|conn| {
            crate::middleware::auth::validate_credential_in_transaction(conn,
                &retry_credential, &salvo::http::Method::POST,
                &format!("/api/v1/upload/sessions/{retry_session}/chunks"))?;
            let current = diesel::RunQueryDsl::first::<UploadSession>(
                us::upload_sessions.filter(us::id.eq(&retry_session))
                    .filter(us::user_id.eq(&retry_user)), conn)?;
            if current.status != "pending" || session_is_expired(&current) {
                return Err(AppError::Conflict("Upload session is not pending".into()));
            }
            let existing = diesel::RunQueryDsl::get_result::<ChunkEntry>(
                diesel::sql_query("SELECT chunk_index,path,size,digest FROM upload_chunks WHERE session_id=? AND chunk_index=?")
                    .bind::<Text,_>(&retry_session).bind::<Integer,_>(index), conn).optional()?;
            existing.map(|entry| {
                if entry.size != size || entry.digest != retry_digest {
                    return Err(AppError::Conflict("Chunk index already has different data".into()));
                }
                Ok((current.uploaded_size, current.total_size))
            }).transpose()
        })
    }).await;
    match existing {
        Ok(Some((uploaded, total))) => {
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "chunk_number":index,"uploaded_size":uploaded,"total_size":total
            }))));
            return;
        }
        Ok(None) => {}
        Err(error) => {
            render_app_error(res, error);
            return;
        }
    }
    let owner = match uid.parse::<Uuid>() {
        Ok(owner) => owner,
        Err(_) => {
            render_app_error(res, AppError::Auth("Invalid user id".into()));
            return;
        }
    };
    let staged = match crate::services::persistence::stage_blob(
        &state.db,
        state.storage.clone(),
        owner,
        format!("chunk-{session_id}-{index}"),
        bytes,
        capacity.clone(),
    )
    .await
    {
        Ok(staged) => staged,
        Err(error) => {
            render_app_error(res, error);
            return;
        }
    };
    if let Err(error) = crate::middleware::auth::validate_credential_for_request(
        &state,
        &credential,
        &salvo::http::Method::POST,
        &format!("/api/v1/upload/sessions/{session_id}/chunks"),
    )
    .await
    {
        render_app_error(res, error);
        return;
    }
    let staging = staged.path.clone();
    let candidate = staging.clone();
    let result = staged.commit(move |conn| {
        let _capacity = capacity;
        use diesel::OptionalExtension;
        use diesel::sql_types::{BigInt, Integer, Text};
        conn.immediate_transaction::<_, AppError, _>(|conn| {
            crate::middleware::auth::validate_credential_in_transaction(conn,
                &credential, &salvo::http::Method::POST,
                &format!("/api/v1/upload/sessions/{session_id}/chunks"))?;
            let current = diesel::RunQueryDsl::first::<UploadSession>(us::upload_sessions.filter(us::id.eq(&session_id)).filter(us::user_id.eq(&uid)), conn).optional()?.ok_or_else(|| AppError::NotFound("Upload session not found".into()))?;
            if current.status != "pending" || session_is_expired(&current) { return Err(AppError::Conflict("Upload session is not pending".into())); }
            let existing = diesel::RunQueryDsl::get_result::<ChunkEntry>(diesel::sql_query("SELECT chunk_index, path, size, digest FROM upload_chunks WHERE session_id = ? AND chunk_index = ?")
                .bind::<Text, _>(&session_id).bind::<Integer, _>(index), conn).optional()?;
            if let Some(existing) = existing {
                if existing.digest != digest || existing.size != size { return Err(AppError::Conflict("Chunk index already has different data".into())); }
                crate::services::persistence::enqueue_gc(conn, &candidate)?;
                return Ok((current.chunk_count, current.uploaded_size, current.total_size, false));
            }
            if current.chunk_count >= MAX_UPLOAD_CHUNKS { return Err(AppError::QuotaExceeded("Upload has reached its maximum chunk count".into())); }
            let uploaded = current.uploaded_size.checked_add(size).ok_or_else(|| AppError::Validation("Upload size overflow".into()))?;
            if uploaded > current.total_size || uploaded > maximum { return Err(AppError::QuotaExceeded("Uploaded data exceeds declared or configured limit".into())); }
            diesel::RunQueryDsl::execute(diesel::sql_query("INSERT INTO upload_chunks(session_id, chunk_index, path, size, digest) VALUES (?, ?, ?, ?, ?)")
                .bind::<Text, _>(&session_id).bind::<Integer, _>(index).bind::<Text, _>(candidate)
                .bind::<BigInt, _>(size).bind::<Text, _>(digest), conn)?;
            let count = current.chunk_count.checked_add(1).ok_or_else(|| AppError::Validation("Too many chunks".into()))?;
            diesel::RunQueryDsl::execute(diesel::update(us::upload_sessions.find(&session_id)).set((us::chunk_count.eq(count), us::uploaded_size.eq(uploaded))), conn)?;
            Ok((count, uploaded, current.total_size, true))
        })
    }).await;
    crate::services::persistence::request_gc(&state.db, &state.storage);
    match result {
        Ok((_, uploaded, total, _)) => res.render(Json(ApiResponse::ok(
            serde_json::json!({"chunk_number":index,"uploaded_size":uploaded,"total_size":total}),
        ))),
        Err(error) => render_app_error(res, error),
    }
}

#[derive(diesel::QueryableByName)]
struct ChunkEntry {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    chunk_index: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    path: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    size: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    digest: String,
}

#[derive(diesel::QueryableByName)]
struct UploadResult {
    #[diesel(sql_type = diesel::sql_types::Text)]
    file_id: String,
}

fn response_file(file: File) -> FileResponse {
    FileResponse {
        id: file.id.parse().unwrap_or_default(),
        name: file.name,
        mime_type: file.mime_type,
        size: file.size,
        owner_id: file.owner_id.parse().unwrap_or_default(),
        parent_id: file.parent_id.and_then(|id| id.parse().ok()),
        is_folder: file.is_folder,
        is_trashed: file.is_trashed,
        trashed_at: None,
        created_at: file.created_at.parse().unwrap_or_else(|_| Utc::now()),
        updated_at: file.updated_at.parse().unwrap_or_else(|_| Utc::now()),
        shared_with_count: 0,
        document_id: None,
    }
}

async fn completed_file(
    db: &crate::state::DbPool,
    id: &str,
    user: &str,
) -> Result<Option<File>, AppError> {
    let (id, user) = (id.to_owned(), user.to_owned());
    db.run(move |conn| {
        conn.transaction::<_, AppError, _>(|conn| {
        use diesel::OptionalExtension;
        use diesel::sql_types::Text;
        let result = diesel::RunQueryDsl::get_result::<UploadResult>(diesel::sql_query("SELECT upload_results.file_id FROM upload_results JOIN upload_sessions ON upload_sessions.id = upload_results.session_id WHERE upload_results.session_id = ? AND upload_sessions.user_id = ? AND upload_sessions.status = 'completed'")
            .bind::<Text, _>(id).bind::<Text, _>(&user), conn).optional()?;
        result.map(|result| {
            let file = diesel::RunQueryDsl::first::<File>(f::files.find(result.file_id), conn)?;
            crate::services::persistence::authorize(conn, &file, &user, "viewer")?;
            Ok(file)
        }).transpose()
        })
    }).await
}

// ---------------------------------------------------------------------------
// POST /upload/sessions/:id/complete
// ---------------------------------------------------------------------------

#[handler]
pub async fn complete_upload(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let Some(uid) = get_user_id(depot) else {
        render_app_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_app_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let id: String = req.param("id").unwrap_or_default();
    static CAPACITY: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2)));
    let capacity = match CAPACITY.clone().try_acquire_owned() {
        Ok(permit) => std::sync::Arc::new(permit),
        Err(_) => {
            render_app_error(
                res,
                AppError::RateLimit("Upload completion capacity is busy; retry later".into()),
            );
            return;
        }
    };
    let completed = tokio::spawn(async move {
        let mut response = Response::new();
        complete_upload_work(state, uid, id, credential, capacity, &mut response).await;
        response
    })
    .await;
    match completed {
        Ok(mut response) => {
            if let Some(status) = response.status_code {
                res.status_code(status);
            }
            res.headers_mut().extend(response.headers().clone());
            res.body(response.take_body());
        }
        Err(_) => render_app_error(
            res,
            AppError::Internal("Upload completion worker failed".into()),
        ),
    }
}

async fn complete_upload_work(
    state: std::sync::Arc<crate::state::AppState>,
    uid: String,
    id: String,
    credential: crate::middleware::auth::AuthenticatedCredential,
    capacity: std::sync::Arc<tokio::sync::OwnedSemaphorePermit>,
    res: &mut Response,
) {
    if let Err(error) = crate::middleware::auth::validate_credential_for_request(
        &state,
        &credential,
        &salvo::http::Method::POST,
        &format!("/api/v1/upload/sessions/{id}/complete"),
    )
    .await
    {
        render_app_error(res, error);
        return;
    }
    let mut provider = state.db.clone();
    let session = match us::upload_sessions
        .filter(us::id.eq(&id))
        .filter(us::user_id.eq(&uid))
        .first::<UploadSession>(&mut provider)
        .await
        .optional()
    {
        Ok(Some(session)) => session,
        Ok(None) => {
            render_app_error(res, AppError::NotFound("Upload session not found".into()));
            return;
        }
        Err(error) => {
            render_app_error(res, error.into());
            return;
        }
    };
    if session.status == "completed" {
        match completed_file(&state.db, &id, &uid).await {
            Ok(Some(file)) => res.render(Json(ApiResponse::ok(response_file(file)))),
            Ok(None) => render_app_error(
                res,
                AppError::NotFound("Completed upload file not found".into()),
            ),
            Err(error) => render_app_error(res, error),
        }
        return;
    }
    if session.status != "pending"
        || session_is_expired(&session)
        || session.uploaded_size != session.total_size
        || session.chunk_count <= 0
        || session.chunk_count > MAX_UPLOAD_CHUNKS
    {
        render_app_error(
            res,
            AppError::Conflict("Upload is expired, incomplete or not pending".into()),
        );
        return;
    }
    let room = match verify_upload_destination(
        &mut provider,
        &uid,
        session.parent_id.as_deref(),
        session.room_id.as_deref(),
    )
    .await
    {
        Ok(room) => room,
        Err(error) => {
            render_app_error(res, error);
            return;
        }
    };
    use diesel::sql_types::Text;
    let chunks = match diesel::sql_query("SELECT chunk_index, path, size, digest FROM upload_chunks WHERE session_id = ? ORDER BY chunk_index LIMIT ?")
        .bind::<Text, _>(&id).bind::<diesel::sql_types::Integer, _>(MAX_UPLOAD_CHUNKS + 1).load::<ChunkEntry>(&mut provider).await {
        Ok(chunks) => chunks,
        Err(error) => { render_app_error(res, error.into()); return; }
    };
    if chunks.len() != session.chunk_count as usize
        || chunks
            .iter()
            .enumerate()
            .any(|(index, chunk)| chunk.chunk_index as usize != index + 1)
    {
        if let Ok(Some(file)) = completed_file(&state.db, &id, &uid).await {
            res.render(Json(ApiResponse::ok(response_file(file))));
            return;
        }
        render_app_error(
            res,
            AppError::Validation("Chunk indices must be contiguous starting at 1".into()),
        );
        return;
    }
    let maximum = max_upload_bytes(state.config.storage.max_upload_size_mb);
    if session.total_size > maximum {
        render_app_error(
            res,
            AppError::QuotaExceeded("Upload exceeds configured limit".into()),
        );
        return;
    }
    let mut merged = Vec::new();
    use sha2::{Digest, Sha256};
    for chunk in &chunks {
        let bytes = match state.storage.read_file(&chunk.path).await {
            Ok(bytes) => bytes,
            Err(error) => {
                if let Ok(Some(file)) = completed_file(&state.db, &id, &uid).await {
                    res.render(Json(ApiResponse::ok(response_file(file))));
                    return;
                }
                render_app_error(res, error);
                return;
            }
        };
        if bytes.len() as i64 != chunk.size
            || format!("{:x}", Sha256::digest(&bytes)) != chunk.digest
            || merged.len() as i64 + chunk.size > session.total_size
        {
            render_app_error(
                res,
                AppError::Validation("Chunk data failed integrity validation".into()),
            );
            return;
        }
        merged.extend_from_slice(&bytes);
    }
    if merged.len() as i64 != session.total_size {
        render_app_error(
            res,
            AppError::Validation("Merged size does not match total_size".into()),
        );
        return;
    }
    let owner = match uid.parse() {
        Ok(owner) => owner,
        Err(_) => {
            render_app_error(res, AppError::Validation("Invalid user id".into()));
            return;
        }
    };
    let staged = match crate::services::persistence::stage_blob(
        &state.db,
        state.storage.clone(),
        owner,
        session.file_name.clone(),
        std::sync::Arc::new(merged),
        capacity.clone(),
    )
    .await
    {
        Ok(staged) => staged,
        Err(error) => {
            render_app_error(res, error);
            return;
        }
    };
    let path = staged.path.clone();
    if let Err(error) = crate::middleware::auth::validate_credential_for_request(
        &state,
        &credential,
        &salvo::http::Method::POST,
        &format!("/api/v1/upload/sessions/{id}/complete"),
    )
    .await
    {
        render_app_error(res, error);
        return;
    }
    let candidate = path.clone();
    let result = staged
        .commit(move |conn| {
            let _capacity = capacity;
            use diesel::OptionalExtension;
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                crate::middleware::auth::validate_credential_in_transaction(
                    conn,
                    &credential,
                    &salvo::http::Method::POST,
                    &format!("/api/v1/upload/sessions/{id}/complete"),
                )?;
                let current = diesel::RunQueryDsl::first::<UploadSession>(
                    us::upload_sessions
                        .filter(us::id.eq(&id))
                        .filter(us::user_id.eq(&uid)),
                    conn,
                )
                .optional()?
                .ok_or_else(|| AppError::NotFound("Upload session not found".into()))?;
                if current.status == "completed" {
                    let result = diesel::RunQueryDsl::get_result::<UploadResult>(
                        diesel::sql_query(
                            "SELECT file_id FROM upload_results WHERE session_id = ?",
                        )
                        .bind::<Text, _>(&id),
                        conn,
                    )?;
                    let file =
                        diesel::RunQueryDsl::first::<File>(f::files.find(result.file_id), conn)?;
                    crate::services::persistence::authorize(conn, &file, &uid, "viewer")?;
                    crate::services::persistence::enqueue_gc(conn, &candidate)?;
                    return Ok(file);
                }
                if current.status != "pending"
                    || session_is_expired(&current)
                    || current.uploaded_size != session.total_size
                    || current.chunk_count != session.chunk_count
                {
                    return Err(AppError::Conflict("Upload changed while completing".into()));
                }
                if let Some(parent_id) = &session.parent_id {
                    let parent =
                        diesel::RunQueryDsl::first::<File>(f::files.find(parent_id), conn)?;
                    if !parent.is_folder || parent.room_id != room {
                        return Err(AppError::Conflict("Upload destination changed".into()));
                    }
                    crate::services::persistence::authorize(conn, &parent, &uid, "editor")?;
                } else if let Some(room_id) = &room {
                    crate::services::persistence::authorize_room(conn, room_id, &uid, "editor")?;
                }
                let file_id = Uuid::new_v4().to_string();
                let now = Utc::now().to_rfc3339();
                let file = NewFile {
                    id: file_id.clone(),
                    name: session.file_name,
                    path: candidate,
                    mime_type: session.mime_type,
                    size: session.total_size,
                    owner_id: uid.clone(),
                    parent_id: session.parent_id,
                    is_folder: false,
                    is_trashed: false,
                    trashed_at: None,
                    created_at: now.clone(),
                    updated_at: now,
                    room_id: room,
                    thumbnail_path: None,
                    sort_order: 0,
                };
                // Release only this session's reservation inside the same atomic
                // commit that charges the final file. Errors restore pending.
                diesel::RunQueryDsl::execute(
                    diesel::update(us::upload_sessions.find(&id)).set(us::status.eq("processing")),
                    conn,
                )?;
                crate::services::persistence::adjust_quota(conn, &uid, session.total_size)?;
                diesel::RunQueryDsl::execute(diesel::insert_into(f::files).values(&file), conn)?;
                diesel::RunQueryDsl::execute(
                    diesel::sql_query(
                        "INSERT INTO upload_results(session_id, file_id) VALUES (?, ?)",
                    )
                    .bind::<Text, _>(&id)
                    .bind::<Text, _>(&file_id),
                    conn,
                )?;
                diesel::RunQueryDsl::execute(
                    diesel::update(us::upload_sessions.find(&id)).set(us::status.eq("completed")),
                    conn,
                )?;
                for chunk in chunks {
                    crate::services::persistence::enqueue_gc(conn, &chunk.path)?;
                }
                diesel::RunQueryDsl::execute(
                    diesel::sql_query("DELETE FROM upload_chunks WHERE session_id = ?")
                        .bind::<Text, _>(&id),
                    conn,
                )?;
                Ok(diesel::RunQueryDsl::first::<File>(
                    f::files.find(file_id),
                    conn,
                )?)
            })
        })
        .await;
    crate::services::persistence::request_gc(&state.db, &state.storage);
    match result {
        Ok(file) => {
            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(response_file(file))));
        }
        Err(error) => render_app_error(res, error),
    }
}

// ---------------------------------------------------------------------------
// DELETE /upload/sessions/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn cancel_upload(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let Some(uid) = get_user_id(depot) else {
        render_app_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let id: String = req.param("id").unwrap_or_default();
    let result = state.db.run(move |conn| {
        use diesel::OptionalExtension;
        use diesel::sql_types::Text;
        conn.immediate_transaction::<_, AppError, _>(|conn| {
            let session = diesel::RunQueryDsl::first::<UploadSession>(us::upload_sessions.filter(us::id.eq(&id)).filter(us::user_id.eq(&uid)), conn).optional()?.ok_or_else(|| AppError::NotFound("Upload session not found".into()))?;
            if session.status != "pending" { return Err(AppError::Conflict("Completed upload cannot be cancelled".into())); }
            // Stop accepting data immediately and retire a bounded page. The
            // maintenance worker completes a legacy highly-fragmented session.
            diesel::RunQueryDsl::execute(diesel::update(us::upload_sessions.find(&id)).set(us::expires_at.eq(Utc::now().to_rfc3339())), conn)?;
            let chunks = diesel::RunQueryDsl::load::<ChunkEntry>(diesel::sql_query("SELECT chunk_index, path, size, digest FROM upload_chunks WHERE session_id = ? ORDER BY chunk_index LIMIT 100").bind::<Text, _>(&id), conn)?;
            for chunk in chunks {
                crate::services::persistence::enqueue_gc(conn, &chunk.path)?;
                diesel::RunQueryDsl::execute(diesel::sql_query("DELETE FROM upload_chunks WHERE session_id = ? AND chunk_index = ?").bind::<Text, _>(&id).bind::<diesel::sql_types::Integer, _>(chunk.chunk_index), conn)?;
            }
            let remaining = diesel::RunQueryDsl::get_result::<ChunkEntry>(diesel::sql_query("SELECT chunk_index, path, size, digest FROM upload_chunks WHERE session_id = ? LIMIT 1").bind::<Text, _>(&id), conn).optional()?;
            if remaining.is_none() { diesel::RunQueryDsl::execute(diesel::delete(us::upload_sessions.find(&id)), conn)?; }
            Ok(())
        })
    }).await;
    match result {
        Ok(()) => {
            crate::services::persistence::request_gc(&state.db, &state.storage);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({"message":"Upload session cancelled"}),
            )));
        }
        Err(error) => render_app_error(res, error),
    }
}
