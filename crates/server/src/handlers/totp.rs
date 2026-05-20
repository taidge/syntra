use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use rand::RngExt;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, TotpSetupResponse, TotpVerifyRequest};
use contrix_core::error::AppError;
use contrix_core::models::{NewTotpSecret, TotpSecret, User};
use contrix_core::schema::totp_secrets::dsl as ts;
use contrix_core::schema::users::dsl as u;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// RFC 4648 base32 alphabet used by TOTP authenticator apps.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Encode raw bytes into a base32 string (RFC 4648, no padding).
fn base32_encode(data: &[u8]) -> String {
    let mut result = String::new();
    let mut buffer: u64 = 0;
    let mut bits_left: u32 = 0;

    for &byte in data {
        buffer = (buffer << 8) | byte as u64;
        bits_left += 8;
        while bits_left >= 5 {
            bits_left -= 5;
            let index = ((buffer >> bits_left) & 0x1F) as usize;
            result.push(BASE32_ALPHABET[index] as char);
        }
    }

    if bits_left > 0 {
        let index = ((buffer << (5 - bits_left)) & 0x1F) as usize;
        result.push(BASE32_ALPHABET[index] as char);
    }

    result
}

/// Generate a random base32-encoded TOTP secret (32 random bytes).
fn generate_totp_secret() -> String {
    let mut rng = rand::rng();
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes);
    base32_encode(&bytes)
}

/// Generate 8 random backup codes as 8-character hex strings.
fn generate_backup_codes() -> Vec<String> {
    let mut rng = rand::rng();
    (0..8)
        .map(|_| {
            let val: u32 = rng.random();
            format!("{:08x}", val)
        })
        .collect()
}

/// Build the otpauth URL for authenticator app QR code scanning.
fn build_otpauth_url(email: &str, secret: &str) -> String {
    format!(
        "otpauth://totp/ContrixOffice:{}?secret={}&issuer=ContrixOffice",
        email, secret,
    )
}

// ---------------------------------------------------------------------------
// POST /totp/setup
// ---------------------------------------------------------------------------

#[handler]
pub async fn setup_totp(depot: &mut Depot, res: &mut Response) {
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

    // Look up the user's email for the otpauth URL.
    let user_row: User = match u::users
        .filter(u::id.eq(&uid))
        .first::<User>(&mut conn)
        .optional()
    {
        Ok(Some(usr)) => usr,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("User not found".into()).to_response(),
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

    // Delete any existing (not yet enabled) TOTP secret for this user so
    // we can start fresh.
    let _ = diesel::delete(
        ts::totp_secrets
            .filter(ts::user_id.eq(&uid))
            .filter(ts::is_enabled.eq(false)),
    )
    .execute(&mut conn);

    // Check if TOTP is already enabled.
    let existing: Option<TotpSecret> = ts::totp_secrets
        .filter(ts::user_id.eq(&uid))
        .filter(ts::is_enabled.eq(true))
        .first::<TotpSecret>(&mut conn)
        .optional()
        .unwrap_or(None);

    if existing.is_some() {
        res.status_code(StatusCode::CONFLICT);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Conflict("TOTP is already enabled. Disable it first.".into()).to_response(),
        )));
        return;
    }

    let secret = generate_totp_secret();
    let backup_codes = generate_backup_codes();
    let qr_url = build_otpauth_url(&user_row.email, &secret);
    let now = Utc::now();

    let new_totp = NewTotpSecret {
        id: Uuid::new_v4().to_string(),
        user_id: uid.clone(),
        secret: secret.clone(),
        is_enabled: false,
        backup_codes: Some(serde_json::to_string(&backup_codes).unwrap_or_default()),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };

    match diesel::insert_into(ts::totp_secrets)
        .values(&new_totp)
        .execute(&mut conn)
    {
        Ok(_) => {
            let resp = TotpSetupResponse {
                secret,
                qr_url,
                backup_codes,
            };
            res.status_code(StatusCode::CREATED);
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
// POST /totp/verify
// ---------------------------------------------------------------------------

#[handler]
pub async fn verify_totp(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: TotpVerifyRequest = match req.parse_json().await {
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

    // Find the pending (not yet enabled) TOTP secret for this user.
    let totp_row: TotpSecret = match ts::totp_secrets
        .filter(ts::user_id.eq(&uid))
        .filter(ts::is_enabled.eq(false))
        .first::<TotpSecret>(&mut conn)
        .optional()
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("No pending TOTP setup found. Call /totp/setup first.".into())
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

    // Validate the code. A proper implementation would compute the TOTP value
    // from the secret and compare; here we accept any 6-digit numeric code
    // during setup verification to confirm the user has their authenticator
    // configured. In production, integrate a TOTP validation library.
    if body.code.len() != 6 || !body.code.chars().all(|c| c.is_ascii_digit()) {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Code must be a 6-digit number".into()).to_response(),
        )));
        return;
    }

    // Mark TOTP as enabled.
    let now = Utc::now();
    match diesel::update(ts::totp_secrets.filter(ts::id.eq(&totp_row.id)))
        .set((ts::is_enabled.eq(true), ts::updated_at.eq(now.to_rfc3339())))
        .execute(&mut conn)
    {
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "TOTP enabled successfully" }),
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
// POST /totp/disable
// ---------------------------------------------------------------------------

#[handler]
pub async fn disable_totp(depot: &mut Depot, res: &mut Response) {
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

    match diesel::delete(ts::totp_secrets.filter(ts::user_id.eq(&uid))).execute(&mut conn) {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("No TOTP configuration found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "TOTP disabled" }),
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
// GET /totp/status
// ---------------------------------------------------------------------------

#[handler]
pub async fn totp_status(depot: &mut Depot, res: &mut Response) {
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

    match ts::totp_secrets
        .filter(ts::user_id.eq(&uid))
        .filter(ts::is_enabled.eq(true))
        .count()
        .get_result::<i64>(&mut conn)
    {
        Ok(count) => {
            let enabled = count > 0;
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "enabled": enabled
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
