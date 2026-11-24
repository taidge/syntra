use chrono::Utc;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use syntra_core::dto::{ApiResponse, PasswordPolicyRequest, PasswordPolicyResponse};
use syntra_core::error::AppError;
use syntra_core::models::NewPasswordPolicy;
use syntra_core::schema::password_policies::dsl as pp;

use crate::middleware::auth::{get_user_id, get_user_role};
use crate::services::password::PasswordService;

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
// GET /admin/password-policy — get current policy
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_policy(depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);

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

    let policy = PasswordService::get_policy(&mut conn).await;

    let resp = PasswordPolicyResponse {
        min_length: policy.min_length,
        require_uppercase: policy.require_uppercase,
        require_lowercase: policy.require_lowercase,
        require_digit: policy.require_digit,
        require_special: policy.require_special,
        max_age_days: policy.max_age_days,
    };

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(resp)));
}

// ---------------------------------------------------------------------------
// PUT /admin/password-policy — update policy
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_policy(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

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

    let body: PasswordPolicyRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Invalid request body".into()).to_response(),
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

    // Fetch existing policy to merge partial updates.
    let existing = PasswordService::get_policy(&mut conn).await;
    let now = Utc::now().to_rfc3339();

    let new_min_length = body.min_length.unwrap_or(existing.min_length);
    let new_require_uppercase = body.require_uppercase.unwrap_or(existing.require_uppercase);
    let new_require_lowercase = body.require_lowercase.unwrap_or(existing.require_lowercase);
    let new_require_digit = body.require_digit.unwrap_or(existing.require_digit);
    let new_require_special = body.require_special.unwrap_or(existing.require_special);
    let new_max_age_days = if body.max_age_days.is_some() {
        body.max_age_days
    } else {
        existing.max_age_days
    };

    // Delete existing policy row(s) and insert a fresh one (upsert).
    let _ = diesel::delete(pp::password_policies)
        .execute(&mut conn)
        .await;

    let new_policy = NewPasswordPolicy {
        id: Uuid::new_v4().to_string(),
        min_length: new_min_length,
        require_uppercase: new_require_uppercase,
        require_lowercase: new_require_lowercase,
        require_digit: new_require_digit,
        require_special: new_require_special,
        max_age_days: new_max_age_days,
        updated_by: uid,
        updated_at: now,
    };

    match diesel::insert_into(pp::password_policies)
        .values(&new_policy)
        .execute(&mut conn)
        .await
    {
        Ok(_) => {
            let resp = PasswordPolicyResponse {
                min_length: new_policy.min_length,
                require_uppercase: new_policy.require_uppercase,
                require_lowercase: new_policy.require_lowercase,
                require_digit: new_policy.require_digit,
                require_special: new_policy.require_special,
                max_age_days: new_policy.max_age_days,
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
