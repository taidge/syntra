use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, FileResponse, MoveFileRequest};
use contrix_core::error::AppError;
use contrix_core::models::{File, NewFile};
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;

use crate::handlers::access::{
    check_room_access, load_accessible_file, load_accessible_folder, load_accessible_target_folder,
};
use crate::middleware::auth::get_user_id;
use crate::services::{StorageBackend, StorageService};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn render_error(res: &mut Response, error: AppError) {
    crate::middleware::request_observability::record_database_response("files", &error);
    res.status_code(
        StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
    );
    res.render(Json(ApiResponse::<()>::err(error.to_response())));
}

// Destination-access failures retain the endpoint's established 403 contract;
// database failures use the shared 500/503 classification instead.
fn render_access_error(res: &mut Response, error: AppError) {
    let error = match error {
        AppError::Database(_) | AppError::Internal(_) | AppError::Forbidden(_) => error,
        other => AppError::Forbidden(other.to_string()),
    };
    render_error(res, error);
}

async fn document_id_for_file(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    file_id: &str,
) -> Option<Uuid> {
    d::documents
        .filter(d::file_id.eq(file_id))
        .select(d::id)
        .first::<String>(conn)
        .await
        .optional()
        .ok()
        .flatten()
        .and_then(|id| id.parse().ok())
}

async fn file_model_to_response(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    row: File,
) -> FileResponse {
    let document_id = document_id_for_file(conn, &row.id).await;
    let shared_with_count = crate::handlers::access::count_shares_for_file(conn, &row.id).await;
    FileResponse {
        id: row.id.parse().unwrap_or_default(),
        name: row.name,
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
        shared_with_count,
        document_id,
    }
}

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

fn max_upload_bytes(max_upload_size_mb: u64) -> i64 {
    let bytes = (max_upload_size_mb as u128).saturating_mul(1024 * 1024);
    bytes.min(i64::MAX as u128) as i64
}

fn safe_attachment_name(file_name: &str) -> String {
    StorageService::sanitize_file_name(file_name)
}

// ---------------------------------------------------------------------------
// GET /files
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_files(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let parent_id_param: Option<String> = req.query("parent_id");
    let page: u32 = req.query("page").unwrap_or(1);
    let per_page: u32 = req.query::<u32>("per_page").unwrap_or(50).clamp(1, 100);
    let offset_val = i64::from(page.saturating_sub(1)) * i64::from(per_page);
    let limit_val = per_page as i64;

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let result: Result<Vec<File>, AppError> = if let Some(pid) = parent_id_param {
        let parent = match load_accessible_folder(&mut conn, &pid, &uid, "viewer").await {
            Ok(Some(row)) => row,
            Ok(None) => {
                res.status_code(StatusCode::NOT_FOUND);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::NotFound("Folder not found".into()).to_response(),
                )));
                return;
            }
            Err(e) => {
                render_access_error(res, e);
                return;
            }
        };

        let mut query = f::files
            .into_boxed()
            .filter(f::parent_id.eq(&pid))
            .filter(f::is_trashed.eq(false))
            .filter(diesel::dsl::sql::<diesel::sql_types::Bool>("((files.room_id IS NULL AND files.owner_id = ")
                .bind::<diesel::sql_types::Text,_>(&uid)
                .sql(") OR EXISTS (SELECT 1 FROM room_members JOIN rooms ON rooms.id=room_members.room_id JOIN users room_owner ON room_owner.id=rooms.owner_id WHERE room_members.room_id=files.room_id AND room_owner.tenant_id IS (SELECT tenant_id FROM users WHERE id=room_members.user_id) AND room_members.user_id = ")
                .bind::<diesel::sql_types::Text,_>(&uid)
                .sql(")) AND EXISTS (SELECT 1 FROM users owner_user JOIN users actor_user ON actor_user.id = ")
                .bind::<diesel::sql_types::Text,_>(&uid)
                .sql(" WHERE owner_user.id=files.owner_id AND owner_user.tenant_id IS actor_user.tenant_id)"))
            .order((f::sort_order.asc(), f::is_folder.desc(), f::name.asc()))
            .limit(limit_val)
            .offset(offset_val);

        if parent.room_id.is_none() {
            query = query.filter(f::owner_id.eq(&uid));
        }

        query
            .load::<File>(&mut conn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    } else {
        f::files
            .filter(f::owner_id.eq(&uid))
            .filter(f::parent_id.is_null())
            .filter(f::is_trashed.eq(false))
            .filter(diesel::dsl::sql::<diesel::sql_types::Bool>("((files.room_id IS NULL AND files.owner_id = ")
                .bind::<diesel::sql_types::Text,_>(&uid)
                .sql(") OR EXISTS (SELECT 1 FROM room_members JOIN rooms ON rooms.id=room_members.room_id JOIN users room_owner ON room_owner.id=rooms.owner_id WHERE room_members.room_id=files.room_id AND room_owner.tenant_id IS (SELECT tenant_id FROM users WHERE id=room_members.user_id) AND room_members.user_id = ")
                .bind::<diesel::sql_types::Text,_>(&uid)
                .sql(")) AND EXISTS (SELECT 1 FROM users owner_user JOIN users actor_user ON actor_user.id = ")
                .bind::<diesel::sql_types::Text,_>(&uid)
                .sql(" WHERE owner_user.id=files.owner_id AND owner_user.tenant_id IS actor_user.tenant_id)"))
            .order((f::sort_order.asc(), f::is_folder.desc(), f::name.asc()))
            .limit(limit_val)
            .offset(offset_val)
            .load::<File>(&mut conn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    };

    match result {
        Ok(rows) => {
            let mut file_list = Vec::with_capacity(rows.len());
            for row in rows {
                file_list.push(file_model_to_response(&mut conn, row).await);
            }
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(file_list)));
        }
        Err(e) => {
            render_error(res, e);
        }
    }
}

