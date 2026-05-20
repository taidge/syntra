use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{NewUserSetting, UserSetting};
use contrix_core::schema::user_settings::dsl as us;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /settings  (List all settings for current user)
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_settings(depot: &mut Depot, res: &mut Response) {
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

    match us::user_settings
        .filter(us::user_id.eq(&uid))
        .order(us::setting_key.asc())
        .load::<UserSetting>(&mut conn)
    {
        Ok(rows) => {
            let settings: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|s| {
                    serde_json::json!({
                        "id": s.id,
                        "user_id": s.user_id,
                        "key": s.setting_key,
                        "value": s.setting_value,
                        "updated_at": s.updated_at,
                    })
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(settings)));
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
// GET /settings/:key  (Get a specific setting)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_setting(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let key_param: String = req.param("key").unwrap_or_default();

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

    match us::user_settings
        .filter(us::user_id.eq(&uid))
        .filter(us::setting_key.eq(&key_param))
        .first::<UserSetting>(&mut conn)
        .optional()
    {
        Ok(Some(s)) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "id": s.id,
                "user_id": s.user_id,
                "key": s.setting_key,
                "value": s.setting_value,
                "updated_at": s.updated_at,
            }))));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Setting not found".into()).to_response(),
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
// PUT /settings/:key  (Set a setting — upsert)
// ---------------------------------------------------------------------------

#[handler]
pub async fn set_setting(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let key_param: String = req.param("key").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct SetSettingRequest {
        value: String,
    }

    let body: SetSettingRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

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

    // Check if setting already exists.
    let existing = us::user_settings
        .filter(us::user_id.eq(&uid))
        .filter(us::setting_key.eq(&key_param))
        .first::<UserSetting>(&mut conn)
        .optional();

    match existing {
        Ok(Some(record)) => {
            // Update existing.
            if let Err(e) = diesel::update(us::user_settings.filter(us::id.eq(&record.id)))
                .set((
                    us::setting_value.eq(&body.value),
                    us::updated_at.eq(now.to_rfc3339()),
                ))
                .execute(&mut conn)
            {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Database(e.to_string()).to_response(),
                )));
                return;
            }

            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "id": record.id,
                "user_id": uid,
                "key": key_param,
                "value": body.value,
                "updated_at": now.to_rfc3339(),
            }))));
        }
        Ok(None) => {
            // Insert new.
            let setting_id = Uuid::new_v4().to_string();
            let new_setting = NewUserSetting {
                id: setting_id.clone(),
                user_id: uid.clone(),
                setting_key: key_param.clone(),
                setting_value: body.value.clone(),
                updated_at: now.to_rfc3339(),
            };

            if let Err(e) = diesel::insert_into(us::user_settings)
                .values(&new_setting)
                .execute(&mut conn)
            {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Database(e.to_string()).to_response(),
                )));
                return;
            }

            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "id": setting_id,
                "user_id": uid,
                "key": key_param,
                "value": body.value,
                "updated_at": now.to_rfc3339(),
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
// DELETE /settings/:key  (Delete a setting)
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_setting(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let key_param: String = req.param("key").unwrap_or_default();

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
        us::user_settings
            .filter(us::user_id.eq(&uid))
            .filter(us::setting_key.eq(&key_param)),
    )
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Setting not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Setting deleted" }),
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
// DELETE /settings  (Delete all settings for user)
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_all_settings(depot: &mut Depot, res: &mut Response) {
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

    match diesel::delete(us::user_settings.filter(us::user_id.eq(&uid))).execute(&mut conn) {
        Ok(count) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "message": "All settings deleted",
                "deleted_count": count,
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
