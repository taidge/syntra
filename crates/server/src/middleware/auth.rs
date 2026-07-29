use std::sync::Arc;

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use salvo::prelude::*;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

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

    // WebSocket connections cannot set an Authorization header from the
    // browser, so fall back to a `?token=` query parameter for those routes.
    let token = match header_token.or_else(|| req.query::<String>("token")) {
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
            depot.insert(USER_ID_KEY, data.claims.sub);
            depot.insert(USER_ROLE_KEY, data.claims.role);
        }
        Err(_) => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(serde_json::json!({
                "success": false,
                "error": { "code": 401, "message": "Invalid or expired token" }
            })));
            ctrl.skip_rest();
        }
    }
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