// ---------------------------------------------------------------------------
// POST /files/upload
// ---------------------------------------------------------------------------

#[handler]
pub async fn upload_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Auth("Not authenticated".into()).to_response(),
        )));
        return;
    };
    static UPLOAD_CAPACITY: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));
    let upload_capacity = match UPLOAD_CAPACITY.clone().try_acquire_owned() {
        Ok(permit) => std::sync::Arc::new(permit),
        Err(_) => {
            res.status_code(StatusCode::TOO_MANY_REQUESTS);
            res.render(Json(ApiResponse::<()>::err(
                AppError::RateLimit("Upload capacity is busy; retry later".into()).to_response(),
            )));
            return;
        }
    };

    // Salvo's default form limit is 64 KiB. Apply the configured file budget
    // before the first form read, with a bounded allowance for multipart headers.
    let multipart_limit =
        usize::try_from(max_upload_bytes(state.config.storage.max_upload_size_mb))
            .unwrap_or(usize::MAX)
            .saturating_add(64 * 1024);
    let retained_body = super::request_body::RetainedBody::install_with_timeouts(
        req,
        std::time::Duration::from_secs(state.config.storage.upload_idle_timeout_seconds),
        std::time::Duration::from_secs(state.config.storage.upload_total_timeout_seconds),
    );
    let rejection_budget = multipart_limit.saturating_add(1024 * 1024);
    if req
        .headers()
        .get("content-length")
        .and_then(|length| length.to_str().ok())
        .and_then(|length| length.parse::<u64>().ok())
        .is_some_and(|length| length > multipart_limit as u64)
    {
        // A small fixed allowance covers common over-limit clients without
        // turning a rejected multi-gigabyte or stalled body into unbounded work.
        let drained = retained_body
            .drain(rejection_budget, std::time::Duration::from_secs(2))
            .await;
        super::request_body::close_if_incomplete(req, res, drained);
        res.status_code(StatusCode::PAYLOAD_TOO_LARGE);
        res.render(Json(ApiResponse::<()>::err(
            AppError::QuotaExceeded(format!(
                "Upload exceeds configured limit of {} MB",
                state.config.storage.max_upload_size_mb
            ))
            .to_response(),
        )));
        return;
    }
    req.set_secure_max_size(multipart_limit);
    if let Err(error) = req.form_data().await {
        let drained = retained_body
            .drain(rejection_budget, std::time::Duration::from_secs(2))
            .await;
        super::request_body::close_if_incomplete(req, res, drained);
        let too_large = super::request_body::exceeds_limit(&error);
        let timed_out = retained_body.timed_out();
        let error = if timed_out {
            AppError::Timeout("Reading the upload body exceeded its configured deadline".into())
        } else if too_large {
            AppError::QuotaExceeded(format!(
                "Upload exceeds configured limit of {} MB",
                state.config.storage.max_upload_size_mb
            ))
        } else {
            AppError::Validation(format!("Invalid multipart upload: {error}"))
        };
        res.status_code(if timed_out {
            StatusCode::REQUEST_TIMEOUT
        } else if too_large {
            StatusCode::PAYLOAD_TOO_LARGE
        } else {
            StatusCode::BAD_REQUEST
        });
        res.render(Json(ApiResponse::<()>::err(error.to_response())));
        return;
    }
    // Multipart stops at its closing boundary, before a chunked transport's
    // terminal frame. Keep the incoming body alive until HTTP framing ends.
    let complete = retained_body
        .drain(multipart_limit, std::time::Duration::from_secs(2))
        .await;
    if !complete {
        super::request_body::close_if_incomplete(req, res, false);
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Multipart transport did not finish within its budget".into())
                .to_response(),
        )));
        return;
    }
    drop(retained_body);

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
        .map(StorageService::sanitize_file_name)
        .unwrap_or_else(|| "upload".to_string());
    let content_type = file_part.content_type().map(|m| m.to_string());

    let temporary_path = file_part.path().to_owned();
    let read_capacity = upload_capacity.clone();
    let read = tokio::spawn(async move {
        let _capacity = read_capacity;
        tokio::fs::read(temporary_path).await
    })
    .await;
    let data = match read {
        Ok(Ok(data)) => std::sync::Arc::new(data),
        error => {
            tracing::warn!(?error, "Upload staging read failed");
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Storage("Could not read upload".into()).to_response(),
            )));
            return;
        }
    };

    let size_val = data.len() as i64;
    let max_size = max_upload_bytes(state.config.storage.max_upload_size_mb);
    if size_val > max_size {
        res.status_code(StatusCode::PAYLOAD_TOO_LARGE);
        res.render(Json(ApiResponse::<()>::err(
            AppError::QuotaExceeded(format!(
                "Upload exceeds configured limit of {} MB",
                state.config.storage.max_upload_size_mb
            ))
            .to_response(),
        )));
        return;
    }

    let owner_uuid: Uuid = match uid.parse() {
        Ok(uuid) => uuid,
        Err(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Invalid user id".into()).to_response(),
            )));
            return;
        }
    };

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let room_id_for_new_file = if let Some(parent_id) = parent_id_param.as_deref() {
        match load_accessible_target_folder(&mut conn, parent_id, &uid, "editor").await {
            Ok(Some(parent)) => {
                if let Some(requested_room_id) = room_id_param.as_deref()
                    && parent.room_id.as_deref() != Some(requested_room_id)
                {
                    res.status_code(StatusCode::BAD_REQUEST);
                    res.render(Json(ApiResponse::<()>::err(
                        AppError::Validation("room_id must match the parent folder room".into())
                            .to_response(),
                    )));
                    return;
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
                render_access_error(res, e);
                return;
            }
        }
    } else if let Some(room_id) = room_id_param.as_deref() {
        match check_room_access(&mut conn, &uid, room_id, "editor").await {
            Ok(()) => Some(room_id.to_string()),
            Err(e) => {
                render_access_error(res, e);
                return;
            }
        }
    } else {
        None
    };

    let quota_owner = uid.clone();
    if let Err(error) = state
        .db
        .run(move |conn| {
            crate::services::persistence::check_staging_quota(conn, &quota_owner, size_val)
        })
        .await
    {
        res.status_code(
            StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        );
        res.render(Json(ApiResponse::<()>::err(error.to_response())));
        return;
    }
    if let Err(error) = crate::middleware::auth::validate_credential_for_request(
        &state,
        &credential,
        &salvo::http::Method::POST,
        "/api/v1/files/upload",
    )
    .await
    {
        res.status_code(
            StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        );
        res.render(Json(ApiResponse::<()>::err(error.to_response())));
        return;
    }
    let staged = match crate::services::persistence::stage_blob(
        &state.db,
        state.storage.clone(),
        owner_uuid,
        file_name.clone(),
        data.clone(),
        upload_capacity.clone(),
    )
    .await
    {
        Ok(staged) => staged,
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
            return;
        }
    };
    let storage_path = staged.path.clone();

    let file_id_val = Uuid::new_v4();
    let now = Utc::now();

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

    let imported_format = std::path::Path::new(&file_name)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "docx" | "xlsx" | "pptx"));
    let commit_owner = uid.clone();
    let commit_parent = parent_id_param.clone();
    let source_version = contrix_core::models::NewFileVersion {
        id: Uuid::new_v4().to_string(),
        file_id: file_id_val.to_string(),
        version_number: 0,
        path: storage_path.clone(),
        size: size_val,
        created_by: uid.clone(),
        created_at: now.to_rfc3339(),
    };
    let commit_credential = credential.clone();
    let commit_capacity = upload_capacity.clone();
    let result = staged
        .commit(move |connection| {
            let _capacity = commit_capacity;
            connection.immediate_transaction::<_, AppError, _>(|connection| {
                crate::middleware::auth::validate_credential_in_transaction(
                    connection,
                    &commit_credential,
                    &salvo::http::Method::POST,
                    "/api/v1/files/upload",
                )?;
                if let Some(parent) = commit_parent {
                    let folder =
                        diesel::RunQueryDsl::first::<File>(f::files.find(parent), connection)?;
                    crate::services::persistence::authorize(
                        connection,
                        &folder,
                        &commit_owner,
                        "editor",
                    )?;
                    if !folder.is_folder {
                        return Err(AppError::Validation("Parent is not a folder".into()));
                    }
                    if folder.room_id != new_file.room_id {
                        return Err(AppError::Conflict(
                            "Parent folder moved during upload; retry in its current room".into(),
                        ));
                    }
                }
                crate::services::tenancy::user_tenant(connection, &commit_owner)?;
                if let Some(room) = &new_file.room_id {
                    crate::services::persistence::authorize_room(
                        connection,
                        room,
                        &commit_owner,
                        "editor",
                    )?;
                }
                diesel::RunQueryDsl::execute(
                    diesel::insert_into(f::files).values(&new_file),
                    connection,
                )?;
                crate::services::persistence::adjust_quota(connection, &commit_owner, size_val)?;
                if imported_format {
                    diesel::RunQueryDsl::execute(
                        diesel::insert_into(contrix_core::schema::file_versions::table)
                            .values(&source_version),
                        connection,
                    )?;
                }
                Ok(())
            })
        })
        .await;
    if let Err(error) = result {
        res.status_code(
            StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        );
        res.render(Json(ApiResponse::<()>::err(error.to_response())));
        return;
    }

    // Generate thumbnail for image uploads.
    let mut thumbnail_path_val: Option<String> = None;
    if let Some(ref ct) = content_type
        && ct.starts_with("image/")
        && let Some(ref thumb_svc) = state.thumbnail
    {
        match thumb_svc
            .generate_thumbnail(&state.storage, &storage_path, &file_id_val.to_string())
            .await
        {
            Ok(thumb_rel) => {
                let _ = diesel::update(f::files.filter(f::id.eq(file_id_val.to_string())))
                    .set(f::thumbnail_path.eq(Some(&thumb_rel)))
                    .execute(&mut conn)
                    .await;
                thumbnail_path_val = Some(thumb_rel);
                tracing::debug!("Thumbnail generated for file {file_id_val}");
            }
            Err(e) => {
                tracing::warn!("Failed to generate thumbnail for {file_id_val}: {e}");
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
    )
    .await;

    let mut imported_doc_id: Option<Uuid> = None;
    let mut import_warnings: Vec<String> = Vec::new();
    let ext = std::path::Path::new(&file_name)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    if let Some(ext) = ext.filter(|ext| matches!(ext.as_str(), "docx" | "xlsx" | "pptx")) {
        static IMPORT_WORKERS: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
            std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2)));
        let permit = IMPORT_WORKERS.clone().acquire_owned().await;
        let parse_capacity = upload_capacity.clone();
        let parse_result = match permit {
            Ok(permit) => tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let _capacity = parse_capacity;
                match ext.as_str() {
                    "docx" => contrix_export::docx::docx_to_writer(&data)
                        .and_then(|document| {
                            serde_json::to_value(document).map_err(|e| e.to_string())
                        })
                        .map(|value| (value, contrix_core::models::DocumentType::Writer)),
                    "xlsx" => contrix_export::xlsx::xlsx_to_workbook(&data)
                        .and_then(|document| {
                            serde_json::to_value(document).map_err(|e| e.to_string())
                        })
                        .map(|value| (value, contrix_core::models::DocumentType::Calc)),
                    _ => contrix_export::pptx::pptx_to_presentation(&data)
                        .and_then(|document| {
                            serde_json::to_value(document).map_err(|e| e.to_string())
                        })
                        .map(|value| (value, contrix_core::models::DocumentType::Slides)),
                }
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|result| result),
            Err(error) => Err(error.to_string()),
        };
        match parse_result {
            Ok((content, doc_type)) => {
                import_warnings = content
                    .pointer("/metadata/import_warnings")
                    .and_then(|value| value.as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let new_doc_id = Uuid::new_v4();
                let document = contrix_core::models::NewDocument {
                    id: new_doc_id.to_string(),
                    file_id: file_id_val.to_string(),
                    doc_type: doc_type.as_str().to_owned(),
                    content: Some(content.to_string()),
                    version: 1,
                    created_at: now.to_rfc3339(),
                    updated_at: now.to_rfc3339(),
                };
                let import_source_path = storage_path.clone();
                let import_user = uid.clone();
                let import_credential = credential.clone();
                let import_capacity = upload_capacity.clone();
                let persist = state
                    .db
                    .run(move |connection| {
                        let _capacity = import_capacity;
                        connection.immediate_transaction::<_, AppError, _>(|connection| {
                            crate::middleware::auth::validate_credential_in_transaction(
                                connection,
                                &import_credential,
                                &salvo::http::Method::POST,
                                "/api/v1/files/upload",
                            )?;
                            let file = diesel::RunQueryDsl::first::<File>(
                                f::files.find(&document.file_id),
                                connection,
                            )?;
                            crate::services::persistence::authorize(
                                connection,
                                &file,
                                &import_user,
                                "editor",
                            )?;
                            if file.path != import_source_path {
                                return Err(AppError::Conflict(
                                    "Upload changed during import".into(),
                                ));
                            }
                            diesel::RunQueryDsl::execute(
                                diesel::insert_into(contrix_core::schema::documents::table)
                                    .values(&document),
                                connection,
                            )?;
                            Ok(())
                        })
                    })
                    .await;
                match persist {
                    Ok(()) => imported_doc_id = Some(new_doc_id),
                    Err(error) => {
                        tracing::warn!(%file_id_val,%error,"Document import skipped after source changed or became unavailable");
                        import_warnings.push("Import could not be committed because the source changed or became unavailable. Refresh the file state before editing.".into());
                    }
                }
            }
            Err(error) => import_warnings.push(format!(
                "Import failed: {error}. The original upload is retained for download."
            )),
        }
    }

    let file_resp = FileResponse {
        id: file_id_val,
        name: file_name,
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
    let mut response = serde_json::to_value(file_resp).expect("FileResponse serializes");
    response["import_warnings"] = serde_json::json!(import_warnings);
    if imported_format {
        response["original_file_id"] = serde_json::json!(file_id_val);
        response["original_download_url"] = serde_json::json!(format!(
            "/api/v1/files/{file_id_val}/download?original=true"
        ));
    }
    res.render(Json(ApiResponse::ok(response)));
}

