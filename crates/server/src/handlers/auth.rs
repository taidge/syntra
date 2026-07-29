use std::sync::Arc;

use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::Utc;
use diesel::prelude::*;
use jsonwebtoken::{EncodingKey, Header, encode};
use salvo::prelude::*;
use uuid::Uuid;

use serde::Deserialize;

use contrix_core::dto::{
    ApiResponse, AuthResponse, ForgotPasswordRequest, LoginRequest, RegisterRequest,
    ResetPasswordRequest, UserProfile,
};
use contrix_core::error::AppError;
use contrix_core::models::{
    NewLoginAttempt, NewPasswordResetToken, NewUser, PasswordResetToken, User, UserRole,
};
use contrix_core::schema::login_attempts::dsl as la;
use contrix_core::schema::password_reset_tokens::dsl as prt;
use contrix_core::schema::users::dsl as u;

use crate::middleware::auth::{JwtClaims, get_user_id};
use crate::services::password::PasswordService;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a signed JWT for the given user.
fn generate_jwt(
    user_id: &str,
    role: &str,
    secret: &str,
    expiry_hours: u64,
) -> Result<String, AppError> {
    let exp = (Utc::now().timestamp() as u64) + expiry_hours * 3600;
    let claims = JwtClaims {
        sub: user_id.to_owned(),
        role: role.to_owned(),
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("JWT generation failed: {e}")))
}

// ---------------------------------------------------------------------------
// POST /auth/register
// ---------------------------------------------------------------------------

#[handler]
pub async fn register(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();

    let body: RegisterRequest = match req.parse_json::<RegisterRequest>().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    // Get a connection from the pool.
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

    // Check uniqueness of email.
    let email_count: i64 = match u::users
        .filter(u::email.eq(&body.email))
        .count()
        .get_result(&mut conn)
    {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if email_count > 0 {
        res.status_code(StatusCode::CONFLICT);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Conflict("Email already registered".into()).to_response(),
        )));
        return;
    }

    // Check uniqueness of username.
    let username_count: i64 = match u::users
        .filter(u::username.eq(&body.username))
        .count()
        .get_result(&mut conn)
    {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if username_count > 0 {
        res.status_code(StatusCode::CONFLICT);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Conflict("Username already taken".into()).to_response(),
        )));
        return;
    }

    // Enforce the configured password policy before accepting the account.
    if let Err(e) = PasswordService::validate_password(&mut conn, &body.password) {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(e.to_response())));
        return;
    }

    // Hash password.
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = match Argon2::default().hash_password(body.password.as_bytes(), &salt) {
        Ok(h) => h.to_string(),
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Internal(format!("Password hashing failed: {e}")).to_response(),
            )));
            return;
        }
    };

    let user_id_val = Uuid::new_v4();
    let now = Utc::now();
    let role = UserRole::Editor;
    let default_quota = (state.config.storage.default_quota_mb as i64) * 1024 * 1024;

    let new_user = NewUser {
        id: user_id_val.to_string(),
        username: body.username.clone(),
        email: body.email.clone(),
        password_hash,
        display_name: None,
        avatar_url: None,
        role: role.as_str().to_string(),
        storage_used: 0,
        storage_quota: Some(default_quota),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        last_login_at: Some(now.to_rfc3339()),
    };

    if let Err(e) = diesel::insert_into(u::users)
        .values(&new_user)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    let token = match generate_jwt(
        &user_id_val.to_string(),
        role.as_str(),
        &state.config.auth.jwt_secret,
        state.config.auth.jwt_expiry_hours,
    ) {
        Ok(t) => t,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let profile = UserProfile {
        id: user_id_val,
        username: body.username,
        email: body.email,
        display_name: None,
        avatar_url: None,
        role,
        storage_used: 0,
        storage_quota: default_quota,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(AuthResponse {
        token,
        user: profile,
    })));
}

// ---------------------------------------------------------------------------
// POST /auth/login
// ---------------------------------------------------------------------------

