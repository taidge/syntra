use std::sync::Arc;

use chrono::{Duration, Utc};
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{
    ApiResponse, CreateUploadSessionRequest, FileResponse, UploadSessionResponse,
};
use contrix_core::error::AppError;
use contrix_core::models::{NewFile, NewUploadSession, UploadSession};
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::upload_sessions::dsl as us;
use contrix_core::schema::users::dsl as u_dsl;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn session_to_response(row: UploadSession) -> UploadSessionResponse {
    UploadSessionResponse {
        id: row.id.parse().unwrap_or_default(),
        file_name: row.file_name,
        total_size: row.total_size,
        uploaded_size: row.uploaded_size,
        chunk_count: row.chunk_count,
        status: row.status,
        created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
        expires_at: row.expires_at.parse().unwrap_or_else(|_| Utc::now()),
    }
}

/// Return the directory where chunks for a given session are stored.
fn chunks_dir(base_path: &str, session_id: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(base_path)
        .join("chunks")
        .join(session_id)
}

// ---------------------------------------------------------------------------
// POST /upload/sessions
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_upload_session(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
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

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let session_id = Uuid::new_v4();
    let now = Utc::now();
    let expires_at = now + Duration::hours(24);

    let new_session = NewUploadSession {
        id: session_id.to_string(),
        user_id: uid.clone(),
        file_name: body.file_name.clone(),
        mime_type: body.mime_type.clone(),
        total_size: body.total_size,
        uploaded_size: 0,
        chunk_count: 0,
        parent_id: body.parent_id.map(|pid| pid.to_string()),
        room_id: body.room_id.map(|rid| rid.to_string()),
        status: "pending".to_string(),
        created_at: now.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };

    if let Err(e) = diesel::insert_into(us::upload_sessions)
        .values(&new_session)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    // Create the chunks directory for this session.
    let chunk_dir = chunks_dir(&state.config.storage.base_path, &session_id.to_string());
    if let Err(e) = tokio::fs::create_dir_all(&chunk_dir).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to create chunks directory: {}", e)).to_response(),
        )));
        return;
    }

    let resp = UploadSessionResponse {
        id: session_id,
        file_name: body.file_name,
        total_size: body.total_size,
        uploaded_size: 0,
        chunk_count: 0,
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
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
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

    let session_id_param: String = req.param("id").unwrap_or_default();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    // Verify the session exists and belongs to the user.
    let session = match us::upload_sessions
        .filter(us::id.eq(&session_id_param))
        .filter(us::user_id.eq(&uid))
        .first::<UploadSession>(&mut conn)
        .optional()
    {
        Ok(Some(s)) => s,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Upload session not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if session.status != "pending" {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Upload session is not in pending state".into()).to_response(),
        )));
        return;
    }

    // Read the chunk from the multipart upload.
    let file_part = match req.file("chunk").await {
        Some(fp) => fp,
        None => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("No chunk data provided".into()).to_response(),
            )));
            return;
        }
    };

    let chunk_data = match tokio::fs::read(file_part.path()).await {
        Ok(d) => d,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Storage(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let chunk_size = chunk_data.len() as i64;
    let new_chunk_count = session.chunk_count + 1;
    let new_uploaded_size = session.uploaded_size + chunk_size;

    // Save the chunk to disk.
    let chunk_path = chunks_dir(&state.config.storage.base_path, &session_id_param)
        .join(format!("{}", new_chunk_count));
    if let Err(e) = tokio::fs::write(&chunk_path, &chunk_data).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to write chunk: {}", e)).to_response(),
        )));
        return;
    }

    // Update the session record.
    if let Err(e) = diesel::update(us::upload_sessions.filter(us::id.eq(&session_id_param)))
        .set((
            us::chunk_count.eq(new_chunk_count),
            us::uploaded_size.eq(new_uploaded_size),
        ))
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "chunk_number": new_chunk_count,
        "uploaded_size": new_uploaded_size,
        "total_size": session.total_size,
    }))));
}

// ---------------------------------------------------------------------------
// POST /upload/sessions/:id/complete
// ---------------------------------------------------------------------------

