use std::sync::Arc;

use chrono::Utc;
use salvo::prelude::*;

use contrix_core::dto::{ApiResponse, BackupResponse};
use contrix_core::error::AppError;

use crate::middleware::auth::{get_user_id, get_user_role};
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

/// Derive the absolute path of the SQLite database file from the connection URL.
///
/// The URL is expected to be in the form `sqlite://path/to/db.sqlite` or simply
/// a file path.
fn db_path_from_url(url: &str) -> String {
    url.strip_prefix("sqlite://").unwrap_or(url).to_string()
}

/// Return the backup directory path (sibling `data/backups/` relative to the DB).
fn backup_dir() -> std::path::PathBuf {
    std::path::PathBuf::from("data/backups")
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

    let db_file = db_path_from_url(&state.config.database.url);
    let now = Utc::now();
    let timestamp = now.format("%Y%m%d_%H%M%S").to_string();
    let backup_filename = format!("backup_{}.db", timestamp);
    let backup_path = backup_dir().join(&backup_filename);

    // Ensure backup directory exists.
    if let Err(e) = tokio::fs::create_dir_all(backup_dir()).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to create backup directory: {}", e)).to_response(),
        )));
        return;
    }

    // Copy the database file.
    if let Err(e) = tokio::fs::copy(&db_file, &backup_path).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to copy database: {}", e)).to_response(),
        )));
        return;
    }

    // Get the size of the backup file.
    let size = match tokio::fs::metadata(&backup_path).await {
        Ok(meta) => meta.len() as i64,
        Err(_) => 0,
    };

    let resp = BackupResponse {
        id: timestamp,
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

    let _state = depot.obtain::<Arc<AppState>>().unwrap().clone();
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
    let dir = backup_dir();
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
                if !file_name.ends_with(".db") {
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

    let _state = depot.obtain::<Arc<AppState>>().unwrap().clone();
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
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Invalid filename".into()).to_response(),
        )));
        return;
    }

    let file_path = backup_dir().join(&filename);

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
    if body.filename.contains("..") || body.filename.contains('/') || body.filename.contains('\\') {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Invalid filename".into()).to_response(),
        )));
        return;
    }

    let backup_file = backup_dir().join(&body.filename);
    let db_file = db_path_from_url(&state.config.database.url);

    // Verify the backup file exists.
    if !backup_file.exists() {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Backup file not found".into()).to_response(),
        )));
        return;
    }

    // Copy the backup file over the current database.
    if let Err(e) = tokio::fs::copy(&backup_file, &db_file).await {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Storage(format!("Failed to restore database: {}", e)).to_response(),
        )));
        return;
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Database restored from backup",
        "filename": body.filename,
    }))));
}