#[handler]
pub async fn login(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();

    let body: LoginRequest = match req.parse_json::<LoginRequest>().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    // Get a connection from the pool.
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

    // --- Brute-force protection: check failed login attempts in last 15 min ---
    let ip_address = format!("{}", req.remote_addr());

    let cutoff = (Utc::now() - chrono::Duration::minutes(15)).to_rfc3339();
    let failed_count: i64 = la::login_attempts
        .filter(la::email.eq(&body.email))
        .filter(la::success.eq(false))
        .filter(la::attempted_at.gt(&cutoff))
        .count()
        .get_result(&mut conn)
        .unwrap_or(0);

    if failed_count >= 5 {
        res.status_code(StatusCode::TOO_MANY_REQUESTS);
        res.render(Json(ApiResponse::<()>::err(
            AppError::RateLimit("Too many login attempts, try again in 15 minutes".into())
                .to_response(),
        )));
        return;
    }

    // Look up user by email.
    let user_row: User = match u::users
        .filter(u::email.eq(&body.email))
        .first::<User>(&mut conn)
        .optional()
    {
        Ok(Some(usr)) => usr,
        Ok(None) => {
            // Record failed attempt
            let _ = diesel::insert_into(la::login_attempts)
                .values(&NewLoginAttempt {
                    id: Uuid::new_v4().to_string(),
                    ip_address: ip_address.clone(),
                    email: body.email.clone(),
                    success: false,
                    attempted_at: Utc::now().to_rfc3339(),
                })
                .execute(&mut conn);
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Invalid email or password".into()).to_response(),
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

    // Verify password.
    let parsed_hash = match PasswordHash::new(&user_row.password_hash) {
        Ok(h) => h,
        Err(_) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Internal("Password hash corrupt".into()).to_response(),
            )));
            return;
        }
    };

    if Argon2::default()
        .verify_password(body.password.as_bytes(), &parsed_hash)
        .is_err()
    {
        // Record failed attempt
        let _ = diesel::insert_into(la::login_attempts)
            .values(&NewLoginAttempt {
                id: Uuid::new_v4().to_string(),
                ip_address: ip_address.clone(),
                email: body.email.clone(),
                success: false,
                attempted_at: Utc::now().to_rfc3339(),
            })
            .execute(&mut conn);
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Auth("Invalid email or password".into()).to_response(),
        )));
        return;
    }

    // Reject disabled accounts even with valid credentials — a disabled user
    // must not be able to mint a fresh token.
    if user_row.role == "disabled" {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("This account has been disabled".into()).to_response(),
        )));
        return;
    }

    // Record successful login attempt
    let _ = diesel::insert_into(la::login_attempts)
        .values(&NewLoginAttempt {
            id: Uuid::new_v4().to_string(),
            ip_address: ip_address.clone(),
            email: body.email.clone(),
            success: true,
            attempted_at: Utc::now().to_rfc3339(),
        })
        .execute(&mut conn);

    // Audit the successful authentication.
    let user_agent = req
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    crate::services::AuditService::log(
        &mut conn,
        &user_row.id,
        "auth.login",
        "user",
        &user_row.id,
        None,
        Some(&ip_address),
        user_agent.as_deref(),
    );

    // Update last_login_at.
    let now = Utc::now();
    let _ = diesel::update(u::users.filter(u::id.eq(&user_row.id)))
        .set(u::last_login_at.eq(Some(now.to_rfc3339())))
        .execute(&mut conn);

    let user_id_uuid: Uuid = user_row.id.parse().unwrap_or_default();
    let role = UserRole::from_str(&user_row.role);

    let token = match generate_jwt(
        &user_row.id,
        &user_row.role,
        &state.config.auth.jwt_secret,
        state.config.auth.jwt_expiry_hours,
    ) {
        Ok(t) => t,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let profile = UserProfile {
        id: user_id_uuid,
        username: user_row.username,
        email: user_row.email,
        display_name: user_row.display_name,
        avatar_url: user_row.avatar_url,
        role,
        storage_used: user_row.storage_used,
        storage_quota: user_row.storage_quota.unwrap_or(0),
    };

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(AuthResponse {
        token,
        user: profile,
    })));
}

// ---------------------------------------------------------------------------
// GET /auth/me
// ---------------------------------------------------------------------------

#[handler]
pub async fn me(depot: &mut Depot, res: &mut Response) {
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
        .first::<User>(&mut conn)
        .optional()
    {
        Ok(Some(user_row)) => {
            let profile = UserProfile {
                id: user_row.id.parse().unwrap_or_default(),
                username: user_row.username,
                email: user_row.email,
                display_name: user_row.display_name,
                avatar_url: user_row.avatar_url,
                role: UserRole::from_str(&user_row.role),
                storage_used: user_row.storage_used,
                storage_quota: user_row.storage_quota.unwrap_or(0),
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(profile)));
        }
        Ok(None) => {
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
// PUT /auth/me
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct UpdateMeRequest {
    pub display_name: Option<String>,
}

#[handler]
pub async fn update_me(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: UpdateMeRequest = match req.parse_json::<UpdateMeRequest>().await {
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

    let now = Utc::now().to_rfc3339();
    if let Err(e) = diesel::update(u::users.filter(u::id.eq(&uid)))
        .set((
            u::display_name.eq(&body.display_name),
            u::updated_at.eq(&now),
        ))
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    match u::users
        .filter(u::id.eq(&uid))
        .first::<User>(&mut conn)
        .optional()
    {
        Ok(Some(user_row)) => {
            let profile = UserProfile {
                id: user_row.id.parse().unwrap_or_default(),
                username: user_row.username,
                email: user_row.email,
                display_name: user_row.display_name,
                avatar_url: user_row.avatar_url,
                role: UserRole::from_str(&user_row.role),
                storage_used: user_row.storage_used,
                storage_quota: user_row.storage_quota.unwrap_or(0),
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(profile)));
        }
        Ok(None) => {
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
// PUT /auth/password
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[handler]
pub async fn change_password(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: ChangePasswordRequest = match req.parse_json::<ChangePasswordRequest>().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.new_password.len() < 8 {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Password must be at least 8 characters".into()).to_response(),
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

    let parsed_hash = match PasswordHash::new(&user_row.password_hash) {
        Ok(h) => h,
        Err(_) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Internal("Password hash corrupt".into()).to_response(),
            )));
            return;
        }
    };

    if Argon2::default()
        .verify_password(body.current_password.as_bytes(), &parsed_hash)
        .is_err()
    {
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Auth("Current password is incorrect".into()).to_response(),
        )));
        return;
    }

    let salt = SaltString::generate(&mut OsRng);
    let new_hash = match Argon2::default().hash_password(body.new_password.as_bytes(), &salt) {
        Ok(h) => h.to_string(),
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Internal(format!("Password hashing failed: {e}")).to_response(),
            )));
            return;
        }
    };

    let now = Utc::now().to_rfc3339();
    if let Err(e) = diesel::update(u::users.filter(u::id.eq(&uid)))
        .set((u::password_hash.eq(&new_hash), u::updated_at.eq(&now)))
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(
        serde_json::json!({ "success": true }),
    )));
}

