use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::Utc;
use contrix_core::error::AppError;
use contrix_core::schema::api_keys::dsl as ak;
use diesel::prelude::*;
use diesel::sql_types::{Integer, Nullable, Text};
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
    /// Unique session nonce. Older issued tokens remain readable; newly issued
    /// tokens always carry a fresh id even for two logins in the same second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jti: Option<String>,
}

/// Keys used to store / retrieve data in the Salvo [`Depot`].
const USER_ID_KEY: &str = "current_user_id";
const USER_ROLE_KEY: &str = "current_user_role";
const CREDENTIAL_KEY: &str = "authenticated_credential";

#[derive(Clone)]
pub struct AuthenticatedCredential {
    pub user_id: String,
    pub token_hash: String,
    pub api_key_id: Option<String>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
}

pub fn get_credential(depot: &Depot) -> Option<AuthenticatedCredential> {
    depot
        .get::<AuthenticatedCredential>(CREDENTIAL_KEY)
        .ok()
        .cloned()
}

/// Long-lived transports recheck the originating credential before processing
/// messages and periodically while idle. Never retain the raw bearer token.
pub async fn validate_credential(
    state: &AppState,
    credential: &AuthenticatedCredential,
    path: &str,
) -> Result<(), contrix_core::error::AppError> {
    validate_credential_for_request(state, credential, &salvo::http::Method::GET, path).await
}

/// Recheck an in-flight HTTP request using its actual method. A write key can
/// be downgraded while a large body is being read; safe-method scope checks
/// must not authorize the subsequent mutation.
pub async fn validate_credential_for_request(
    state: &AppState,
    credential: &AuthenticatedCredential,
    method: &salvo::http::Method,
    path: &str,
) -> Result<(), contrix_core::error::AppError> {
    let (credential, method, path) = (credential.clone(), method.clone(), path.to_owned());
    state
        .db
        .run(move |conn| {
            conn.transaction(|conn| {
                validate_credential_in_transaction(conn, &credential, &method, &path)
            })
        })
        .await
}

/// Use inside the same IMMEDIATE transaction as a protected mutation. This
/// orders a credential revocation or downgrade against the metadata commit,
/// including when storage I/O happened after the initial HTTP authentication.
pub fn validate_credential_in_transaction(
    conn: &mut diesel::SqliteConnection,
    credential: &AuthenticatedCredential,
    method: &salvo::http::Method,
    path: &str,
) -> Result<(), contrix_core::error::AppError> {
    use contrix_core::error::AppError;
    if credential
        .expires_at
        .is_some_and(|expiry| expiry <= Utc::now())
    {
        return Err(AppError::Auth("Credential expired".into()));
    }
    let expired = |value: &str| {
        chrono::DateTime::parse_from_rfc3339(value).map_or(true, |expiry| expiry <= Utc::now())
    };
    if let Some(key_id) = &credential.api_key_id {
        let row = diesel::OptionalExtension::optional(diesel::RunQueryDsl::first::<(
            String,
            String,
            Option<String>,
        )>(
            ak::api_keys
                .inner_join(u::users.on(ak::user_id.eq(u::id)))
                .filter(ak::id.eq(key_id))
                .filter(ak::user_id.eq(&credential.user_id))
                .filter(ak::key_hash.eq(&credential.token_hash))
                .filter(ak::is_active.eq(true))
                .select((u::role, ak::permissions, ak::expires_at)),
            conn,
        ))?
        .ok_or_else(|| AppError::Auth("API key revoked".into()))?;
        if row.0 == "disabled" || row.2.as_deref().is_some_and(expired) {
            return Err(AppError::Auth("API key expired or account disabled".into()));
        }
        // Mutation handlers retain the original URI. Match the initial HTTP
        // authentication's decoded, non-empty route segments on every recheck.
        let path =
            authorization_path(path).map_err(|message| AppError::Validation(message.into()))?;
        if !api_key_allows_request(&row.1, method, &path) {
            return Err(AppError::Forbidden(
                "API key does not permit this operation".into(),
            ));
        }
    } else {
        let (role, expiry) =
            diesel::OptionalExtension::optional(diesel::RunQueryDsl::first::<(String, String)>(
                s::sessions
                    .inner_join(u::users.on(s::user_id.eq(u::id)))
                    .filter(s::token_hash.eq(&credential.token_hash))
                    .filter(s::user_id.eq(&credential.user_id))
                    .filter(s::is_active.eq(true))
                    .select((u::role, s::expires_at)),
                conn,
            ))?
            .ok_or_else(|| AppError::Auth("Session has been revoked or expired".into()))?;
        if role == "disabled" || expired(&expiry) {
            return Err(AppError::Auth("Session has been revoked or expired".into()));
        }
    }
    crate::services::tenancy::user_tenant(conn, &credential.user_id)?;
    Ok(())
}

