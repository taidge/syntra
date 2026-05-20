use std::io::{Cursor, Write as IoWrite};
use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{NewFile, NewFileReadStatus, NewFileTag};
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::file_read_status::dsl as frs;
use contrix_core::schema::file_tags::dsl as ft;
use contrix_core::schema::files::dsl as f;

use crate::middleware::auth::get_user_id;
use crate::services::StorageService;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn read_file_with_document_fallback(
    conn: &mut diesel::SqliteConnection,
    base_path: &str,
    file_id: &str,
    file_path: &str,
) -> Result<Vec<u8>, AppError> {
    let storage = StorageService::new(base_path);
    match storage.read_file(file_path) {
        Ok(data) => Ok(data),
        Err(storage_err) => {
            let document_content = d::documents
                .filter(d::file_id.eq(file_id))
                .select(d::content)
                .first::<Option<String>>(conn)
                .optional()
                .map_err(|e| AppError::Database(e.to_string()))?;

            if let Some(Some(content)) = document_content {
                Ok(content.into_bytes())
            } else {
                Err(storage_err)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Request payloads
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub struct BatchDeleteRequest {
    pub file_ids: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct BatchMoveRequest {
    pub file_ids: Vec<String>,
    pub target_folder_id: String,
}

#[derive(serde::Deserialize)]
pub struct BatchTagRequest {
    pub file_ids: Vec<String>,
    pub tag_id: String,
}

#[derive(serde::Deserialize)]
pub struct BatchDownloadRequest {
    pub file_ids: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct BatchMarkReadRequest {
    pub file_ids: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct BatchDuplicateRequest {
    pub file_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// POST /files/batch/delete
// ---------------------------------------------------------------------------

#[handler]
pub async fn batch_delete(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: BatchDeleteRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.file_ids.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("file_ids must not be empty".into()).to_response(),
        )));
        return;
    }

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

    let now = Utc::now();
    let mut deleted_count: usize = 0;

    for file_id in &body.file_ids {
        match diesel::update(
            f::files
                .filter(f::id.eq(file_id))
                .filter(f::owner_id.eq(&uid)),
        )
        .set((
            f::is_trashed.eq(true),
            f::trashed_at.eq(Some(now.to_rfc3339())),
            f::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn)
        {
            Ok(n) => deleted_count += n,
            Err(e) => {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Database(e.to_string()).to_response(),
                )));
                return;
            }
        }
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Batch delete completed",
        "deleted_count": deleted_count,
    }))));
}

// ---------------------------------------------------------------------------
// POST /files/batch/move
// ---------------------------------------------------------------------------

#[handler]
pub async fn batch_move(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: BatchMoveRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.file_ids.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("file_ids must not be empty".into()).to_response(),
        )));
        return;
    }

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

    let now = Utc::now();
    let mut moved_count: usize = 0;

    for file_id in &body.file_ids {
        match diesel::update(
            f::files
                .filter(f::id.eq(file_id))
                .filter(f::owner_id.eq(&uid)),
        )
        .set((
            f::parent_id.eq(&body.target_folder_id),
            f::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn)
        {
            Ok(n) => moved_count += n,
            Err(e) => {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Database(e.to_string()).to_response(),
                )));
                return;
            }
        }
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Batch move completed",
        "moved_count": moved_count,
    }))));
}

// ---------------------------------------------------------------------------
// POST /files/batch/tag
// ---------------------------------------------------------------------------

#[handler]
pub async fn batch_tag(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let _uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let body: BatchTagRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.file_ids.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("file_ids must not be empty".into()).to_response(),
        )));
        return;
    }

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

    let now = Utc::now();
    let mut tagged_count: usize = 0;

    for file_id in &body.file_ids {
        let new_file_tag = NewFileTag {
            id: Uuid::new_v4().to_string(),
            file_id: file_id.clone(),
            tag_id: body.tag_id.clone(),
            created_at: now.to_rfc3339(),
        };

        match diesel::insert_into(ft::file_tags)
            .values(&new_file_tag)
            .execute(&mut conn)
        {
            Ok(n) => tagged_count += n,
            Err(_) => {
                // Ignore duplicate tag assignments (UNIQUE constraint).
                continue;
            }
        }
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Batch tag completed",
        "tagged_count": tagged_count,
    }))));
}

// ---------------------------------------------------------------------------
// POST /files/batch/download
// ---------------------------------------------------------------------------

