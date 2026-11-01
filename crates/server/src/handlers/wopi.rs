use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::error::AppError;
use contrix_core::models::{File, FileLock, NewFile, NewFileLock, NewVersion};
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::file_locks::dsl as fl;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::users::dsl as u;
use contrix_core::schema::versions::dsl as v;

use crate::middleware::auth::JwtClaims;
use crate::services::StorageBackend;
use crate::state::AppState;

const WOPI_LOCK_TTL_MINUTES: i64 = 30;
const WOPI_FILE_NAME_MAX: usize = 255;
const WOPI_SESSION_TTL_MINUTES: i64 = 15;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn extract_wopi_token(req: &mut Request) -> Result<String, AppError> {
    if let Some(token) = req.query::<String>("access_token") {
        return Ok(token);
    }
    if let Some(token) = req.form::<String>("access_token").await {
        return Ok(token);
    }

    Err(AppError::Auth("Missing access_token parameter".into()))
}

fn validate_jwt_access_token(token: &str, state: &AppState) -> Result<String, AppError> {
    let secret = &state.config.auth.jwt_secret;
    let validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);

    let data = jsonwebtoken::decode::<JwtClaims>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|_| AppError::Auth("Invalid or expired access token".into()))?;

    Ok(data.claims.sub)
}

/// Validate a short-lived WOPI session token bound to the requested file.
async fn validate_wopi_token(
    req: &mut Request,
    state: &AppState,
    file_id: &str,
) -> Result<String, AppError> {
    let token = extract_wopi_token(req).await?;
    state
        .wopi_sessions
        .validate(&token, file_id)
        .ok_or_else(|| AppError::Auth("Invalid or expired WOPI session token".into()))
}

fn file_id_from_wopi_src(wopi_src: &str) -> Option<String> {
    let marker = "/wopi/files/";
    let lower = wopi_src.to_ascii_lowercase();
    let start = lower.find(marker)? + marker.len();
    let rest = &wopi_src[start..];
    let file_id = rest
        .split(['?', '#', '/'])
        .next()
        .unwrap_or_default()
        .trim();
    if file_id.is_empty() {
        None
    } else {
        Some(file_id.to_string())
    }
}

async fn exchange_wopi_session_token(
    state: &AppState,
    access_token: &str,
    wopi_src: &str,
) -> Result<String, AppError> {
    let user_id = validate_jwt_access_token(access_token, state)?;
    let file_id = file_id_from_wopi_src(wopi_src)
        .ok_or_else(|| AppError::Validation("Unable to resolve file id from WOPISrc".into()))?;
    Ok(state.wopi_sessions.create(
        &user_id,
        &file_id,
        Duration::minutes(WOPI_SESSION_TTL_MINUTES),
    ))
}

fn request_base_url(req: &Request) -> String {
    let scheme = req
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().scheme_str())
        .unwrap_or("http");
    let host = req
        .headers()
        .get("x-forwarded-host")
        .or_else(|| req.headers().get("host"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");

    format!("{scheme}://{host}")
}

fn app_error_status(err: &AppError) -> StatusCode {
    match err.status_code() {
        400 => StatusCode::BAD_REQUEST,
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        404 => StatusCode::NOT_FOUND,
        409 => StatusCode::CONFLICT,
        413 => StatusCode::PAYLOAD_TOO_LARGE,
        429 => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn render_wopi_error(res: &mut Response, err: AppError) {
    res.status_code(app_error_status(&err));
    res.headers_mut()
        .insert("content-type", "text/plain; charset=utf-8".parse().unwrap());
    res.render(err.to_string());
}

fn render_lock_conflict(res: &mut Response, lock: &str, reason: &str) {
    res.status_code(StatusCode::CONFLICT);
    res.headers_mut().insert(
        "X-WOPI-Lock",
        lock.parse().unwrap_or_else(|_| "".parse().unwrap()),
    );
    res.headers_mut().insert(
        "X-WOPI-LockFailureReason",
        reason.parse().unwrap_or_else(|_| "".parse().unwrap()),
    );
    res.headers_mut()
        .insert("content-type", "text/plain; charset=utf-8".parse().unwrap());
    res.render(reason.to_string());
}

fn wopi_lock_expiry() -> String {
    (Utc::now() + Duration::minutes(WOPI_LOCK_TTL_MINUTES)).to_rfc3339()
}

fn doc_kind_from_mime(mime_type: Option<&str>) -> &'static str {
    match mime_type.unwrap_or_default() {
        mime if mime.contains("contrix.calc") => "calc",
        mime if mime.contains("contrix.slides") => "slides",
        mime if mime.contains("contrix.whiteboard") => "whiteboard",
        mime if mime.contains("contrix.writer") => "writer",
        _ => "document",
    }
}

fn host_url(base_url: &str, doc_kind: &str, mode: &str) -> String {
    format!("{base_url}/api/v1/wopi/host/{doc_kind}/{mode}")
}

fn close_url(base_url: &str) -> String {
    format!("{base_url}/files")
}

fn sanitize_file_name(name: &str) -> String {
    let cleaned = name
        .trim()
        .replace(['\\', '/', ':', '*', '?', '"', '<', '>', '|'], "_");
    if cleaned.is_empty() {
        "Untitled".to_string()
    } else {
        cleaned
    }
}

async fn delete_previous_blob_if_safe(storage: &StorageBackend, old_path: &str) {
    if old_path.is_empty() || Path::new(old_path).is_absolute() {
        return;
    }

    let _ = storage.delete_file(old_path).await;
}

async fn load_owned_file(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    file_id: &str,
    owner_id: &str,
) -> Result<Option<File>, AppError> {
    f::files
        .filter(f::id.eq(file_id))
        .filter(f::owner_id.eq(owner_id))
        .filter(f::is_folder.eq(false))
        .filter(f::is_trashed.eq(false))
        .first::<File>(conn)
        .await
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))
}

