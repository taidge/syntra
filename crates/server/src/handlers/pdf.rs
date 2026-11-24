//! Authenticated PDF viewing, immutable saves, history restoration and editable copies.

use std::sync::{Arc, LazyLock};

use chrono::Utc;
use diesel::prelude::*;
use horme::{OptionalExtension, RunQueryDsl};
use salvo::prelude::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use syntra_core::dto::ApiResponse;
use syntra_core::error::AppError;
use syntra_core::models::{DocumentType, File, FileVersion, NewDocument, NewFile, NewFileVersion};
use syntra_core::schema::{documents::dsl as d, file_versions::dsl as fv, files::dsl as f};
use syntra_export::pdf_import::{MAX_PDF_SOURCE_BYTES, PdfInfo};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::handlers::access::load_accessible_file;
use crate::middleware::auth::{AuthenticatedCredential, get_credential, get_user_id};
use crate::services::persistence;
use crate::state::AppState;

static CAPACITY: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

fn error_response(res: &mut Response, error: AppError) {
    res.status_code(
        StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
    );
    res.render(Json(ApiResponse::<()>::err(error.to_response())));
}

fn user(depot: &Depot) -> Result<String, AppError> {
    get_user_id(depot).ok_or_else(|| AppError::Auth("Not authenticated".into()))
}

fn capacity() -> Result<Arc<OwnedSemaphorePermit>, AppError> {
    CAPACITY
        .clone()
        .try_acquire_owned()
        .map(Arc::new)
        .map_err(|_| AppError::RateLimit("PDF processing is busy; retry shortly".into()))
}

fn is_pdf(file: &File) -> bool {
    file.mime_type.as_deref().is_some_and(|mime| {
        mime.split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/pdf"))
    }) || std::path::Path::new(&file.name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

async fn source(state: &AppState, id: &str, actor: &str, role: &str) -> Result<File, AppError> {
    load_accessible_file(&mut state.db.clone(), id, actor, role)
        .await?
        .filter(|file| !file.is_folder && !file.is_trashed && is_pdf(file))
        .ok_or_else(|| AppError::NotFound("PDF file not found".into()))
}

async fn source_bytes(state: &AppState, path: &str, size: i64) -> Result<Arc<Vec<u8>>, AppError> {
    if !(0..=MAX_PDF_SOURCE_BYTES as i64).contains(&size) {
        return Err(AppError::QuotaExceeded(
            "PDF exceeds the 32 MiB processing limit".into(),
        ));
    }
    let bytes = state.storage.read_file(path).await?;
    if bytes.len() > MAX_PDF_SOURCE_BYTES {
        return Err(AppError::QuotaExceeded(
            "PDF exceeds the 32 MiB processing limit".into(),
        ));
    }
    Ok(Arc::new(bytes))
}

pub(super) fn etag(bytes: &[u8]) -> String {
    format!("\"{:x}\"", Sha256::digest(bytes))
}

fn expected_etag(value: Option<&str>) -> Result<String, AppError> {
    let value = value.unwrap_or_default().trim();
    if value.len() == 66
        && value.starts_with('"')
        && value.ends_with('"')
        && value.as_bytes()[1..65]
            .iter()
            .all(|value| value.is_ascii_hexdigit())
    {
        Ok(value.to_owned())
    } else {
        Err(AppError::Validation(
            "A strong current PDF ETag is required; reload the PDF before saving".into(),
        ))
    }
}

async fn inspect(
    bytes: Arc<Vec<u8>>,
    permit: Arc<OwnedSemaphorePermit>,
) -> Result<PdfInfo, AppError> {
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        syntra_export::pdf_import::validate_pdf(&bytes).map_err(AppError::Validation)
    })
    .await
    .map_err(|_| AppError::Validation("PDF could not be processed".into()))?
}

