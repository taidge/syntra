use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, FileResponse, MoveFileRequest};
use contrix_core::error::AppError;
use contrix_core::models::{File, NewFile};
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::users::dsl as u;

use crate::middleware::auth::get_user_id;
use crate::services::StorageService;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if user has at least `required_role` access to a room.
/// Returns Ok(()) if allowed, Err with 403 if not.
///
/// Role hierarchy: admin > editor > viewer.
///
#[allow(dead_code)]
fn check_room_access(
    conn: &mut diesel::SqliteConnection,
    user_id: &str,
    room_id: &str,
    required_role: &str,
) -> Result<(), AppError> {
    use contrix_core::schema::room_members::dsl as rm;

    let member = rm::room_members
        .filter(rm::room_id.eq(room_id))
        .filter(rm::user_id.eq(user_id))
        .select(rm::role)
        .first::<String>(conn)
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))?;

    match member {
        Some(role) => {
            let allowed = match required_role {
                "viewer" => true, // any role can view
                "editor" => role == "owner" || role == "editor" || role == "admin",
                "admin" => role == "owner" || role == "admin",
                _ => false,
            };
            if allowed {
                Ok(())
            } else {
                Err(AppError::Forbidden("Insufficient room permissions".into()))
            }
        }
        None => Err(AppError::Forbidden("Not a member of this room".into())),
    }
}

fn authorize_file_access(
    conn: &mut diesel::SqliteConnection,
    row: &File,
    user_id: &str,
    required_role: &str,
) -> Result<(), AppError> {
    if let Some(room_id) = row.room_id.as_deref() {
        check_room_access(conn, user_id, room_id, required_role)
    } else if row.owner_id == user_id {
        Ok(())
    } else {
        Err(AppError::NotFound("File not found".into()))
    }
}

fn load_accessible_file(
    conn: &mut diesel::SqliteConnection,
    file_id: &str,
    user_id: &str,
    required_role: &str,
) -> Result<Option<File>, AppError> {
    let row = f::files
        .filter(f::id.eq(file_id))
        .first::<File>(conn)
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))?;

    match row {
        Some(row) => {
            authorize_file_access(conn, &row, user_id, required_role)?;
            Ok(Some(row))
        }
        None => Ok(None),
    }
}

fn load_accessible_folder(
    conn: &mut diesel::SqliteConnection,
    folder_id: &str,
    user_id: &str,
    required_role: &str,
) -> Result<Option<File>, AppError> {
    match load_accessible_file(conn, folder_id, user_id, required_role)? {
        Some(row) if row.is_folder => Ok(Some(row)),
        Some(_) => Err(AppError::Validation("Target is not a folder".into())),
        None => Ok(None),
    }
}

fn file_model_to_response(row: File) -> FileResponse {
    FileResponse {
        id: row.id.parse().unwrap_or_default(),
        name: row.name,
        path: row.path,
        mime_type: row.mime_type,
        size: row.size,
        owner_id: row.owner_id.parse().unwrap_or_default(),
        parent_id: row.parent_id.as_deref().and_then(|s| s.parse().ok()),
        is_folder: row.is_folder,
        is_trashed: row.is_trashed,
        trashed_at: row
            .trashed_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc)),
        created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
        updated_at: row.updated_at.parse().unwrap_or_else(|_| Utc::now()),
        shared_with_count: 0,
        document_id: None,
    }
}

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
// GET /files
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_files(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let parent_id_param: Option<String> = req.query("parent_id");
    let page: u32 = req.query("page").unwrap_or(1);
    let per_page: u32 = req.query("per_page").unwrap_or(50);
    let offset_val = ((page.saturating_sub(1)) * per_page) as i64;
    let limit_val = per_page as i64;

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

    let result: Result<Vec<File>, AppError> = if let Some(pid) = parent_id_param {
        let parent = match load_accessible_folder(&mut conn, &pid, &uid, "viewer") {
            Ok(Some(row)) => row,
            Ok(None) => {
                res.status_code(StatusCode::NOT_FOUND);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::NotFound("Folder not found".into()).to_response(),
                )));
                return;
            }
            Err(e) => {
                res.status_code(StatusCode::FORBIDDEN);
                res.render(Json(ApiResponse::<()>::err(e.to_response())));
                return;
            }
        };

        let mut query = f::files
            .into_boxed()
            .filter(f::parent_id.eq(&pid))
            .filter(f::is_trashed.eq(false))
            .order((f::sort_order.asc(), f::is_folder.desc(), f::name.asc()))
            .limit(limit_val)
            .offset(offset_val);

        if parent.room_id.is_none() {
            query = query.filter(f::owner_id.eq(&uid));
        }

        query
            .load::<File>(&mut conn)
            .map_err(|e| AppError::Database(e.to_string()))
    } else {
        f::files
            .filter(f::owner_id.eq(&uid))
            .filter(f::parent_id.is_null())
            .filter(f::is_trashed.eq(false))
            .order((f::sort_order.asc(), f::is_folder.desc(), f::name.asc()))
            .limit(limit_val)
            .offset(offset_val)
            .load::<File>(&mut conn)
            .map_err(|e| AppError::Database(e.to_string()))
    };

    match result {
        Ok(rows) => {
            let file_list: Vec<FileResponse> =
                rows.into_iter().map(file_model_to_response).collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(file_list)));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
        }
    }
}

