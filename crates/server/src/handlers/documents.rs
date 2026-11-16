use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{
    ApiResponse, CreateDocumentRequest, DocumentResponse, UpdateDocumentRequest,
};
use contrix_core::error::AppError;
use contrix_core::models::{DocumentType, NewDocument, NewFile};
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::versions::dsl as v;
use contrix_export::ExportFormat;

use crate::handlers::access::{check_room_access, load_accessible_target_folder};
use crate::middleware::auth::get_user_id;
use crate::middleware::request_observability::{record_failure, with_request_database_observation};
use crate::services::{StorageBackend, StorageService};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
    // Sanitize the user-supplied document name before it becomes part of a
    // storage file name. `save_file` sanitizes again, but doing it here keeps
    // the stored path predictable and removes any ambiguity for other backends.
    let safe = StorageService::sanitize_file_name(name);
    format!("{safe}.contrix.json")
}

async fn persist_document_blob(
    storage: &StorageBackend,
    owner_id: &str,
    storage_name: &str,
    content: &str,
) -> Result<(String, i64), AppError> {
    let owner_uuid: Uuid = owner_id
        .parse()
        .map_err(|_| AppError::Validation("Invalid owner id".into()))?;
    let path = storage
        .save_file(&owner_uuid, storage_name, content.as_bytes())
        .await?;
    Ok((path, content.len() as i64))
}

fn render_error(res: &mut Response, error: AppError) {
    crate::middleware::request_observability::record_database_response("documents", &error);
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

async fn resolve_document_id(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    identifier: &str,
    user: &str,
    role: &str,
) -> Result<Option<String>, AppError> {
    let file =
        crate::handlers::access::load_accessible_document(conn, identifier, user, role).await?;
    Ok(file
        .filter(|file| !file.is_trashed)
        .map(|_| identifier.to_owned()))
}

// ---------------------------------------------------------------------------
// POST /documents
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_document(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
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
    let content = default_content(&body.document_type, &body.name);
    let content_str = serde_json::to_string(&content).unwrap_or_default();
    let storage_name = document_storage_name(&body.name);
    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let room_id_for_new_document = if let Some(parent_id) = parent_id_str.as_deref() {
        match load_accessible_target_folder(&mut conn, parent_id, &uid, "editor").await {
            Ok(Some(parent)) => {
                if let Some(requested_room_id) = requested_room_id.as_deref()
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
    } else if let Some(room_id) = requested_room_id.as_deref() {
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

    let (path_val, size_val) =
        match persist_document_blob(&state.storage, &uid, &storage_name, &content_str).await {
            Ok(value) => value,
            Err(error) => {
                render_error(res, error);
                return;
            }
        };
    // Create the file record.
    let new_file = NewFile {
        id: file_id_val.to_string(),
        name: body.name.clone(),
        path: path_val.clone(),
        mime_type: Some(mime_for_doc_type(&body.document_type).to_string()),
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

    // Create the document record.
    let new_doc = NewDocument {
        id: doc_id_val.to_string(),
        file_id: file_id_val.to_string(),
        doc_type: body.document_type.as_str().to_string(),
        content: Some(content_str.clone()),
        version: 1,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };

    let creating_user = uid.clone();
    let result =
        crate::services::persistence::commit_blob(&state.db, path_val.clone(), move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                crate::middleware::auth::validate_credential_in_transaction(
                    conn,
                    &credential,
                    &salvo::http::Method::POST,
                    "/api/v1/documents",
                )?;
                if let Some(parent_id) = &new_file.parent_id {
                    use contrix_core::models::File;
                    let parent =
                        diesel::RunQueryDsl::first::<File>(f::files.find(parent_id), conn)?;
                    if !parent.is_folder || parent.room_id != new_file.room_id {
                        return Err(AppError::Conflict("Parent folder changed".into()));
                    }
                    crate::services::persistence::authorize(
                        conn,
                        &parent,
                        &creating_user,
                        "editor",
                    )?;
                } else if let Some(room_id) = &new_file.room_id {
                    crate::services::persistence::authorize_room(
                        conn,
                        room_id,
                        &creating_user,
                        "editor",
                    )?;
                }
                crate::services::persistence::adjust_quota(conn, &creating_user, size_val)?;
                diesel::RunQueryDsl::execute(
                    diesel::insert_into(f::files).values(&new_file),
                    conn,
                )?;
                diesel::RunQueryDsl::execute(
                    diesel::insert_into(d::documents).values(&new_doc),
                    conn,
                )?;
                Ok(())
            })
        })
        .await;
    if let Err(error) = result {
        render_error(res, error);
        return;
    }

    crate::services::WebhookService::fire_event(
        &state.db,
        "document.created",
        &serde_json::json!({
            "id": doc_id_val.to_string(),
            "file_id": file_id_val.to_string(),
            "name": body.name,
            "owner_id": uid,
            "document_type": body.document_type.as_str(),
            "created_at": now.to_rfc3339(),
        }),
    )
    .await;

    let resp = DocumentResponse {
        id: doc_id_val,
        file_id: file_id_val,
        name: body.name,
        document_type: body.document_type,
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param, &uid, "viewer").await
    {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            render_error(res, e);
            return;
        }
    };

    let row = d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
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
        .await
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
            let doc_type = DocumentType::parse_lossy(&doc_type_str);
            let content: serde_json::Value = content_opt
                .as_deref()
                .and_then(|v| serde_json::from_str(v).ok())
                .unwrap_or_default();
            let content = if doc_type == DocumentType::Writer {
                crate::services::legacy_writer_layout::for_response(
                    &state.db,
                    &state.storage,
                    &file_id_val,
                    version_val,
                    content,
                )
                .await
            } else if doc_type == DocumentType::Slides {
                crate::services::legacy_slides_layout::for_response(
                    &state.db,
                    &state.storage,
                    &file_id_val,
                    version_val,
                    content,
                )
                .await
            } else {
                content
            };
            let resp = DocumentResponse {
                id: id_val.parse().unwrap_or_default(),
                file_id: file_id_val.parse().unwrap_or_default(),
                name: name_val,
                document_type: doc_type,
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
            render_error(res, AppError::Database(e.to_string()));
        }
    }
}

