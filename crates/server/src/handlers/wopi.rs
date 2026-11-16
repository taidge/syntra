use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::error::AppError;
use contrix_core::models::{File, FileLock, NewFile, NewFileLock};
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::file_locks::dsl as fl;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::users::dsl as u;

use crate::handlers::access::{authorize_file_access, load_accessible_file};
use crate::services::StorageBackend;
use crate::services::wopi_sessions::{
    WopiSession, validate_active_jwt_session, validate_session_hash,
};
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

/// Validate a short-lived WOPI session token bound to the requested file.
async fn validate_wopi_token(
    req: &mut Request,
    state: &AppState,
    file_id: &str,
) -> Result<String, AppError> {
    Ok(validate_wopi_session(req, state, file_id).await?.user_id)
}

async fn validate_wopi_session(
    req: &mut Request,
    state: &AppState,
    file_id: &str,
) -> Result<WopiSession, AppError> {
    let token = extract_wopi_token(req).await?;
    let session = state
        .wopi_sessions
        .validate(&token, file_id)
        .ok_or_else(|| AppError::Auth("Invalid or expired WOPI session token".into()))?;
    validate_session_hash(state, &session.user_id, &session.source_token_hash).await?;
    Ok(session)
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
    let source = validate_active_jwt_session(state, access_token).await?;
    let file_id = file_id_from_wopi_src(wopi_src)
        .ok_or_else(|| AppError::Validation("Unable to resolve file id from WOPISrc".into()))?;
    let mut conn = state.db.clone();
    load_authorized_file(&mut conn, &file_id, &source.user_id, "viewer")
        .await?
        .ok_or_else(|| AppError::NotFound("File not found".into()))?;
    Ok(state.wopi_sessions.create(
        &source,
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

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn script_json_string(value: &str) -> String {
    // JSON quoting alone does not protect an HTML script element: the HTML
    // parser recognizes </script> even inside a JavaScript string literal.
    serde_json::to_string(value)
        .expect("serializing a string cannot fail")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

async fn load_authorized_file(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    file_id: &str,
    user_id: &str,
    required_role: &str,
) -> Result<Option<File>, AppError> {
    Ok(load_accessible_file(conn, file_id, user_id, required_role)
        .await?
        .filter(|file| !file.is_folder && !file.is_trashed))
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
            // Readers must not delete a lock another request just refreshed.
            // Expired rows are replaced under the write transaction below.
            Ok(None)
        } else {
            Ok(Some(lock))
        }
    } else {
        Ok(None)
    }
}

enum LockChange {
    Applied,
    Conflict(String),
}

async fn change_wopi_lock(
    state: &AppState,
    file_id: &str,
    user_id: &str,
    operation: &str,
    requested: &str,
    old: &str,
    session: &WopiSession,
) -> Result<LockChange, AppError> {
    if requested.is_empty() || requested.len() > 1024 || old.len() > 1024 {
        return Err(AppError::Validation(
            "A WOPI lock token of 1–1024 bytes is required".into(),
        ));
    }
    let (file_id, user_id, operation, requested, old) = (
        file_id.to_owned(),
        user_id.to_owned(),
        operation.to_owned(),
        requested.to_owned(),
        old.to_owned(),
    );
    let session = session.clone();
    state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                session.validate_write_in_transaction(conn)?;
                let file = diesel::RunQueryDsl::first::<File>(f::files.find(&file_id), conn)?;
                crate::services::persistence::authorize(conn, &file, &user_id, "editor")?;
                let mut existing =
                    diesel::OptionalExtension::optional(diesel::RunQueryDsl::first::<FileLock>(
                        fl::file_locks.filter(fl::file_id.eq(&file_id)),
                        conn,
                    ))?;
                if existing.as_ref().is_some_and(|lock| {
                    DateTime::parse_from_rfc3339(&lock.expires_at)
                        .is_ok_and(|expires| expires.with_timezone(&Utc) <= Utc::now())
                }) {
                    diesel::RunQueryDsl::execute(
                        diesel::delete(fl::file_locks.filter(fl::file_id.eq(&file_id))),
                        conn,
                    )?;
                    existing = None;
                }
                match (operation.as_str(), existing) {
                    ("LOCK", Some(lock)) => {
                        let expected = if old.is_empty() { &requested } else { &old };
                        if lock.locked_by != user_id || lock.lock_token != *expected {
                            return Ok(LockChange::Conflict(lock.lock_token));
                        }
                        diesel::RunQueryDsl::execute(
                            diesel::update(fl::file_locks.find(lock.id)).set((
                                fl::lock_token.eq(&requested),
                                fl::expires_at.eq(wopi_lock_expiry()),
                            )),
                            conn,
                        )?;
                    }
                    ("LOCK", None) if old.is_empty() => {
                        diesel::RunQueryDsl::execute(
                            diesel::insert_into(fl::file_locks).values(NewFileLock {
                                id: Uuid::new_v4().to_string(),
                                file_id,
                                locked_by: user_id,
                                lock_token: requested,
                                lock_type: "wopi".into(),
                                expires_at: wopi_lock_expiry(),
                                created_at: Utc::now().to_rfc3339(),
                            }),
                            conn,
                        )?;
                    }
                    ("UNLOCK" | "REFRESH_LOCK", Some(lock)) => {
                        if lock.locked_by != user_id || lock.lock_token != requested {
                            return Ok(LockChange::Conflict(lock.lock_token));
                        }
                        if operation == "UNLOCK" {
                            diesel::RunQueryDsl::execute(
                                diesel::delete(fl::file_locks.find(lock.id)),
                                conn,
                            )?;
                        } else {
                            diesel::RunQueryDsl::execute(
                                diesel::update(fl::file_locks.find(lock.id))
                                    .set(fl::expires_at.eq(wopi_lock_expiry())),
                                conn,
                            )?;
                        }
                    }
                    _ => return Ok(LockChange::Conflict(String::new())),
                }
                Ok(LockChange::Applied)
            })
        })
        .await
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
    lock_token: &str,
    session: &WopiSession,
    source: &File,
) -> Result<String, AppError> {
    if file_row.is_folder {
        return Err(AppError::Conflict(
            "A folder already uses the requested target name".into(),
        ));
    }
    let authorization = session.write_authorization(source, Some(lock_token))?;
    let actor_id = session.user_id.as_str();
    let linked_document = d::documents
        .filter(d::file_id.eq(&file_row.id))
        .select((d::id, d::version))
        .first::<(String, i32)>(conn)
        .await
        .optional()?;
    if let Some((document_id, current_version)) = linked_document {
        let content = std::str::from_utf8(data)
            .map_err(|_| AppError::Validation("Linked document content must be UTF-8".into()))?
            .to_owned();
        let _: serde_json::Value = serde_json::from_str(&content).map_err(|_| {
            AppError::Validation("Linked document content must be valid JSON".into())
        })?;
        return crate::services::persistence::commit_document_with_wopi_authorization(
            &state.db,
            &state.storage,
            &document_id,
            actor_id,
            current_version,
            content,
            Some("WOPI save".into()),
            Some(lock_token.to_owned()),
            &authorization,
        )
        .await
        .map(|version| version.to_string());
    }

    let owner_uuid = file_row
        .owner_id
        .parse()
        .map_err(|_| AppError::Validation("Invalid owner id".into()))?;
    let new_path = state
        .storage
        .save_file(&owner_uuid, &file_row.name, data)
        .await?;
    let committed_path = new_path.clone();
    let original = file_row.clone();
    let actor = actor_id.to_owned();
    let token = lock_token.to_owned();
    let new_size =
        i64::try_from(data.len()).map_err(|_| AppError::Validation("File too large".into()))?;
    let result = crate::services::persistence::commit_blob(&state.db, new_path, move |conn| {
        conn.immediate_transaction::<_, AppError, _>(|conn| {
            authorization.validate_in_transaction(conn)?;
            let current = diesel::RunQueryDsl::first::<File>(f::files.find(&original.id), conn)?;
            if current.is_folder {
                return Err(AppError::Conflict(
                    "A folder already uses the requested target name".into(),
                ));
            }
            crate::services::persistence::authorize(conn, &current, &actor, "editor")?;
            crate::services::persistence::check_file_lock(conn, &current.id, &actor, Some(&token))?;
            if current.path != original.path || current.updated_at != original.updated_at {
                return Err(AppError::Conflict(
                    "File changed; reload before saving".into(),
                ));
            }
            let now = Utc::now().to_rfc3339();
            diesel::RunQueryDsl::execute(
                diesel::update(f::files.find(&current.id)).set((
                    f::path.eq(&committed_path),
                    f::size.eq(new_size),
                    f::updated_at.eq(&now),
                )),
                conn,
            )?;
            crate::services::persistence::replace_file_quota(
                conn,
                &current.owner_id,
                &current.path,
                current.size,
                new_size,
            )?;
            crate::services::persistence::enqueue_gc(conn, &current.path)?;
            Ok(now)
        })
    })
    .await;
    crate::services::persistence::request_gc(&state.db, &state.storage);
    result
}