// ---------------------------------------------------------------------------
// POST /files/upload
// ---------------------------------------------------------------------------

#[handler]
pub async fn upload_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let parent_id_param: Option<String> = req.form("parent_id").await;
    let room_id_param: Option<String> = req.form("room_id").await;

    let file_part = match req.file("file").await {
        Some(fp) => fp,
        None => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("No file provided".into()).to_response(),
            )));
            return;
        }
    };

    let file_name = file_part
        .name()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "upload".to_string());
    let content_type = file_part.content_type().map(|m| m.to_string());

    let data = match tokio::fs::read(file_part.path()).await {
        Ok(d) => d,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Storage(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let owner_uuid: Uuid = uid.parse().unwrap_or_default();
    let storage = StorageService::new(&state.config.storage.base_path);
    let storage_path = match storage.save_file(&owner_uuid, &file_name, &data) {
        Ok(p) => p,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let file_id_val = Uuid::new_v4();
    let now = Utc::now();
    let size_val = data.len() as i64;

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

    let room_id_for_new_file = if let Some(parent_id) = parent_id_param.as_deref() {
        match load_accessible_folder(&mut conn, parent_id, &uid, "editor") {
            Ok(Some(parent)) => {
                if let Some(requested_room_id) = room_id_param.as_deref() {
                    if parent.room_id.as_deref() != Some(requested_room_id) {
                        res.status_code(StatusCode::BAD_REQUEST);
                        res.render(Json(ApiResponse::<()>::err(
                            AppError::Validation(
                                "room_id must match the parent folder room".into(),
                            )
                            .to_response(),
                        )));
                        return;
                    }
                }
                parent.room_id.clone()
            }
            Ok(None) => {
                res.status_code(StatusCode::NOT_FOUND);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::NotFound("Parent folder not found".into()).to_response(),
                )));
                return;
            }
            Err(e) => {
                res.status_code(StatusCode::FORBIDDEN);
                res.render(Json(ApiResponse::<()>::err(e.to_response())));
                return;
            }
        }
    } else if let Some(room_id) = room_id_param.as_deref() {
        match check_room_access(&mut conn, &uid, room_id, "editor") {
            Ok(()) => Some(room_id.to_string()),
            Err(e) => {
                res.status_code(StatusCode::FORBIDDEN);
                res.render(Json(ApiResponse::<()>::err(e.to_response())));
                return;
            }
        }
    } else {
        None
    };

    let new_file = NewFile {
        id: file_id_val.to_string(),
        name: file_name.clone(),
        path: storage_path.clone(),
        mime_type: content_type.clone(),
        size: size_val,
        owner_id: uid.clone(),
        parent_id: parent_id_param.clone(),
        is_folder: false,
        is_trashed: false,
        trashed_at: None,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        room_id: room_id_for_new_file.clone(),
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
    let _ = diesel::update(u::users.filter(u::id.eq(&uid)))
        .set(u::storage_used.eq(u::storage_used + size_val))
        .execute(&mut conn);

    // Generate thumbnail for image uploads.
    let mut thumbnail_path_val: Option<String> = None;
    if let Some(ref ct) = content_type {
        if ct.starts_with("image/") {
            if let Some(ref thumb_svc) = state.thumbnail {
                match thumb_svc.generate_thumbnail(
                    &storage_path,
                    &file_id_val.to_string(),
                ) {
                    Ok(thumb_rel) => {
                        let _ = diesel::update(f::files.filter(f::id.eq(file_id_val.to_string())))
                            .set(f::thumbnail_path.eq(Some(&thumb_rel)))
                            .execute(&mut conn);
                        thumbnail_path_val = Some(thumb_rel);
                        tracing::debug!("Thumbnail generated for file {file_id_val}");
                    }
                    Err(e) => {
                        tracing::warn!("Failed to generate thumbnail for {file_id_val}: {e}");
                    }
                }
            }
        }
    }
    let _ = thumbnail_path_val;

    crate::services::WebhookService::fire_event(
        &state.db,
        "file.uploaded",
        &serde_json::json!({
            "id": file_id_val.to_string(),
            "name": &file_name,
            "owner_id": &uid,
            "mime_type": &content_type,
            "size": size_val,
            "parent_id": &parent_id_param,
            "room_id": &room_id_for_new_file,
            "created_at": now.to_rfc3339(),
        }),
    );

    let mut imported_doc_id: Option<Uuid> = None;
    let ext = std::path::Path::new(&file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());

    if let Some(ext) = ext.as_deref() {
        let parse_result: Result<(String, contrix_core::models::DocumentType), String> = match ext {
            "docx" => contrix_export::docx::docx_to_writer(&data)
                .and_then(|d| serde_json::to_string(&d).map_err(|e| e.to_string()))
                .map(|json| (json, contrix_core::models::DocumentType::Writer)),
            "xlsx" => contrix_export::xlsx::xlsx_to_workbook(&data)
                .and_then(|w| serde_json::to_string(&w).map_err(|e| e.to_string()))
                .map(|json| (json, contrix_core::models::DocumentType::Calc)),
            "pptx" => contrix_export::pptx::pptx_to_presentation(&data)
                .and_then(|p| serde_json::to_string(&p).map_err(|e| e.to_string()))
                .map(|json| (json, contrix_core::models::DocumentType::Slides)),
            _ => Err("not an office format".into()),
        };

        if let Ok((content_str, doc_type)) = parse_result {
            let new_doc_id = Uuid::new_v4();
            let now_str = now.to_rfc3339();
            let new_doc = contrix_core::models::NewDocument {
                id: new_doc_id.to_string(),
                file_id: file_id_val.to_string(),
                doc_type: doc_type.as_str().to_string(),
                content: Some(content_str),
                version: 1,
                created_at: now_str.clone(),
                updated_at: now_str,
            };
            if let Err(e) = diesel::insert_into(contrix_core::schema::documents::dsl::documents)
                .values(&new_doc)
                .execute(&mut conn)
            {
                tracing::warn!("auto-import: failed to insert document for {file_id_val}: {e}");
            } else {
                imported_doc_id = Some(new_doc_id);
            }
        }
    }

    let file_resp = FileResponse {
        id: file_id_val,
        name: file_name,
        path: storage_path,
        mime_type: content_type,
        size: size_val,
        owner_id: owner_uuid,
        parent_id: parent_id_param
            .as_deref()
            .and_then(|s| s.parse::<uuid::Uuid>().ok()),
        is_folder: false,
        is_trashed: false,
        trashed_at: None,
        created_at: now,
        updated_at: now,
        shared_with_count: 0,
        document_id: imported_doc_id,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(file_resp)));
}

