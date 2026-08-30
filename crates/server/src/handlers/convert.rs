use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use std::sync::Arc;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{ConversionJob, DocumentType, NewConversionJob};
use contrix_core::schema::conversion_jobs::dsl as cj;
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;
use contrix_export::ExportFormat;

use crate::middleware::auth::get_user_id;
use crate::services::{StorageBackend, StorageService};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Attempt to convert a document and update the conversion job record.
async fn perform_conversion(
    conn: &mut diesel::SqliteConnection,
    job_id: &str,
    source_file_id: &str,
    target_format: &str,
    storage: &StorageBackend,
) {
    // Resolve the export format.
    let export_format = match ExportFormat::from_extension(target_format) {
        Some(fmt) => fmt,
        None => {
            let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
                .set((
                    cj::status.eq("failed"),
                    cj::error_message
                        .eq(Some(format!("Unsupported target format: {target_format}"))),
                    cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
                ))
                .execute(conn);
            return;
        }
    };

    // Read the source document content via the documents table.
    let doc_row = d::documents
        .filter(d::file_id.eq(source_file_id))
        .select((d::doc_type, d::content))
        .first::<(String, Option<String>)>(conn)
        .optional();

    let (doc_type_str, content_opt) = match doc_row {
        Ok(Some(r)) => r,
        Ok(None) => {
            let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
                .set((
                    cj::status.eq("failed"),
                    cj::error_message.eq(Some("No document found for source file".to_string())),
                    cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
                ))
                .execute(conn);
            return;
        }
        Err(e) => {
            let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
                .set((
                    cj::status.eq("failed"),
                    cj::error_message.eq(Some(format!("Database error: {e}"))),
                    cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
                ))
                .execute(conn);
            return;
        }
    };

    let content_str = content_opt.unwrap_or_default();
    let doc_type = DocumentType::parse_lossy(&doc_type_str);
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
        (DocumentType::Writer, ExportFormat::Docx) => {
            match serde_json::from_value::<contrix_doc_engine::WriterDocument>(content) {
                Ok(doc) => contrix_export::docx::writer_to_docx(&doc),
                Err(e) => Err(format!("Failed to parse writer document: {e}")),
            }
        }
        (DocumentType::Calc, ExportFormat::Xlsx) => {
            match serde_json::from_value::<contrix_doc_engine::Workbook>(content) {
                Ok(wb) => contrix_export::xlsx::workbook_to_xlsx(&wb),
                Err(e) => Err(format!("Failed to parse workbook: {e}")),
            }
        }
        (DocumentType::Slides, ExportFormat::Pptx) => {
            match serde_json::from_value::<contrix_doc_engine::Presentation>(content) {
                Ok(pres) => contrix_export::pptx::presentation_to_pptx(&pres),
                Err(e) => Err(format!("Failed to parse presentation: {e}")),
            }
        }
        (_, ExportFormat::Json) => Ok(content_str.into_bytes()),
        _ => Err(format!(
            "Export format '{}' not supported for document type '{}'",
            target_format, doc_type_str
        )),
    };

    match exported {
        Ok(data) => {
            let output_filename = format!("{}.{}", job_id, export_format.extension());
            let output_relative = format!("conversions/{}", output_filename);

            if let Err(e) = storage
                .write_file(&output_relative, &data, "application/octet-stream")
                .await
            {
                let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
                    .set((
                        cj::status.eq("failed"),
                        cj::error_message.eq(Some(format!("Failed to write output file: {e}"))),
                        cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
                    ))
                    .execute(conn);
                return;
            }

            let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
                .set((
                    cj::status.eq("completed"),
                    cj::output_path.eq(Some(&output_relative)),
                    cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
                ))
                .execute(conn);

            tracing::info!("Conversion job {job_id} completed: {output_relative}");
        }
        Err(msg) => {
            let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
                .set((
                    cj::status.eq("failed"),
                    cj::error_message.eq(Some(&msg)),
                    cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
                ))
                .execute(conn);

            tracing::warn!("Conversion job {job_id} failed: {msg}");
        }
    }
}