// ---------------------------------------------------------------------------
// PUT /documents/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_document(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let Some(uid) = get_user_id(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let id: String = req.param("id").unwrap_or_default();
    // Check access before allocating a large body; the commit transaction
    // checks it again after parsing, so a concurrent revocation still wins.
    match resolve_document_id(&mut state.db.clone(), &id, &uid, "editor").await {
        Ok(Some(_)) => {}
        Ok(None) => {
            render_error(res, AppError::NotFound("Document not found".into()));
            return;
        }
        Err(error) => {
            render_error(res, error);
            return;
        }
    }
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    static CAPACITY: std::sync::LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)));
    let capacity = match CAPACITY.clone().try_acquire_owned() {
        Ok(permit) => std::sync::Arc::new(permit),
        Err(_) => {
            record_failure("document_save_capacity", "capacity_exhausted");
            render_error(
                res,
                AppError::RateLimit("Document save capacity is busy; retry later".into()),
            );
            return;
        }
    };
    const REQUEST_BYTES: usize = contrix_doc_engine::MAX_DOCUMENT_CONTENT_BYTES + 64 * 1024;
    const REJECTION_BYTES: usize = REQUEST_BYTES + 1024 * 1024;
    let retained = super::request_body::RetainedBody::install_with_timeouts(
        req,
        std::time::Duration::from_secs(state.config.storage.upload_idle_timeout_seconds),
        std::time::Duration::from_secs(state.config.storage.upload_total_timeout_seconds),
    );
    if req
        .headers()
        .get("content-length")
        .and_then(|length| length.to_str().ok())
        .and_then(|length| length.parse::<u64>().ok())
        .is_some_and(|length| length > REQUEST_BYTES as u64)
    {
        let drained = retained
            .drain(REJECTION_BYTES, std::time::Duration::from_secs(2))
            .await;
        super::request_body::close_if_incomplete(req, res, drained);
        render_error(
            res,
            AppError::QuotaExceeded(
                "Document request exceeds the 20 MiB model plus envelope budget".into(),
            ),
        );
        return;
    }
    if !req
        .content_type()
        .is_some_and(|content_type| content_type.subtype().as_str() == "json")
    {
        let drained = retained
            .drain(REJECTION_BYTES, std::time::Duration::from_secs(2))
            .await;
        super::request_body::close_if_incomplete(req, res, drained);
        render_error(
            res,
            AppError::Validation("Expected a JSON request body".into()),
        );
        return;
    }
    req.set_secure_max_size(REQUEST_BYTES);
    let payload = match req.payload().await {
        Ok(payload) => payload.clone(),
        Err(error) => {
            let drained = retained
                .drain(REJECTION_BYTES, std::time::Duration::from_secs(2))
                .await;
            super::request_body::close_if_incomplete(req, res, drained);
            let error = if retained.timed_out() {
                AppError::Timeout(
                    "Reading the document body exceeded its configured deadline".into(),
                )
            } else if super::request_body::exceeds_limit(&error) {
                AppError::QuotaExceeded(
                    "Document request exceeds the configured JSON budget".into(),
                )
            } else {
                AppError::Validation(error.to_string())
            };
            render_error(res, error);
            return;
        }
    };
    drop(retained);
    let parse_capacity = capacity.clone();
    let parsed = tokio::task::spawn_blocking(move || {
        let _capacity = parse_capacity;
        let body: UpdateDocumentRequest = serde_json::from_slice(&payload).map_err(|error| {
            AppError::Validation(format!(
                "Invalid document JSON at line {}, column {} ({:?})",
                error.line(),
                error.column(),
                error.classify()
            ))
        })?;
        let content = serde_json::to_string(&body.content)
            .map_err(|error| AppError::Validation(error.to_string()))?;
        if content.len() > contrix_doc_engine::MAX_DOCUMENT_CONTENT_BYTES {
            return Err(AppError::QuotaExceeded(
                "Document content exceeds 20 MiB".into(),
            ));
        }
        Ok::<_, AppError>((body, content))
    })
    .await;
    let (body, content) = match parsed {
        Ok(Ok(parsed)) => parsed,
        Ok(Err(error)) => {
            render_error(res, error);
            return;
        }
        Err(_) => {
            render_error(
                res,
                AppError::Internal("Document parsing worker failed".into()),
            );
            return;
        }
    };
    let Some(expected) = body.expected_version.filter(|version| *version > 0) else {
        render_error(
            res,
            AppError::Validation("A positive expected_version is required".into()),
        );
        return;
    };
    // The owned task retains admission through storage and SQL, even if the
    // HTTP client disconnects while a background commit can still succeed.
    let committed = tokio::spawn(with_request_database_observation(async move {
        let (notify, completion) = tokio::sync::oneshot::channel();
        let result = crate::services::persistence::commit_document_with_credential(
            &state.db,
            &state.storage,
            &id,
            &uid,
            expected,
            content,
            &credential,
            notify,
        )
        .await;
        let publish = async move {
            let _capacity = capacity;
            let Ok(version) = completion.await else {
                return;
            };
            if let Ok(document_id) = Uuid::parse_str(&id) {
                state
                    .collab
                    .publish_snapshot(document_id, body.content, version as u64)
                    .await;
            }
            crate::services::WebhookService::fire_event(
                &state.db,
                "document.updated",
                &serde_json::json!({"id":id,"version":version}),
            )
            .await;
        };
        if result.is_ok() {
            publish.await;
        } else {
            // Return the HTTP error while retaining ownership of any late commit.
            tokio::spawn(with_request_database_observation(publish));
        }
        result
    }))
    .await;
    match committed {
        Ok(Ok(version)) => {
            res.render(Json(ApiResponse::ok(
                serde_json::json!({"version":version,"message":"Document updated"}),
            )));
        }
        Ok(Err(error)) => render_error(res, error),
        Err(_) => render_error(
            res,
            AppError::Internal("Document commit worker failed".into()),
        ),
    }
}

