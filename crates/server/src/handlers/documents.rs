use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{
    ApiResponse, CreateDocumentRequest, DocumentResponse, UpdateDocumentRequest,
};
use contrix_core::error::AppError;
use contrix_core::models::{DocumentType, File, NewDocument, NewFile, NewVersion};
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::room_members::dsl as rm;
use contrix_core::schema::users::dsl as u;
use contrix_core::schema::versions::dsl as v;
use contrix_export::ExportFormat;

use crate::middleware::auth::get_user_id;
use crate::services::StorageService;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn update_search_index(
    state: &AppState,
    name: &str,
    doc_id: &str,
    content: &str,
    doc_type: &str,
    owner_id: &str,
) {
    // Best-effort search index update via Tantivy — log errors but never fail the request.
    if let Some(ref search_svc) = state.search {
        if let Err(e) = search_svc.index_document(doc_id, name, content, doc_type, owner_id) {
            tracing::warn!("Failed to update search index for document {doc_id}: {e}");
        } else {
            tracing::debug!("Search index updated for document {doc_id}: {name}");
        }
    } else {
        tracing::debug!("Search index not available — skipping update for document {doc_id}");
    }
}

fn default_content(doc_type: &DocumentType, name: &str) -> serde_json::Value {
    match doc_type {
        DocumentType::Writer => {
            let doc = contrix_doc_engine::WriterDocument::new(name.to_string());
            serde_json::to_value(&doc).unwrap_or_default()
        }
        DocumentType::Calc => {
            let wb = contrix_doc_engine::Workbook::new(name.to_string());
            serde_json::to_value(&wb).unwrap_or_default()
        }
        DocumentType::Slides => {
            let pres = contrix_doc_engine::Presentation::new(name.to_string());
            serde_json::to_value(&pres).unwrap_or_default()
        }
        DocumentType::Whiteboard => {
            let wb = contrix_doc_engine::Whiteboard::new(name.to_string());
            serde_json::to_value(&wb).unwrap_or_default()
        }
    }
}

fn mime_for_doc_type(dt: &DocumentType) -> &'static str {
    match dt {
        DocumentType::Writer => "application/vnd.contrix.writer",
        DocumentType::Calc => "application/vnd.contrix.calc",
        DocumentType::Slides => "application/vnd.contrix.slides",
        DocumentType::Whiteboard => "application/vnd.contrix.whiteboard",
    }
}

fn document_storage_name(name: &str) -> String {
    format!("{name}.contrix.json")
}

fn persist_document_blob(
    base_path: &str,
    owner_id: &str,
    storage_name: &str,
    content: &str,
) -> Result<(String, i64), AppError> {
    let owner_uuid: Uuid = owner_id
        .parse()
        .map_err(|_| AppError::Validation("Invalid owner id".into()))?;
    let storage = StorageService::new(base_path);
    let path = storage.save_file(&owner_uuid, storage_name, content.as_bytes())?;
    Ok((path, content.len() as i64))
}

fn delete_previous_blob_if_safe(base_path: &str, old_path: &str) {
    let path = std::path::Path::new(old_path);
    if old_path.is_empty() || path.is_absolute() {
        return;
    }

    let storage = StorageService::new(base_path);
    let _ = storage.delete_file(old_path);
}

fn check_room_access(
    conn: &mut diesel::SqliteConnection,
    user_id: &str,
    room_id: &str,
    required_role: &str,
) -> Result<(), AppError> {
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
                "viewer" => true,
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

fn load_accessible_folder(
    conn: &mut diesel::SqliteConnection,
    folder_id: &str,
    user_id: &str,
    required_role: &str,
) -> Result<Option<File>, AppError> {
    let row = f::files
        .filter(f::id.eq(folder_id))
        .filter(f::is_folder.eq(true))
        .filter(f::is_trashed.eq(false))
        .first::<File>(conn)
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))?;

    match row {
        Some(folder) => {
            if let Some(room_id) = folder.room_id.as_deref() {
                check_room_access(conn, user_id, room_id, required_role)?;
                Ok(Some(folder))
            } else if folder.owner_id == user_id {
                Ok(Some(folder))
            } else {
                Err(AppError::Forbidden("Access denied".into()))
            }
        }
        None => Ok(None),
    }
}