async fn load_active_lock(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    file_id: &str,
) -> Result<Option<FileLock>, AppError> {
    let lock = fl::file_locks
        .filter(fl::file_id.eq(file_id))
        .first::<FileLock>(conn)
        .await
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))?;

    if let Some(lock) = lock {
        let is_expired = DateTime::parse_from_rfc3339(&lock.expires_at)
            .map(|dt| dt.with_timezone(&Utc) <= Utc::now())
            .unwrap_or(false);
        if is_expired {
            let _ = diesel::delete(fl::file_locks.filter(fl::id.eq(&lock.id)))
                .execute(conn)
                .await;
            Ok(None)
        } else {
            Ok(Some(lock))
        }
    } else {
        Ok(None)
    }
}

async fn read_file_bytes(
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

async fn save_file_bytes(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    state: &AppState,
    file_row: &File,
    data: &[u8],
    actor_id: &str,
) -> Result<String, AppError> {
    let linked_document = d::documents
        .filter(d::file_id.eq(&file_row.id))
        .select((d::id, d::version, d::content))
        .first::<(String, i32, Option<String>)>(conn)
        .await
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))?;

    let document_content = if linked_document.is_some() {
        let content = std::str::from_utf8(data)
            .map_err(|_| AppError::Validation("Linked document content must be UTF-8".into()))?
            .to_string();
        let _: serde_json::Value = serde_json::from_str(&content).map_err(|_| {
            AppError::Validation("Linked document content must be valid JSON".into())
        })?;
        Some(content)
    } else {
        None
    };

    let owner_uuid: Uuid = file_row
        .owner_id
        .parse()
        .map_err(|_| AppError::Validation("Invalid owner id".into()))?;
    let new_path = state
        .storage
        .save_file(&owner_uuid, &file_row.name, data)
        .await?;

    let now = Utc::now();
    let now_string = now.to_rfc3339();
    let new_size = data.len() as i64;
    let size_diff = new_size - file_row.size;

    diesel::update(f::files.filter(f::id.eq(&file_row.id)))
        .set((
            f::path.eq(&new_path),
            f::size.eq(new_size),
            f::updated_at.eq(&now_string),
        ))
        .execute(conn)
        .await
        .map_err(|e| AppError::Database(e.to_string()))?;

    let _ = diesel::update(u::users.filter(u::id.eq(&file_row.owner_id)))
        .set(u::storage_used.eq(u::storage_used + size_diff))
        .execute(conn)
        .await;

    delete_previous_blob_if_safe(&state.storage, &file_row.path).await;

    if let Some((document_id, current_version, old_content)) = linked_document {
        let snapshot = NewVersion {
            id: Uuid::new_v4().to_string(),
            document_id,
            version_number: current_version,
            content: old_content,
            created_by: actor_id.to_string(),
            label: Some("WOPI save".to_string()),
            created_at: now_string.clone(),
        };
        let _ = diesel::insert_into(v::versions)
            .values(&snapshot)
            .execute(conn)
            .await;

        diesel::update(d::documents.filter(d::file_id.eq(&file_row.id)))
            .set((
                d::content.eq(document_content),
                d::version.eq(current_version + 1),
                d::updated_at.eq(&now_string),
            ))
            .execute(conn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        Ok((current_version + 1).to_string())
    } else {
        Ok(now.timestamp_millis().to_string())
    }
}

async fn sibling_exists(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    owner_id: &str,
    parent_id: &Option<String>,
    name: &str,
    exclude_id: Option<&str>,
) -> Result<bool, AppError> {
    let mut query = f::files
        .into_boxed()
        .filter(f::owner_id.eq(owner_id))
        .filter(f::parent_id.eq(parent_id.clone()))
        .filter(f::name.eq(name))
        .filter(f::is_trashed.eq(false));

    if let Some(file_id) = exclude_id {
        query = query.filter(f::id.ne(file_id));
    }

    query
        .count()
        .get_result::<i64>(conn)
        .await
        .map(|count| count > 0)
        .map_err(|e| AppError::Database(e.to_string()))
}