/// Enqueue a persisted conversion row for background execution. The database
/// row is the durable queue: pending jobs can be resumed after a restart.
pub fn enqueue_conversion(
    pool: crate::state::DbPool,
    storage: Arc<StorageBackend>,
    job_id: String,
    source_file_id: String,
    target_format: String,
) {
    tokio::spawn(async move {
        let Ok(mut conn) = pool.get() else {
            tracing::error!(
                job_id,
                "conversion worker could not acquire a database connection"
            );
            return;
        };
        let claimed = diesel::update(
            cj::conversion_jobs
                .filter(cj::id.eq(&job_id))
                .filter(cj::status.eq("pending")),
        )
        .set(cj::status.eq("processing"))
        .execute(&mut conn)
        .unwrap_or(0);
        if claimed == 0 {
            return;
        }
        perform_conversion(
            &mut conn,
            &job_id,
            &source_file_id,
            &target_format,
            &storage,
        )
        .await;
    });
}

/// Resume durable jobs left pending/processing by an interrupted process.
pub fn resume_pending_conversions(pool: &crate::state::DbPool, storage: Arc<StorageBackend>) {
    let Ok(mut conn) = pool.get() else {
        return;
    };
    let jobs = cj::conversion_jobs
        .filter(cj::status.eq_any(["pending", "processing"]))
        .select((cj::id, cj::source_file_id, cj::target_format))
        .load::<(String, String, String)>(&mut conn)
        .unwrap_or_default();
    for (id, source, target) in jobs {
        let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(&id)))
            .set(cj::status.eq("pending"))
            .execute(&mut conn);
        enqueue_conversion(pool.clone(), storage.clone(), id, source, target);
    }
}

// ---------------------------------------------------------------------------
// POST /convert  (Create a conversion job)
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_conversion(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    #[derive(serde::Deserialize)]
    struct CreateConversionRequest {
        file_id: String,
        target_format: String,
    }

    let body: CreateConversionRequest = match req.parse_json().await {
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

    // Verify the source file exists and belongs to the user.
    match f::files
        .filter(f::id.eq(&body.file_id))
        .filter(f::owner_id.eq(&uid))
        .select(f::id)
        .first::<String>(&mut conn)
        .optional()
    {
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Source file not found".into()).to_response(),
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
        Ok(Some(_)) => {}
    }

    let now = Utc::now();
    let job_id = Uuid::new_v4().to_string();

    let new_job = NewConversionJob {
        id: job_id.clone(),
        user_id: uid.clone(),
        source_file_id: body.file_id.clone(),
        target_format: body.target_format.clone(),
        status: "pending".to_string(),
        output_path: None,
        error_message: None,
        created_at: now.to_rfc3339(),
        completed_at: None,
    };

    if let Err(e) = diesel::insert_into(cj::conversion_jobs)
        .values(&new_job)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    drop(conn);
    enqueue_conversion(
        state.db.clone(),
        state.storage.clone(),
        job_id.clone(),
        body.file_id.clone(),
        body.target_format.clone(),
    );

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "id": job_id,
        "user_id": uid,
        "source_file_id": body.file_id,
        "target_format": body.target_format,
        "status": "pending",
        "output_path": null,
        "error_message": null,
        "created_at": now.to_rfc3339(),
        "completed_at": null,
    }))));
}

// ---------------------------------------------------------------------------
// POST /convert/:id/retry
// ---------------------------------------------------------------------------

