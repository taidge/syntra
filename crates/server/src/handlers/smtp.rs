use chrono::Utc;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, SmtpSettingsRequest, SmtpSettingsResponse};
use contrix_core::error::AppError;
use contrix_core::models::{NewSmtpSettings, SmtpSettings};
use contrix_core::schema::smtp_settings::dsl as smtp;

use crate::middleware::auth::{get_user_id, get_user_role};

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
// GET /admin/smtp — get SMTP settings (password hidden)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_smtp(depot: &mut Depot, res: &mut Response) {
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

    match smtp::smtp_settings
        .first::<SmtpSettings>(&mut conn)
        .await
        .optional()
    {
        Ok(Some(settings)) => {
            let resp = SmtpSettingsResponse {
                host: settings.host,
                port: settings.port,
                username: settings.username,
                // Password is intentionally omitted from the response DTO
                from_email: settings.from_email,
                from_name: settings.from_name,
                use_tls: settings.use_tls,
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(resp)));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("SMTP settings not configured".into()).to_response(),
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
// PUT /admin/smtp — update SMTP settings (upsert)
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_smtp(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: SmtpSettingsRequest = match req.parse_json().await {
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

    let now = Utc::now().to_rfc3339();

    // Delete any existing row(s) and insert the new settings (upsert pattern).
    let _ = diesel::delete(smtp::smtp_settings).execute(&mut conn).await;

    let new_settings = NewSmtpSettings {
        id: Uuid::new_v4().to_string(),
        host: body.host.clone(),
        port: body.port.unwrap_or(587),
        username: body.username.clone(),
        password: body.password.clone(),
        from_email: body.from_email.clone(),
        from_name: body
            .from_name
            .clone()
            .unwrap_or_else(|| "Contrix Office".into()),
        use_tls: body.use_tls.unwrap_or(true),
        updated_by: uid,
        updated_at: now,
    };

    match diesel::insert_into(smtp::smtp_settings)
        .values(&new_settings)
        .execute(&mut conn)
        .await
    {
        Ok(_) => {
            let resp = SmtpSettingsResponse {
                host: new_settings.host,
                port: new_settings.port,
                username: new_settings.username,
                from_email: new_settings.from_email,
                from_name: new_settings.from_name,
                use_tls: new_settings.use_tls,
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
// POST /admin/smtp/test — send a test email
// ---------------------------------------------------------------------------

#[handler]
pub async fn test_smtp(depot: &mut Depot, res: &mut Response) {
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

    // Verify that SMTP settings exist before attempting a test.
    let settings = match smtp::smtp_settings
        .first::<SmtpSettings>(&mut conn)
        .await
        .optional()
    {
        Ok(Some(s)) => s,
        Ok(None) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(
                    "SMTP settings not configured. Please save settings first.".into(),
                )
                .to_response(),
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
    drop(conn);

    // Actually attempt to deliver a test message so the admin gets a real
    // result. lettre's transport is blocking, so run it off the async runtime.
    let pool = state.db.clone();
    let to = settings.from_email.clone();
    let send_result = tokio::spawn(async move {
        crate::services::EmailService::send(
            &pool,
            &to,
            "Contrix Office SMTP test",
            "<p>This is a test email confirming your SMTP settings work.</p>",
        )
        .await
    })
    .await;

    match send_result {
        Ok(Ok(())) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "message": format!("Test email sent to {}", settings.from_email)
            }))));
        }
        Ok(Err(e)) => {
            res.status_code(StatusCode::BAD_GATEWAY);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Internal(format!("SMTP test failed: {e}")).to_response(),
            )));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Internal(format!("SMTP test task failed: {e}")).to_response(),
            )));
        }
    }
}
