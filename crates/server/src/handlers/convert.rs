use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use std::sync::Arc;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{ConversionJob, DocumentType, NewConversionJob};
use contrix_core::schema::conversion_jobs::dsl as cj;
use contrix_core::schema::documents::dsl as d;
use contrix_export::ExportFormat;

use crate::handlers::access::load_accessible_file;
use crate::middleware::auth::get_user_id;
use crate::services::{StorageBackend, StorageService};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn require_source_access(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    file_id: &str,
    user_id: &str,
) -> Result<(), AppError> {
    match load_accessible_file(conn, file_id, user_id, "viewer").await? {
        Some(file) if !file.is_trashed && !file.is_folder => Ok(()),
        _ => Err(AppError::NotFound("Source file not found".into())),
    }
}
fn render_source_error(res: &mut Response, error: AppError) {
    res.status_code(match &error {
        AppError::Forbidden(_) => StatusCode::FORBIDDEN,
        AppError::NotFound(_) => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    });
    res.render(Json(ApiResponse::<()>::err(error.to_response())));
}

// ZIP generation and document parsing are CPU work, independent of a SQL lease.
// Keep the permit in the worker so cancelling a caller cannot exceed the bound.
static EXPORT_WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

// Background jobs have no HTTP caller to retry temporary pool backpressure.
// Each attempt releases its lease before waiting; permanent SQL errors stop.
async fn retry_database<T, F, Fut>(mut query: F) -> horme::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = horme::Result<T>>,
{
    for attempt in 0..4 {
        match query().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let transient = match &error {
                    horme::Error::Acquire(_) => true,
                    horme::Error::Query(diesel::result::Error::QueryBuilderError(_)) => true,
                    horme::Error::Query(diesel::result::Error::DatabaseError(_, info)) => {
                        let message = info.message().to_ascii_lowercase();
                        message.contains("locked") || message.contains("busy")
                    }
                    _ => false,
                };
                if !transient || attempt == 3 {
                    return Err(error);
                }
                tokio::time::sleep(std::time::Duration::from_millis(100 << attempt)).await;
            }
        }
    }
    unreachable!()
}

pub(super) async fn export_content(
    content_str: String,
    doc_type_str: String,
    export_format: ExportFormat,
    target_format: String,
) -> Result<Vec<u8>, String> {
    let permit = EXPORT_WORKERS
        .acquire()
        .await
        .map_err(|_| "Conversion worker unavailable".to_string())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let doc_type = DocumentType::parse_lossy(&doc_type_str);
        let content: serde_json::Value = serde_json::from_str(&content_str).unwrap_or_default();

        // Perform the export based on doc type and format.
        match (&doc_type, &export_format) {
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
                    Ok(mut wb) => {
                        for sheet in &mut wb.sheets { sheet.recalculate(); }
                        if let Some(sheet) = wb.sheets.first() {
                            let rows = sheet.cells.keys().map(|r| u64::from(r.row) + 1).max().unwrap_or(1);
                            let cols = sheet.cells.keys().map(|r| u64::from(r.col) + 1).max().unwrap_or(1);
                            if rows.checked_mul(cols).is_none_or(|cells| cells > 5_000_000) {
                                return Err("CSV output exceeds dense cell budget; use XLSX for sparse workbooks".into());
                            }
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
                    Ok(mut wb) => {
                        for sheet in &mut wb.sheets {sheet.recalculate();}
                        contrix_export::xlsx::workbook_to_xlsx(&wb)
                    },
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
        }
    })
    .await
    .map_err(|_| "Conversion worker failed".to_string())?
}

#[cfg(test)]
mod conversion_worker_tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn worker_exports_writer_content_to_html_and_native_package() {
        let mut document = contrix_doc_engine::WriterDocument::new("Worker export".into());
        document.add_paragraph("Off-thread conversion content");
        let content = serde_json::to_string(&document).unwrap();
        let html = export_content(
            content.clone(),
            "writer".into(),
            ExportFormat::Html,
            "html".into(),
        )
        .await
        .unwrap();
        assert!(
            String::from_utf8(html)
                .unwrap()
                .contains("Off-thread conversion content")
        );
        let package = export_content(content, "writer".into(), ExportFormat::Docx, "docx".into())
            .await
            .unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(package)).unwrap();
        assert!(archive.by_name("word/document.xml").is_ok());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn worker_keeps_invalid_document_failure_and_json_bytes() {
        let invalid = export_content(
            "{}".into(),
            "writer".into(),
            ExportFormat::Docx,
            "docx".into(),
        )
        .await
        .unwrap_err();
        assert!(invalid.starts_with("Failed to parse writer document:"));
        let source = " {\"preserve\":true} ";
        assert_eq!(
            export_content(
                source.into(),
                "calc".into(),
                ExportFormat::Json,
                "json".into()
            )
            .await
            .unwrap(),
            source.as_bytes()
        );
    }
}