#[handler]
pub async fn retry_conversion(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let Some(uid) = get_user_id(depot) else {
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Auth("Not authenticated".into()).to_response(),
        )));
        return;
    };
    let job_id: String = req.param("id").unwrap_or_default();
    let mut conn = match state.db.get() {
        Ok(conn) => conn,
        Err(error) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(error.to_string()).to_response(),
            )));
            return;
        }
    };
    let job = cj::conversion_jobs
        .filter(cj::id.eq(&job_id))
        .filter(cj::user_id.eq(&uid))
        .first::<ConversionJob>(&mut conn)
        .optional();
    let Ok(Some(job)) = job else {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Conversion job not found".into()).to_response(),
        )));
        return;
    };
    if !matches!(job.status.as_str(), "failed" | "cancelled") {
        res.status_code(StatusCode::CONFLICT);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Only failed or cancelled conversions can be retried".into())
                .to_response(),
        )));
        return;
    }
    let claimed = diesel::update(
        cj::conversion_jobs
            .filter(cj::id.eq(&job.id))
            .filter(cj::status.eq_any(["failed", "cancelled"])),
    )
    .set((
        cj::status.eq("pending"),
        cj::output_path.eq(None::<String>),
        cj::error_message.eq(None::<String>),
        cj::completed_at.eq(None::<String>),
    ))
    .execute(&mut conn);
    match claimed {
        Ok(1) => {}
        Ok(_) => {
            res.status_code(StatusCode::CONFLICT);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Conversion was already requeued".into()).to_response(),
            )));
            return;
        }
        Err(error) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(error.to_string()).to_response(),
            )));
            return;
        }
    }
    drop(conn);
    enqueue_conversion(
        state.db.clone(),
        state.storage.clone(),
        job.id.clone(),
        job.source_file_id,
        job.target_format,
    );
    res.status_code(StatusCode::ACCEPTED);
    res.render(Json(ApiResponse::ok(
        serde_json::json!({"id":job.id,"status":"pending"}),
    )));
}

// ---------------------------------------------------------------------------
// GET /convert/:id  (Get conversion job status)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_conversion(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let job_id_param: String = req.param("id").unwrap_or_default();

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

    match cj::conversion_jobs
        .filter(cj::id.eq(&job_id_param))
        .filter(cj::user_id.eq(&uid))
        .first::<ConversionJob>(&mut conn)
        .optional()
    {
        Ok(Some(job)) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "id": job.id,
                "user_id": job.user_id,
                "source_file_id": job.source_file_id,
                "target_format": job.target_format,
                "status": job.status,
                "output_path": job.output_path,
                "error_message": job.error_message,
                "created_at": job.created_at,
                "completed_at": job.completed_at,
            }))));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Conversion job not found".into()).to_response(),
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
// GET /convert/:id/download  (Download converted file)
// ---------------------------------------------------------------------------

#[handler]
pub async fn download_conversion(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let job_id_param: String = req.param("id").unwrap_or_default();

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

    let job = match cj::conversion_jobs
        .filter(cj::id.eq(&job_id_param))
        .filter(cj::user_id.eq(&uid))
        .first::<ConversionJob>(&mut conn)
        .optional()
    {
        Ok(Some(j)) => j,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Conversion job not found".into()).to_response(),
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

    if job.status != "completed" {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation(format!(
                "Conversion job is not completed (status: {})",
                job.status
            ))
            .to_response(),
        )));
        return;
    }

    let output_path = match &job.output_path {
        Some(p) => p.clone(),
        None => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Internal("Conversion output path is missing".into()).to_response(),
            )));
            return;
        }
    };

    let data = match state.storage.read_file(&output_path).await {
        Ok(d) => d,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    // `target_format` is user-supplied; sanitize before placing it in a header
    // to prevent header injection / parse panics.
    let file_name = StorageService::sanitize_file_name(&format!("converted.{}", job.target_format));
    res.status_code(StatusCode::OK);
    res.headers_mut()
        .insert("content-type", "application/octet-stream".parse().unwrap());
    if let Ok(cd) = format!("attachment; filename=\"{}\"", file_name).parse() {
        res.headers_mut().insert("content-disposition", cd);
    }
    res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(data)));
}

// ---------------------------------------------------------------------------
// GET /convert  (List user's conversion jobs)
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_conversions(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    match cj::conversion_jobs
        .filter(cj::user_id.eq(&uid))
        .order(cj::created_at.desc())
        .limit(limit_val)
        .offset(offset_val)
        .load::<ConversionJob>(&mut conn)
    {
        Ok(rows) => {
            let jobs: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|job| {
                    serde_json::json!({
                        "id": job.id,
                        "user_id": job.user_id,
                        "source_file_id": job.source_file_id,
                        "target_format": job.target_format,
                        "status": job.status,
                        "output_path": job.output_path,
                        "error_message": job.error_message,
                        "created_at": job.created_at,
                        "completed_at": job.completed_at,
                    })
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(jobs)));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