// ---------------------------------------------------------------------------
// GET /files/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    match load_accessible_file(&mut conn, &file_id_param, &uid, "viewer").await {
        Ok(Some(row)) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                file_model_to_response(&mut conn, row).await,
            )));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
        }
        Err(e) => {
            render_error(res, e);
        }
    }
}

// ---------------------------------------------------------------------------
// PUT /files/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let new_name = StorageService::sanitize_file_name(&body.name);
    let now = Utc::now();

    let mut conn = state.db.clone();

    let target = match load_accessible_file(&mut conn, &file_id_param, &uid, "editor").await {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            render_access_error(res, e);
            return;
        }
    };

    let rename_id = target.id.clone();
    let rename_user = uid.clone();
    let rename_name = new_name.clone();
    match state
        .db
        .run(move |connection| {
            connection.immediate_transaction::<_, AppError, _>(|connection| {
                crate::middleware::auth::validate_credential_in_transaction(
                    connection,
                    &credential,
                    &method,
                    &request_path,
                )?;
                let file =
                    diesel::RunQueryDsl::first::<File>(f::files.find(&rename_id), connection)?;
                crate::services::persistence::authorize(connection, &file, &rename_user, "editor")?;
                crate::services::persistence::check_file_lock(
                    connection,
                    &file.id,
                    &rename_user,
                    None,
                )?;
                crate::handlers::folders::ensure_available_name(
                    connection,
                    &file.owner_id,
                    file.parent_id.as_deref(),
                    file.room_id.as_deref(),
                    &rename_name,
                    Some(&file.id),
                )?;
                Ok(diesel::RunQueryDsl::execute(
                    diesel::update(f::files.find(&rename_id))
                        .set((f::name.eq(&rename_name), f::updated_at.eq(now.to_rfc3339()))),
                    connection,
                )?)
            })
        })
        .await
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
                    "name": &new_name,
                    "owner_id": &uid,
                    "updated_at": now.to_rfc3339(),
                }),
            )
            .await;
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "File renamed" }),
            )));
        }
        Err(e) => {
            res.status_code(
                StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
        }
    }
}