/// Attempt to convert a document and update the conversion job record.
async fn perform_conversion(
    conn: &mut crate::state::DbPool,
    job_id: &str,
    source_file_id: &str,
    target_format: &str,
    storage: &StorageBackend,
) {
    let author = cj::conversion_jobs
        .filter(cj::id.eq(job_id))
        .select(cj::user_id)
        .first::<String>(conn)
        .await;
    let permitted = match author {
        Ok(author) => require_source_access(conn, source_file_id, &author).await,
        Err(error) => Err(AppError::Database(error.to_string())),
    };
    if let Err(error) = permitted {
        let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
            .set((
                cj::status.eq("failed"),
                cj::error_message.eq(Some(error.to_string())),
                cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
            ))
            .execute(conn)
            .await;
        return;
    }
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
                .execute(conn)
                .await;
            return;
        }
    };

    // Read the source document content via the documents table.
    let database = conn.clone();
    let doc_row = retry_database(|| {
        let mut conn = database.clone();
        async move {
            d::documents
                .filter(d::file_id.eq(source_file_id))
                .select((d::doc_type, d::content))
                .first::<(String, Option<String>)>(&mut conn)
                .await
        }
    })
    .await
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
                .execute(conn)
                .await;
            return;
        }
        Err(e) => {
            let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
                .set((
                    cj::status.eq("failed"),
                    cj::error_message.eq(Some(format!("Database error: {e}"))),
                    cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
                ))
                .execute(conn)
                .await;
            return;
        }
    };

    let exported = export_content(
        content_opt.unwrap_or_default(),
        doc_type_str,
        export_format,
        target_format.to_owned(),
    )
    .await;

    match exported {
        Ok(data) => {
            let output_filename = format!("{}.{}", job_id, export_format.extension());
            let output_relative = format!("conversions/{}/{output_filename}", Uuid::new_v4());

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
                    .execute(conn)
                    .await;
                return;
            }

            let output_path = output_relative.as_str();
            let completed = retry_database(|| {
                let mut conn = database.clone();
                async move {
                    diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
                        .set((
                            cj::status.eq("completed"),
                            cj::output_path.eq(Some(output_path)),
                            cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
                        ))
                        .execute(&mut conn)
                        .await
                }
            })
            .await;
            match completed {
                Ok(_) => tracing::info!("Conversion job {job_id} completed: {output_relative}"),
                Err(error) => {
                    tracing::error!(%job_id, %error, "Could not persist conversion result; durable job remains resumable")
                }
            }
        }
        Err(msg) => {
            let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(job_id)))
                .set((
                    cj::status.eq("failed"),
                    cj::error_message.eq(Some(&msg)),
                    cj::completed_at.eq(Some(Utc::now().to_rfc3339())),
                ))
                .execute(conn)
                .await;

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
        let claimed = retry_database(|| {
            let mut conn = pool.clone();
            let job_id = job_id.as_str();
            async move {
                diesel::update(
                    cj::conversion_jobs
                        .filter(cj::id.eq(job_id))
                        .filter(cj::status.eq("pending")),
                )
                .set(cj::status.eq("processing"))
                .execute(&mut conn)
                .await
            }
        })
        .await;
        let claimed = match claimed {
            Ok(count) => count,
            Err(error) => {
                tracing::error!(%job_id, %error, "Could not claim durable conversion job");
                return;
            }
        };
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
pub async fn resume_pending_conversions(pool: &crate::state::DbPool, storage: Arc<StorageBackend>) {
    let Ok(mut conn) = pool.get() else {
        return;
    };
    let jobs = cj::conversion_jobs
        .filter(cj::status.eq_any(["pending", "processing"]))
        .select((cj::id, cj::source_file_id, cj::target_format))
        .load::<(String, String, String)>(&mut conn)
        .await
        .unwrap_or_default();
    for (id, source, target) in jobs {
        let _ = diesel::update(cj::conversion_jobs.filter(cj::id.eq(&id)))
            .set(cj::status.eq("pending"))
            .execute(&mut conn)
            .await;
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

    if let Err(error) = require_source_access(&mut conn, &body.file_id, &uid).await {
        render_source_error(res, error);
        return;
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
        .await
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
        .await
        .optional();
    let Ok(Some(job)) = job else {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Conversion job not found".into()).to_response(),
        )));
        return;
    };
    if let Err(error) = require_source_access(&mut conn, &job.source_file_id, &uid).await {
        render_source_error(res, error);
        return;
    }
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
    .execute(&mut conn)
    .await;
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
        .await
        .optional()
    {
        Ok(Some(job)) => {
            if let Err(error) = require_source_access(&mut conn, &job.source_file_id, &uid).await {
                render_source_error(res, error);
                return;
            }

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
        .await
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

    if let Err(error) = require_source_access(&mut conn, &job.source_file_id, &uid).await {
        render_source_error(res, error);
        return;
    }
    drop(conn);
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
        .await
    {
        Ok(rows) => {
            let mut accessible = Vec::new();
            for job in rows {
                match require_source_access(&mut conn, &job.source_file_id, &uid).await {
                    Ok(()) => accessible.push(job),
                    Err(AppError::Forbidden(_) | AppError::NotFound(_)) => {}
                    Err(error) => {
                        render_source_error(res, error);
                        return;
                    }
                }
            }
            let jobs: Vec<serde_json::Value> = accessible
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

#[cfg(test)]
mod review_conversion_regressions {
    use super::*;
    #[tokio::test]
    async fn csv_exports_formula_results_after_recalculation() {
        let mut book = contrix_doc_engine::Workbook::new("CSV".into());
        book.sheets[0].set_cell(0, 0, contrix_doc_engine::calc::CellValue::Number(1.0));
        book.sheets[0].set_formula(0, 1, "=A1+2".into());
        let bytes = export_content(
            serde_json::to_string(&book).unwrap(),
            "calc".into(),
            ExportFormat::Csv,
            "csv".into(),
        )
        .await
        .unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), "1,3\r\n");
    }
    #[tokio::test]
    async fn csv_refuses_unbounded_dense_output_but_xlsx_stays_sparse() {
        let mut book = contrix_doc_engine::Workbook::new("Sparse".into());
        book.sheets[0].set_cell(
            1_048_575,
            16_383,
            contrix_doc_engine::calc::CellValue::Number(1.0),
        );
        let content = serde_json::to_string(&book).unwrap();
        assert!(
            export_content(
                content.clone(),
                "calc".into(),
                ExportFormat::Csv,
                "csv".into()
            )
            .await
            .unwrap_err()
            .contains("budget")
        );
        assert!(
            export_content(content, "calc".into(), ExportFormat::Xlsx, "xlsx".into())
                .await
                .is_ok()
        );
    }
}

#[cfg(test)]
mod imported_download_export_regressions {
    use super::*;
    #[tokio::test]
    async fn imported_models_export_latest_values_as_standard_office_packages() {
        use contrix_doc_engine::calc::CellValue;
        let mut workbook = contrix_export::xlsx::xlsx_to_workbook(include_bytes!(
            "../../../export/tests/fixtures/external-formulas.xlsx"
        ))
        .unwrap();
        workbook.sheets[0].set_cell(0, 0, CellValue::Number(20.0));
        let bytes = export_content(
            serde_json::to_string(&workbook).unwrap(),
            "calc".into(),
            ExportFormat::Xlsx,
            "xlsx".into(),
        )
        .await
        .unwrap();
        assert_eq!(&bytes[..2], b"PK");
        let parsed = contrix_export::xlsx::xlsx_to_workbook(&bytes).unwrap();
        assert!(matches!(
            parsed.sheets[0].get_cell(0, 1).unwrap().value,
            CellValue::Number(21.0)
        ));
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("xl/worksheets/sheet1.xml").unwrap(),
            &mut xml,
        )
        .unwrap();
        assert!(xml.contains("<f>A1+1</f>"));
        assert!(xml.contains("<v>21</v>"));

        let mut slides = contrix_export::pptx::pptx_to_presentation(include_bytes!(
            "../../../export/tests/fixtures/external-slide-order.pptx"
        ))
        .unwrap();
        if let contrix_doc_engine::slides::SlideElement::TextBox(text) =
            &mut slides.slides[0].elements[0]
        {
            text.content[0].text = "Edited imported slide".into();
        }
        let bytes = export_content(
            serde_json::to_string(&slides).unwrap(),
            "slides".into(),
            ExportFormat::Pptx,
            "pptx".into(),
        )
        .await
        .unwrap();
        assert_eq!(&bytes[..2], b"PK");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let mut xml = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("ppt/slides/slide1.xml").unwrap(),
            &mut xml,
        )
        .unwrap();
        assert!(xml.contains("Edited imported slide"));

        let mut writer = contrix_doc_engine::WriterDocument::new("Imported Writer".into());
        writer.add_paragraph("Original paragraph");
        let original = contrix_export::docx::writer_to_docx(&writer).unwrap();
        let mut imported = contrix_export::docx::docx_to_writer(&original).unwrap();
        imported.add_paragraph("Edited imported paragraph");
        let bytes = export_content(
            serde_json::to_string(&imported).unwrap(),
            "writer".into(),
            ExportFormat::Docx,
            "docx".into(),
        )
        .await
        .unwrap();
        assert_eq!(&bytes[..2], b"PK");
        let parsed = contrix_export::docx::docx_to_writer(&bytes).unwrap();
        assert!(
            contrix_doc_engine::writer::writer_content_to_plain(
                &serde_json::to_value(parsed).unwrap()
            )
            .contains("Edited imported paragraph")
        );
    }
}
