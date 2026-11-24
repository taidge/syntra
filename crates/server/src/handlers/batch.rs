use std::io::{Cursor, Write as IoWrite};

use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use syntra_core::dto::ApiResponse;
use syntra_core::error::AppError;
use syntra_core::models::{File, NewDocument, NewFile, NewFileReadStatus, NewFileTag};
use syntra_core::schema::documents::dsl as d;
use syntra_core::schema::file_read_status::dsl as frs;
use syntra_core::schema::file_tags::dsl as ft;
use syntra_core::schema::files::dsl as f;
use syntra_core::schema::tags::dsl as t;

use crate::handlers::access::{load_accessible_file, load_accessible_target_folder};
use crate::middleware::auth::get_user_id;
use crate::services::{StorageBackend, StorageService};

fn render_error(res: &mut Response, error: AppError) {
    res.status_code(
        StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
    );
    res.render(Json(ApiResponse::<()>::err(error.to_response())));
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn read_file_with_document_fallback(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    storage: &StorageBackend,
    file_id: &str,
    file_path: &str,
) -> Result<Vec<u8>, AppError> {
    match storage.read_file(file_path).await {
        Ok(data) => Ok(data),
        Err(storage_err) => {
            let document_content = d::documents
                .filter(d::file_id.eq(file_id))
                .select(d::content)
                .first::<Option<String>>(conn)
                .await
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
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let method = req.method().clone();
    let request_path = req.uri().path().to_owned();
    if let Err(error) = crate::middleware::auth::validate_credential_for_request(
        &state,
        &credential,
        &method,
        &request_path,
    )
    .await
    {
        render_error(res, error);
        return;
    }

    if body.file_ids.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("file_ids must not be empty".into()).to_response(),
        )));
        return;
    }

    let mut deleted_count: usize = 0;

    for file_id in &body.file_ids {
        match crate::handlers::folders::trash_file_tree(
            &state.db,
            file_id,
            &uid,
            &credential,
            &method,
            &request_path,
        )
        .await
        {
            Ok(n) => deleted_count += n,
            Err(AppError::NotFound(_) | AppError::Forbidden(_)) => {
                if let Err(error) = crate::middleware::auth::validate_credential_for_request(
                    &state,
                    &credential,
                    &method,
                    &request_path,
                )
                .await
                {
                    render_error(res, error);
                    return;
                }
                continue;
            }
            Err(error) => {
                render_error(res, error);
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
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let method = req.method().clone();
    let request_path = req.uri().path().to_owned();
    if let Err(error) = crate::middleware::auth::validate_credential_for_request(
        &state,
        &credential,
        &method,
        &request_path,
    )
    .await
    {
        render_error(res, error);
        return;
    }

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

    let target_folder = match load_accessible_target_folder(
        &mut conn,
        &body.target_folder_id,
        &uid,
        "editor",
    )
    .await
    {
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

    let mut moved_count: usize = 0;

    for file_id in &body.file_ids {
        match crate::handlers::folders::move_file_tree(
            &state.db,
            file_id,
            &target_folder.id,
            &uid,
            &credential,
            &method,
            &request_path,
        )
        .await
        {
            Ok(n) => moved_count += n,
            Err(AppError::NotFound(_) | AppError::Forbidden(_)) => {
                if let Err(error) = crate::middleware::auth::validate_credential_for_request(
                    &state,
                    &credential,
                    &method,
                    &request_path,
                )
                .await
                {
                    render_error(res, error);
                    return;
                }
                continue;
            }
            Err(error) => {
                render_error(res, error);
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
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let method = req.method().clone();
    let request_path = req.uri().path().to_owned();
    if let Err(error) = crate::middleware::auth::validate_credential_for_request(
        &state,
        &credential,
        &method,
        &request_path,
    )
    .await
    {
        render_error(res, error);
        return;
    }

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

    // The tag itself must belong to the requesting user.
    match t::tags
        .filter(t::id.eq(&body.tag_id))
        .filter(t::owner_id.eq(&uid))
        .select(t::id)
        .first::<String>(&mut conn)
        .await
        .optional()
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Tag not found".into()).to_response(),
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
    }

    let now = Utc::now();
    let mut tagged_count: usize = 0;

    for file_id in &body.file_ids {
        let (file_id, tag_id, actor, write_credential, write_method, write_path) = (
            file_id.clone(),
            body.tag_id.clone(),
            uid.clone(),
            credential.clone(),
            method.clone(),
            request_path.clone(),
        );
        let now = now.to_rfc3339();
        let tagged = state
            .db
            .run(move |conn| {
                conn.immediate_transaction::<_, AppError, _>(|conn| {
                    crate::middleware::auth::validate_credential_in_transaction(
                        conn,
                        &write_credential,
                        &write_method,
                        &write_path,
                    )?;
                    let file =
                        diesel::OptionalExtension::optional(diesel::RunQueryDsl::first::<File>(
                            f::files.find(&file_id),
                            conn,
                        ))?
                        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
                    crate::services::persistence::authorize_including_trashed(
                        conn, &file, &actor, "editor",
                    )?;
                    let tag_exists =
                        diesel::OptionalExtension::optional(diesel::RunQueryDsl::first::<String>(
                            t::tags
                                .filter(t::id.eq(&tag_id))
                                .filter(t::owner_id.eq(&actor))
                                .select(t::id),
                            conn,
                        ))?;
                    if tag_exists.is_none() {
                        return Err(AppError::NotFound("Tag not found".into()));
                    }
                    Ok(diesel::RunQueryDsl::execute(
                        diesel::insert_into(ft::file_tags)
                            .values(NewFileTag {
                                id: Uuid::new_v4().to_string(),
                                file_id,
                                tag_id,
                                created_at: now,
                            })
                            .on_conflict((ft::file_id, ft::tag_id))
                            .do_nothing(),
                        conn,
                    )?)
                })
            })
            .await;
        match tagged {
            Ok(n) => tagged_count += n,
            Err(AppError::NotFound(_) | AppError::Forbidden(_)) => {
                if let Err(error) = crate::middleware::auth::validate_credential_for_request(
                    &state,
                    &credential,
                    &method,
                    &request_path,
                )
                .await
                {
                    render_error(res, error);
                    return;
                }
                continue;
            }
            Err(error) => {
                render_error(res, error);
                return;
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
        let (resolved_file_id, file_name, file_path) =
            match load_accessible_file(&mut conn, file_id, &uid, "viewer").await {
                Ok(Some(row)) if !row.is_folder && !row.is_trashed => (row.id, row.name, row.path),
                Ok(Some(_)) => continue,
                Ok(None) => continue, // Skip files not found or not owned
                Err(_) => continue,
            };

        let data = match read_file_with_document_fallback(
            &mut conn,
            &state.storage,
            &resolved_file_id,
            &file_path,
        )
        .await
        {
            Ok(d) => d,
            Err(_) => continue, // Skip unreadable files
        };

        let file_name = StorageService::sanitize_file_name(&file_name);
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
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let method = req.method().clone();
    let request_path = req.uri().path().to_owned();
    if let Err(error) = crate::middleware::auth::validate_credential_for_request(
        &state,
        &credential,
        &method,
        &request_path,
    )
    .await
    {
        render_error(res, error);
        return;
    }

    if body.file_ids.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("file_ids must not be empty".into()).to_response(),
        )));
        return;
    }

    let now = Utc::now();
    let mut marked_count: usize = 0;

    for file_id in &body.file_ids {
        let (file_id, actor, write_credential, write_method, write_path) = (
            file_id.clone(),
            uid.clone(),
            credential.clone(),
            method.clone(),
            request_path.clone(),
        );
        let now = now.to_rfc3339();
        let marked =
            state
                .db
                .run(move |conn| {
                    conn.immediate_transaction::<_, AppError, _>(|conn| {
                        crate::middleware::auth::validate_credential_in_transaction(
                            conn,
                            &write_credential,
                            &write_method,
                            &write_path,
                        )?;
                        let file = diesel::OptionalExtension::optional(
                            diesel::RunQueryDsl::first::<File>(f::files.find(&file_id), conn),
                        )?
                        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
                        crate::services::persistence::authorize(conn, &file, &actor, "viewer")?;
                        Ok(diesel::RunQueryDsl::execute(
                            diesel::insert_into(frs::file_read_status)
                                .values(NewFileReadStatus {
                                    id: Uuid::new_v4().to_string(),
                                    user_id: actor,
                                    file_id,
                                    read_at: now.clone(),
                                })
                                .on_conflict((frs::user_id, frs::file_id))
                                .do_update()
                                .set(frs::read_at.eq(now)),
                            conn,
                        )?)
                    })
                })
                .await;
        match marked {
            Ok(n) => marked_count += n,
            Err(AppError::NotFound(_) | AppError::Forbidden(_)) => {
                if let Err(error) = crate::middleware::auth::validate_credential_for_request(
                    &state,
                    &credential,
                    &method,
                    &request_path,
                )
                .await
                {
                    render_error(res, error);
                    return;
                }
                continue;
            }
            Err(error) => {
                render_error(res, error);
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
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let method = req.method().clone();
    let request_path = req.uri().path().to_owned();
    if let Err(error) = crate::middleware::auth::validate_credential_for_request(
        &state,
        &credential,
        &method,
        &request_path,
    )
    .await
    {
        render_error(res, error);
        return;
    }

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
    let mut duplicated_ids: Vec<String> = Vec::new();

    for file_id in &body.file_ids {
        let original = match load_accessible_file(&mut conn, file_id, &uid, "editor").await {
            Ok(Some(row)) if !row.is_folder && !row.is_trashed => row,
            Ok(_) | Err(AppError::Forbidden(_) | AppError::NotFound(_)) => continue,
            Err(error) => {
                render_error(res, error);
                return;
            }
        };
        let data = match read_file_with_document_fallback(
            &mut conn,
            &state.storage,
            &original.id,
            &original.path,
        )
        .await
        {
            Ok(data) => data,
            Err(error) => {
                render_error(res, error);
                return;
            }
        };
        let copy_name = format!("Copy of {}", original.name);
        let new_file_id = Uuid::new_v4().to_string();
        let storage_path = match state
            .storage
            .save_file(&owner_uuid, &copy_name, &data)
            .await
        {
            Ok(path) => path,
            Err(error) => {
                render_error(res, error);
                return;
            }
        };
        let mut new_file = NewFile {
            id: new_file_id.clone(),
            name: copy_name.clone(),
            path: storage_path.clone(),
            mime_type: original.mime_type.clone(),
            size: data.len() as i64,
            owner_id: uid.clone(),
            parent_id: original.parent_id.clone(),
            is_folder: false,
            is_trashed: false,
            trashed_at: None,
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
            room_id: original.room_id.clone(),
            thumbnail_path: None,
            sort_order: 0,
        };
        let actor = uid.clone();
        let (credential, method, request_path) =
            (credential.clone(), method.clone(), request_path.clone());
        let insertion =
            crate::services::persistence::commit_blob(&state.db, storage_path, move |conn| {
                conn.immediate_transaction::<_, AppError, _>(|conn| {
                    crate::middleware::auth::validate_credential_in_transaction(
                        conn,
                        &credential,
                        &method,
                        &request_path,
                    )?;
                    let current =
                        diesel::RunQueryDsl::first::<File>(f::files.find(&original.id), conn)?;
                    crate::services::persistence::authorize(conn, &current, &actor, "editor")?;
                    if current.path != original.path
                        || current.parent_id != original.parent_id
                        || current.room_id != original.room_id
                        || current.updated_at != original.updated_at
                    {
                        return Err(AppError::Conflict(
                            "Source changed; reload before duplicating".into(),
                        ));
                    }
                    if let Some(parent) = &current.parent_id {
                        let parent =
                            diesel::RunQueryDsl::first::<File>(f::files.find(parent), conn)?;
                        crate::services::persistence::authorize(conn, &parent, &actor, "editor")?;
                        if !parent.is_folder || parent.room_id != current.room_id {
                            return Err(AppError::Conflict("Destination folder changed".into()));
                        }
                    }
                    for index in 0..1000 {
                        let name = if index == 0 {
                            copy_name.clone()
                        } else {
                            format!("Copy {index} of {}", original.name)
                        };
                        match crate::handlers::folders::ensure_available_name(
                            conn,
                            &actor,
                            current.parent_id.as_deref(),
                            current.room_id.as_deref(),
                            &name,
                            None,
                        ) {
                            Ok(()) => {
                                new_file.name = name;
                                break;
                            }
                            Err(AppError::Conflict(_)) if index < 999 => continue,
                            Err(error) => return Err(error),
                        }
                    }
                    crate::services::persistence::adjust_quota(conn, &actor, new_file.size)?;
                    diesel::RunQueryDsl::execute(
                        diesel::insert_into(f::files).values(&new_file),
                        conn,
                    )?;
                    let linked = diesel::OptionalExtension::optional(
                        diesel::RunQueryDsl::first::<(String, Option<String>)>(
                            d::documents
                                .filter(d::file_id.eq(&current.id))
                                .select((d::doc_type, d::content)),
                            conn,
                        ),
                    )?;
                    if let Some((doc_type, content)) = linked {
                        diesel::RunQueryDsl::execute(
                            diesel::insert_into(d::documents).values(NewDocument {
                                id: Uuid::new_v4().to_string(),
                                file_id: new_file.id.clone(),
                                doc_type,
                                content,
                                version: 1,
                                created_at: new_file.created_at.clone(),
                                updated_at: new_file.updated_at.clone(),
                            }),
                            conn,
                        )?;
                    }
                    Ok(())
                })
            })
            .await;
        match insertion {
            Ok(()) => duplicated_ids.push(new_file_id),
            Err(AppError::NotFound(_) | AppError::Forbidden(_)) => continue,
            Err(error) => {
                render_error(res, error);
                return;
            }
        }
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Batch duplicate completed",
        "duplicated_count": duplicated_ids.len(),
        "new_file_ids": duplicated_ids,
    }))));
}
