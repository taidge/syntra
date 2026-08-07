use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use salvo::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::state::AppState;
use contrix_core::schema::sessions::dsl as s;
use contrix_core::schema::users::dsl as u;

/// JWT claims embedded in every access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Subject — the user's UUID as a string.
    pub sub: String,
    /// User role (e.g. "admin", "editor", "viewer").
    pub role: String,
    /// Expiration timestamp (seconds since epoch).
    pub exp: u64,
}

/// Keys used to store / retrieve data in the Salvo [`Depot`].
const USER_ID_KEY: &str = "current_user_id";
const USER_ROLE_KEY: &str = "current_user_role";

/// Hash a bearer token before storing or looking it up in the session table.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Salvo handler that validates the `Authorization: Bearer <token>` header,
/// decodes the JWT, and injects the user id and role into the depot.
#[handler]
pub async fn jwt_auth_middleware(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    let header_token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
        .map(|t| t.to_string());

    // Browser WebSocket handshakes cannot set an Authorization header. Permit
    // `?token=` only on an actual WebSocket upgrade route; accepting it on
    // ordinary HTTP APIs would leak bearer tokens into URLs and proxy logs.
    let websocket_query_token = (req.uri().path().starts_with("/api/v1/ws/")
        && req
            .headers()
            .get("upgrade")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket")))
    .then(|| req.query::<String>("token"))
    .flatten();
    let token = match header_token.or(websocket_query_token) {
        Some(t) if !t.is_empty() => t,
        _ => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(serde_json::json!({
                "success": false,
                "error": { "code": 401, "message": "Missing or invalid Authorization header" }
            })));
            ctrl.skip_rest();
            return;
        }
    };

    let state = match depot.obtain::<Arc<AppState>>() {
        Ok(s) => s.clone(),
        Err(_) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(serde_json::json!({
                "success": false,
                "error": { "code": 500, "message": "Internal server error" }
            })));
            ctrl.skip_rest();
            return;
        }
    };

    let secret = &state.config.auth.jwt_secret;
    let validation = Validation::new(Algorithm::HS256);

    match decode::<JwtClaims>(
        &token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    ) {
        Ok(data) => {
            let mut conn = match state.db.get() {
                Ok(conn) => conn,
                Err(_) => {
                    reject(res, ctrl, "Authentication service unavailable");
                    return;
                }
            };
            let token_hash = hash_token(&token);
            let now = Utc::now().to_rfc3339();
            let active_session = s::sessions
                .inner_join(u::users.on(s::user_id.eq(u::id)))
                .filter(s::token_hash.eq(&token_hash))
                .filter(s::user_id.eq(&data.claims.sub))
                .filter(s::is_active.eq(true))
                .filter(s::expires_at.gt(&now))
                .select((s::id, u::role))
                .first::<(String, String)>(&mut conn)
                .optional();

            match active_session {
                Ok(Some((session_id, current_role))) if current_role != "disabled" => {
                    let _ = diesel::update(s::sessions.filter(s::id.eq(session_id)))
                        .set(s::last_activity_at.eq(&now))
                        .execute(&mut conn);
                    depot.insert(USER_ID_KEY, data.claims.sub);
                    // The database role is authoritative; the JWT copy can be
                    // stale after an administrator changes permissions.
                    depot.insert(USER_ROLE_KEY, current_role);
                }
                Ok(_) => reject(res, ctrl, "Session has been revoked or expired"),
                Err(_) => reject(res, ctrl, "Authentication service unavailable"),
            }
        }
        Err(_) => reject(res, ctrl, "Invalid or expired token"),
    }
}

fn reject(res: &mut Response, ctrl: &mut FlowCtrl, message: &str) {
    res.status_code(StatusCode::UNAUTHORIZED);
    res.render(Json(serde_json::json!({
        "success": false,
        "error": { "code": 401, "message": message }
    })));
    ctrl.skip_rest();
}

/// Extract the current user's UUID string from the depot.
///
/// Returns `None` if the auth middleware has not run or injection failed.
pub fn get_user_id(depot: &Depot) -> Option<String> {
    depot.get::<String>(USER_ID_KEY).ok().cloned()
}

/// Extract the current user's role string from the depot.
pub fn get_user_role(depot: &Depot) -> Option<String> {
    depot.get::<String>(USER_ROLE_KEY).ok().cloned()
}
