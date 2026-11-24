use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use syntra_core::dto::{ApiResponse, BrandingResponse, UpdateBrandingRequest};
use syntra_core::error::AppError;
use syntra_core::models::{BrandingSettings, NewBrandingSettings};
use syntra_core::schema::branding_settings::dsl as bs;

use crate::middleware::auth::{get_user_id, get_user_role};

const APP_NAME: &str = "Syntra Office";
const LOGO_URL: &str = "/icons/syntra.svg";
const FAVICON_URL: &str = "/favicon.ico";

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

fn branding_to_response(row: BrandingSettings) -> BrandingResponse {
    BrandingResponse {
        logo_url: row.logo_url.or_else(|| Some(LOGO_URL.into())),
        favicon_url: row.favicon_url.or_else(|| Some(FAVICON_URL.into())),
        app_name: row.app_name,
        primary_color: row.primary_color,
        custom_css: row.custom_css,
    }
}

fn default_branding() -> BrandingResponse {
    BrandingResponse {
        logo_url: Some(LOGO_URL.into()),
        favicon_url: Some(FAVICON_URL.into()),
        app_name: APP_NAME.into(),
        primary_color: None,
        custom_css: None,
    }
}

// ---------------------------------------------------------------------------
// GET /admin/branding
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_branding(depot: &mut Depot, res: &mut Response) {
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

    match bs::branding_settings
        .first::<BrandingSettings>(&mut conn)
        .await
        .optional()
    {
        Ok(Some(row)) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(branding_to_response(row))));
        }
        Ok(None) => {
            // No branding settings exist yet; return defaults.
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(default_branding())));
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
// PUT /admin/branding
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_branding(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: UpdateBrandingRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
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

    let now = Utc::now();

    // Check if branding settings already exist.
    let existing = bs::branding_settings
        .first::<BrandingSettings>(&mut conn)
        .await
        .optional();

    match existing {
        Ok(Some(row)) => {
            // Update existing record.
            if let Some(ref logo_url) = body.logo_url {
                let _ = diesel::update(bs::branding_settings.filter(bs::id.eq(&row.id)))
                    .set(bs::logo_url.eq(Some(logo_url)))
                    .execute(&mut conn)
                    .await;
            }
            if let Some(ref favicon_url) = body.favicon_url {
                let _ = diesel::update(bs::branding_settings.filter(bs::id.eq(&row.id)))
                    .set(bs::favicon_url.eq(Some(favicon_url)))
                    .execute(&mut conn)
                    .await;
            }
            if let Some(ref app_name) = body.app_name {
                let _ = diesel::update(bs::branding_settings.filter(bs::id.eq(&row.id)))
                    .set(bs::app_name.eq(app_name))
                    .execute(&mut conn)
                    .await;
            }
            if let Some(ref primary_color) = body.primary_color {
                let _ = diesel::update(bs::branding_settings.filter(bs::id.eq(&row.id)))
                    .set(bs::primary_color.eq(Some(primary_color)))
                    .execute(&mut conn)
                    .await;
            }
            if let Some(ref custom_css) = body.custom_css {
                let _ = diesel::update(bs::branding_settings.filter(bs::id.eq(&row.id)))
                    .set(bs::custom_css.eq(Some(custom_css)))
                    .execute(&mut conn)
                    .await;
            }

            // Always update metadata.
            let _ = diesel::update(bs::branding_settings.filter(bs::id.eq(&row.id)))
                .set((bs::updated_by.eq(&uid), bs::updated_at.eq(now.to_rfc3339())))
                .execute(&mut conn)
                .await;

            // Return updated record.
            match bs::branding_settings
                .first::<BrandingSettings>(&mut conn)
                .await
                .optional()
            {
                Ok(Some(updated)) => {
                    res.status_code(StatusCode::OK);
                    res.render(Json(ApiResponse::ok(branding_to_response(updated))));
                }
                _ => {
                    res.status_code(StatusCode::OK);
                    res.render(Json(ApiResponse::ok(
                        serde_json::json!({ "message": "Branding updated" }),
                    )));
                }
            }
        }
        Ok(None) => {
            // Insert new branding settings.
            let new_branding = NewBrandingSettings {
                id: Uuid::new_v4().to_string(),
                logo_url: body.logo_url.clone(),
                favicon_url: body.favicon_url.clone(),
                app_name: body.app_name.clone().unwrap_or_else(|| APP_NAME.into()),
                primary_color: body.primary_color.clone(),
                custom_css: body.custom_css.clone(),
                updated_by: uid.clone(),
                updated_at: now.to_rfc3339(),
            };

            if let Err(e) = diesel::insert_into(bs::branding_settings)
                .values(&new_branding)
                .execute(&mut conn)
                .await
            {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Database(e.to_string()).to_response(),
                )));
                return;
            }

            let resp = BrandingResponse {
                logo_url: body.logo_url.or_else(|| Some(LOGO_URL.into())),
                favicon_url: body.favicon_url.or_else(|| Some(FAVICON_URL.into())),
                app_name: body.app_name.unwrap_or_else(|| APP_NAME.into()),
                primary_color: body.primary_color,
                custom_css: body.custom_css,
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
// DELETE /admin/branding
// ---------------------------------------------------------------------------

#[handler]
pub async fn reset_branding(depot: &mut Depot, res: &mut Response) {
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

    // Delete all branding settings to reset to defaults.
    let _ = diesel::delete(bs::branding_settings)
        .execute(&mut conn)
        .await;

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(default_branding())));
}