async fn sibling_exists(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    scope: &File,
    name: &str,
    exclude_id: Option<&str>,
) -> Result<bool, AppError> {
    let mut query = f::files
        .into_boxed()
        .filter(f::name.eq(name))
        .filter(f::is_trashed.eq(false));
    query = if let Some(parent_id) = &scope.parent_id {
        query.filter(f::parent_id.eq(parent_id))
    } else {
        query.filter(f::parent_id.is_null())
    };
    query = if let Some(room) = &scope.room_id {
        query.filter(f::room_id.eq(room))
    } else {
        query
            .filter(f::room_id.is_null())
            .filter(f::owner_id.eq(&scope.owner_id))
    };

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

fn sibling_exists_sync(
    conn: &mut SqliteConnection,
    scope: &File,
    name: &str,
    exclude: Option<&str>,
) -> Result<bool, AppError> {
    let mut query = f::files
        .into_boxed()
        .filter(f::name.eq(name))
        .filter(f::is_trashed.eq(false));
    query = if let Some(parent) = &scope.parent_id {
        query.filter(f::parent_id.eq(parent))
    } else {
        query.filter(f::parent_id.is_null())
    };
    query = if let Some(room) = &scope.room_id {
        query.filter(f::room_id.eq(room))
    } else {
        query
            .filter(f::room_id.is_null())
            .filter(f::owner_id.eq(&scope.owner_id))
    };
    if let Some(exclude) = exclude {
        query = query.filter(f::id.ne(exclude));
    }
    Ok(diesel::RunQueryDsl::get_result::<i64>(query.count(), conn)? != 0)
}

async fn next_available_name(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    scope: &File,
    desired_name: &str,
    exclude_id: Option<&str>,
) -> Result<String, AppError> {
    let desired_name = sanitize_file_name(desired_name);
    if !sibling_exists(conn, scope, &desired_name, exclude_id).await? {
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
        if !sibling_exists(conn, scope, &candidate, exclude_id).await? {
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
                render_wopi_error(res, e);
                return;
            }
        };
    let status_message = if mode == "edit" {
        "Opening editor"
    } else {
        "Opening document"
    };
    let display_wopi_src = escape_html(&wopi_src);
    let script_wopi_src = script_json_string(&wopi_src);
    let script_nonce = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

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
      <div class="meta" id="meta">WOPISrc: {display_wopi_src}</div>
    </main>
    <script nonce="{script_nonce}">
      const routePrefix = "{route_prefix}";
      const wopiSessionToken = {wopi_session_token:?};
      const initialWopiSrc = {script_wopi_src};

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
    res.headers_mut().insert(
        "content-security-policy",
        crate::middleware::security_headers::content_security_policy(Some(&script_nonce))
            .parse()
            .unwrap(),
    );
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

    let file_row = match load_authorized_file(&mut conn, &file_id_param, &uid, "viewer").await {
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
    let can_write = authorize_file_access(&mut conn, &file_row, &uid, "editor")
        .await
        .is_ok();

    let info = serde_json::json!({
        "BaseFileName": base_file_name.clone(),
        "BreadcrumbDocName": base_file_name,
        "OwnerId": owner_id,
        "Size": file_row.size,
        "UserId": uid,
        "UserFriendlyName": display_name.unwrap_or(username),
        "Version": version.clone(),
        "LastModifiedTime": version,
        "ReadOnly": !can_write,
        "UserCanWrite": can_write,
        "UserCanRename": can_write,
        "UserCanNotWriteRelative": !can_write,
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

    let file_row = match load_authorized_file(&mut conn, &file_id_param, &uid, "viewer").await {
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
    let session = match validate_wopi_session(req, &state, &file_id_param).await {
        Ok(session) => session,
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };
    let uid = session.user_id.clone();
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

    let file_row = match load_authorized_file(&mut conn, &file_id_param, &uid, "editor").await {
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

    let item_version = match save_file_bytes(
        &mut conn,
        &state,
        &file_row,
        &data,
        &request_lock,
        &session,
        &file_row,
    )
    .await
    {
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
    let session = match validate_wopi_session(req, &state, &file_id_param).await {
        Ok(session) => session,
        Err(e) => {
            render_wopi_error(res, e);
            return;
        }
    };
    let uid = session.user_id.clone();
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

    let required_role = if override_header == "GET_LOCK" {
        "viewer"
    } else {
        "editor"
    };
    let file_row = match load_authorized_file(&mut conn, &file_id_param, &uid, required_role).await
    {
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
        "LOCK" | "UNLOCK" | "REFRESH_LOCK" => {
            match change_wopi_lock(
                &state,
                &file_id_param,
                &uid,
                &override_header,
                &request_lock,
                &old_lock,
                &session,
            )
            .await
            {
                Ok(LockChange::Applied) => {
                    res.status_code(StatusCode::OK);
                }
                Ok(LockChange::Conflict(current)) => {
                    render_lock_conflict(res, &current, "Lock mismatch");
                }
                Err(error) => render_wopi_error(res, error),
            }
        }
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
            let mut target_query = f::files
                .into_boxed()
                .filter(f::name.eq(&desired_name))
                .filter(f::is_trashed.eq(false));
            target_query = if let Some(parent) = &file_row.parent_id {
                target_query.filter(f::parent_id.eq(parent))
            } else {
                target_query.filter(f::parent_id.is_null())
            };
            target_query = if let Some(room) = &file_row.room_id {
                target_query.filter(f::room_id.eq(room))
            } else {
                target_query
                    .filter(f::room_id.is_null())
                    .filter(f::owner_id.eq(&file_row.owner_id))
            };
            let existing_target = match target_query.first::<File>(&mut conn).await.optional() {
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
                        &file_row,
                        &desired_name,
                        None,
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
                        let valid_name =
                            match next_available_name(&mut conn, &file_row, &desired_name, None)
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

                let item_version = match save_file_bytes(
                    &mut conn,
                    &state,
                    &existing_target,
                    &data,
                    &request_lock,
                    &session,
                    &file_row,
                )
                .await
                {
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

            let result_id = new_file.id.clone();
            let result_name = new_file.name.clone();
            let orphan_path = new_file.path.clone();
            let actor = uid.clone();
            let token = request_lock.clone();
            let original = file_row.clone();
            let session = session.clone();
            let insertion =
                crate::services::persistence::commit_blob(&state.db, orphan_path, move |conn| {
                    conn.immediate_transaction::<_, AppError, _>(|conn| {
                        session.validate_write_in_transaction(conn)?;
                        let current =
                            diesel::RunQueryDsl::first::<File>(f::files.find(&original.id), conn)?;
                        crate::services::persistence::authorize(conn, &current, &actor, "editor")?;
                        crate::services::persistence::check_file_lock(
                            conn,
                            &current.id,
                            &actor,
                            Some(&token),
                        )?;
                        if current.parent_id != original.parent_id
                            || current.room_id != original.room_id
                        {
                            return Err(AppError::Conflict(
                                "File moved; reload before saving".into(),
                            ));
                        }
                        if sibling_exists_sync(conn, &current, &new_file.name, None)? {
                            return Err(AppError::Conflict(
                                "A sibling item with that name already exists".into(),
                            ));
                        }
                        crate::services::persistence::adjust_quota(
                            conn,
                            &new_file.owner_id,
                            new_file.size,
                        )?;
                        diesel::RunQueryDsl::execute(
                            diesel::insert_into(f::files).values(&new_file),
                            conn,
                        )?;
                        Ok(())
                    })
                })
                .await;
            if let Err(error) = insertion {
                render_wopi_error(res, error);
                return;
            }

            res.status_code(StatusCode::OK);
            res.render(Json(put_relative_response(
                &base_url,
                &result_id,
                &result_name,
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

            match sibling_exists(&mut conn, &file_row, &requested_name, Some(&file_row.id)).await {
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

            let actor = uid.clone();
            let token = request_lock.clone();
            let target_name = requested_name.clone();
            let session = session.clone();
            let renaming = state
                .db
                .run(move |conn| {
                    conn.immediate_transaction::<_, AppError, _>(|conn| {
                        session.validate_write_in_transaction(conn)?;
                        let current =
                            diesel::RunQueryDsl::first::<File>(f::files.find(&file_row.id), conn)?;
                        crate::services::persistence::authorize(conn, &current, &actor, "editor")?;
                        crate::services::persistence::check_file_lock(
                            conn,
                            &current.id,
                            &actor,
                            Some(&token),
                        )?;
                        if sibling_exists_sync(conn, &current, &target_name, Some(&current.id))? {
                            return Err(AppError::Conflict(
                                "A sibling item with that name already exists".into(),
                            ));
                        }
                        let now = Utc::now().to_rfc3339();
                        diesel::RunQueryDsl::execute(
                            diesel::update(f::files.find(&current.id))
                                .set((f::name.eq(target_name), f::updated_at.eq(&now))),
                            conn,
                        )?;
                        Ok(now)
                    })
                })
                .await;
            let now = match renaming {
                Ok(now) => now,
                Err(error) => {
                    render_wopi_error(res, error);
                    return;
                }
            };

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