// ---------------------------------------------------------------------------
// DELETE /files/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let Some(uid) = get_user_id(depot) else {
        res.status_code(StatusCode::UNAUTHORIZED);
        return;
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

    let id: String = req.param("id").unwrap_or_default();
    match crate::handlers::folders::trash_file_tree(
        &state.db,
        &id,
        &uid,
        &credential,
        &method,
        &request_path,
    )
    .await
    {
        Ok(_) => {
            crate::services::WebhookService::fire_event(
                &state.db,
                "file.deleted",
                &serde_json::json!({"id":id,"deleted_by":uid,"deleted_at":Utc::now().to_rfc3339()}),
            )
            .await;
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({"message":"File trashed"}),
            )));
        }
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
        }
    }
}

// ---------------------------------------------------------------------------
// POST /files/:id/move
// ---------------------------------------------------------------------------

#[handler]
pub async fn move_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let Some(uid) = get_user_id(depot) else {
        res.status_code(StatusCode::UNAUTHORIZED);
        return;
    };
    let id: String = req.param("id").unwrap_or_default();
    let body: MoveFileRequest = match req.parse_json().await {
        Ok(body) => body,
        Err(error) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(error.to_string()).to_response(),
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

    match crate::handlers::folders::move_file_tree(
        &state.db,
        &id,
        &body.target_folder_id.to_string(),
        &uid,
        &credential,
        &method,
        &request_path,
    )
    .await
    {
        Ok(_) => {
            crate::services::WebhookService::fire_event(
                &state.db,
                "file.moved",
                &serde_json::json!({"id":id,"parent_id":body.target_folder_id,"moved_by":uid}),
            )
            .await;
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({"message":"File moved"}),
            )));
        }
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
        }
    }
}

