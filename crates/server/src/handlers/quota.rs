use std::sync::Arc;

use diesel::prelude::*;
use salvo::prelude::*;

use contrix_core::dto::{ApiResponse, QuotaResponse, UpdateQuotaRequest};
use contrix_core::error::AppError;
use contrix_core::schema::users::dsl as u;

use crate::middleware::auth::{get_user_id, get_user_role};
use crate::state::AppState;

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

// ---------------------------------------------------------------------------
// GET /quota — get current user's quota info
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_my_quota(depot: &mut Depot, res: &mut Response) {
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

    match u::users
        .filter(u::id.eq(&uid))
        .select((u::id, u::storage_used, u::storage_quota))
        .first::<(String, i64, Option<i64>)>(&mut conn)
    {
        Ok((user_id, used, quota)) => {
            let quota_val = quota.unwrap_or(0);
            let usage_percent = if quota_val > 0 {
                (used as f64 / quota_val as f64) * 100.0
            } else {
                0.0
            };
            let resp = QuotaResponse {
                user_id: user_id.parse().unwrap_or_default(),
                storage_used: used,
                storage_quota: quota_val,
                usage_percent,
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(resp)));
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
// GET /admin/quota/:user_id — get a specific user's quota (admin)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_user_quota(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let target_user_id: String = req.param("user_id").unwrap_or_default();

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

    match u::users
        .filter(u::id.eq(&target_user_id))
        .select((u::id, u::storage_used, u::storage_quota))
        .first::<(String, i64, Option<i64>)>(&mut conn)
    {
        Ok((user_id, used, quota)) => {
            let quota_val = quota.unwrap_or(0);
            let usage_percent = if quota_val > 0 {
                (used as f64 / quota_val as f64) * 100.0
            } else {
                0.0
            };
            let resp = QuotaResponse {
                user_id: user_id.parse().unwrap_or_default(),
                storage_used: used,
                storage_quota: quota_val,
                usage_percent,
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(resp)));
        }
        Err(diesel::result::Error::NotFound) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("User not found".into()).to_response(),
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
// PUT /admin/quota — set user quota (admin)
// ---------------------------------------------------------------------------

#[handler]
pub async fn set_user_quota(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();

    let body: UpdateQuotaRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Invalid request body".into()).to_response(),
            )));
            return;
        }
    };

    let target_user_id = body.user_id.to_string();
    let quota_bytes = body.storage_quota;
    if quota_bytes < 0 {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("storage_quota must be non-negative bytes".into()).to_response(),
        )));
        return;
    }

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

    match diesel::update(u::users.filter(u::id.eq(&target_user_id)))
        .set(u::storage_quota.eq(Some(quota_bytes)))
        .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("User not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            // Fetch updated values
            match u::users
                .filter(u::id.eq(&target_user_id))
                .select((u::id, u::storage_used, u::storage_quota))
                .first::<(String, i64, Option<i64>)>(&mut conn)
            {
                Ok((user_id, used, quota)) => {
                    let quota_val = quota.unwrap_or(0);
                    let usage_percent = if quota_val > 0 {
                        (used as f64 / quota_val as f64) * 100.0
                    } else {
                        0.0
                    };
                    let resp = QuotaResponse {
                        user_id: user_id.parse().unwrap_or_default(),
                        storage_used: used,
                        storage_quota: quota_val,
                        usage_percent,
                    };
                    res.status_code(StatusCode::OK);
                    res.render(Json(ApiResponse::ok(resp)));
                }
                Err(e) => {
                    res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                    res.render(Json(ApiResponse::<()>::err(
                        AppError::Database(e.to_string()).to_response(),
                    )));
                }
            }
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
