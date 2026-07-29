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

/// Decode an RFC 4648 base32 string (case-insensitive, padding/whitespace
/// tolerated) into raw bytes.
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut buffer: u64 = 0;
    let mut bits_left: u32 = 0;
    let mut out = Vec::new();
    for c in s.chars() {
        if c == '=' || c.is_whitespace() {
            continue;
        }
        let upper = c.to_ascii_uppercase() as u8;
        let val = BASE32_ALPHABET.iter().position(|&b| b == upper)? as u64;
        buffer = (buffer << 5) | val;
        bits_left += 5;
        if bits_left >= 8 {
            bits_left -= 8;
            out.push((buffer >> bits_left) as u8);
        }
    }
    Some(out)
}

/// SHA-1 digest (RFC 3174). Implemented locally to avoid pulling an extra
/// crypto dependency just for TOTP.
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let ml = (data.len() as u64).wrapping_mul(8);

    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&ml.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().enumerate().take(16) {
            *word = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// HMAC-SHA1 of `msg` keyed by `key`.
fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..20].copy_from_slice(&sha1(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= block[i];
        opad[i] ^= block[i];
    }

    let mut inner = Vec::with_capacity(64 + msg.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(msg);
    let inner_hash = sha1(&inner);

    let mut outer = Vec::with_capacity(84);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_hash);
    sha1(&outer)
}

/// Compute the 6-digit HOTP/TOTP value for a counter (RFC 6238, SHA-1).
fn totp_value(secret: &[u8], counter: u64) -> u32 {
    let hs = hmac_sha1(secret, &counter.to_be_bytes());
    let offset = (hs[19] & 0x0f) as usize;
    let bin = ((hs[offset] as u32 & 0x7f) << 24)
        | ((hs[offset + 1] as u32) << 16)
        | ((hs[offset + 2] as u32) << 8)
        | (hs[offset + 3] as u32);
    bin % 1_000_000
}

/// Verify a user-supplied 6-digit code against a base32 secret, allowing a
/// ±1 time-step (30s) window to tolerate clock skew.
fn verify_totp_code(secret_b32: &str, code: &str) -> bool {
    let code_num: u32 = match code.parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let secret = match base32_decode(secret_b32) {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    let step = (Utc::now().timestamp() / 30) as i64;
    for w in [-1i64, 0, 1] {
        let counter = (step + w) as u64;
        if totp_value(&secret, counter) == code_num {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod totp_tests {
    use super::*;

    #[test]
    fn rfc6238_sha1_vector() {
        // RFC 6238 Appendix B: seed "12345678901234567890", SHA-1.
        // T=59 -> counter 1 -> 8-digit 94287082 -> 6-digit 287082.
        let secret = b"12345678901234567890";
        assert_eq!(totp_value(secret, 1), 287082);
    }

    #[test]
    fn base32_round_trip() {
        let bytes = b"hello world";
        let encoded = base32_encode(bytes);
        assert_eq!(base32_decode(&encoded).as_deref(), Some(&bytes[..]));
    }
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

    // Validate the code format, then verify it against the stored secret using
    // the real RFC 6238 algorithm (with a ±1 step window for clock skew).
    if body.code.len() != 6 || !body.code.chars().all(|c| c.is_ascii_digit()) {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Code must be a 6-digit number".into()).to_response(),
        )));
        return;
    }

    if !verify_totp_code(&totp_row.secret, &body.code) {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Invalid authentication code".into()).to_response(),
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