#[handler]
pub async fn complete_upload(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
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

    let session_id_param: String = req.param("id").unwrap_or_default();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    // Verify the session exists and belongs to the user.
    let session = match us::upload_sessions
        .filter(us::id.eq(&session_id_param))
        .filter(us::user_id.eq(&uid))
        .first::<UploadSession>(&mut conn)
        .optional()
    {
        Ok(Some(s)) => s,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Upload session not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if session.status != "pending" {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Upload session is not in pending state".into()).to_response(),
        )));
        return;
    }

    if session.chunk_count == 0 {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("No chunks uploaded".into()).to_response(),
        )));
        return;
    }

    // Merge all chunks into the final file.
    let owner_uuid: Uuid = uid.parse().unwrap_or_default();
    let storage_base = &state.config.storage.base_path;
    let user_dir = std::path::PathBuf::from(storage_base).join(owner_uuid.to_string());

    if let Err(e) = tokio::fs::create_dir_all(&user_dir).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to create user directory: {}", e)).to_response(),
        )));
        return;
    }

    let final_path = user_dir.join(&session.file_name);
    let mut merged_data: Vec<u8> = Vec::new();

    for i in 1..=session.chunk_count {
        let chunk_path =
            chunks_dir(&state.config.storage.base_path, &session_id_param).join(format!("{}", i));
        match tokio::fs::read(&chunk_path).await {
            Ok(data) => merged_data.extend_from_slice(&data),
            Err(e) => {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Storage(format!("Failed to read chunk {}: {}", i, e)).to_response(),
                )));
                return;
            }
        }
    }

    if let Err(e) = tokio::fs::write(&final_path, &merged_data).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to write final file: {}", e)).to_response(),
        )));
        return;
    }

    // Clean up chunks directory.
    let _ = tokio::fs::remove_dir_all(chunks_dir(
        &state.config.storage.base_path,
        &session_id_param,
    ))
    .await;

    // Create the File record.
    let file_id_val = Uuid::new_v4();
    let now = Utc::now();
    let size_val = merged_data.len() as i64;
    let storage_path = format!("{}/{}", owner_uuid, session.file_name);

    let new_file = NewFile {
        id: file_id_val.to_string(),
        name: session.file_name.clone(),
        path: storage_path.clone(),
        mime_type: session.mime_type.clone(),
        size: size_val,
        owner_id: uid.clone(),
        parent_id: session.parent_id.clone(),
        is_folder: false,
        is_trashed: false,
        trashed_at: None,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        room_id: session.room_id.clone(),
        thumbnail_path: None,
        sort_order: 0,
    };

    if let Err(e) = diesel::insert_into(f::files)
        .values(&new_file)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    // Update user storage_used.
    let _ = diesel::update(u_dsl::users.filter(u_dsl::id.eq(&uid)))
        .set(u_dsl::storage_used.eq(u_dsl::storage_used + size_val))
        .execute(&mut conn);

    // Mark the session as completed.
    let _ = diesel::update(us::upload_sessions.filter(us::id.eq(&session_id_param)))
        .set(us::status.eq("completed"))
        .execute(&mut conn);

    let file_resp = FileResponse {
        id: file_id_val,
        name: session.file_name,
        path: storage_path,
        mime_type: session.mime_type,
        size: size_val,
        owner_id: owner_uuid,
        parent_id: session
            .parent_id
            .as_deref()
            .and_then(|s| s.parse::<uuid::Uuid>().ok()),
        is_folder: false,
        is_trashed: false,
        trashed_at: None,
        created_at: now,
        updated_at: now,
        shared_with_count: 0,
        document_id: None,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(file_resp)));
}

// ---------------------------------------------------------------------------
// DELETE /upload/sessions/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn cancel_upload(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
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

    let session_id_param: String = req.param("id").unwrap_or_default();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    // Clean up chunks directory.
    let _ = tokio::fs::remove_dir_all(chunks_dir(
        &state.config.storage.base_path,
        &session_id_param,
    ))
    .await;

    // Delete the session record (only if owned by the user).
    match diesel::delete(
        us::upload_sessions
            .filter(us::id.eq(&session_id_param))
            .filter(us::user_id.eq(&uid)),
    )
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Upload session not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Upload session cancelled" }),
            )));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
