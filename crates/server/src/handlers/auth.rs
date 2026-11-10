use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
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
    NewLoginAttempt, NewPasswordResetToken, NewSession, NewUser, PasswordResetToken, TotpSecret,
    User, UserRole,
};
use contrix_core::schema::login_attempts::dsl as la;
use contrix_core::schema::password_reset_tokens::dsl as prt;
use contrix_core::schema::sessions::dsl as sess;
use contrix_core::schema::totp_secrets::dsl as ts;
use contrix_core::schema::users::dsl as u;

use crate::middleware::auth::{JwtClaims, get_user_id, hash_token};
use crate::services::password::PasswordService;
use crate::services::password_crypto;

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
    generate_jwt_with_expiry(user_id, role, secret, exp)
}

fn generate_jwt_with_expiry(
    user_id: &str,
    role: &str,
    secret: &str,
    exp: u64,
) -> Result<String, AppError> {
    let claims = JwtClaims {
        sub: user_id.to_owned(),
        role: role.to_owned(),
        exp,
        jti: Some(Uuid::new_v4().to_string()),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| AppError::Internal(format!("JWT generation failed: {e}")))
}

#[cfg(test)]
mod token_tests {
    use super::*;

    #[test]
    fn same_user_role_and_expiry_still_generate_distinct_session_tokens() {
        let secret = "test-only-jti-secret";
        let expiry = Utc::now().timestamp() as u64 + 3600;
        let first = generate_jwt_with_expiry("user", "editor", secret, expiry).unwrap();
        let second = generate_jwt_with_expiry("user", "editor", secret, expiry).unwrap();
        assert_ne!(first, second);
        let decode = |token: &str| {
            jsonwebtoken::decode::<JwtClaims>(
                token,
                &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
                &jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256),
            )
            .unwrap()
            .claims
        };
        let (first, second) = (decode(&first), decode(&second));
        assert_eq!(first.exp, second.exp);
        assert_ne!(first.jti, second.jti);
        assert!(first.jti.is_some());
    }

    #[test]
    fn previously_issued_claims_without_a_nonce_are_still_decodable() {
        let claims: JwtClaims =
            serde_json::from_value(serde_json::json!({"sub":"legacy","role":"editor","exp":123}))
                .unwrap();
        assert!(claims.jti.is_none());
    }
}

async fn create_session(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    user_id: &str,
    token: &str,
    ip_address: Option<String>,
    user_agent: Option<String>,
    expiry_hours: u64,
) -> Result<(), AppError> {
    let now = Utc::now();
    let session = NewSession {
        id: Uuid::new_v4().to_string(),
        user_id: user_id.to_owned(),
        token_hash: hash_token(token),
        ip_address,
        user_agent,
        is_active: true,
        created_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::hours(expiry_hours as i64)).to_rfc3339(),
        last_activity_at: now.to_rfc3339(),
    };
    diesel::insert_into(sess::sessions)
        .values(&session)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(|e| AppError::Database(format!("Failed to create session: {e}")))
}

async fn revoke_user_sessions(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    user_id: &str,
) {
    let _ = diesel::update(
        sess::sessions
            .filter(sess::user_id.eq(user_id))
            .filter(sess::is_active.eq(true)),
    )
    .set(sess::is_active.eq(false))
    .execute(conn)
    .await;
}

// ---------------------------------------------------------------------------
// POST /auth/register
// ---------------------------------------------------------------------------