async fn body(
    req: &mut Request,
    res: &mut Response,
    state: &AppState,
    limit: usize,
) -> Result<Vec<u8>, AppError> {
    let retained = super::request_body::RetainedBody::install_with_timeouts(
        req,
        std::time::Duration::from_secs(state.config.storage.upload_idle_timeout_seconds),
        std::time::Duration::from_secs(state.config.storage.upload_total_timeout_seconds),
    );
    req.set_secure_max_size(limit);
    match req.payload().await {
        Ok(bytes) => Ok(bytes.to_vec()),
        Err(error) => {
            let oversized = super::request_body::exceeds_limit(&error);
            let timed_out = retained.timed_out();
            let drained = retained
                .drain(
                    limit.saturating_add(1024 * 1024),
                    std::time::Duration::from_secs(2),
                )
                .await;
            super::request_body::close_if_incomplete(req, res, drained);
            Err(if timed_out {
                AppError::Timeout("Reading PDF data exceeded its configured deadline".into())
            } else if oversized {
                AppError::QuotaExceeded("PDF request exceeds the processing-size limit".into())
            } else {
                AppError::Validation(format!("Invalid PDF request body: {error}"))
            })
        }
    }
}

/// GET /files/{id}/pdf[?version=0] -- the same authorization applies to history.
#[handler]
pub async fn get_pdf(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let result = async {
        let actor = user(depot)?;
        let id = req.param::<String>("id").unwrap_or_default();
        let file = source(&state, &id, &actor, "viewer").await?;
        let permit = capacity()?;
        let mut conn = state.db.clone();
        let latest = fv::file_versions
            .filter(fv::file_id.eq(&id))
            .select(diesel::dsl::max(fv::version_number))
            .first::<Option<i32>>(&mut conn)
            .await?;
        let requested = req
            .query::<String>("version")
            .map(|value| {
                value
                    .parse::<i32>()
                    .map_err(|_| AppError::Validation("Invalid PDF version".into()))
            })
            .transpose()?;
        let (path, size, version_number) = if let Some(version) = requested {
            if version < 0 {
                return Err(AppError::Validation(
                    "PDF version cannot be negative".into(),
                ));
            }
            match fv::file_versions
                .filter(fv::file_id.eq(&id))
                .filter(fv::version_number.eq(version))
                .first::<FileVersion>(&mut conn)
                .await
                .optional()?
            {
                Some(version) => (version.path, version.size, version.version_number),
                None if version == 0 && latest.is_none() => (file.path.clone(), file.size, 0),
                None => return Err(AppError::NotFound("PDF version not found".into())),
            }
        } else {
            (file.path.clone(), file.size, latest.unwrap_or(0))
        };
        let bytes = source_bytes(&state, &path, size).await?;
        let info = inspect(bytes.clone(), permit).await?;
        let can_edit = !info.encrypted && source(&state, &id, &actor, "editor").await.is_ok();
        Ok::<_, AppError>((bytes, info, can_edit, version_number))
    }
    .await;
    match result {
        Ok((bytes, info, can_edit, version)) => {
            res.status_code(StatusCode::OK);
            for (name, value) in [
                ("content-type", "application/pdf".to_owned()),
                ("content-disposition", "inline".to_owned()),
                ("cache-control", "private, no-store".to_owned()),
                ("x-content-type-options", "nosniff".to_owned()),
                ("etag", etag(&bytes)),
                ("x-pdf-can-edit", can_edit.to_string()),
                ("x-pdf-encrypted", info.encrypted.to_string()),
                ("x-pdf-version", version.to_string()),
                ("x-pdf-page-count", info.page_count.to_string()),
            ] {
                if let Ok(value) = value.parse() {
                    res.headers_mut().insert(name, value);
                }
            }
            res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(
                (*bytes).clone(),
            )));
        }
        Err(error) => error_response(res, error),
    }
}