#[handler]
pub async fn batch_download(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: BatchDownloadRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.file_ids.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("file_ids must not be empty".into()).to_response(),
        )));
        return;
    }

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

    let buf = Cursor::new(Vec::new());
    let mut zip_writer = zip::ZipWriter::new(buf);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for file_id in &body.file_ids {
        let file_row = f::files
            .filter(f::id.eq(file_id))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_folder.eq(false))
            .select((f::id, f::name, f::path))
            .first::<(String, String, String)>(&mut conn)
            .optional();

        let (resolved_file_id, file_name, file_path) = match file_row {
            Ok(Some(r)) => r,
            Ok(None) => continue, // Skip files not found or not owned
            Err(_) => continue,
        };

        let data = match read_file_with_document_fallback(
            &mut conn,
            &state.config.storage.base_path,
            &resolved_file_id,
            &file_path,
        ) {
            Ok(d) => d,
            Err(_) => continue, // Skip unreadable files
        };

        if zip_writer.start_file(&file_name, options).is_ok() {
            let _ = zip_writer.write_all(&data);
        }
    }

    let cursor = match zip_writer.finish() {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Internal(format!("Failed to create ZIP: {}", e)).to_response(),
            )));
            return;
        }
    };

    let zip_data = cursor.into_inner();

    res.status_code(StatusCode::OK);
    res.headers_mut()
        .insert("content-type", "application/zip".parse().unwrap());
    res.headers_mut().insert(
        "content-disposition",
        "attachment; filename=\"download.zip\"".parse().unwrap(),
    );
    res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(
        zip_data,
    )));
}

// ---------------------------------------------------------------------------
// POST /files/batch/mark-read
// ---------------------------------------------------------------------------

#[handler]
pub async fn batch_mark_read(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: BatchMarkReadRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.file_ids.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("file_ids must not be empty".into()).to_response(),
        )));
        return;
    }

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

    let now = Utc::now();
    let mut marked_count: usize = 0;

    for file_id in &body.file_ids {
        // Try to update existing read status first (upsert).
        let updated = diesel::update(
            frs::file_read_status
                .filter(frs::user_id.eq(&uid))
                .filter(frs::file_id.eq(file_id)),
        )
        .set(frs::read_at.eq(now.to_rfc3339()))
        .execute(&mut conn);

        match updated {
            Ok(0) => {
                // Insert new read status.
                let new_status = NewFileReadStatus {
                    id: Uuid::new_v4().to_string(),
                    user_id: uid.clone(),
                    file_id: file_id.clone(),
                    read_at: now.to_rfc3339(),
                };

                match diesel::insert_into(frs::file_read_status)
                    .values(&new_status)
                    .execute(&mut conn)
                {
                    Ok(n) => marked_count += n,
                    Err(_) => {
                        // Ignore duplicate (UNIQUE constraint).
                        continue;
                    }
                }
            }
            Ok(n) => marked_count += n,
            Err(e) => {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Database(e.to_string()).to_response(),
                )));
                return;
            }
        }
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Batch mark-read completed",
        "marked_count": marked_count,
    }))));
}

// ---------------------------------------------------------------------------
// POST /files/batch/duplicate
// ---------------------------------------------------------------------------

#[handler]
pub async fn batch_duplicate(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: BatchDuplicateRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.file_ids.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("file_ids must not be empty".into()).to_response(),
        )));
        return;
    }

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

    let now = Utc::now();
    let owner_uuid: Uuid = uid.parse().unwrap_or_default();
    let storage = StorageService::new(&state.config.storage.base_path);
    let mut duplicated_ids: Vec<String> = Vec::new();

    for file_id in &body.file_ids {
        // Fetch the original file record.
        let file_row = f::files
            .filter(f::id.eq(file_id))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_folder.eq(false))
            .select((f::id, f::name, f::path, f::mime_type, f::size, f::parent_id))
            .first::<(String, String, String, Option<String>, i64, Option<String>)>(&mut conn)
            .optional();

        let (resolved_file_id, original_name, original_path, mime_type, _size, parent_id) =
            match file_row {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => continue,
            };

        // Read the original file data.
        let data = match read_file_with_document_fallback(
            &mut conn,
            &state.config.storage.base_path,
            &resolved_file_id,
            &original_path,
        ) {
            Ok(d) => d,
            Err(_) => continue,
        };

        // Create the copy.
        let copy_name = format!("Copy of {}", original_name);
        let new_file_id = Uuid::new_v4();

        let storage_path = match storage.save_file(&owner_uuid, &copy_name, &data) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let new_file = NewFile {
            id: new_file_id.to_string(),
            name: copy_name,
            path: storage_path,
            mime_type,
            size: data.len() as i64,
            owner_id: uid.clone(),
            parent_id,
            is_folder: false,
            is_trashed: false,
            trashed_at: None,
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
            room_id: None,
            thumbnail_path: None,
            sort_order: 0,
        };

        match diesel::insert_into(f::files)
            .values(&new_file)
            .execute(&mut conn)
        {
            Ok(_) => {
                duplicated_ids.push(new_file_id.to_string());
            }
            Err(_) => continue,
        }
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Batch duplicate completed",
        "duplicated_count": duplicated_ids.len(),
        "new_file_ids": duplicated_ids,
    }))));
}