// ---------------------------------------------------------------------------
// DELETE /documents/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_document(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let Some(uid) = get_user_id(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let id: String = req.param("id").unwrap_or_default();
    let deleted_id = id.clone();
    let result = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                use contrix_core::models::{Document, File};
                let document =
                    diesel::RunQueryDsl::first::<Document>(d::documents.find(&id), conn)?;
                let file =
                    diesel::RunQueryDsl::first::<File>(f::files.find(&document.file_id), conn)?;
                crate::services::persistence::authorize(conn, &file, &uid, "admin")?;
                crate::services::persistence::check_file_lock(conn, &file.id, &uid, None)?;
                let now = Utc::now().to_rfc3339();
                diesel::RunQueryDsl::execute(
                    diesel::update(f::files.find(&file.id)).set((
                        f::is_trashed.eq(true),
                        f::trashed_at.eq(Some(&now)),
                        f::updated_at.eq(&now),
                    )),
                    conn,
                )?;
                Ok(())
            })
        })
        .await;
    match result {
        Ok(()) => {
            crate::services::WebhookService::fire_event(
                &state.db,
                "document.deleted",
                &serde_json::json!({"id":deleted_id}),
            )
            .await;
            res.render(Json(ApiResponse::ok(
                serde_json::json!({"message":"Document deleted"}),
            )));
        }
        Err(error) => render_error(res, error),
    }
}

// ---------------------------------------------------------------------------
// GET /documents/:id/content
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_document_content(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param, &uid, "viewer").await
    {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            render_error(res, e);
            return;
        }
    };

    let row = d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
        .select((d::content, d::version))
        .first::<(Option<String>, i32)>(&mut conn)
        .await
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
            render_error(res, AppError::Database(e.to_string()));
        }
    }
}

// ---------------------------------------------------------------------------
// POST /documents/:id/export/:format
// ---------------------------------------------------------------------------