async fn commit_pdf(
    state: &AppState,
    actor: String,
    credential: AuthenticatedCredential,
    request_path: String,
    request_method: salvo::http::Method,
    original: File,
    bytes: Arc<Vec<u8>>,
    permit: Arc<OwnedSemaphorePermit>,
) -> Result<serde_json::Value, AppError> {
    let owner: Uuid = original
        .owner_id
        .parse()
        .map_err(|_| AppError::Validation("Invalid file owner".into()))?;
    let size = bytes.len() as i64;
    let actor_for_quota = actor.clone();
    let original_for_quota = original.clone();
    state
        .db
        .run(move |conn| {
            persistence::authorize(conn, &original_for_quota, &actor_for_quota, "editor")?;
            persistence::check_staging_quota(conn, &original_for_quota.owner_id, size)
        })
        .await?;
    let new_etag = etag(&bytes);
    let staged = persistence::stage_blob(
        &state.db,
        state.storage.clone(),
        owner,
        original.name.clone(),
        bytes,
        permit.clone(),
    )
    .await?;
    let committed_path = staged.path.clone();
    let result = staged.commit(move |conn| {
        let _permit = permit;
        conn.immediate_transaction::<_, AppError, _>(|conn| {
            crate::middleware::auth::validate_credential_in_transaction(conn, &credential, &request_method, &request_path)?;
            let current = diesel::RunQueryDsl::first::<File>(f::files.find(&original.id), conn)?;
            persistence::authorize(conn, &current, &actor, "editor")?;
            persistence::check_file_lock(conn, &current.id, &actor, None)?;
            if current.is_folder || !is_pdf(&current) { return Err(AppError::Validation("The file is no longer a PDF".into())); }
            if current.path != original.path || current.updated_at != original.updated_at {
                return Err(AppError::Conflict("PDF changed; reload before saving. Your local changes have not been applied.".into()));
            }
            let linked = diesel::RunQueryDsl::first::<i64>(d::documents.filter(d::file_id.eq(&current.id)).count(), conn)?;
            if linked != 0 { return Err(AppError::Validation("Native document content cannot be replaced with PDF bytes".into())); }
            let maximum = diesel::RunQueryDsl::first::<Option<i32>>(fv::file_versions.filter(fv::file_id.eq(&current.id)).select(diesel::dsl::max(fv::version_number)), conn)?;
            let now = Utc::now().to_rfc3339();
            if maximum.is_none() {
                let original_version = NewFileVersion { id:Uuid::new_v4().to_string(),file_id:current.id.clone(),version_number:0,path:current.path.clone(),size:current.size,created_by:actor.clone(),created_at:current.created_at.clone() };
                diesel::RunQueryDsl::execute(diesel::insert_into(fv::file_versions).values(&original_version),conn)?;
            }
            let next = maximum.unwrap_or(0).checked_add(1).ok_or_else(||AppError::Conflict("PDF version exhausted".into()))?;
            let version = NewFileVersion { id:Uuid::new_v4().to_string(),file_id:current.id.clone(),version_number:next,path:committed_path.clone(),size,created_by:actor,created_at:now.clone() };
            diesel::RunQueryDsl::execute(diesel::insert_into(fv::file_versions).values(&version),conn)?;
            persistence::replace_file_quota(conn,&current.owner_id,&current.path,current.size,size)?;
            diesel::RunQueryDsl::execute(diesel::update(f::files.find(&current.id)).set((f::path.eq(&committed_path),f::size.eq(size),f::mime_type.eq(Some("application/pdf")),f::updated_at.eq(&now))),conn)?;
            Ok(serde_json::json!({"file_id":current.id,"etag":new_etag,"version_number":next,"size":size,"updated_at":now}))
        })
    }).await;
    persistence::request_gc(&state.db, &state.storage);
    result
}

