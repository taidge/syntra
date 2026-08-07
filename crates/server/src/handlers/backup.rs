use std::sync::Arc;

use chrono::Utc;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, BackupResponse};
use contrix_core::error::AppError;

use crate::middleware::auth::{get_user_id, get_user_role};
use crate::services::database_backup;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Verify the caller has admin role; returns `false` and writes a 403 if not.
fn require_admin(depot: &Depot, res: &mut Response) -> bool {
    match get_user_role(depot) {
        Some(role) if role == "admin" => true,
        _ => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Forbidden("Admin access required".into()).to_response(),
            )));
            false
        }
    }
}

/// Keep SQLite's non-Send serialized buffer inside a synchronous scope so the
/// Salvo handler never carries it across an await point.
fn serialize_database(state: &AppState) -> Result<Vec<u8>, AppError> {
    let mut conn = state
        .db
        .get()
        .map_err(|error| AppError::Database(error.to_string()))?;
    let snapshot = conn.serialize_database_to_buffer();
    Ok(snapshot.as_slice().to_vec())
}

fn file_backed_database_path(state: &AppState, res: &mut Response) -> Option<std::path::PathBuf> {
    match database_backup::sqlite_url_to_path(&state.config.database.url) {
        Ok(path) if database_backup::is_file_backed(&path) => Some(path),
        Ok(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Database backup requires file-backed SQLite".into())
                    .to_response(),
            )));
            None
        }
        Err(error) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(error.to_string()).to_response(),
            )));
            None
        }
    }
}

// ---------------------------------------------------------------------------
// POST /admin/backup
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_backup(depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let _uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let Some(db_file) = file_backed_database_path(&state, res) else {
        return;
    };
    let now = Utc::now();
    let backup_filename = database_backup::new_backup_filename(now);
    let backup_path = database_backup::backup_directory(&db_file).join(&backup_filename);

    // Ensure backup directory exists.
    if let Err(e) = tokio::fs::create_dir_all(database_backup::backup_directory(&db_file)).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to create backup directory: {}", e)).to_response(),
        )));
        return;
    }

    // Serialize through a live SQLite connection so committed WAL data is part
    // of one consistent snapshot.
    let snapshot_bytes = match serialize_database(&state) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
            return;
        }
    };
    let temporary = backup_path.with_extension(format!("tmp-{}", Uuid::new_v4().simple()));
    if let Err(e) = tokio::fs::write(&temporary, snapshot_bytes).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to write database snapshot: {e}")).to_response(),
        )));
        return;
    }
    if let Err(e) = tokio::fs::rename(&temporary, &backup_path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to finalize database snapshot: {e}")).to_response(),
        )));
        return;
    }

    // Get the size of the backup file.
    let size = match tokio::fs::metadata(&backup_path).await {
        Ok(meta) => meta.len() as i64,
        Err(_) => 0,
    };

    let resp = BackupResponse {
        id: backup_filename
            .strip_prefix("backup_")
            .and_then(|name| name.strip_suffix(".db"))
            .unwrap_or(&backup_filename)
            .to_string(),
        file_name: backup_filename,
        size,
        created_at: now,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(resp)));
}

// ---------------------------------------------------------------------------
// GET /admin/backup
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_backups(depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let _uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    // Ensure backup directory exists before listing.
    let Some(db_file) = file_backed_database_path(&state, res) else {
        return;
    };
    let dir = database_backup::backup_directory(&db_file);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to access backup directory: {}", e)).to_response(),
        )));
        return;
    }

    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Storage(format!("Failed to read backup directory: {}", e)).to_response(),
            )));
            return;
        }
    };

    let mut backups: Vec<BackupResponse> = Vec::new();
    loop {
        match entries.next_entry().await {
            Ok(Some(entry)) => {
                let file_name = entry.file_name().to_string_lossy().to_string();
                if !database_backup::is_valid_backup_filename(&file_name) {
                    continue;
                }
                let meta = entry.metadata().await;
                let size = meta.as_ref().map(|m| m.len() as i64).unwrap_or(0);
                let created_at = meta
                    .as_ref()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .map(chrono::DateTime::<Utc>::from)
                    .unwrap_or_else(Utc::now);

                // Extract the timestamp portion from the filename for the id.
                let id = file_name
                    .strip_prefix("backup_")
                    .and_then(|s| s.strip_suffix(".db"))
                    .unwrap_or(&file_name)
                    .to_string();

                backups.push(BackupResponse {
                    id,
                    file_name,
                    size,
                    created_at,
                });
            }
            Ok(None) => break,
            Err(e) => {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Storage(format!("Failed to read backup entry: {}", e)).to_response(),
                )));
                return;
            }
        }
    }

    // Sort by created_at descending.
    backups.sort_by_key(|backup| std::cmp::Reverse(backup.created_at));

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(backups)));
}

// ---------------------------------------------------------------------------
// GET /admin/backup/:filename/download
// ---------------------------------------------------------------------------

#[handler]
pub async fn download_backup(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let _uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let filename: String = req.param("filename").unwrap_or_default();

    // Sanitize filename to prevent path traversal.
    if !database_backup::is_valid_backup_filename(&filename) {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Invalid filename".into()).to_response(),
        )));
        return;
    }

    let Some(db_file) = file_backed_database_path(&state, res) else {
        return;
    };
    let file_path = database_backup::backup_directory(&db_file).join(&filename);

    let data = match tokio::fs::read(&file_path).await {
        Ok(d) => d,
        Err(_) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Backup file not found".into()).to_response(),
            )));
            return;
        }
    };

    res.status_code(StatusCode::OK);
    res.headers_mut()
        .insert("content-type", "application/octet-stream".parse().unwrap());
    res.headers_mut().insert(
        "content-disposition",
        format!("attachment; filename=\"{}\"", filename)
            .parse()
            .unwrap(),
    );
    res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(data)));
}

// ---------------------------------------------------------------------------
// POST /admin/backup/restore
// ---------------------------------------------------------------------------

#[handler]
pub async fn restore_backup(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let _uid = match get_user_id(depot) {
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
    struct RestoreRequest {
        filename: String,
    }

    let body: RestoreRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    // Sanitize filename to prevent path traversal.
    if !database_backup::is_valid_backup_filename(&body.filename) {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Invalid filename".into()).to_response(),
        )));
        return;
    }

    let Some(db_file) = file_backed_database_path(&state, res) else {
        return;
    };
    let backup_file = database_backup::backup_directory(&db_file).join(&body.filename);

    // Verify the backup file exists.
    if !backup_file.exists() {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Backup file not found".into()).to_response(),
        )));
        return;
    }

    // Never overwrite a database that is still open by the connection pool.
    // Validate and stage it for an atomic pre-pool switch at the next restart.
    if let Err(e) = database_backup::stage_restore(&backup_file, &db_file).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to stage database restore: {e}")).to_response(),
        )));
        return;
    }

    res.status_code(StatusCode::ACCEPTED);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Database restore staged; restart the server to apply it",
        "filename": body.filename,
        "restart_required": true,
    }))));
}