#[handler]
pub async fn export_document(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param, &uid, "viewer").await
    {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            render_error(res, e);
            return;
        }
    };

    let row = d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
        .select((d::doc_type, d::content, f::name, d::file_id, d::version))
        .first::<(String, Option<String>, String, String, i32)>(&mut conn)
        .await
        .optional();

    let (doc_type_str, content_opt, file_name, source_file_id, source_version) = match row {
        Ok(Some(r)) => r,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let prepared = if export_format == ExportFormat::Pdf {
        crate::services::pdf_resources::prepare_document(
            &state.db,
            &state.storage,
            &uid,
            &source_file_id,
            source_version,
            &doc_type_str,
            content_opt.unwrap_or_default(),
        )
        .await
    } else {
        Ok(content_opt.unwrap_or_default())
    };
    let exported = match prepared {
        Ok(content) => {
            super::convert::export_content(content, doc_type_str, export_format, format_str).await
        }
        Err(error) => Err(error),
    };

    match exported {
        Ok(data) => {
            // Sanitize the name before it goes into the header to avoid header
            // injection (CR/LF/quote) and parse panics on odd characters.
            let safe_stem = StorageService::sanitize_file_name(&file_name);
            let out_name = format!("{}.{}", safe_stem, export_format.extension());
            res.status_code(StatusCode::OK);
            if let Ok(ct) = export_format.mime_type().parse() {
                res.headers_mut().insert("content-type", ct);
            }
            if let Ok(cd) = format!("attachment; filename=\"{out_name}\"").parse() {
                res.headers_mut().insert("content-disposition", cd);
            }
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param, &uid, "viewer").await
    {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            render_error(res, e);
            return;
        }
    };

    // Verify ownership.
    let count: i64 = match d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
        .count()
        .get_result(&mut conn)
        .await
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
        .await
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
            render_error(res, AppError::Database(e.to_string()));
        }
    }
}

// ---------------------------------------------------------------------------
// POST /documents/:id/restore/:version_id
// ---------------------------------------------------------------------------

#[handler]
pub async fn restore_version(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let Some(uid) = get_user_id(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let id: String = req.param("id").unwrap_or_default();
    let version_id: String = req.param("version_id").unwrap_or_default();
    #[derive(serde::Deserialize)]
    struct RestoreRequest {
        expected_version: i32,
    }
    let expected = match req.parse_json::<RestoreRequest>().await {
        Ok(body) if body.expected_version > 0 => body.expected_version,
        _ => {
            render_error(
                res,
                AppError::Validation("A positive expected_version is required".into()),
            );
            return;
        }
    };
    let mut conn = state.db.clone();
    if let Err(error) =
        crate::handlers::access::load_accessible_document(&mut conn, &id, &uid, "editor")
            .await
            .and_then(|file| {
                file.filter(|file| !file.is_trashed)
                    .ok_or_else(|| AppError::NotFound("Document not found".into()))
                    .map(|_| ())
            })
    {
        render_error(res, error);
        return;
    }
    let content = match v::versions
        .filter(v::id.eq(&version_id))
        .filter(v::document_id.eq(&id))
        .select(v::content)
        .first::<Option<String>>(&mut conn)
        .await
        .optional()
    {
        Ok(Some(Some(content))) => content,
        Ok(_) => {
            render_error(res, AppError::NotFound("Version content not found".into()));
            return;
        }
        Err(error) => {
            render_error(res, error.into());
            return;
        }
    };
    let snapshot = match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(content) => content,
        Err(error) => {
            render_error(
                res,
                AppError::Internal(format!("Invalid version content: {error}")),
            );
            return;
        }
    };
    match crate::services::persistence::restore_document_with_credential(
        &state.db,
        &state.storage,
        &id,
        &version_id,
        &uid,
        expected,
        content,
        &credential,
    )
    .await
    {
        Ok(version) => {
            if let Ok(document_id) = Uuid::parse_str(&id) {
                state
                    .collab
                    .publish_snapshot(document_id, snapshot, version as u64)
                    .await;
            }
            res.render(Json(ApiResponse::ok(
                serde_json::json!({"version":version,"message":"Version restored"}),
            )));
        }
        Err(error) => render_error(res, error),
    }
}

// ---------------------------------------------------------------------------
// GET /documents/:id/versions/diff?v1={version_id}&v2={version_id}
// ---------------------------------------------------------------------------

#[handler]
pub async fn diff_versions(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param, &uid, "viewer").await
    {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            render_error(res, e);
            return;
        }
    };

    // Verify ownership.
    let count: i64 = d::documents
        .inner_join(f::files.on(d::file_id.eq(f::id)))
        .filter(d::id.eq(&resolved_doc_id))
        .count()
        .get_result(&mut conn)
        .await
        .unwrap_or_default();

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
        .await
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
            render_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    // Fetch version 2.
    let ver2 = v::versions
        .filter(v::id.eq(&v2_id))
        .filter(v::document_id.eq(&resolved_doc_id))
        .select((v::version_number, v::content))
        .first::<(i32, Option<String>)>(&mut conn)
        .await
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
            render_error(res, AppError::Database(e.to_string()));
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