/// PUT /files/{id}/pdf -- a bounded binary replacement, never a mutable blob overwrite.
#[handler]
pub async fn save_pdf(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let result = async {
        let actor = user(depot)?;
        let credential =
            get_credential(depot).ok_or_else(|| AppError::Auth("Not authenticated".into()))?;
        let id = req.param::<String>("id").unwrap_or_default();
        let original = source(&state, &id, &actor, "editor").await?;
        let expected = expected_etag(
            req.headers()
                .get("if-match")
                .and_then(|header| header.to_str().ok()),
        )?;
        let permit = capacity()?;
        let current_bytes = source_bytes(&state, &original.path, original.size).await?;
        if etag(&current_bytes) != expected {
            return Err(AppError::Conflict(
                "PDF changed; reload before saving. Your local changes have not been applied."
                    .into(),
            ));
        }
        drop(current_bytes);
        if !req
            .headers()
            .get("content-type")
            .and_then(|header| header.to_str().ok())
            .is_some_and(|content_type| {
                content_type
                    .split(';')
                    .next()
                    .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/pdf"))
            })
        {
            return Err(AppError::Validation(
                "PDF saves require application/pdf content".into(),
            ));
        }
        let configured_limit = state
            .config
            .storage
            .max_upload_size_mb
            .saturating_mul(1024 * 1024);
        let bytes = Arc::new(
            body(
                req,
                res,
                &state,
                MAX_PDF_SOURCE_BYTES.min(usize::try_from(configured_limit).unwrap_or(usize::MAX)),
            )
            .await?,
        );
        let info = inspect(bytes.clone(), permit.clone()).await?;
        if info.encrypted {
            return Err(AppError::Validation(
                "Unlock the PDF before saving an edited copy".into(),
            ));
        }
        commit_pdf(
            &state,
            actor,
            credential,
            req.uri().path().to_owned(),
            salvo::http::Method::PUT,
            original,
            bytes,
            permit,
        )
        .await
    }
    .await;
    match result {
        Ok(result) => res.render(Json(ApiResponse::ok(result))),
        Err(error) => error_response(res, error),
    }
}

#[derive(Deserialize)]
struct RestoreRequest {
    expected_etag: String,
    version_number: i32,
}

/// POST /files/{id}/pdf/restore -- restoration appends history rather than deleting it.
#[handler]
pub async fn restore_pdf(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let result = async {
        let actor = user(depot)?;
        let credential =
            get_credential(depot).ok_or_else(|| AppError::Auth("Not authenticated".into()))?;
        let id = req.param::<String>("id").unwrap_or_default();
        let original = source(&state, &id, &actor, "editor").await?;
        let permit = capacity()?;
        let payload = body(req, res, &state, 64 * 1024).await?;
        let request: RestoreRequest = serde_json::from_slice(&payload).map_err(|error| {
            AppError::Validation(format!("Invalid restoration request: {error}"))
        })?;
        let expected = expected_etag(Some(&request.expected_etag))?;
        if request.version_number < 0 {
            return Err(AppError::Validation(
                "PDF version cannot be negative".into(),
            ));
        }
        let current = source_bytes(&state, &original.path, original.size).await?;
        if etag(&current) != expected {
            return Err(AppError::Conflict(
                "PDF changed; reload before restoring a version".into(),
            ));
        }
        let version = fv::file_versions
            .filter(fv::file_id.eq(&id))
            .filter(fv::version_number.eq(request.version_number))
            .first::<FileVersion>(&mut state.db.clone())
            .await
            .optional()?
            .ok_or_else(|| AppError::NotFound("PDF version not found".into()))?;
        let bytes = source_bytes(&state, &version.path, version.size).await?;
        inspect(bytes.clone(), permit.clone()).await?;
        commit_pdf(
            &state,
            actor,
            credential,
            req.uri().path().to_owned(),
            salvo::http::Method::POST,
            original,
            bytes,
            permit,
        )
        .await
    }
    .await;
    match result {
        Ok(result) => res.render(Json(ApiResponse::ok(result))),
        Err(error) => error_response(res, error),
    }
}

#[derive(Deserialize)]
struct ConvertRequest {
    expected_etag: String,
    name: Option<String>,
    #[serde(default)]
    ocr_pages: Option<Vec<String>>,
}