fn resolve_document_id(
    conn: &mut diesel::SqliteConnection,
    identifier: &str,
) -> Result<Option<String>, AppError> {
    d::documents
        .filter(d::id.eq(identifier).or(d::file_id.eq(identifier)))
        .select(d::id)
        .first::<String>(conn)
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))
}

// ---------------------------------------------------------------------------
// POST /documents
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_document(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: CreateDocumentRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let file_id_val = Uuid::new_v4();
    let doc_id_val = Uuid::new_v4();
    let now = Utc::now();
    let parent_id_str = body.parent_id.map(|pid| pid.to_string());
    let requested_room_id = body.room_id.map(|rid| rid.to_string());
    let content = default_content(&body.doc_type, &body.name);
    let content_str = serde_json::to_string(&content).unwrap_or_default();
    let storage_name = document_storage_name(&body.name);
    let (path_val, size_val) = match persist_document_blob(
        &state.config.storage.base_path,
        &uid,
        &storage_name,
        &content_str,
    ) {
        Ok(result) => result,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
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

    let room_id_for_new_document = if let Some(parent_id) = parent_id_str.as_deref() {
        match load_accessible_folder(&mut conn, parent_id, &uid, "editor") {
            Ok(Some(parent)) => {
                if let Some(requested_room_id) = requested_room_id.as_deref() {
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
    } else if let Some(room_id) = requested_room_id.as_deref() {
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

    // Create the file record.
    let new_file = NewFile {
        id: file_id_val.to_string(),
        name: body.name.clone(),
        path: path_val.clone(),
        mime_type: Some(mime_for_doc_type(&body.doc_type).to_string()),
        size: size_val,
        owner_id: uid.clone(),
        parent_id: parent_id_str,
        is_folder: false,
        is_trashed: false,
        trashed_at: None,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        room_id: room_id_for_new_document,
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

    let _ = diesel::update(u::users.filter(u::id.eq(&uid)))
        .set(u::storage_used.eq(u::storage_used + size_val))
        .execute(&mut conn);

    // Create the document record.
    let new_doc = NewDocument {
        id: doc_id_val.to_string(),
        file_id: file_id_val.to_string(),
        doc_type: body.doc_type.as_str().to_string(),
        content: Some(content_str.clone()),
        version: 1,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };

    if let Err(e) = diesel::insert_into(d::documents)
        .values(&new_doc)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    // Best-effort search index update.
    update_search_index(
        &state,
        &body.name,
        &doc_id_val.to_string(),
        &content_str,
        body.doc_type.as_str(),
        &uid,
    );

    crate::services::WebhookService::fire_event(
        &state.db,
        "document.created",
        &serde_json::json!({
            "id": doc_id_val.to_string(),
            "file_id": file_id_val.to_string(),
            "name": body.name,
            "owner_id": uid,
            "doc_type": body.doc_type.as_str(),
            "created_at": now.to_rfc3339(),
        }),
    );

    let resp = DocumentResponse {
        id: doc_id_val,
        file_id: file_id_val,
        name: body.name,
        doc_type: body.doc_type,
        content,
        version: 1,
        owner_id: uid.parse().unwrap_or_default(),
        created_at: now,
        updated_at: now,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(resp)));
}

// ---------------------------------------------------------------------------
// GET /documents/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_document(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

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

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param) {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let row = d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
        .filter(f::owner_id.eq(&uid))
        .select((
            d::id,
            d::file_id,
            f::name,
            d::doc_type,
            d::version,
            d::content,
            f::owner_id,
            d::created_at,
            d::updated_at,
        ))
        .first::<(
            String,
            String,
            String,
            String,
            i32,
            Option<String>,
            String,
            String,
            String,
        )>(&mut conn)
        .optional();

    match row {
        Ok(Some((
            id_val,
            file_id_val,
            name_val,
            doc_type_str,
            version_val,
            content_opt,
            owner_val,
            created_val,
            updated_val,
        ))) => {
            let doc_type = DocumentType::from_str(&doc_type_str);
            let content: serde_json::Value = content_opt
                .as_deref()
                .and_then(|v| serde_json::from_str(v).ok())
                .unwrap_or_default();
            let resp = DocumentResponse {
                id: id_val.parse().unwrap_or_default(),
                file_id: file_id_val.parse().unwrap_or_default(),
                name: name_val,
                doc_type,
                content,
                version: version_val,
                owner_id: owner_val.parse().unwrap_or_default(),
                created_at: created_val.parse().unwrap_or_else(|_| Utc::now()),
                updated_at: updated_val.parse().unwrap_or_else(|_| Utc::now()),
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(resp)));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
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
// PUT /documents/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_document(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

    let body: UpdateDocumentRequest = match req.parse_json().await {
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

    // Fetch current version and backing file details.
    let current = d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&doc_id_param).or(d::file_id.eq(&doc_id_param)))
        .filter(f::owner_id.eq(&uid))
        .select((
            d::id,
            d::file_id,
            f::name,
            f::path,
            f::size,
            f::owner_id,
            d::version,
            d::content,
            d::doc_type,
        ))
        .first::<(
            String,
            String,
            String,
            String,
            i64,
            String,
            i32,
            Option<String>,
            String,
        )>(&mut conn)
        .optional();

    let (
        resolved_doc_id,
        file_id_val,
        file_name,
        old_path,
        old_size,
        owner_id,
        current_version,
        old_content_opt,
        doc_type_str,
    ) = match current {
        Ok(Some(r)) => r,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
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

    let new_version = current_version + 1;
    let now = Utc::now();
    let content_str = serde_json::to_string(&body.content).unwrap_or_default();
    let storage_name = document_storage_name(&file_name);
    let (new_path, new_size) = match persist_document_blob(
        &state.config.storage.base_path,
        &owner_id,
        &storage_name,
        &content_str,
    ) {
        Ok(result) => result,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };
    let size_diff = new_size - old_size;

    // Create a version snapshot of the old content.
    let new_ver = NewVersion {
        id: Uuid::new_v4().to_string(),
        document_id: resolved_doc_id.clone(),
        version_number: current_version,
        content: old_content_opt,
        created_by: uid.clone(),
        label: None,
        created_at: now.to_rfc3339(),
    };
    let _ = diesel::insert_into(v::versions)
        .values(&new_ver)
        .execute(&mut conn);

    // Update the document.
    match diesel::update(d::documents.filter(d::id.eq(&resolved_doc_id)))
        .set((
            d::content.eq(Some(&content_str)),
            d::version.eq(new_version),
            d::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn)
    {
        Ok(_) => {
            let _ = diesel::update(f::files.filter(f::id.eq(&file_id_val)))
                .set((
                    f::path.eq(&new_path),
                    f::size.eq(new_size),
                    f::updated_at.eq(now.to_rfc3339()),
                ))
                .execute(&mut conn);
            let _ = diesel::update(u::users.filter(u::id.eq(&owner_id)))
                .set(u::storage_used.eq(u::storage_used + size_diff))
                .execute(&mut conn);
            delete_previous_blob_if_safe(&state.config.storage.base_path, &old_path);

            // Best-effort search index update.
            update_search_index(
                &state,
                &file_name,
                &resolved_doc_id,
                &content_str,
                &doc_type_str,
                &owner_id,
            );

            crate::services::WebhookService::fire_event(
                &state.db,
                "document.updated",
                &serde_json::json!({
                    "id": resolved_doc_id,
                    "file_id": file_id_val,
                    "name": file_name,
                    "owner_id": owner_id,
                    "doc_type": doc_type_str,
                    "version": new_version,
                    "updated_at": now.to_rfc3339(),
                }),
            );

            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "version": new_version,
                "message": "Document updated"
            }))));
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
// DELETE /documents/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_document(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();
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

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param) {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    // Fetch the associated file_id.
    let file_id_row = d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
        .filter(f::owner_id.eq(&uid))
        .select(d::file_id)
        .first::<String>(&mut conn)
        .optional();

    let file_id_val = match file_id_row {
        Ok(Some(fid)) => fid,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
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

    // Trash the file.
    let _ = diesel::update(f::files.filter(f::id.eq(&file_id_val)))
        .set((
            f::is_trashed.eq(true),
            f::trashed_at.eq(Some(now.to_rfc3339())),
            f::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn);

    if let Some(ref search_svc) = state.search {
        if let Err(e) = search_svc.remove_document(&resolved_doc_id) {
            tracing::warn!("Failed to remove document {resolved_doc_id} from search index: {e}");
        }
    }

    crate::services::WebhookService::fire_event(
        &state.db,
        "document.deleted",
        &serde_json::json!({
            "id": resolved_doc_id,
            "file_id": file_id_val,
            "owner_id": uid,
            "deleted_at": now.to_rfc3339(),
        }),
    );

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(
        serde_json::json!({ "message": "Document deleted" }),
    )));
}

// ---------------------------------------------------------------------------
// GET /documents/:id/content
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_document_content(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

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

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param) {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let row = d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
        .filter(f::owner_id.eq(&uid))
        .select((d::content, d::version))
        .first::<(Option<String>, i32)>(&mut conn)
        .optional();

    match row {
        Ok(Some((content_opt, version_val))) => {
            let content: serde_json::Value = content_opt
                .as_deref()
                .and_then(|v| serde_json::from_str(v).ok())
                .unwrap_or_default();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "content": content,
                "version": version_val,
            }))));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
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
// POST /documents/:id/export/:format
// ---------------------------------------------------------------------------

#[handler]
pub async fn export_document(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();
    let format_str: String = req.param("format").unwrap_or_default();

    let export_format = match ExportFormat::from_extension(&format_str) {
        Some(fmt) => fmt,
        None => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(format!("Unsupported export format: {format_str}"))
                    .to_response(),
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

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param) {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let row = d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
        .filter(f::owner_id.eq(&uid))
        .select((d::doc_type, d::content, f::name))
        .first::<(String, Option<String>, String)>(&mut conn)
        .optional();

    let (doc_type_str, content_opt, file_name) = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
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

    let content_str = content_opt.unwrap_or_default();
    let doc_type = DocumentType::from_str(&doc_type_str);
    let content: serde_json::Value = serde_json::from_str(&content_str).unwrap_or_default();

    // Perform the export based on doc type and format.
    let exported: Result<Vec<u8>, String> = match (&doc_type, &export_format) {
        (DocumentType::Writer, ExportFormat::Html) => {
            match serde_json::from_value::<contrix_doc_engine::WriterDocument>(content) {
                Ok(doc) => {
                    let html = contrix_export::html::writer_to_html(&doc);
                    Ok(html.into_bytes())
                }
                Err(e) => Err(format!("Failed to parse writer document: {e}")),
            }
        }
        (DocumentType::Writer, ExportFormat::Markdown) => {
            match serde_json::from_value::<contrix_doc_engine::WriterDocument>(content) {
                Ok(doc) => {
                    let md = contrix_export::markdown::writer_to_markdown(&doc);
                    Ok(md.into_bytes())
                }
                Err(e) => Err(format!("Failed to parse writer document: {e}")),
            }
        }
        (DocumentType::Calc, ExportFormat::Csv) => {
            match serde_json::from_value::<contrix_doc_engine::Workbook>(content) {
                Ok(wb) => {
                    if let Some(sheet) = wb.sheets.first() {
                        let csv = contrix_export::csv::sheet_to_csv(sheet);
                        Ok(csv.into_bytes())
                    } else {
                        Ok(Vec::new())
                    }
                }
                Err(e) => Err(format!("Failed to parse workbook: {e}")),
            }
        }
        (_, ExportFormat::Json) => Ok(content_str.into_bytes()),
        _ => Err(format!(
            "Export format '{format_str}' not supported for document type '{doc_type_str}'"
        )),
    };

    match exported {
        Ok(data) => {
            let out_name = format!("{}.{}", file_name, export_format.extension());
            res.status_code(StatusCode::OK);
            res.headers_mut()
                .insert("content-type", export_format.mime_type().parse().unwrap());
            res.headers_mut().insert(
                "content-disposition",
                format!("attachment; filename=\"{out_name}\"")
                    .parse()
                    .unwrap(),
            );
            res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(data)));
        }
        Err(msg) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(msg).to_response(),
            )));
        }
    }
}

