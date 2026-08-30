use chrono::Utc;
use contrix_core::dto::{ApiResponse, FileResponse};
use contrix_core::error::AppError;
use contrix_core::models::File;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::users::dsl as u;
use diesel::prelude::*;
use salvo::prelude::*;

use crate::middleware::auth::get_user_id;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn file_model_to_response(row: File) -> FileResponse {
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
        shared_with_count: 0,
        document_id: None,
    }
}

// ---------------------------------------------------------------------------
// GET /trash
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_trashed(depot: &mut Depot, res: &mut Response) {
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

    match f::files
        .filter(f::owner_id.eq(&uid))
        .filter(f::is_trashed.eq(true))
        .order(f::trashed_at.desc())
        .load::<File>(&mut conn)
    {
        Ok(rows) => {
            let mut file_list: Vec<FileResponse> =
                rows.into_iter().map(file_model_to_response).collect();
            for r in &mut file_list {
                r.shared_with_count =
                    crate::handlers::access::count_shares_for_file(&mut conn, &r.id.to_string());
            }
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(file_list)));
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
// POST /trash/:id/restore
// ---------------------------------------------------------------------------

#[handler]
pub async fn restore_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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
    let now = Utc::now();

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

    match diesel::update(
        f::files
            .filter(f::id.eq(&file_id_param))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_trashed.eq(true)),
    )
    .set((
        f::is_trashed.eq(false),
        f::trashed_at.eq(None::<String>),
        f::updated_at.eq(now.to_rfc3339()),
    ))
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Trashed file not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "File restored" }),
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
// DELETE /trash/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn permanently_delete(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    // Fetch the file to get its path and size for cleanup.
    let file_row = match f::files
        .filter(f::id.eq(&file_id_param))
        .filter(f::owner_id.eq(&uid))
        .filter(f::is_trashed.eq(true))
        .first::<File>(&mut conn)
        .optional()
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Trashed file not found".into()).to_response(),
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

    let file_size = file_row.size;
    let file_path = file_row.path.clone();
    let is_folder = file_row.is_folder;

    // Delete from DB.
    if let Err(e) = diesel::delete(f::files.filter(f::id.eq(&file_id_param))).execute(&mut conn) {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    // Delete from physical storage (only for non-folder files).
    if !is_folder {
        let _ = state.storage.delete_file(&file_path).await;

        // Update user's storage_used.
        let _ = diesel::update(u::users.filter(u::id.eq(&uid)))
            .set(u::storage_used.eq(u::storage_used - file_size))
            .execute(&mut conn);
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(
        serde_json::json!({ "message": "File permanently deleted" }),
    )));
}

// ---------------------------------------------------------------------------
// DELETE /trash
// ---------------------------------------------------------------------------

#[handler]
pub async fn empty_trash(depot: &mut Depot, res: &mut Response) {
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

    // Fetch all trashed files to get paths and sizes.
    let trashed_files: Vec<File> = match f::files
        .filter(f::owner_id.eq(&uid))
        .filter(f::is_trashed.eq(true))
        .load::<File>(&mut conn)
    {
        Ok(rows) => rows,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let mut total_freed: i64 = 0;

    for file_row in &trashed_files {
        if !file_row.is_folder {
            let _ = state.storage.delete_file(&file_row.path).await;
            total_freed += file_row.size;
        }
    }

    // Delete all trashed files from DB.
    match diesel::delete(
        f::files
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_trashed.eq(true)),
    )
    .execute(&mut conn)
    {
        Ok(count) => {
            // Update user's storage_used.
            if total_freed > 0 {
                let _ = diesel::update(u::users.filter(u::id.eq(&uid)))
                    .set(u::storage_used.eq(u::storage_used - total_freed))
                    .execute(&mut conn);
            }

            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Trash emptied", "count": count }),
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
// GET /trash/count
// ---------------------------------------------------------------------------

#[handler]
pub async fn trash_count(depot: &mut Depot, res: &mut Response) {
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

    match f::files
        .filter(f::owner_id.eq(&uid))
        .filter(f::is_trashed.eq(true))
        .count()
        .get_result::<i64>(&mut conn)
    {
        Ok(count) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({ "count": count }))));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