/// POST /files/{id}/pdf/convert -- a separate editable Writer, leaving the PDF unchanged.
#[handler]
pub async fn convert_pdf(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let result = async {
        let actor = user(depot)?;
        let credential = get_credential(depot).ok_or_else(|| AppError::Auth("Not authenticated".into()))?;
        let id = req.param::<String>("id").unwrap_or_default();
        let original = source(&state,&id,&actor,"viewer").await?;
        let permit=capacity()?;
        let payload=body(req,res,&state,syntra_doc_engine::MAX_DOCUMENT_CONTENT_BYTES + 64 * 1024).await?;
        let request: ConvertRequest=serde_json::from_slice(&payload).map_err(|error|AppError::Validation(format!("Invalid conversion request: {error}")))?;
        let expected=expected_etag(Some(&request.expected_etag))?;
        let bytes=source_bytes(&state,&original.path,original.size).await?;
        if etag(&bytes)!=expected { return Err(AppError::Conflict("PDF changed; reload it before conversion".into())); }
        let title=request.name.unwrap_or_else(||format!("{} (editable)",std::path::Path::new(&original.name).file_stem().and_then(|stem|stem.to_str()).unwrap_or("PDF")));
        let title=title.trim().to_owned();
        if title.is_empty() || title.chars().count()>200 { return Err(AppError::Validation("Converted document name must contain 1 to 200 characters".into())); }
        let native_title=title.clone();
        let native_permit=permit.clone();
        let imported=tokio::task::spawn_blocking(move || {
            let _permit=native_permit;
            match request.ocr_pages {
                Some(pages)=>syntra_export::pdf_import::pdf_to_writer_with_pages(&bytes,&native_title,pages),
                None=>syntra_export::pdf_import::pdf_to_writer(&bytes,&native_title),
            }
        }).await.map_err(|_|AppError::Validation("PDF conversion could not be completed".into()))?.map_err(AppError::Validation)?;
        let warnings=imported.warnings;
        let page_count=imported.page_count;
        let content=serde_json::to_string(&imported.document).map_err(|error|AppError::Internal(error.to_string()))?;
        let size=content.len() as i64;
        let owner=actor.parse().map_err(|_|AppError::Validation("Invalid file owner".into()))?;
        let quota_actor=actor.clone();
        state.db.run(move |conn| persistence::check_staging_quota(conn,&quota_actor,size)).await?;
        let staged=persistence::stage_blob(&state.db,state.storage.clone(),owner,"pdf-converted.syntra.json".into(),Arc::new(content.as_bytes().to_vec()),permit.clone()).await?;
        let file_id=Uuid::new_v4().to_string();
        let document_id=Uuid::new_v4().to_string();
        let now=Utc::now().to_rfc3339();
        let new_file=NewFile { id:file_id.clone(),name:title,path:staged.path.clone(),mime_type:Some(DocumentType::Writer.native_mime_type().into()),size,owner_id:actor.clone(),parent_id:None,is_folder:false,is_trashed:false,trashed_at:None,created_at:now.clone(),updated_at:now.clone(),room_id:None,thumbnail_path:None,sort_order:0 };
        let document=NewDocument { id:document_id.clone(),file_id:file_id.clone(),doc_type:"writer".into(),content:Some(content),version:1,created_at:now.clone(),updated_at:now };
        let request_path=req.uri().path().to_owned();
        staged.commit(move |conn| {
            let _permit=permit;
            conn.immediate_transaction::<_,AppError,_>(|conn| {
                crate::middleware::auth::validate_credential_in_transaction(conn,&credential,&salvo::http::Method::POST,&request_path)?;
                let current=diesel::RunQueryDsl::first::<File>(f::files.find(&original.id),conn)?;
                persistence::authorize(conn,&current,&actor,"viewer")?;
                if !is_pdf(&current) || current.is_folder || current.path!=original.path || current.updated_at!=original.updated_at { return Err(AppError::Conflict("PDF changed during conversion; reload before retrying".into())); }
                persistence::adjust_quota(conn,&actor,size)?;
                diesel::RunQueryDsl::execute(diesel::insert_into(f::files).values(&new_file),conn)?;
                diesel::RunQueryDsl::execute(diesel::insert_into(d::documents).values(&document),conn)?;
                Ok(serde_json::json!({"document_id":document_id,"file_id":file_id,"warnings":warnings,"page_count":page_count}))
            })
        }).await
    }.await;
    match result {
        Ok(result) => {
            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(result)));
        }
        Err(error) => error_response(res, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compare_and_swap_requires_a_single_strong_content_hash() {
        assert!(expected_etag(None).is_err());
        assert!(expected_etag(Some("*")).is_err());
        assert!(expected_etag(Some(&format!("W/{}", etag(b"original")))).is_err());
        assert!(expected_etag(Some(&etag(b"original"))).is_ok());
        assert_ne!(etag(b"original"), etag(b"edited"));
    }
}