// ---------------------------------------------------------------------------
// GET /documents/:id/versions
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_versions(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

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

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param) {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    // Verify ownership.
    let count: i64 = match d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
        .filter(f::owner_id.eq(&uid))
        .count()
        .get_result(&mut conn)
    {
        Ok(c) => c,
        Err(_) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
    };

    if count == 0 {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Document not found".into()).to_response(),
        )));
        return;
    }

    match v::versions
        .filter(v::document_id.eq(&resolved_doc_id))
        .order(v::version_number.desc())
        .select((
            v::id,
            v::version_number,
            v::created_by,
            v::created_at,
            v::label,
        ))
        .load::<(String, i32, String, String, Option<String>)>(&mut conn)
    {
        Ok(rows) => {
            let version_list: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|(vid, vnum, vby, vat, vlabel)| {
                    serde_json::json!({
                        "id": vid,
                        "version_number": vnum,
                        "created_by": vby,
                        "created_at": vat,
                        "label": vlabel,
                    })
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(version_list)));
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
// POST /documents/:id/restore/:version_id
// ---------------------------------------------------------------------------

#[handler]
pub async fn restore_version(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();
    let version_id_param: String = req.param("version_id").unwrap_or_default();

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

    // Verify ownership and get current version.
    let current = d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&doc_id_param).or(d::file_id.eq(&doc_id_param)))
        .filter(f::owner_id.eq(&uid))
        .select((
            d::id,
            d::file_id,
            f::name,
            f::path,
            f::size,
            f::owner_id,
            d::version,
            d::content,
        ))
        .first::<(
            String,
            String,
            String,
            String,
            i64,
            String,
            i32,
            Option<String>,
        )>(&mut conn)
        .optional();

    let (
        resolved_doc_id,
        file_id_val,
        file_name,
        old_path,
        old_size,
        owner_id,
        current_version,
        current_content,
    ) = match current {
        Ok(Some(r)) => r,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
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

    // Fetch the version to restore.
    let version_row = v::versions
        .filter(v::id.eq(&version_id_param))
        .filter(v::document_id.eq(&resolved_doc_id))
        .select(v::content)
        .first::<Option<String>>(&mut conn)
        .optional();

    let restored_content = match version_row {
        Ok(Some(c)) => c,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Version not found".into()).to_response(),
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

    let now = Utc::now();
    let new_version = current_version + 1;
    let restored_string = restored_content.clone().unwrap_or_default();
    let storage_name = document_storage_name(&file_name);
    let (new_path, new_size) = match persist_document_blob(
        &state.config.storage.base_path,
        &owner_id,
        &storage_name,
        &restored_string,
    ) {
        Ok(result) => result,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };
    let size_diff = new_size - old_size;

    // Snapshot current content before restoring.
    let snap = NewVersion {
        id: Uuid::new_v4().to_string(),
        document_id: resolved_doc_id.clone(),
        version_number: current_version,
        content: current_content,
        created_by: uid.clone(),
        label: Some("Before restore".to_string()),
        created_at: now.to_rfc3339(),
    };
    let _ = diesel::insert_into(v::versions)
        .values(&snap)
        .execute(&mut conn);

    // Apply the restored content.
    let _ = diesel::update(d::documents.filter(d::id.eq(&resolved_doc_id)))
        .set((
            d::content.eq(&restored_content),
            d::version.eq(new_version),
            d::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn);
    let _ = diesel::update(f::files.filter(f::id.eq(&file_id_val)))
        .set((
            f::path.eq(&new_path),
            f::size.eq(new_size),
            f::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn);
    let _ = diesel::update(u::users.filter(u::id.eq(&owner_id)))
        .set(u::storage_used.eq(u::storage_used + size_diff))
        .execute(&mut conn);
    delete_previous_blob_if_safe(&state.config.storage.base_path, &old_path);

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "version": new_version,
        "message": "Version restored"
    }))));
}

