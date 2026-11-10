use chrono::{Duration, Utc};
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::FileLock;
use contrix_core::schema::file_locks::dsl as fl;

use crate::handlers::access::load_accessible_file;
use crate::middleware::auth::get_user_id;

/// Authorize the caller for `required_role` on a file, rendering the proper
/// error and returning `false` when access is denied or the file is missing.
async fn ensure_file_access(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    file_id: &str,
    user_id: &str,
    required_role: &str,
    res: &mut Response,
) -> bool {
    match load_accessible_file(conn, file_id, user_id, required_role).await {
        Ok(Some(_)) => true,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
            false
        }
        Err(e) => {
            let code = match &e {
                AppError::Forbidden(_) => StatusCode::FORBIDDEN,
                AppError::NotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            res.status_code(code);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            false
        }
    }
}

// ---------------------------------------------------------------------------
// POST /files/:file_id/lock  (Acquire lock)
// ---------------------------------------------------------------------------

#[handler]
pub async fn acquire_lock(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("file_id").unwrap_or_default();

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

    // Verify the file exists and the caller may edit it (locking is a write).
    if !ensure_file_access(&mut conn, &file_id_param, &uid, "editor", res).await {
        return;
    }

    match crate::services::persistence::acquire_file_lock(
        &state.db,
        &file_id_param,
        &uid,
        &Uuid::new_v4().to_string(),
    )
    .await
    {
        Ok(lock) => {
            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(lock)));
        }
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
        }
    }
}

// ---------------------------------------------------------------------------
// DELETE /files/:file_id/lock  (Release lock)
// ---------------------------------------------------------------------------

#[handler]
pub async fn release_lock(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("file_id").unwrap_or_default();

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

    match diesel::delete(
        fl::file_locks
            .filter(fl::file_id.eq(&file_id_param))
            .filter(fl::locked_by.eq(&uid)),
    )
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("No lock found for this file by current user".into())
                    .to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Lock released" }),
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
// GET /files/:file_id/lock  (Check lock status)
// ---------------------------------------------------------------------------

#[handler]
pub async fn check_lock(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("file_id").unwrap_or_default();

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

    if !ensure_file_access(&mut conn, &file_id_param, &uid, "viewer", res).await {
        return;
    }

    match fl::file_locks
        .filter(fl::file_id.eq(&file_id_param))
        .filter(fl::expires_at.gt(Utc::now().to_rfc3339()))
        .first::<FileLock>(&mut conn)
        .await
        .optional()
    {
        Ok(Some(lock)) => {
            // Only expose the opaque lock_token to the lock holder; other
            // collaborators just need to know it is locked and by whom.
            let lock_token = if lock.locked_by == uid {
                Some(lock.lock_token.clone())
            } else {
                None
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "locked": true,
                "id": lock.id,
                "file_id": lock.file_id,
                "locked_by": lock.locked_by,
                "lock_token": lock_token,
                "lock_type": lock.lock_type,
                "expires_at": lock.expires_at,
                "created_at": lock.created_at,
            }))));
        }
        Ok(None) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "locked": false,
            }))));
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
// POST /files/:file_id/lock/refresh  (Refresh lock)
// ---------------------------------------------------------------------------

#[handler]
pub async fn refresh_lock(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("file_id").unwrap_or_default();

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

    if !ensure_file_access(&mut conn, &file_id_param, &uid, "editor", res).await {
        return;
    }
    let new_expires = (Utc::now() + Duration::minutes(30)).to_rfc3339();

    match diesel::update(
        fl::file_locks
            .filter(fl::file_id.eq(&file_id_param))
            .filter(fl::locked_by.eq(&uid))
            .filter(fl::expires_at.gt(Utc::now().to_rfc3339())),
    )
    .set(fl::expires_at.eq(&new_expires))
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("No lock found for this file by current user".into())
                    .to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "message": "Lock refreshed",
                "expires_at": new_expires,
            }))));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