// ---------------------------------------------------------------------------
// GET /files/:id/download
// ---------------------------------------------------------------------------

#[handler]
pub async fn download_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let file_row = match load_accessible_file(&mut conn, &file_id_param, &uid, "viewer").await {
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
            render_access_error(res, e);
            return;
        }
    };

    let resolved_file_id = file_row.id.clone();
    let file_name = file_row.name.clone();
    let file_path = if req.query::<bool>("original").unwrap_or(false) {
        use contrix_core::schema::file_versions::dsl as fv;
        match fv::file_versions
            .filter(fv::file_id.eq(&resolved_file_id))
            .filter(fv::version_number.eq(0))
            .select(fv::path)
            .first::<String>(&mut conn)
            .await
            .optional()
        {
            Ok(Some(path)) => path,
            Ok(None) => {
                res.status_code(StatusCode::NOT_FOUND);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::NotFound("Original upload not retained".into()).to_response(),
                )));
                return;
            }
            Err(error) => {
                render_error(res, AppError::from(error));
                return;
            }
        }
    } else {
        file_row.path.clone()
    };
    let mime_type_val = file_row.mime_type.clone();

    let read_result = if req.query::<bool>("original").unwrap_or(false) {
        state.storage.read_file(&file_path).await
    } else {
        read_file_with_document_fallback(&mut conn, &state.storage, &resolved_file_id, &file_path)
            .await
    };
    let mut data = match read_result {
        Ok(d) => d,
        Err(e) => {
            render_access_error(res, e);
            return;
        }
    };

    let mut ct = mime_type_val
        .clone()
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    // Edited imports are stored as native JSON. Download their current Office
    // representation rather than returning JSON with an Office MIME/filename.
    if !req.query::<bool>("original").unwrap_or(false) && file_path.ends_with(".contrix.json") {
        use contrix_export::ExportFormat;
        let extension = std::path::Path::new(&file_name)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let choices = [
            mime_type_val.as_deref().and_then(ExportFormat::from_mime),
            ExportFormat::from_extension(extension),
        ];
        let document = d::documents
            .filter(d::file_id.eq(&resolved_file_id))
            .select((d::doc_type, d::content))
            .first::<(String, Option<String>)>(&mut conn)
            .await
            .optional();
        match document {
            Ok(Some((kind, Some(content)))) => {
                let format = choices.into_iter().flatten().find(|format| {
                    matches!(
                        (kind.as_str(), format),
                        ("writer", ExportFormat::Docx)
                            | ("calc", ExportFormat::Xlsx)
                            | ("slides", ExportFormat::Pptx)
                    )
                });
                if let Some(format) = format {
                    ct = format.mime_type().to_owned();
                    match crate::handlers::convert::export_content(
                        content,
                        kind,
                        format,
                        format.extension().to_owned(),
                    )
                    .await
                    {
                        Ok(bytes) => data = bytes,
                        Err(error) => {
                            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                            res.render(Json(ApiResponse::<()>::err(
                                AppError::Internal(error).to_response(),
                            )));
                            return;
                        }
                    }
                }
            }
            Err(error) => {
                render_error(res, AppError::from(error));
                return;
            }
            _ => {}
        }
    }
    res.status_code(StatusCode::OK);
    res.headers_mut().insert(
        "content-type",
        ct.parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );
    res.headers_mut().insert(
        "content-disposition",
        format!(
            "attachment; filename=\"{}\"",
            safe_attachment_name(&file_name)
        )
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

    let now = Utc::now();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let target = match load_accessible_file(&mut conn, &file_id_param, &uid, "editor").await {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            render_access_error(res, e);
            return;
        }
    };

    let result = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                crate::middleware::auth::validate_credential_in_transaction(
                    conn,
                    &credential,
                    &method,
                    &request_path,
                )?;
                let file = diesel::RunQueryDsl::first::<File>(f::files.find(&target.id), conn)?;
                crate::services::persistence::authorize_including_trashed(
                    conn, &file, &uid, "editor",
                )?;
                Ok(diesel::RunQueryDsl::execute(
                    diesel::update(f::files.find(&target.id)).set((
                        f::sort_order.eq(body.sort_order),
                        f::updated_at.eq(now.to_rfc3339()),
                    )),
                    conn,
                )?)
            })
        })
        .await;
    match result {
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
        Err(error) => render_error(res, error),
    }
}