// ---------------------------------------------------------------------------
// GET /documents/:id/versions/diff?v1={version_id}&v2={version_id}
// ---------------------------------------------------------------------------

#[handler]
pub async fn diff_versions(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

    let v1_id: String = match req.query::<String>("v1") {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Missing query parameter 'v1'".into()).to_response(),
            )));
            return;
        }
    };

    let v2_id: String = match req.query::<String>("v2") {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Missing query parameter 'v2'".into()).to_response(),
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

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param) {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    // Verify ownership.
    let count: i64 = match d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
        .filter(f::owner_id.eq(&uid))
        .count()
        .get_result(&mut conn)
    {
        Ok(c) => c,
        Err(_) => 0,
    };

    if count == 0 {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Document not found".into()).to_response(),
        )));
        return;
    }

    // Fetch version 1.
    let ver1 = v::versions
        .filter(v::id.eq(&v1_id))
        .filter(v::document_id.eq(&resolved_doc_id))
        .select((v::version_number, v::content))
        .first::<(i32, Option<String>)>(&mut conn)
        .optional();

    let (v1_number, v1_content_str) = match ver1 {
        Ok(Some((num, content))) => (num, content.unwrap_or_default()),
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Version v1 not found".into()).to_response(),
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

    // Fetch version 2.
    let ver2 = v::versions
        .filter(v::id.eq(&v2_id))
        .filter(v::document_id.eq(&resolved_doc_id))
        .select((v::version_number, v::content))
        .first::<(i32, Option<String>)>(&mut conn)
        .optional();

    let (v2_number, v2_content_str) = match ver2 {
        Ok(Some((num, content))) => (num, content.unwrap_or_default()),
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Version v2 not found".into()).to_response(),
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

    // Parse both content JSON strings.
    let v1_json: serde_json::Value = serde_json::from_str(&v1_content_str)
        .unwrap_or(serde_json::Value::Object(Default::default()));
    let v2_json: serde_json::Value = serde_json::from_str(&v2_content_str)
        .unwrap_or(serde_json::Value::Object(Default::default()));

    // Compute diff: compare top-level keys.
    let mut changes = Vec::new();

    if let (Some(obj1), Some(obj2)) = (v1_json.as_object(), v2_json.as_object()) {
        // Find removed and changed keys.
        for (key, val1) in obj1 {
            match obj2.get(key) {
                None => {
                    changes.push(serde_json::json!({
                        "field": key,
                        "type": "removed",
                        "old_value": val1,
                    }));
                }
                Some(val2) if val1 != val2 => {
                    changes.push(serde_json::json!({
                        "field": key,
                        "type": "changed",
                        "old_value": val1,
                        "new_value": val2,
                    }));
                }
                _ => {}
            }
        }
        // Find added keys.
        for (key, val2) in obj2 {
            if !obj1.contains_key(key) {
                changes.push(serde_json::json!({
                    "field": key,
                    "type": "added",
                    "new_value": val2,
                }));
            }
        }
    } else {
        // If either is not an object, just compare as whole values.
        if v1_json != v2_json {
            changes.push(serde_json::json!({
                "field": "_root",
                "type": "changed",
                "old_value": v1_json,
                "new_value": v2_json,
            }));
        }
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "v1": v1_number,
        "v2": v2_number,
        "changes": changes,
    }))));
}
