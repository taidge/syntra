use std::sync::Arc;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::Utc;
use diesel::prelude::*;
use rand::RngExt;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, TotpSetupRequest, TotpSetupResponse, TotpVerifyRequest};
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

/// Generate eight 128-bit backup codes as 32-character hex strings.
fn generate_backup_codes() -> Vec<String> {
    let mut rng = rand::rng();
    (0..8)
        .map(|_| {
            let mut bytes = [0u8; 16];
            rng.fill(&mut bytes);
            bytes.iter().map(|byte| format!("{byte:02x}")).collect()
        })
        .collect()
}

fn hash_backup_codes(codes: &[String]) -> Result<Vec<String>, AppError> {
    codes
        .iter()
        .map(|code| {
            let salt = SaltString::generate(&mut OsRng);
            Argon2::default()
                .hash_password(code.as_bytes(), &salt)
                .map(|hash| hash.to_string())
                .map_err(|e| AppError::Internal(format!("Failed to hash backup code: {e}")))
        })
        .collect()
}

/// Build the otpauth URL for authenticator app QR code scanning.
fn build_otpauth_url(email: &str, secret: &str) -> String {
    format!(
        "otpauth://totp/ContrixOffice:{}?secret={}&issuer=ContrixOffice",
        encode_uri_component(email),
        secret,
    )
}

/// Percent-encode arbitrary UTF-8 for one URI component. Authenticator labels
/// use `:` as a separator, so only RFC 3986 unreserved bytes remain literal.
fn encode_uri_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
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
pub(crate) fn verify_totp_code(secret_b32: &str, code: &str) -> bool {
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let code_num: u32 = match code.parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    if current_totp_code(secret_b32).as_deref() == Some(code) {
        return true;
    }
    let secret = match base32_decode(secret_b32) {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };
    let step = Utc::now().timestamp() / 30;
    for w in [-1i64, 1] {
        let counter = (step + w) as u64;
        if totp_value(&secret, counter) == code_num {
            return true;
        }
    }
    false
}

/// Verify the active TOTP code or atomically consume one backup code.
///
/// Backup codes are stored as Argon2 hashes. The optimistic `backup_codes`
/// predicate prevents two concurrent requests from consuming the same code.
pub(crate) fn verify_second_factor(
    conn: &mut diesel::SqliteConnection,
    totp: &TotpSecret,
    code: &str,
) -> Result<bool, AppError> {
    if verify_totp_code(&totp.secret, code) {
        return Ok(true);
    }
    if !matches!(code.len(), 8 | 32) || !code.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(false);
    }

    let Some(serialized) = totp.backup_codes.as_deref() else {
        return Ok(false);
    };
    let codes: Vec<String> = serde_json::from_str(serialized)
        .map_err(|e| AppError::Internal(format!("Invalid stored backup codes: {e}")))?;
    let Some(matched_index) = codes.iter().position(|stored| {
        PasswordHash::new(stored).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(code.as_bytes(), &hash)
                .is_ok()
        }) || stored.eq_ignore_ascii_case(code)
    }) else {
        return Ok(false);
    };

    let mut remaining = Vec::with_capacity(codes.len().saturating_sub(1));
    for (index, stored) in codes.into_iter().enumerate() {
        if index == matched_index {
            continue;
        }
        if PasswordHash::new(&stored).is_ok() {
            remaining.push(stored);
        } else {
            // Transparently upgrade backup codes created by older versions,
            // which stored the random codes in plaintext JSON.
            remaining.extend(hash_backup_codes(&[stored])?);
        }
    }
    let updated = serde_json::to_string(&remaining)
        .map_err(|e| AppError::Internal(format!("Failed to serialize backup codes: {e}")))?;
    let changed = diesel::update(
        ts::totp_secrets
            .filter(ts::id.eq(&totp.id))
            .filter(ts::backup_codes.eq(totp.backup_codes.clone())),
    )
    .set((
        ts::backup_codes.eq(Some(updated)),
        ts::updated_at.eq(Utc::now().to_rfc3339()),
    ))
    .execute(conn)
    .map_err(|e| AppError::Database(format!("Failed to consume backup code: {e}")))?;
    Ok(changed == 1)
}