async fn next_available_name(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    owner_id: &str,
    parent_id: &Option<String>,
    desired_name: &str,
    exclude_id: Option<&str>,
) -> Result<String, AppError> {
    let desired_name = sanitize_file_name(desired_name);
    if !sibling_exists(conn, owner_id, parent_id, &desired_name, exclude_id).await? {
        return Ok(desired_name);
    }

    let path = Path::new(&desired_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Untitled");
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");

    let mut index = 1;
    loop {
        let candidate = if ext.is_empty() {
            format!("{stem} ({index})")
        } else {
            format!("{stem} ({index}).{ext}")
        };
        if !sibling_exists(conn, owner_id, parent_id, &candidate, exclude_id).await? {
            return Ok(candidate);
        }
        index += 1;
    }
}

fn derive_relative_target_name(req: &Request, current_name: &str) -> Option<String> {
    if let Some(relative_target) = req
        .headers()
        .get("X-WOPI-RelativeTarget")
        .and_then(|v| v.to_str().ok())
    {
        return Some(sanitize_file_name(relative_target));
    }

    let suggested_target = req
        .headers()
        .get("X-WOPI-SuggestedTarget")
        .and_then(|v| v.to_str().ok())?;
    if suggested_target.starts_with('.') {
        let stem = Path::new(current_name)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(current_name);
        Some(sanitize_file_name(&format!("{stem}{suggested_target}")))
    } else {
        Some(sanitize_file_name(suggested_target))
    }
}

fn put_relative_response(
    base_url: &str,
    file_id: &str,
    file_name: &str,
    doc_kind: &str,
) -> serde_json::Value {
    serde_json::json!({
        "Name": file_name,
        "Url": format!("{base_url}/api/v1/wopi/files/{file_id}"),
        "HostViewUrl": host_url(base_url, doc_kind, "view"),
        "HostEditUrl": host_url(base_url, doc_kind, "edit"),
        "HostEditNewUrl": host_url(base_url, doc_kind, "edit"),
    })
}

// ---------------------------------------------------------------------------
// GET /wopi/discovery
// ---------------------------------------------------------------------------

#[handler]
pub async fn wopi_discovery(req: &mut Request, res: &mut Response) {
    let base_url = request_base_url(req);
    let writer_view = host_url(&base_url, "writer", "view");
    let writer_edit = host_url(&base_url, "writer", "edit");
    let calc_view = host_url(&base_url, "calc", "view");
    let calc_edit = host_url(&base_url, "calc", "edit");
    let slides_view = host_url(&base_url, "slides", "view");
    let slides_edit = host_url(&base_url, "slides", "edit");

    let xml = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<wopi-discovery>
  <net-zone name="external-http">
    <app name="Contrix Writer" favIconUrl="{base_url}/favicon.ico">
      <action name="view" default="true" ext="doc" urlsrc="{writer_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" default="true" ext="doc" requires="locks,update" urlsrc="{writer_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="view" ext="docx" urlsrc="{writer_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" ext="docx" requires="locks,update" urlsrc="{writer_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="view" ext="odt" urlsrc="{writer_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" ext="odt" requires="locks,update" urlsrc="{writer_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="view" ext="rtf" urlsrc="{writer_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" ext="rtf" requires="locks,update" urlsrc="{writer_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="view" ext="txt" urlsrc="{writer_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" ext="txt" requires="locks,update" urlsrc="{writer_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="embedview" ext="docx" urlsrc="{writer_view}?embed=1&amp;wopisrc=WOPI_SOURCE&amp;"/>
      <action name="mobileView" ext="docx" urlsrc="{writer_view}?mobile=1&amp;wopisrc=WOPI_SOURCE&amp;"/>
      <action name="mobileEdit" ext="docx" requires="locks,update" urlsrc="{writer_edit}?mobile=1&amp;wopisrc=WOPI_SOURCE&amp;"/>
    </app>
    <app name="Contrix Calc" favIconUrl="{base_url}/favicon.ico">
      <action name="view" default="true" ext="xls" urlsrc="{calc_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" default="true" ext="xls" requires="locks,update" urlsrc="{calc_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="view" ext="xlsx" urlsrc="{calc_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" ext="xlsx" requires="locks,update" urlsrc="{calc_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="view" ext="ods" urlsrc="{calc_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" ext="ods" requires="locks,update" urlsrc="{calc_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="view" ext="csv" urlsrc="{calc_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" ext="csv" requires="locks,update" urlsrc="{calc_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="embedview" ext="xlsx" urlsrc="{calc_view}?embed=1&amp;wopisrc=WOPI_SOURCE&amp;"/>
      <action name="mobileView" ext="xlsx" urlsrc="{calc_view}?mobile=1&amp;wopisrc=WOPI_SOURCE&amp;"/>
      <action name="mobileEdit" ext="xlsx" requires="locks,update" urlsrc="{calc_edit}?mobile=1&amp;wopisrc=WOPI_SOURCE&amp;"/>
    </app>
    <app name="Contrix Slides" favIconUrl="{base_url}/favicon.ico">
      <action name="view" default="true" ext="ppt" urlsrc="{slides_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" default="true" ext="ppt" requires="locks,update" urlsrc="{slides_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="view" ext="pptx" urlsrc="{slides_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" ext="pptx" requires="locks,update" urlsrc="{slides_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="view" ext="odp" urlsrc="{slides_view}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="edit" ext="odp" requires="locks,update" urlsrc="{slides_edit}?wopisrc=WOPI_SOURCE&amp;"/>
      <action name="embedview" ext="pptx" urlsrc="{slides_view}?embed=1&amp;wopisrc=WOPI_SOURCE&amp;"/>
      <action name="mobileView" ext="pptx" urlsrc="{slides_view}?mobile=1&amp;wopisrc=WOPI_SOURCE&amp;"/>
      <action name="mobileEdit" ext="pptx" requires="locks,update" urlsrc="{slides_edit}?mobile=1&amp;wopisrc=WOPI_SOURCE&amp;"/>
    </app>
  </net-zone>
</wopi-discovery>"#
    );

    res.status_code(StatusCode::OK);
    res.headers_mut()
        .insert("content-type", "application/xml".parse().unwrap());
    res.render(xml);
}

// ---------------------------------------------------------------------------
// GET /wopi/capabilities
// ---------------------------------------------------------------------------

#[handler]
pub async fn wopi_capabilities(res: &mut Response) {
    let capabilities = serde_json::json!({
        "capabilities": {
            "locks": true,
            "get_file": true,
            "put_file": true,
            "put_relative": true,
            "rename_file": true,
            "get_lock": true,
            "unlock": true,
            "refresh_lock": true,
        }
    });
    res.status_code(StatusCode::OK);
    res.render(Json(capabilities));
}

// ---------------------------------------------------------------------------
// GET/POST /wopi/host/:doc_kind/:mode
// ---------------------------------------------------------------------------

#[handler]
pub async fn wopi_host_page(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = match depot.obtain::<Arc<AppState>>() {
        Ok(state) => state.clone(),
        Err(_) => {
            render_wopi_error(
                res,
                AppError::Internal("Application state unavailable".into()),
            );
            return;
        }
    };
    let doc_kind = req
        .param::<String>("doc_kind")
        .unwrap_or_else(|| "document".to_string());
    let mode = req
        .param::<String>("mode")
        .unwrap_or_else(|| "view".to_string());
    let route_prefix = match doc_kind.as_str() {
        "writer" => "/writer",
        "calc" => "/calc",
        "slides" => "/slides",
        "whiteboard" => "/whiteboard",
        _ => "/documents",
    };
    let title = match doc_kind.as_str() {
        "writer" => "Contrix Writer",
        "calc" => "Contrix Calc",
        "slides" => "Contrix Slides",
        "whiteboard" => "Contrix Whiteboard",
        _ => "Contrix Document",
    };
    let incoming_access_token = if let Some(token) = req.query::<String>("access_token") {
        token
    } else {
        req.form::<String>("access_token").await.unwrap_or_default()
    };
    let wopi_src = req
        .query::<String>("wopisrc")
        .or_else(|| req.query::<String>("WOPISrc"))
        .unwrap_or_default();
    let wopi_session_token =
        match exchange_wopi_session_token(&state, &incoming_access_token, &wopi_src).await {
            Ok(token) => token,
            Err(e) => {
                tracing::warn!("Failed to exchange WOPI session token: {e}");
                String::new()
            }
        };
    let status_message = if mode == "edit" {
        "Opening editor"
    } else {
        "Opening document"
    };

    let html = format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title}</title>
    <style>
      :root {{
        color-scheme: light;
        --bg: #f3efe6;
        --panel: rgba(255, 255, 255, 0.88);
        --text: #172033;
        --muted: #5d6574;
        --accent: #9f3f24;
        --border: rgba(23, 32, 51, 0.12);
      }}
      * {{ box-sizing: border-box; }}
      body {{
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
        background:
          radial-gradient(circle at top left, rgba(159, 63, 36, 0.15), transparent 35%),
          linear-gradient(135deg, #f7f2e7, #e7eef4);
        color: var(--text);
        font: 16px/1.5 "Segoe UI", sans-serif;
      }}
      main {{
        width: min(520px, calc(100vw - 32px));
        padding: 32px;
        border-radius: 24px;
        background: var(--panel);
        border: 1px solid var(--border);
        box-shadow: 0 20px 60px rgba(23, 32, 51, 0.12);
      }}
      h1 {{ margin: 0 0 8px; font-size: 28px; }}
      p {{ margin: 0; color: var(--muted); }}
      .pulse {{
        width: 14px;
        height: 14px;
        margin-bottom: 20px;
        border-radius: 999px;
        background: var(--accent);
        box-shadow: 0 0 0 0 rgba(159, 63, 36, 0.35);
        animation: pulse 1.6s infinite;
      }}
      .fallback {{
        margin-top: 20px;
        display: inline-flex;
        align-items: center;
        justify-content: center;
        min-height: 44px;
        padding: 0 18px;
        border-radius: 999px;
        background: var(--text);
        color: white;
        text-decoration: none;
      }}
      .meta {{
        margin-top: 16px;
        font-size: 13px;
        color: var(--muted);
        word-break: break-all;
      }}
      @keyframes pulse {{
        0% {{ box-shadow: 0 0 0 0 rgba(159, 63, 36, 0.35); }}
        70% {{ box-shadow: 0 0 0 18px rgba(159, 63, 36, 0); }}
        100% {{ box-shadow: 0 0 0 0 rgba(159, 63, 36, 0); }}
      }}
    </style>
  </head>
  <body>
    <main>
      <div class="pulse"></div>
      <h1>{title}</h1>
      <p>{status_message}...</p>
      <a id="fallback" class="fallback" href="/files">Open Contrix</a>
      <div class="meta" id="meta">WOPISrc: {wopi_src}</div>
    </main>
    <script>
      const routePrefix = "{route_prefix}";
      const wopiSessionToken = {wopi_session_token:?};
      const initialWopiSrc = {wopi_src:?};

      function scrubSensitiveQuery() {{
        try {{
          const url = new URL(window.location.href);
          if (url.searchParams.has("access_token")) {{
            url.searchParams.delete("access_token");
            window.history.replaceState(null, document.title, url.pathname + url.search + url.hash);
          }}
        }} catch (_) {{}}
      }}

      function extractFileId(value) {{
        if (!value) return "";
        try {{
          const parsed = new URL(value, window.location.origin);
          const parts = parsed.pathname.split("/").filter(Boolean);
          const filesIndex = parts.lastIndexOf("files");
          if (filesIndex >= 0 && filesIndex + 1 < parts.length) {{
            return parts[filesIndex + 1];
          }}
          return parts.length ? parts[parts.length - 1] : "";
        }} catch (_) {{
          return "";
        }}
      }}

      async function start() {{
        scrubSensitiveQuery();
        const params = new URLSearchParams(window.location.search);
        const wopiSrc = params.get("wopisrc") || params.get("WOPISrc") || initialWopiSrc;
        const fileId = extractFileId(wopiSrc);
        if (!fileId) {{
          document.getElementById("meta").textContent = "Unable to resolve file id from WOPISrc.";
          return;
        }}

        const target = `${{routePrefix}}/${{encodeURIComponent(fileId)}}`;
        if (wopiSessionToken) {{
          sessionStorage.setItem("contrix.wopi", JSON.stringify({{
            token: wopiSessionToken,
            file_id: fileId
          }}));
        }}
        document.getElementById("fallback").href = target;
        document.getElementById("meta").textContent = `File id: ${{fileId}}`;
        window.location.replace(target);
      }}

      start();
    </script>
  </body>
</html>"#
    );

    res.status_code(StatusCode::OK);
    res.headers_mut()
        .insert("content-type", "text/html; charset=utf-8".parse().unwrap());
    res.headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    res.headers_mut()
        .insert("referrer-policy", "no-referrer".parse().unwrap());
    res.render(html);
}

// ---------------------------------------------------------------------------
// GET /wopi/files/:file_id  (CheckFileInfo)
// ---------------------------------------------------------------------------

#[handler]
pub async fn check_file_info(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let file_id_param: String = req.param("file_id").unwrap_or_default();
    let uid = match validate_wopi_token(req, &state, &file_id_param).await {
        Ok(id) => id,
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };
    let base_url = request_base_url(req);

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_wopi_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let file_row = match load_owned_file(&mut conn, &file_id_param, &uid).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            render_wopi_error(res, AppError::NotFound("File not found".into()));
            return;
        }
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };

    let (username, display_name) = u::users
        .filter(u::id.eq(&uid))
        .select((u::username, u::display_name))
        .first::<(String, Option<String>)>(&mut conn)
        .await
        .unwrap_or(("unknown".to_string(), None));

    let lock_info = match load_active_lock(&mut conn, &file_id_param).await {
        Ok(lock) => lock,
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };
    let doc_kind = doc_kind_from_mime(file_row.mime_type.as_deref());
    let extension = Path::new(&file_row.name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string();
    let base_file_name = file_row.name.clone();
    let owner_id = file_row.owner_id.clone();
    let version = file_row.updated_at.clone();

    let info = serde_json::json!({
        "BaseFileName": base_file_name.clone(),
        "BreadcrumbDocName": base_file_name,
        "OwnerId": owner_id,
        "Size": file_row.size,
        "UserId": uid,
        "UserFriendlyName": display_name.unwrap_or(username),
        "Version": version.clone(),
        "LastModifiedTime": version,
        "ReadOnly": false,
        "UserCanWrite": true,
        "UserCanRename": true,
        "UserCanNotWriteRelative": false,
        "SupportsLocks": true,
        "SupportsGetLock": true,
        "SupportsUpdate": true,
        "SupportsRename": true,
        "SupportsUserInfo": false,
        "SupportsFolders": false,
        "SupportsDeleteFile": false,
        "FileNameMaxLength": WOPI_FILE_NAME_MAX,
        "FileExtension": extension,
        "HostViewUrl": host_url(&base_url, doc_kind, "view"),
        "HostEditUrl": host_url(&base_url, doc_kind, "edit"),
        "CloseUrl": close_url(&base_url),
        "LockId": lock_info.as_ref().map(|lock| lock.lock_token.clone()),
    });

    res.status_code(StatusCode::OK);
    res.render(Json(info));
}

// ---------------------------------------------------------------------------
// GET /wopi/files/:file_id/contents  (GetFile)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_file_contents(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let file_id_param: String = req.param("file_id").unwrap_or_default();
    let uid = match validate_wopi_token(req, &state, &file_id_param).await {
        Ok(id) => id,
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_wopi_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let file_row = match load_owned_file(&mut conn, &file_id_param, &uid).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            render_wopi_error(res, AppError::NotFound("File not found".into()));
            return;
        }
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };

    let data = match read_file_bytes(&mut conn, &state.storage, &file_row.id, &file_row.path).await
    {
        Ok(bytes) => bytes,
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };

    let ct = file_row
        .mime_type
        .unwrap_or_else(|| "application/octet-stream".to_string());
    res.status_code(StatusCode::OK);
    res.headers_mut().insert(
        "content-type",
        ct.parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );
    res.headers_mut()
        .insert("content-length", data.len().to_string().parse().unwrap());
    res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(data)));
}

// ---------------------------------------------------------------------------
// POST /wopi/files/:file_id/contents  (PutFile)
// ---------------------------------------------------------------------------

#[handler]
pub async fn put_file_contents(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let file_id_param: String = req.param("file_id").unwrap_or_default();
    let uid = match validate_wopi_token(req, &state, &file_id_param).await {
        Ok(id) => id,
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };
    let request_lock = req
        .headers()
        .get("X-WOPI-Lock")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_wopi_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let file_row = match load_owned_file(&mut conn, &file_id_param, &uid).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            render_wopi_error(res, AppError::NotFound("File not found".into()));
            return;
        }
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };

    match load_active_lock(&mut conn, &file_id_param).await {
        Ok(Some(lock)) if lock.lock_token != request_lock => {
            render_lock_conflict(res, &lock.lock_token, "Lock mismatch");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    }

    let data = match req.payload().await {
        Ok(bytes) => bytes.to_vec(),
        Err(e) => {
            render_wopi_error(res, AppError::Validation(e.to_string()));
            return;
        }
    };

    let item_version = match save_file_bytes(&mut conn, &state, &file_row, &data, &uid).await {
        Ok(version) => version,
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };

    res.status_code(StatusCode::OK);
    res.headers_mut().insert(
        "X-WOPI-ItemVersion",
        item_version.parse().unwrap_or_else(|_| "".parse().unwrap()),
    );
}

// ---------------------------------------------------------------------------
// POST /wopi/files/:file_id  (Lock / Unlock / RefreshLock / GetLock)
// ---------------------------------------------------------------------------

#[handler]
pub async fn wopi_file_operation(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let file_id_param: String = req.param("file_id").unwrap_or_default();
    let uid = match validate_wopi_token(req, &state, &file_id_param).await {
        Ok(id) => id,
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };
    let override_header = req
        .headers()
        .get("X-WOPI-Override")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_uppercase();
    let request_lock = req
        .headers()
        .get("X-WOPI-Lock")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let old_lock = req
        .headers()
        .get("X-WOPI-OldLock")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            render_wopi_error(res, AppError::Database(e.to_string()));
            return;
        }
    };

    let file_row = match load_owned_file(&mut conn, &file_id_param, &uid).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            render_wopi_error(res, AppError::NotFound("File not found".into()));
            return;
        }
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };

    match override_header.as_str() {
        "LOCK" => {
            let existing = match load_active_lock(&mut conn, &file_id_param).await {
                Ok(lock) => lock,
                Err(e) => {
                    render_wopi_error(res, e);
                    return;
                }
            };

            match existing {
                Some(existing_lock) if !old_lock.is_empty() => {
                    if existing_lock.lock_token != old_lock {
                        render_lock_conflict(res, &existing_lock.lock_token, "Lock mismatch");
                        return;
                    }

                    if let Err(e) =
                        diesel::update(fl::file_locks.filter(fl::id.eq(&existing_lock.id)))
                            .set((
                                fl::lock_token.eq(&request_lock),
                                fl::locked_by.eq(&uid),
                                fl::expires_at.eq(wopi_lock_expiry()),
                            ))
                            .execute(&mut conn)
                            .await
                    {
                        render_wopi_error(res, AppError::Database(e.to_string()));
                        return;
                    }

                    res.status_code(StatusCode::OK);
                }
                Some(existing_lock) if existing_lock.lock_token == request_lock => {
                    if let Err(e) =
                        diesel::update(fl::file_locks.filter(fl::id.eq(&existing_lock.id)))
                            .set((
                                fl::locked_by.eq(&uid),
                                fl::expires_at.eq(wopi_lock_expiry()),
                            ))
                            .execute(&mut conn)
                            .await
                    {
                        render_wopi_error(res, AppError::Database(e.to_string()));
                        return;
                    }

                    res.status_code(StatusCode::OK);
                }
                Some(existing_lock) => {
                    render_lock_conflict(
                        res,
                        &existing_lock.lock_token,
                        "File is locked with a different token",
                    );
                }
                None => {
                    let now = Utc::now().to_rfc3339();
                    let new_lock = NewFileLock {
                        id: Uuid::new_v4().to_string(),
                        file_id: file_id_param.clone(),
                        locked_by: uid.clone(),
                        lock_token: request_lock.clone(),
                        lock_type: "wopi".to_string(),
                        expires_at: wopi_lock_expiry(),
                        created_at: now,
                    };

                    if let Err(e) = diesel::insert_into(fl::file_locks)
                        .values(&new_lock)
                        .execute(&mut conn)
                        .await
                    {
                        render_wopi_error(res, AppError::Database(e.to_string()));
                        return;
                    }

                    res.status_code(StatusCode::OK);
                }
            }
        }
        "UNLOCK" => match load_active_lock(&mut conn, &file_row.id).await {
            Ok(Some(existing_lock)) if existing_lock.lock_token == request_lock => {
                if let Err(e) = diesel::delete(fl::file_locks.filter(fl::id.eq(&existing_lock.id)))
                    .execute(&mut conn)
                    .await
                {
                    render_wopi_error(res, AppError::Database(e.to_string()));
                    return;
                }
                res.status_code(StatusCode::OK);
            }
            Ok(Some(existing_lock)) => {
                render_lock_conflict(res, &existing_lock.lock_token, "Lock mismatch");
            }
            Ok(None) => {
                render_lock_conflict(res, "", "File is not locked");
            }
            Err(e) => {
                render_wopi_error(res, e);
            }
        },
        "REFRESH_LOCK" => match load_active_lock(&mut conn, &file_row.id).await {
            Ok(Some(existing_lock)) if existing_lock.lock_token == request_lock => {
                if let Err(e) = diesel::update(fl::file_locks.filter(fl::id.eq(&existing_lock.id)))
                    .set((
                        fl::locked_by.eq(&uid),
                        fl::expires_at.eq(wopi_lock_expiry()),
                    ))
                    .execute(&mut conn)
                    .await
                {
                    render_wopi_error(res, AppError::Database(e.to_string()));
                    return;
                }
                res.status_code(StatusCode::OK);
            }
            Ok(Some(existing_lock)) => {
                render_lock_conflict(res, &existing_lock.lock_token, "Lock mismatch");
            }
            Ok(None) => {
                render_lock_conflict(res, "", "File is not locked");
            }
            Err(e) => {
                render_wopi_error(res, e);
            }
        },
        "GET_LOCK" => match load_active_lock(&mut conn, &file_row.id).await {
            Ok(Some(existing_lock)) => {
                res.status_code(StatusCode::OK);
                res.headers_mut().insert(
                    "X-WOPI-Lock",
                    existing_lock
                        .lock_token
                        .parse()
                        .unwrap_or_else(|_| "".parse().unwrap()),
                );
            }
            Ok(None) => {
                res.status_code(StatusCode::OK);
                res.headers_mut().insert("X-WOPI-Lock", "".parse().unwrap());
            }
            Err(e) => {
                render_wopi_error(res, e);
            }
        },
        "PUT_RELATIVE" => {
            match load_active_lock(&mut conn, &file_row.id).await {
                Ok(Some(existing_lock)) if existing_lock.lock_token != request_lock => {
                    render_lock_conflict(res, &existing_lock.lock_token, "Lock mismatch");
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    render_wopi_error(res, e);
                    return;
                }
            }

            let desired_name = match derive_relative_target_name(req, &file_row.name) {
                Some(name) => name,
                None => {
                    render_wopi_error(
                        res,
                        AppError::Validation(
                            "Missing X-WOPI-SuggestedTarget or X-WOPI-RelativeTarget".into(),
                        ),
                    );
                    return;
                }
            };
            let overwrite = req
                .headers()
                .get("X-WOPI-OverwriteRelativeTarget")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            let existing_target = match f::files
                .filter(f::owner_id.eq(&file_row.owner_id))
                .filter(f::parent_id.eq(file_row.parent_id.clone()))
                .filter(f::name.eq(&desired_name))
                .filter(f::is_trashed.eq(false))
                .first::<File>(&mut conn)
                .await
                .optional()
            {
                Ok(target) => target,
                Err(e) => {
                    render_wopi_error(res, AppError::Database(e.to_string()));
                    return;
                }
            };
            let data = match req.payload().await {
                Ok(bytes) => bytes.to_vec(),
                Err(e) => {
                    render_wopi_error(res, AppError::Validation(e.to_string()));
                    return;
                }
            };
            let base_url = request_base_url(req);
            let doc_kind = doc_kind_from_mime(file_row.mime_type.as_deref());

            if let Some(existing_target) = existing_target {
                if !overwrite {
                    let valid_name = match next_available_name(
                        &mut conn,
                        &file_row.owner_id,
                        &file_row.parent_id,
                        &desired_name,
                        Some(&existing_target.id),
                    )
                    .await
                    {
                        Ok(name) => name,
                        Err(e) => {
                            render_wopi_error(res, e);
                            return;
                        }
                    };
                    res.status_code(StatusCode::CONFLICT);
                    res.headers_mut().insert(
                        "X-WOPI-ValidRelativeTarget",
                        valid_name.parse().unwrap_or_else(|_| "".parse().unwrap()),
                    );
                    return;
                }

                match load_active_lock(&mut conn, &existing_target.id).await {
                    Ok(Some(target_lock)) if target_lock.lock_token != request_lock => {
                        let valid_name = match next_available_name(
                            &mut conn,
                            &file_row.owner_id,
                            &file_row.parent_id,
                            &desired_name,
                            Some(&existing_target.id),
                        )
                        .await
                        {
                            Ok(name) => name,
                            Err(e) => {
                                render_wopi_error(res, e);
                                return;
                            }
                        };
                        res.status_code(StatusCode::CONFLICT);
                        res.headers_mut().insert(
                            "X-WOPI-ValidRelativeTarget",
                            valid_name.parse().unwrap_or_else(|_| "".parse().unwrap()),
                        );
                        return;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        render_wopi_error(res, e);
                        return;
                    }
                }

                let item_version =
                    match save_file_bytes(&mut conn, &state, &existing_target, &data, &uid).await {
                        Ok(version) => version,
                        Err(e) => {
                            render_wopi_error(res, e);
                            return;
                        }
                    };
                res.status_code(StatusCode::OK);
                res.headers_mut().insert(
                    "X-WOPI-ItemVersion",
                    item_version.parse().unwrap_or_else(|_| "".parse().unwrap()),
                );
                res.render(Json(put_relative_response(
                    &base_url,
                    &existing_target.id,
                    &existing_target.name,
                    doc_kind,
                )));
                return;
            }

            let owner_uuid: Uuid = match file_row.owner_id.parse() {
                Ok(uuid) => uuid,
                Err(_) => {
                    render_wopi_error(res, AppError::Validation("Invalid owner id".into()));
                    return;
                }
            };
            let path = match state
                .storage
                .save_file(&owner_uuid, &desired_name, &data)
                .await
            {
                Ok(path) => path,
                Err(e) => {
                    render_wopi_error(res, e);
                    return;
                }
            };
            let now = Utc::now().to_rfc3339();
            let new_file = NewFile {
                id: Uuid::new_v4().to_string(),
                name: desired_name.clone(),
                path,
                mime_type: file_row.mime_type.clone(),
                size: data.len() as i64,
                owner_id: file_row.owner_id.clone(),
                parent_id: file_row.parent_id.clone(),
                is_folder: false,
                is_trashed: false,
                trashed_at: None,
                created_at: now.clone(),
                updated_at: now,
                room_id: file_row.room_id.clone(),
                thumbnail_path: None,
                sort_order: 0,
            };

            if let Err(e) = diesel::insert_into(f::files)
                .values(&new_file)
                .execute(&mut conn)
                .await
            {
                render_wopi_error(res, AppError::Database(e.to_string()));
                return;
            }

            let _ = diesel::update(u::users.filter(u::id.eq(&file_row.owner_id)))
                .set(u::storage_used.eq(u::storage_used + data.len() as i64))
                .execute(&mut conn)
                .await;

            res.status_code(StatusCode::OK);
            res.render(Json(put_relative_response(
                &base_url,
                &new_file.id,
                &new_file.name,
                doc_kind,
            )));
        }
        "RENAME_FILE" => {
            match load_active_lock(&mut conn, &file_row.id).await {
                Ok(Some(existing_lock)) if existing_lock.lock_token != request_lock => {
                    render_lock_conflict(res, &existing_lock.lock_token, "Lock mismatch");
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    render_wopi_error(res, e);
                    return;
                }
            }

            let requested_name = req
                .headers()
                .get("X-WOPI-RequestedName")
                .and_then(|v| v.to_str().ok())
                .map(sanitize_file_name)
                .unwrap_or_default();

            if requested_name.is_empty() {
                render_wopi_error(
                    res,
                    AppError::Validation("Missing X-WOPI-RequestedName".into()),
                );
                return;
            }

            if requested_name.len() > WOPI_FILE_NAME_MAX {
                render_wopi_error(
                    res,
                    AppError::Validation("Requested name exceeds WOPI file name limit".into()),
                );
                return;
            }

            match sibling_exists(
                &mut conn,
                &file_row.owner_id,
                &file_row.parent_id,
                &requested_name,
                Some(&file_row.id),
            )
            .await
            {
                Ok(true) => {
                    render_wopi_error(
                        res,
                        AppError::Conflict("A sibling item with that name already exists".into()),
                    );
                    return;
                }
                Ok(false) => {}
                Err(e) => {
                    render_wopi_error(res, e);
                    return;
                }
            }

            let now = Utc::now().to_rfc3339();
            if let Err(e) = diesel::update(f::files.filter(f::id.eq(&file_row.id)))
                .set((f::name.eq(&requested_name), f::updated_at.eq(&now)))
                .execute(&mut conn)
                .await
            {
                render_wopi_error(res, AppError::Database(e.to_string()));
                return;
            }

            res.status_code(StatusCode::OK);
            res.headers_mut().insert(
                "X-WOPI-ItemVersion",
                now.parse().unwrap_or_else(|_| "".parse().unwrap()),
            );
            res.render(Json(serde_json::json!({ "Name": requested_name })));
        }
        _ => {
            render_wopi_error(
                res,
                AppError::Validation(format!("Unsupported X-WOPI-Override: {override_header}")),
            );
        }
    }
}