/// Canonicalize the comma-separated scopes accepted by API key endpoints.
///
/// `read` authorizes safe HTTP methods, `write` authorizes mutating methods,
/// `<resource>:read` and `<resource>:write` restrict access to one API route
/// family, and `*` authorizes everything. Unknown scopes are rejected instead
/// of being stored as permissions that the server cannot enforce.
pub fn normalize_api_key_permissions(value: &str) -> Result<String, &'static str> {
    let mut scopes = BTreeSet::new();

    for scope in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let scope = scope.to_ascii_lowercase();
        if matches!(scope.as_str(), "*" | "read" | "write") {
            scopes.insert(scope);
            continue;
        }
        let Some((resource, action)) = scope.split_once(':') else {
            return Err("Invalid API key permission scope");
        };
        if !matches!(action, "read" | "write") || !is_api_resource(resource) {
            return Err("Invalid API key permission scope");
        }
        scopes.insert(scope);
    }

    if scopes.contains("*") {
        if scopes.len() != 1 {
            return Err("Wildcard permission '*' cannot be combined with other scopes");
        }
        return Ok("*".to_string());
    }
    if scopes.is_empty() {
        return Err("At least one API key permission is required");
    }
    Ok(scopes.into_iter().collect::<Vec<_>>().join(","))
}

fn is_api_resource(resource: &str) -> bool {
    matches!(
        resource,
        "admin"
            | "ai"
            | "auth"
            | "comments"
            | "convert"
            | "documents"
            | "favorites"
            | "files"
            | "folders"
            | "groups"
            | "invitations"
            | "notification-preferences"
            | "notifications"
            | "presence"
            | "quota"
            | "recent"
            | "rooms"
            | "search"
            | "sessions"
            | "settings"
            | "shares"
            | "tags"
            | "templates"
            | "totp"
            | "trash"
            | "upload"
            | "webhooks"
            | "ws"
    )
}

/// Salvo matches decoded non-empty path segments. Authorization must use the
/// same representation: raw `%61pi-keys` or duplicate slashes are not a separate
/// permission boundary from the route they resolve to.
fn authorization_path(path: &str) -> Result<String, &'static str> {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = if bytes[index] == b'%' {
            let high = *bytes.get(index + 1).ok_or("Invalid encoded request path")?;
            let low = *bytes.get(index + 2).ok_or("Invalid encoded request path")?;
            let byte = hex(high)
                .zip(hex(low))
                .map(|(high, low)| high * 16 + low)
                .ok_or("Invalid encoded request path")?;
            // Encoded separators are not accepted on protected API routes;
            // joining them could disagree with the router's segment boundary.
            if matches!(byte, b'/' | b'\\') {
                return Err("Encoded path separators are not permitted");
            }
            index += 3;
            byte
        } else {
            index += 1;
            bytes[index - 1]
        };
        if byte.is_ascii_control() || matches!(byte, b'\\' | b'?' | b'#') {
            return Err("Invalid request path");
        }
        decoded.push(byte);
    }
    let decoded = String::from_utf8(decoded).map_err(|_| "Invalid UTF-8 request path")?;
    Ok(format!(
        "/{}",
        decoded
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .join("/")
    ))
}