/// Return the current six-digit code for a base32 secret. This is useful for
/// protocol-level tests and does not expose any stored user secret.
pub fn current_totp_code(secret_b32: &str) -> Option<String> {
    let secret = base32_decode(secret_b32)?;
    if secret.is_empty() {
        return None;
    }
    let counter = (Utc::now().timestamp() / 30) as u64;
    Some(format!("{:06}", totp_value(&secret, counter)))
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

    #[test]
    fn totp_verification_requires_exactly_six_digits() {
        let secret = base32_encode(b"12345678901234567890");
        let code = current_totp_code(&secret).unwrap();
        assert!(verify_totp_code(&secret, &code));
        assert!(!verify_totp_code(&secret, "12345"));
        assert!(!verify_totp_code(&secret, "12345a"));
    }

    #[test]
    fn otpauth_label_percent_encodes_email_component() {
        let uri = build_otpauth_url("user+测试@example.com", "ABC234");
        assert_eq!(
            uri,
            "otpauth://totp/ContrixOffice:user%2B%E6%B5%8B%E8%AF%95%40example.com?secret=ABC234&issuer=ContrixOffice"
        );
    }
}

// ---------------------------------------------------------------------------
// POST /totp/setup
// ---------------------------------------------------------------------------

#[handler]
pub async fn setup_totp(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: TotpSetupRequest = match req.parse_json().await {
        Ok(body) => body,
        Err(error) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(error.to_string()).to_response(),
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
    let password_valid = PasswordHash::new(&user_row.password_hash).is_ok_and(|hash| {
        Argon2::default()
            .verify_password(body.current_password.as_bytes(), &hash)
            .is_ok()
    });
    if !password_valid {
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Auth("Current password is incorrect".into()).to_response(),
        )));
        return;
    }

    // Delete any existing (not yet enabled) TOTP secret for this user so
    // we can start fresh.
    if let Err(error) = diesel::delete(
        ts::totp_secrets
            .filter(ts::user_id.eq(&uid))
            .filter(ts::is_enabled.eq(false)),
    )
    .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(format!("Failed to replace pending TOTP setup: {error}"))
                .to_response(),
        )));
        return;
    }

    // Check if TOTP is already enabled.
    let existing: Option<TotpSecret> = match ts::totp_secrets
        .filter(ts::user_id.eq(&uid))
        .filter(ts::is_enabled.eq(true))
        .first::<TotpSecret>(&mut conn)
        .optional()
    {
        Ok(existing) => existing,
        Err(error) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(error.to_string()).to_response(),
            )));
            return;
        }
    };

    if existing.is_some() {
        res.status_code(StatusCode::CONFLICT);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Conflict("TOTP is already enabled. Disable it first.".into()).to_response(),
        )));
        return;
    }

    let secret = generate_totp_secret();
    let backup_codes = generate_backup_codes();
    let backup_code_hashes = match hash_backup_codes(&backup_codes) {
        Ok(hashes) => hashes,
        Err(error) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
            return;
        }
    };
    let qr_url = build_otpauth_url(&user_row.email, &secret);
    let now = Utc::now();

    let new_totp = NewTotpSecret {
        id: Uuid::new_v4().to_string(),
        user_id: uid.clone(),
        secret: secret.clone(),
        is_enabled: false,
        backup_codes: Some(serde_json::to_string(&backup_code_hashes).unwrap_or_default()),
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
pub async fn disable_totp(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: TotpVerifyRequest = match req.parse_json().await {
        Ok(body) => body,
        Err(error) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(error.to_string()).to_response(),
            )));
            return;
        }
    };
    let totp = match ts::totp_secrets
        .filter(ts::user_id.eq(&uid))
        .filter(ts::is_enabled.eq(true))
        .first::<TotpSecret>(&mut conn)
        .optional()
    {
        Ok(Some(totp)) => totp,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("No enabled TOTP configuration found".into()).to_response(),
            )));
            return;
        }
        Err(error) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(error.to_string()).to_response(),
            )));
            return;
        }
    };
    match verify_second_factor(&mut conn, &totp, &body.code) {
        Ok(true) => {}
        Ok(false) => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Invalid authentication code".into()).to_response(),
            )));
            return;
        }
        Err(error) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
            return;
        }
    }

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