// ---------------------------------------------------------------------------
// GET /files/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();

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

    match load_accessible_file(&mut conn, &file_id_param, &uid, "viewer") {
        Ok(Some(row)) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(file_model_to_response(row))));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
        }
        Err(e) => {
            let status = match e {
                AppError::Forbidden(_) => StatusCode::FORBIDDEN,
                AppError::NotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            res.status_code(status);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
        }
    }
}

// ---------------------------------------------------------------------------
// PUT /files/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct RenameRequest {
        name: String,
    }

    let body: RenameRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let now = Utc::now();

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

    let target = match load_accessible_file(&mut conn, &file_id_param, &uid, "editor") {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    match diesel::update(f::files.filter(f::id.eq(&target.id)))
        .set((f::name.eq(&body.name), f::updated_at.eq(now.to_rfc3339())))
        .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            crate::services::WebhookService::fire_event(
                &state.db,
                "file.updated",
                &serde_json::json!({
                    "id": &target.id,
                    "name": &body.name,
                    "owner_id": &uid,
                    "updated_at": now.to_rfc3339(),
                }),
            );
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "File renamed" }),
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

// ---------------------------------------------------------------------------
// DELETE /files/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();
    let now = Utc::now();

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

    let target = match load_accessible_file(&mut conn, &file_id_param, &uid, "editor") {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    match diesel::update(f::files.filter(f::id.eq(&target.id)))
        .set((
            f::is_trashed.eq(true),
            f::trashed_at.eq(Some(now.to_rfc3339())),
            f::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            crate::services::WebhookService::fire_event(
                &state.db,
                "file.deleted",
                &serde_json::json!({
                    "id": &target.id,
                    "name": &target.name,
                    "owner_id": &uid,
                    "deleted_at": now.to_rfc3339(),
                }),
            );
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "File trashed" }),
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

// ---------------------------------------------------------------------------
// POST /files/:id/move
// ---------------------------------------------------------------------------

#[handler]
pub async fn move_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();

    let body: MoveFileRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let now = Utc::now();

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

    let source = match load_accessible_file(&mut conn, &file_id_param, &uid, "editor") {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let target_folder = match load_accessible_folder(
        &mut conn,
        &body.target_folder_id.to_string(),
        &uid,
        "editor",
    ) {
        Ok(Some(folder)) => folder,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Target folder not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    match diesel::update(f::files.filter(f::id.eq(&source.id)))
        .set((
            f::parent_id.eq(body.target_folder_id.to_string()),
            f::room_id.eq(target_folder.room_id.clone()),
            f::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "File moved" }),
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

// ---------------------------------------------------------------------------
// GET /files/:id/download
// ---------------------------------------------------------------------------

#[handler]
pub async fn download_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();

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

    let file_row = match load_accessible_file(&mut conn, &file_id_param, &uid, "viewer") {
        Ok(Some(row)) if !row.is_folder => row,
        Ok(Some(_)) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Cannot download a folder".into()).to_response(),
            )));
            return;
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            let status = match e {
                AppError::Forbidden(_) => StatusCode::FORBIDDEN,
                AppError::NotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            res.status_code(status);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let resolved_file_id = file_row.id.clone();
    let file_name = file_row.name.clone();
    let file_path = file_row.path.clone();
    let mime_type_val = file_row.mime_type.clone();

    let data = match read_file_with_document_fallback(
        &mut conn,
        &state.config.storage.base_path,
        &resolved_file_id,
        &file_path,
    ) {
        Ok(d) => d,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let ct = mime_type_val.unwrap_or_else(|| "application/octet-stream".to_string());
    res.status_code(StatusCode::OK);
    res.headers_mut().insert(
        "content-type",
        ct.parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );
    res.headers_mut().insert(
        "content-disposition",
        format!("attachment; filename=\"{}\"", file_name)
            .parse()
            .unwrap(),
    );
    res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(data)));
}

// ---------------------------------------------------------------------------
// PUT /files/:id/order
// ---------------------------------------------------------------------------

#[handler]
pub async fn set_file_order(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct SetOrderRequest {
        sort_order: i32,
    }

    let body: SetOrderRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let now = Utc::now();

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

    let target = match load_accessible_file(&mut conn, &file_id_param, &uid, "editor") {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    match diesel::update(f::files.filter(f::id.eq(&target.id)))
        .set((
            f::sort_order.eq(body.sort_order),
            f::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Sort order updated" }),
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
