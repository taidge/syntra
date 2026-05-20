use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{ConversionJob, DocumentType, NewConversionJob};
use contrix_core::schema::conversion_jobs::dsl as cj;
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;
use contrix_export::ExportFormat;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Attempt to convert a document and update the conversion job record.
fn perform_conversion(
    conn: &mut diesel::SqliteConnection,
    job_id: &str,
    source_file_id: &str,
    target_format: &str,
    base_path: &str,
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
            "Export format '{}' not supported for document type '{}'",
            target_format, doc_type_str
        )),
    };

    match exported {
        Ok(data) => {
            // Save output to data/files/conversions/{job_id}.{format}
            let conversions_dir = std::path::Path::new(base_path).join("conversions");
            if let Err(e) = std::fs::create_dir_all(&conversions_dir) {
                tracing::error!("Failed to create conversions directory: {e}");
                let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
                    .set((
                        cj::status.eq("failed"),
                        cj::error_message
                            .eq(Some(format!("Failed to create output directory: {e}"))),
                        cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
                    ))
                    .execute(conn);
                return;
            }

            let output_filename = format!("{}.{}", job_id, export_format.extension());
            let output_full_path = conversions_dir.join(&output_filename);
            let output_relative = format!("conversions/{}", output_filename);

            if let Err(e) = std::fs::write(&output_full_path, &data) {
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

// ---------------------------------------------------------------------------
// POST /convert  (Create a conversion job)
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_conversion(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    // Attempt conversion immediately.
    perform_conversion(
        &mut conn,
        &job_id,
        &body.file_id,
        &body.target_format,
        &state.config.storage.base_path,
    );

    // Re-read the job to return the up-to-date status.
    let job = cj::conversion_jobs
        .filter(cj::id.eq(&job_id))
        .first::<ConversionJob>(&mut conn)
        .ok();

    let (status, output_path, error_message, completed_at) = match &job {
        Some(j) => (
            j.status.as_str(),
            j.output_path.as_deref(),
            j.error_message.as_deref(),
            j.completed_at.as_deref(),
        ),
        None => ("pending", None, None, None),
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "id": job_id,
        "user_id": uid,
        "source_file_id": body.file_id,
        "target_format": body.target_format,
        "status": status,
        "output_path": output_path,
        "error_message": error_message,
        "created_at": now.to_rfc3339(),
        "completed_at": completed_at,
    }))));
}

// ---------------------------------------------------------------------------
// GET /convert/:id  (Get conversion job status)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_conversion(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let storage = crate::services::StorageService::new(&state.config.storage.base_path);
    let data = match storage.read_file(&output_path) {
        Ok(d) => d,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let file_name = format!("converted.{}", job.target_format);
    res.status_code(StatusCode::OK);
    res.headers_mut()
        .insert("content-type", "application/octet-stream".parse().unwrap());
    res.headers_mut().insert(
        "content-disposition",
        format!("attachment; filename=\"{}\"", file_name)
            .parse()
            .unwrap(),
    );
    res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(data)));
}

// ---------------------------------------------------------------------------
// GET /convert  (List user's conversion jobs)
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_conversions(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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
