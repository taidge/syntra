use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{IntegrationSetting, NewIntegrationSetting};
use contrix_core::schema::integration_settings::dsl as is_dsl;

use crate::middleware::auth::{get_user_id, get_user_role};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /admin/integrations  (List all integration settings)
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_integrations(depot: &mut Depot, res: &mut Response) {
    if get_user_role(depot) != Some("admin".to_string()) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Admin required".into()).to_response(),
        )));
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();

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

    match is_dsl::integration_settings
        .order(is_dsl::service_name.asc())
        .load::<IntegrationSetting>(&mut conn)
    {
        Ok(rows) => {
            let integrations: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|i| {
                    let extra: serde_json::Value = i
                        .extra_config
                        .as_deref()
                        .and_then(|v| serde_json::from_str(v).ok())
                        .unwrap_or(serde_json::Value::Null);
                    serde_json::json!({
                        "id": i.id,
                        "service_name": i.service_name,
                        "is_enabled": i.is_enabled,
                        "api_url": i.api_url,
                        "has_api_key": i.api_key.as_deref().map(|k| !k.is_empty()).unwrap_or(false),
                        "extra_config": extra,
                        "updated_by": i.updated_by,
                        "updated_at": i.updated_at,
                    })
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(integrations)));
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
// GET /admin/integrations/:service_name  (Get specific integration)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_integration(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if get_user_role(depot) != Some("admin".to_string()) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Admin required".into()).to_response(),
        )));
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let service_name_param: String = req.param("service_name").unwrap_or_default();

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

    match is_dsl::integration_settings
        .filter(is_dsl::service_name.eq(&service_name_param))
        .first::<IntegrationSetting>(&mut conn)
        .optional()
    {
        Ok(Some(i)) => {
            let extra: serde_json::Value = i
                .extra_config
                .as_deref()
                .and_then(|v| serde_json::from_str(v).ok())
                .unwrap_or(serde_json::Value::Null);
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "id": i.id,
                "service_name": i.service_name,
                "is_enabled": i.is_enabled,
                "api_url": i.api_url,
                "has_api_key": i.api_key.as_deref().map(|k| !k.is_empty()).unwrap_or(false),
                "extra_config": extra,
                "updated_by": i.updated_by,
                "updated_at": i.updated_at,
            }))));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Integration not found".into()).to_response(),
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
// PUT /admin/integrations/:service_name  (Update integration — upsert)
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_integration(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if get_user_role(depot) != Some("admin".to_string()) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Admin required".into()).to_response(),
        )));
        return;
    }

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

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let service_name_param: String = req.param("service_name").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct UpdateIntegrationRequest {
        is_enabled: Option<bool>,
        api_url: Option<String>,
        api_key: Option<String>,
        extra_config: Option<serde_json::Value>,
    }

    let body: UpdateIntegrationRequest = match req.parse_json().await {
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
    let extra_str = body
        .extra_config
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default());

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

    let existing = is_dsl::integration_settings
        .filter(is_dsl::service_name.eq(&service_name_param))
        .first::<IntegrationSetting>(&mut conn)
        .optional();

    match existing {
        Ok(Some(record)) => {
            let new_enabled = body.is_enabled.unwrap_or(record.is_enabled);
            let new_url = body.api_url.or(record.api_url.clone());
            let new_key = body.api_key.or(record.api_key.clone());
            let new_extra = extra_str.or(record.extra_config.clone());

            if let Err(e) =
                diesel::update(is_dsl::integration_settings.filter(is_dsl::id.eq(&record.id)))
                    .set((
                        is_dsl::is_enabled.eq(new_enabled),
                        is_dsl::api_url.eq(&new_url),
                        is_dsl::api_key.eq(&new_key),
                        is_dsl::extra_config.eq(&new_extra),
                        is_dsl::updated_by.eq(&uid),
                        is_dsl::updated_at.eq(now.to_rfc3339()),
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
                "service_name": service_name_param,
                "is_enabled": new_enabled,
                "api_url": new_url,
                "api_key": new_key,
                "extra_config": body.extra_config,
                "updated_by": uid,
                "updated_at": now.to_rfc3339(),
            }))));
        }
        Ok(None) => {
            let integration_id = Uuid::new_v4().to_string();
            let new_integration = NewIntegrationSetting {
                id: integration_id.clone(),
                service_name: service_name_param.clone(),
                is_enabled: body.is_enabled.unwrap_or(true),
                api_url: body.api_url.clone(),
                api_key: body.api_key.clone(),
                extra_config: extra_str,
                updated_by: uid.clone(),
                updated_at: now.to_rfc3339(),
            };

            if let Err(e) = diesel::insert_into(is_dsl::integration_settings)
                .values(&new_integration)
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
                "id": integration_id,
                "service_name": service_name_param,
                "is_enabled": body.is_enabled.unwrap_or(true),
                "api_url": body.api_url,
                "api_key": body.api_key,
                "extra_config": body.extra_config,
                "updated_by": uid,
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
// DELETE /admin/integrations/:service_name  (Delete integration config)
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_integration(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if get_user_role(depot) != Some("admin".to_string()) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Admin required".into()).to_response(),
        )));
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let service_name_param: String = req.param("service_name").unwrap_or_default();

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
        is_dsl::integration_settings.filter(is_dsl::service_name.eq(&service_name_param)),
    )
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Integration not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Integration deleted" }),
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
