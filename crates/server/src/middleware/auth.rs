use std::sync::Arc;

use chrono::Utc;
use contrix_core::schema::api_keys::dsl as ak;
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

/// Canonicalize the comma-separated scopes accepted by API key endpoints.
///
/// `read` authorizes safe HTTP methods, `write` authorizes mutating methods,
/// and `*` authorizes both. Unknown scopes are rejected instead of being
/// stored as permissions that the server cannot enforce.
pub fn normalize_api_key_permissions(value: &str) -> Result<String, &'static str> {
    let mut read = false;
    let mut write = false;
    let mut wildcard = false;

    for scope in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match scope.to_ascii_lowercase().as_str() {
            "read" => read = true,
            "write" => write = true,
            "*" => wildcard = true,
            _ => return Err("Permissions must be '*', 'read', 'write', or 'read,write'"),
        }
    }

    if wildcard {
        if read || write {
            return Err("Wildcard permission '*' cannot be combined with other scopes");
        }
        return Ok("*".to_string());
    }

    match (read, write) {
        (true, true) => Ok("read,write".to_string()),
        (true, false) => Ok("read".to_string()),
        (false, true) => Ok("write".to_string()),
        (false, false) => Err("At least one API key permission is required"),
    }
}

fn api_key_allows_method(permissions: &str, method: &salvo::http::Method) -> bool {
    if permissions == "*" {
        return true;
    }

    let required_scope = if matches!(
        *method,
        salvo::http::Method::GET
            | salvo::http::Method::HEAD
            | salvo::http::Method::OPTIONS
            | salvo::http::Method::TRACE
    ) {
        "read"
    } else {
        "write"
    };

    permissions
        .split(',')
        .map(str::trim)
        .any(|scope| scope == required_scope)
}

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
        .and_then(|header| {
            let (scheme, token) = header.split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then_some(token)
        })
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
    let token_from_header = header_token.is_some();
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
        Err(_) if token_from_header => {
            // JWT decoding failed. Fall back to a stored API key, which is
            // deliberately accepted only from the Authorization header (not
            // from a WebSocket URL query parameter).
            let mut conn = match state.db.get() {
                Ok(conn) => conn,
                Err(_) => {
                    reject(res, ctrl, "Authentication service unavailable");
                    return;
                }
            };
            let token_hash = hash_token(&token);
            let api_key = ak::api_keys
                .inner_join(u::users.on(ak::user_id.eq(u::id)))
                .filter(ak::key_hash.eq(&token_hash))
                .filter(ak::is_active.eq(true))
                .select((
                    ak::id,
                    ak::user_id,
                    u::role,
                    ak::permissions,
                    ak::expires_at,
                ))
                .first::<(String, String, String, String, Option<String>)>(&mut conn)
                .optional();

            match api_key {
                Ok(Some((key_id, user_id, role, permissions, expires_at)))
                    if role != "disabled"
                        && expires_at.as_deref().is_none_or(|value| {
                            chrono::DateTime::parse_from_rfc3339(value)
                                .is_ok_and(|expires| expires > Utc::now())
                        }) =>
                {
                    let path = req.uri().path();
                    if path == "/api/v1/api-keys" || path.starts_with("/api/v1/api-keys/") {
                        res.status_code(StatusCode::FORBIDDEN);
                        res.render(Json(serde_json::json!({
                            "success": false,
                            "error": {
                                "code": 403,
                                "message": "A JWT session is required to manage API keys"
                            }
                        })));
                        ctrl.skip_rest();
                        return;
                    }

                    if !api_key_allows_method(&permissions, req.method()) {
                        res.status_code(StatusCode::FORBIDDEN);
                        res.render(Json(serde_json::json!({
                            "success": false,
                            "error": {
                                "code": 403,
                                "message": "API key does not permit this operation"
                            }
                        })));
                        ctrl.skip_rest();
                        return;
                    }

                    let _ = diesel::update(ak::api_keys.filter(ak::id.eq(key_id)))
                        .set(ak::last_used_at.eq(Some(Utc::now().to_rfc3339())))
                        .execute(&mut conn);
                    depot.insert(USER_ID_KEY, user_id);
                    depot.insert(USER_ROLE_KEY, role);
                }
                Ok(_) => reject(res, ctrl, "Invalid or expired token"),
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

#[cfg(test)]
mod tests {
    use super::{api_key_allows_method, normalize_api_key_permissions};
    use salvo::http::Method;

    #[test]
    fn api_key_permissions_are_normalized_and_fail_closed() {
        assert_eq!(
            normalize_api_key_permissions("WRITE, read"),
            Ok("read,write".into())
        );
        assert_eq!(normalize_api_key_permissions("*"), Ok("*".into()));
        assert!(normalize_api_key_permissions("").is_err());
        assert!(normalize_api_key_permissions("admin").is_err());
        assert!(normalize_api_key_permissions("*,read").is_err());
    }

    #[test]
    fn api_key_permissions_control_http_methods() {
        assert!(api_key_allows_method("read", &Method::GET));
        assert!(!api_key_allows_method("read", &Method::POST));
        assert!(api_key_allows_method("write", &Method::DELETE));
        assert!(!api_key_allows_method("write", &Method::GET));
        assert!(api_key_allows_method("*", &Method::PATCH));
    }
}
