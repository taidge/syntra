use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{NewWatermarkSettings, WatermarkSettings};
use contrix_core::schema::watermark_settings::dsl as ws;

use crate::middleware::auth::{get_user_id, get_user_role};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /admin/watermark  (Get watermark settings)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_watermark(depot: &mut Depot, res: &mut Response) {
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

    match ws::watermark_settings
        .first::<WatermarkSettings>(&mut conn)
        .optional()
    {
        Ok(Some(settings)) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "id": settings.id,
                "text": settings.text,
                "opacity": settings.opacity,
                "font_size": settings.font_size,
                "rotation": settings.rotation,
                "is_enabled": settings.is_enabled,
                "updated_by": settings.updated_by,
                "updated_at": settings.updated_at,
            }))));
        }
        Ok(None) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "is_enabled": false,
                "text": null,
                "opacity": 0.3,
                "font_size": 24,
                "rotation": -45,
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
// PUT /admin/watermark  (Update watermark settings — upsert)
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_watermark(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    #[derive(serde::Deserialize)]
    struct UpdateWatermarkRequest {
        text: Option<String>,
        opacity: Option<f32>,
        font_size: Option<i32>,
        rotation: Option<i32>,
        is_enabled: Option<bool>,
    }

    let body: UpdateWatermarkRequest = match req.parse_json().await {
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

    let existing = ws::watermark_settings
        .first::<WatermarkSettings>(&mut conn)
        .optional();

    match existing {
        Ok(Some(record)) => {
            let new_text = body.text.or(record.text.clone());
            let new_opacity = body.opacity.unwrap_or(record.opacity);
            let new_font_size = body.font_size.unwrap_or(record.font_size);
            let new_rotation = body.rotation.unwrap_or(record.rotation);
            let new_enabled = body.is_enabled.unwrap_or(record.is_enabled);

            if let Err(e) = diesel::update(ws::watermark_settings.filter(ws::id.eq(&record.id)))
                .set((
                    ws::text.eq(&new_text),
                    ws::opacity.eq(new_opacity),
                    ws::font_size.eq(new_font_size),
                    ws::rotation.eq(new_rotation),
                    ws::is_enabled.eq(new_enabled),
                    ws::updated_by.eq(&uid),
                    ws::updated_at.eq(now.to_rfc3339()),
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
                "text": new_text,
                "opacity": new_opacity,
                "font_size": new_font_size,
                "rotation": new_rotation,
                "is_enabled": new_enabled,
                "updated_by": uid,
                "updated_at": now.to_rfc3339(),
            }))));
        }
        Ok(None) => {
            let settings_id = Uuid::new_v4().to_string();
            let new_settings = NewWatermarkSettings {
                id: settings_id.clone(),
                text: body.text.clone(),
                opacity: body.opacity.unwrap_or(0.3),
                font_size: body.font_size.unwrap_or(24),
                rotation: body.rotation.unwrap_or(-45),
                is_enabled: body.is_enabled.unwrap_or(true),
                updated_by: uid.clone(),
                updated_at: now.to_rfc3339(),
            };

            if let Err(e) = diesel::insert_into(ws::watermark_settings)
                .values(&new_settings)
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
                "id": settings_id,
                "text": body.text,
                "opacity": body.opacity.unwrap_or(0.3),
                "font_size": body.font_size.unwrap_or(24),
                "rotation": body.rotation.unwrap_or(-45),
                "is_enabled": body.is_enabled.unwrap_or(true),
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
// DELETE /admin/watermark  (Disable watermark)
// ---------------------------------------------------------------------------

#[handler]
pub async fn disable_watermark(depot: &mut Depot, res: &mut Response) {
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

    // Set is_enabled = false on all watermark settings rows.
    match diesel::update(ws::watermark_settings)
        .set((
            ws::is_enabled.eq(false),
            ws::updated_by.eq(&uid),
            ws::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn)
    {
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Watermark disabled" }),
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