// ---------------------------------------------------------------------------
// POST /auth/forgot-password
// ---------------------------------------------------------------------------

#[handler]
pub async fn forgot_password(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();

    let body: ForgotPasswordRequest = match req.parse_json::<ForgotPasswordRequest>().await {
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

    // Look up user by email. We always return success to avoid revealing
    // whether the email exists.
    let user_opt: Option<User> = u::users
        .filter(u::email.eq(&body.email))
        .first::<User>(&mut conn)
        .optional()
        .unwrap_or(None);

    if let Some(user_row) = user_opt {
        let token_val = Uuid::new_v4().to_string().replace('-', "");
        let now = Utc::now();
        let expires = now + chrono::Duration::hours(1);

        let new_token = NewPasswordResetToken {
            id: Uuid::new_v4().to_string(),
            user_id: user_row.id.clone(),
            token: token_val.clone(),
            expires_at: expires.to_rfc3339(),
            used: false,
            created_at: now.to_rfc3339(),
        };

        if diesel::insert_into(prt::password_reset_tokens)
            .values(&new_token)
            .execute(&mut conn)
            .is_ok()
        {
            // Deliver the reset link by email (best-effort, off the async
            // runtime since the SMTP transport is blocking).
            let pool = state.db.clone();
            let to = user_row.email.clone();
            let body_html = format!(
                "<p>A password reset was requested for your account.</p>\
                 <p>Use this token to reset your password (valid for 1 hour):</p>\
                 <p><code>{token_val}</code></p>\
                 <p>If you did not request this, you can ignore this email.</p>"
            );
            tokio::task::spawn_blocking(move || {
                crate::services::EmailService::send_notification(
                    &pool,
                    &to,
                    "Password reset",
                    &body_html,
                );
            });
        }
    }

    // Always return success regardless of whether the email was found.
    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(
        serde_json::json!({ "message": "If the email exists, a reset token has been generated" }),
    )));
}

// ---------------------------------------------------------------------------
// POST /auth/reset-password
// ---------------------------------------------------------------------------

#[handler]
pub async fn reset_password(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();

    let body: ResetPasswordRequest = match req.parse_json::<ResetPasswordRequest>().await {
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

    // Find token record.
    let token_row: PasswordResetToken = match prt::password_reset_tokens
        .filter(prt::token.eq(&body.token))
        .first::<PasswordResetToken>(&mut conn)
        .optional()
    {
        Ok(Some(t)) => t,
        Ok(None) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Invalid or expired reset token".into()).to_response(),
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

    // Check if token is already used.
    if token_row.used {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Reset token has already been used".into()).to_response(),
        )));
        return;
    }

    // Check expiry.
    let expires_at = match chrono::DateTime::parse_from_rfc3339(&token_row.expires_at) {
        Ok(dt) => dt.with_timezone(&Utc),
        Err(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Invalid or expired reset token".into()).to_response(),
            )));
            return;
        }
    };

    if Utc::now() > expires_at {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Reset token has expired".into()).to_response(),
        )));
        return;
    }

    // Enforce the configured password policy on the new password.
    if let Err(e) = PasswordService::validate_password(&mut conn, &body.password) {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(e.to_response())));
        return;
    }

    // Hash the new password with Argon2.
    let salt = SaltString::generate(&mut OsRng);
    let new_hash = match Argon2::default().hash_password(body.password.as_bytes(), &salt) {
        Ok(h) => h.to_string(),
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Internal(format!("Password hashing failed: {e}")).to_response(),
            )));
            return;
        }
    };

    // Update user's password_hash.
    if let Err(e) = diesel::update(u::users.filter(u::id.eq(&token_row.user_id)))
        .set(u::password_hash.eq(&new_hash))
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    // Mark token as used.
    let _ = diesel::update(prt::password_reset_tokens.filter(prt::id.eq(&token_row.id)))
        .set(prt::used.eq(true))
        .execute(&mut conn);

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(
        serde_json::json!({ "message": "Password has been reset successfully" }),
    )));
}