#[handler]
pub async fn register(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);

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
        .await
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
        .await
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
    if let Err(e) = PasswordService::validate_password(&mut conn, &body.password).await {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(e.to_response())));
        return;
    }

    // Hash password.
    let password_hash = match password_crypto::hash_password(body.password).await {
        Ok(hash) => hash,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
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

    let session_ip = Some(req.remote_addr().to_string());
    let session_user_agent = req
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let registration = horme::transaction(&mut conn, async |conn| {
        diesel::insert_into(u::users)
            .values(&new_user)
            .execute(conn)
            .await?;
        create_session(
            conn,
            &user_id_val.to_string(),
            &token,
            session_ip,
            session_user_agent,
            state.config.auth.jwt_expiry_hours,
        )
        .await
    })
    .await;
    if let Err(e) = registration {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(e.to_response())));
        return;
    }

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
    let state = crate::require_app_state!(depot, res);

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
        .await
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
        .await
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
                .execute(&mut conn)
                .await;
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
    let password_valid =
        match password_crypto::verify_password(body.password, user_row.password_hash).await {
            Ok(valid) => valid,
            Err(error) => {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(error.to_response())));
                return;
            }
        };

    if !password_valid {
        // Record failed attempt
        let _ = diesel::insert_into(la::login_attempts)
            .values(&NewLoginAttempt {
                id: Uuid::new_v4().to_string(),
                ip_address: ip_address.clone(),
                email: body.email.clone(),
                success: false,
                attempted_at: Utc::now().to_rfc3339(),
            })
            .execute(&mut conn)
            .await;
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

    // Password verification is only the first factor when TOTP is enabled.
    let enabled_totp = match ts::totp_secrets
        .filter(ts::user_id.eq(&user_row.id))
        .filter(ts::is_enabled.eq(true))
        .first::<TotpSecret>(&mut conn)
        .await
        .optional()
    {
        Ok(value) => value,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };
    if let Some(totp) = enabled_totp {
        let code = body.totp_code.as_deref().unwrap_or_default();
        let second_factor_valid =
            match crate::handlers::totp::verify_second_factor(&mut conn, &totp, code).await {
                Ok(valid) => valid,
                Err(error) => {
                    res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                    res.render(Json(ApiResponse::<()>::err(error.to_response())));
                    return;
                }
            };
        if !second_factor_valid {
            let _ = diesel::insert_into(la::login_attempts)
                .values(&NewLoginAttempt {
                    id: Uuid::new_v4().to_string(),
                    ip_address: ip_address.clone(),
                    email: body.email.clone(),
                    success: false,
                    attempted_at: Utc::now().to_rfc3339(),
                })
                .execute(&mut conn)
                .await;
            res.status_code(StatusCode::UNAUTHORIZED);
            let message = if code.is_empty() {
                "TOTP code required"
            } else {
                "Invalid TOTP code"
            };
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth(message.into()).to_response(),
            )));
            return;
        }
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
        .execute(&mut conn)
        .await;

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
        crate::services::AuditContext {
            details: None,
            ip_address: Some(&ip_address),
            user_agent: user_agent.as_deref(),
        },
    )
    .await;

    // Update last_login_at.
    let now = Utc::now();
    let _ = diesel::update(u::users.filter(u::id.eq(&user_row.id)))
        .set(u::last_login_at.eq(Some(now.to_rfc3339())))
        .execute(&mut conn)
        .await;

    let user_id_uuid: Uuid = user_row.id.parse().unwrap_or_default();
    let role = UserRole::parse_lossy(&user_row.role);

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

    if let Err(e) = create_session(
        &mut conn,
        &user_row.id,
        &token,
        Some(ip_address.clone()),
        user_agent.clone(),
        state.config.auth.jwt_expiry_hours,
    )
    .await
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(e.to_response())));
        return;
    }

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
        .await
        .optional()
    {
        Ok(Some(user_row)) => {
            let profile = UserProfile {
                id: user_row.id.parse().unwrap_or_default(),
                username: user_row.username,
                email: user_row.email,
                display_name: user_row.display_name,
                avatar_url: user_row.avatar_url,
                role: UserRole::parse_lossy(&user_row.role),
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
        .await
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
        .await
        .optional()
    {
        Ok(Some(user_row)) => {
            let profile = UserProfile {
                id: user_row.id.parse().unwrap_or_default(),
                username: user_row.username,
                email: user_row.email,
                display_name: user_row.display_name,
                avatar_url: user_row.avatar_url,
                role: UserRole::parse_lossy(&user_row.role),
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
        .await
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

    let password_valid =
        match password_crypto::verify_password(body.current_password, user_row.password_hash).await
        {
            Ok(valid) => valid,
            Err(error) => {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(error.to_response())));
                return;
            }
        };

    if !password_valid {
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Auth("Current password is incorrect".into()).to_response(),
        )));
        return;
    }

    let new_hash = match password_crypto::hash_password(body.new_password).await {
        Ok(hash) => hash,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let now = Utc::now().to_rfc3339();
    if let Err(e) = diesel::update(u::users.filter(u::id.eq(&uid)))
        .set((u::password_hash.eq(&new_hash), u::updated_at.eq(&now)))
        .execute(&mut conn)
        .await
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }
    // Changing a password invalidates every previously issued access token,
    // including the token used for this request.
    revoke_user_sessions(&mut conn, &uid).await;

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
    let state = crate::require_app_state!(depot, res);

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
        .await
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
            .await
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
            tokio::spawn(async move {
                crate::services::EmailService::send_notification(
                    &pool,
                    &to,
                    "Password reset",
                    &body_html,
                )
                .await;
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
    let state = crate::require_app_state!(depot, res);

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
        .await
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
    if let Err(e) = PasswordService::validate_password(&mut conn, &body.password).await {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(e.to_response())));
        return;
    }

    // Hash the new password with Argon2.
    let new_hash = match password_crypto::hash_password(body.password).await {
        Ok(hash) => hash,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    // Consume the capability and update credentials in one write transaction.
    // Argon2 stays outside the transaction, while the conditional claim makes
    // simultaneous requests with the same token mutually exclusive.
    let reset = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                let now = Utc::now().to_rfc3339();
                let claimed = diesel::RunQueryDsl::execute(
                    diesel::update(
                        prt::password_reset_tokens
                            .filter(prt::id.eq(&token_row.id))
                            .filter(prt::used.eq(false))
                            .filter(prt::expires_at.gt(&now)),
                    )
                    .set(prt::used.eq(true)),
                    conn,
                )?;
                if claimed != 1 {
                    return Err(AppError::Validation(
                        "Invalid or expired reset token".into(),
                    ));
                }
                if diesel::RunQueryDsl::execute(
                    diesel::update(u::users.filter(u::id.eq(&token_row.user_id)))
                        .set((u::password_hash.eq(&new_hash), u::updated_at.eq(&now))),
                    conn,
                )? != 1
                {
                    return Err(AppError::NotFound("User not found".into()));
                }
                diesel::RunQueryDsl::execute(
                    diesel::update(sess::sessions.filter(sess::user_id.eq(&token_row.user_id)))
                        .set(sess::is_active.eq(false)),
                    conn,
                )?;
                // Other outstanding reset emails must not remain credentials for
                // the account after a successful password replacement.
                diesel::RunQueryDsl::execute(
                    diesel::update(
                        prt::password_reset_tokens.filter(prt::user_id.eq(&token_row.user_id)),
                    )
                    .set(prt::used.eq(true)),
                    conn,
                )?;
                Ok(())
            })
        })
        .await;
    if let Err(error) = reset {
        res.status_code(
            StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        );
        res.render(Json(ApiResponse::<()>::err(error.to_response())));
        return;
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(
        serde_json::json!({ "message": "Password has been reset successfully" }),
    )));
}