fn api_key_allows_request(permissions: &str, method: &salvo::http::Method, path: &str) -> bool {
    if permissions == "*" {
        return true;
    }

    let required_scope = if path.starts_with("/api/v1/ws/collab/") {
        "write"
    } else if matches!(
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

    let resource = path
        .strip_prefix("/api/v1/")
        .and_then(|path| path.split('/').next())
        .filter(|resource| is_api_resource(resource));
    permissions.split(',').map(str::trim).any(|scope| {
        scope == required_scope
            || resource.is_some_and(|resource| {
                scope
                    .split_once(':')
                    .is_some_and(|parts| parts == (resource, required_scope))
            })
    })
}

/// Hash a bearer token before storing or looking it up in the session table.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

enum RequestCredential {
    Session(JwtClaims),
    ApiKey {
        method: salvo::http::Method,
        path: Result<String, &'static str>,
    },
}

#[derive(diesel::QueryableByName)]
struct RequestIdentity {
    #[diesel(sql_type = Text)]
    credential_id: String,
    #[diesel(sql_type = Text)]
    user_id: String,
    #[diesel(sql_type = Text)]
    role: String,
    #[diesel(sql_type = Nullable<Text>)]
    tenant_id: Option<String>,
    #[diesel(sql_type = Nullable<Integer>)]
    tenant_active: Option<i32>,
    #[diesel(sql_type = Nullable<Text>)]
    permissions: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    expires_at: Option<String>,
    #[diesel(sql_type = Nullable<Text>)]
    last_seen_at: Option<String>,
}

// The tenant assignment is not part of the Diesel User model. A joined SQL
// statement gives session/key, role and tenant activity one read snapshot.
const SESSION_IDENTITY_SQL: &str = "
    SELECT sessions.id AS credential_id, sessions.user_id, users.role, users.tenant_id,
           tenants.is_active AS tenant_active, NULL AS permissions,
           sessions.expires_at, sessions.last_activity_at AS last_seen_at
      FROM sessions JOIN users ON users.id = sessions.user_id
      LEFT JOIN tenants ON tenants.id = users.tenant_id
     WHERE sessions.token_hash = ? AND sessions.user_id = ?
       AND sessions.is_active = 1 AND sessions.expires_at > ?";

const API_KEY_IDENTITY_SQL: &str = "
    SELECT api_keys.id AS credential_id, api_keys.user_id, users.role, users.tenant_id,
           tenants.is_active AS tenant_active, api_keys.permissions,
           api_keys.expires_at, api_keys.last_used_at AS last_seen_at
      FROM api_keys JOIN users ON users.id = api_keys.user_id
      LEFT JOIN tenants ON tenants.id = users.tenant_id
     WHERE api_keys.key_hash = ? AND api_keys.is_active = 1";

const SESSION_ACTIVITY_SQL: &str = "
    UPDATE sessions SET last_activity_at = ?
     WHERE id = ? AND is_active = 1 AND last_activity_at IS ?";
const API_KEY_ACTIVITY_SQL: &str = "
    UPDATE api_keys SET last_used_at = ?
     WHERE id = ? AND is_active = 1 AND last_used_at IS ?";

fn read_request_identity(
    conn: &mut diesel::SqliteConnection,
    credential: &RequestCredential,
    token_hash: &str,
    now: chrono::DateTime<Utc>,
) -> Result<(RequestIdentity, Option<chrono::DateTime<Utc>>), AppError> {
    let (identity, invalid_message) = match credential {
        RequestCredential::Session(claims) => (
            diesel::sql_query(SESSION_IDENTITY_SQL)
                .bind::<Text, _>(token_hash)
                .bind::<Text, _>(&claims.sub)
                .bind::<Text, _>(now.to_rfc3339())
                .get_result::<RequestIdentity>(conn)
                .optional()?,
            "Session has been revoked or expired",
        ),
        RequestCredential::ApiKey { .. } => (
            diesel::sql_query(API_KEY_IDENTITY_SQL)
                .bind::<Text, _>(token_hash)
                .get_result::<RequestIdentity>(conn)
                .optional()?,
            "Invalid or expired token",
        ),
    };
    let identity = identity
        .filter(|identity| identity.role != "disabled")
        .ok_or_else(|| AppError::Auth(invalid_message.into()))?;

    let expires_at = match credential {
        RequestCredential::Session(claims) => i64::try_from(claims.exp)
            .ok()
            .and_then(|value| chrono::DateTime::from_timestamp(value, 0)),
        RequestCredential::ApiKey { method, path } => {
            let expires_at = identity
                .expires_at
                .as_deref()
                .map(chrono::DateTime::parse_from_rfc3339)
                .transpose()
                .map_err(|_| AppError::Auth(invalid_message.into()))?
                .map(|expires| expires.with_timezone(&Utc));
            if expires_at.is_some_and(|expires| expires <= now) {
                return Err(AppError::Auth(invalid_message.into()));
            }
            let path = path
                .as_deref()
                .map_err(|message| AppError::Validation((*message).into()))?;
            if path == "/api/v1/api-keys" || path.starts_with("/api/v1/api-keys/") {
                return Err(AppError::Forbidden(
                    "A JWT session is required to manage API keys".into(),
                ));
            }
            if !api_key_allows_request(
                identity.permissions.as_deref().unwrap_or_default(),
                method,
                path,
            ) {
                return Err(AppError::Forbidden(
                    "API key does not permit this operation".into(),
                ));
            }
            expires_at
        }
    };

    if expires_at.is_some_and(|expires| expires <= now) {
        return Err(AppError::Auth(invalid_message.into()));
    }

    if identity.tenant_id.is_some() && identity.tenant_active != Some(1) {
        return Err(AppError::Forbidden(
            "Permission denied: Tenant is inactive or missing".into(),
        ));
    }
    Ok((identity, expires_at))
}

fn authenticate_request(
    conn: &mut diesel::SqliteConnection,
    credential: RequestCredential,
    token_hash: String,
) -> Result<(AuthenticatedCredential, String), AppError> {
    let now = Utc::now();
    let (mut identity, mut expires_at) =
        read_request_identity(conn, &credential, &token_hash, now)?;
    let is_api_key = matches!(credential, RequestCredential::ApiKey { .. });

    // These timestamps are display metadata. An authenticated request can
    // refresh them once per 30 seconds. Skip issuing a write when fresh and
    // compare the sampled value inside UPDATE to prevent concurrent refreshes
    // from repeatedly writing the same credential or moving its clock back.
    let threshold = now - chrono::Duration::seconds(30);
    if identity.last_seen_at.as_deref().is_none_or(|value| {
        chrono::DateTime::parse_from_rfc3339(value).map_or(true, |seen| seen <= threshold)
    }) {
        let activity_sql = if is_api_key {
            API_KEY_ACTIVITY_SQL
        } else {
            SESSION_ACTIVITY_SQL
        };
        let _ = diesel::sql_query(activity_sql)
            .bind::<Text, _>(now.to_rfc3339())
            .bind::<Text, _>(&identity.credential_id)
            .bind::<Nullable<Text>, _>(&identity.last_seen_at)
            .execute(conn);
        // A pending SQLite write can wait behind another writer. Read current
        // authorization again after that wait, before allowing the request.
        (identity, expires_at) = read_request_identity(conn, &credential, &token_hash, Utc::now())?;
    }

    let role = if identity.tenant_id.is_some() && identity.role == "admin" {
        "tenant_admin".to_owned()
    } else {
        identity.role
    };
    Ok((
        AuthenticatedCredential {
            user_id: identity.user_id,
            token_hash,
            api_key_id: is_api_key.then_some(identity.credential_id),
            expires_at,
        },
        role,
    ))
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
    let mut validation = Validation::new(Algorithm::HS256);
    validation.leeway = 0;

    let credential = match decode::<JwtClaims>(
        &token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    ) {
        Ok(data) => RequestCredential::Session(data.claims),
        // Browser WebSocket URL tokens must be JWTs. API keys are accepted
        // only through the Authorization header.
        Err(_) if token_from_header => RequestCredential::ApiKey {
            method: req.method().clone(),
            path: authorization_path(req.uri().path()),
        },
        Err(_) => {
            reject(
                res,
                ctrl,
                StatusCode::UNAUTHORIZED,
                "Invalid or expired token",
            );
            return;
        }
    };

    let stage = match &credential {
        RequestCredential::Session(_) => "session_identity",
        RequestCredential::ApiKey { .. } => "api_key_identity",
    };
    let token_hash = hash_token(&token);
    match state
        .db
        .run(move |conn| authenticate_request(conn, credential, token_hash))
        .await
    {
        Ok((credential, role)) => {
            depot.insert(USER_ID_KEY, credential.user_id.clone());
            depot.insert(USER_ROLE_KEY, role);
            depot.insert(CREDENTIAL_KEY, credential);
        }
        Err(AppError::Auth(message)) => reject(res, ctrl, StatusCode::UNAUTHORIZED, &message),
        Err(AppError::Forbidden(message)) => reject(res, ctrl, StatusCode::FORBIDDEN, &message),
        Err(AppError::Validation(message)) => reject(res, ctrl, StatusCode::BAD_REQUEST, &message),
        Err(error) => {
            let cause = super::request_observability::database_cause(&error);
            if error.is_transient_database() {
                super::request_observability::record_failure(stage, cause);
                reject(
                    res,
                    ctrl,
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Authentication service temporarily unavailable",
                );
            } else {
                tracing::error!(stage, cause, "Authentication database failed");
                reject(
                    res,
                    ctrl,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error",
                );
            }
        }
    }
}

fn reject(res: &mut Response, ctrl: &mut FlowCtrl, status: StatusCode, message: &str) {
    res.status_code(status);
    res.render(Json(serde_json::json!({
        "success": false,
        "error": { "code": status.as_u16(), "message": message }
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
    use super::{api_key_allows_request, authorization_path, normalize_api_key_permissions};
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
        assert_eq!(
            normalize_api_key_permissions("files:write, documents:read,files:write"),
            Ok("documents:read,files:write".into())
        );
        assert!(normalize_api_key_permissions("unknown:read").is_err());
        assert!(normalize_api_key_permissions("files:delete").is_err());
    }

    #[test]
    fn api_key_permissions_control_http_methods() {
        assert!(api_key_allows_request(
            "read",
            &Method::GET,
            "/api/v1/files"
        ));
        assert!(!api_key_allows_request(
            "read",
            &Method::POST,
            "/api/v1/files"
        ));
        assert!(api_key_allows_request(
            "files:write",
            &Method::DELETE,
            "/api/v1/files/123"
        ));
        assert!(!api_key_allows_request(
            "files:write",
            &Method::POST,
            "/api/v1/documents"
        ));
        assert!(api_key_allows_request(
            "documents:read,files:write",
            &Method::GET,
            "/api/v1/documents/123"
        ));
        assert!(api_key_allows_request(
            "*",
            &Method::PATCH,
            "/api/v1/admin/users/123"
        ));
    }
    #[test]
    fn collaborative_upgrade_requires_write_capability_even_though_it_uses_get() {
        for scope in ["read", "ws:read", "files:write"] {
            assert!(!api_key_allows_request(
                scope,
                &Method::GET,
                "/api/v1/ws/collab/doc"
            ));
        }
        for scope in ["write", "ws:write", "*"] {
            assert!(api_key_allows_request(
                scope,
                &Method::GET,
                "/api/v1/ws/collab/doc"
            ));
        }
        assert!(api_key_allows_request(
            "ws:read",
            &Method::GET,
            "/api/v1/ws/notifications"
        ));
    }

    #[test]
    fn authorization_uses_decoded_router_segments_and_rejects_ambiguous_separators() {
        assert_eq!(
            authorization_path("/api//v1/%61pi-keys/"),
            Ok("/api/v1/api-keys".into())
        );
        assert_eq!(
            authorization_path("/api/v1/ws/%63ollab/doc"),
            Ok("/api/v1/ws/collab/doc".into())
        );
        for path in [
            "/api/v1%2fapi-keys",
            "/api/v1/%5capi-keys",
            "/api/v1/%",
            "/api/v1/%ff",
        ] {
            assert!(authorization_path(path).is_err());
        }
    }
}
