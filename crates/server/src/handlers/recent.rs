use chrono::Utc;
use contrix_core::dto::{ApiResponse, FileResponse};
use contrix_core::error::AppError;
use contrix_core::models::File;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::recent_files::dsl as rf;
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
// GET /recent
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_recent(depot: &mut Depot, res: &mut Response) {
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

    // Get recent file IDs ordered by accessed_at DESC, limit 50.
    let recent_file_ids: Vec<String> = match rf::recent_files
        .filter(rf::user_id.eq(&uid))
        .order(rf::accessed_at.desc())
        .limit(50)
        .select(rf::file_id)
        .load::<String>(&mut conn)
    {
        Ok(ids) => ids,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if recent_file_ids.is_empty() {
        res.status_code(StatusCode::OK);
        res.render(Json(ApiResponse::ok(Vec::<FileResponse>::new())));
        return;
    }

    match f::files
        .filter(f::id.eq_any(&recent_file_ids))
        .load::<File>(&mut conn)
    {
        Ok(rows) => {
            // Preserve the recent ordering by building a lookup and sorting.
            let mut file_map: std::collections::HashMap<String, File> =
                rows.into_iter().map(|row| (row.id.clone(), row)).collect();
            let mut file_list: Vec<FileResponse> = recent_file_ids
                .into_iter()
                .filter_map(|id| file_map.remove(&id))
                .map(file_model_to_response)
                .collect();
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
// DELETE /recent
// ---------------------------------------------------------------------------

#[handler]
pub async fn clear_recent(depot: &mut Depot, res: &mut Response) {
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

    match diesel::delete(rf::recent_files.filter(rf::user_id.eq(&uid))).execute(&mut conn) {
        Ok(count) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Recent files cleared", "count": count }),
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
