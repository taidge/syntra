use chrono::Utc;
use diesel::prelude::*;
use horme::RunQueryDsl;
use rand::Rng;
use salvo::prelude::*;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use syntra_core::dto::ApiResponse;
use syntra_core::error::AppError;
use syntra_core::models::NewApiKey;
use syntra_core::schema::api_keys::dsl as ak;

use crate::middleware::auth::{get_user_id, normalize_api_key_permissions};

const MAX_API_KEY_NAME_CHARS: usize = 100;

fn validate_api_key_name(value: &str) -> Result<String, &'static str> {
    let value = value.trim();
    if value.is_empty() {
        return Err("API key name cannot be empty");
    }
    if value.chars().count() > MAX_API_KEY_NAME_CHARS {
        return Err("API key name cannot exceed 100 characters");
    }
    Ok(value.to_string())
}

fn normalize_expiration(value: Option<&str>) -> Result<Option<String>, &'static str> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let expiration = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| "expires_at must be an RFC 3339 timestamp")?;
    if expiration <= Utc::now() {
        return Err("expires_at must be in the future");
    }
    Ok(Some(expiration.to_rfc3339()))
}

// ---------------------------------------------------------------------------
// POST /api-keys — create a new API key
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_api_key(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    #[derive(serde::Deserialize)]
    struct CreateApiKeyRequest {
        name: String,
        permissions: Option<String>,
        expires_at: Option<String>,
    }

    let body: CreateApiKeyRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let name = match validate_api_key_name(&body.name) {
        Ok(value) => value,
        Err(message) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(message.into()).to_response(),
            )));
            return;
        }
    };
    let permissions =
        match normalize_api_key_permissions(body.permissions.as_deref().unwrap_or("*")) {
            Ok(value) => value,
            Err(message) => {
                res.status_code(StatusCode::BAD_REQUEST);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Validation(message.into()).to_response(),
                )));
                return;
            }
        };
    let expires_at = match normalize_expiration(body.expires_at.as_deref()) {
        Ok(value) => value,
        Err(message) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(message.into()).to_response(),
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

    // Generate a random 32-byte key and encode as hex
    let mut raw_key = [0u8; 32];
    rand::rng().fill_bytes(&mut raw_key);
    let random_part: String = raw_key.iter().map(|b| format!("{:02x}", b)).collect();
    let full_key = format!("ctx_{random_part}");

    // Store a recognizable non-secret prefix for the management UI.
    let key_prefix = full_key[..12].to_string();

    // Hash the full key with SHA-256
    let mut hasher = Sha256::new();
    hasher.update(full_key.as_bytes());
    let key_hash = format!("{:x}", hasher.finalize());

    let now = Utc::now();
    let new_key = NewApiKey {
        id: Uuid::new_v4().to_string(),
        user_id: uid,
        name: name.clone(),
        key_hash,
        key_prefix: key_prefix.clone(),
        permissions: permissions.clone(),
        last_used_at: None,
        expires_at: expires_at.clone(),
        is_active: true,
        created_at: now.to_rfc3339(),
    };

    let key_id = new_key.id.clone();
    if let Err(e) = diesel::insert_into(ak::api_keys)
        .values(&new_key)
        .execute(&mut conn)
        .await
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "id": key_id,
        "name": name,
        "key": full_key,
        "key_prefix": key_prefix,
        "permissions": permissions,
        "expires_at": expires_at,
        "created_at": now.to_rfc3339(),
    }))));
}

// ---------------------------------------------------------------------------
// GET /api-keys — list user's API keys
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_api_keys(depot: &mut Depot, res: &mut Response) {
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

    match ak::api_keys
        .filter(ak::user_id.eq(&uid))
        .order(ak::created_at.desc())
        .select((
            ak::id,
            ak::name,
            ak::key_prefix,
            ak::permissions,
            ak::last_used_at,
            ak::expires_at,
            ak::is_active,
            ak::created_at,
        ))
        .load::<(
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            bool,
            String,
        )>(&mut conn)
        .await
    {
        Ok(rows) => {
            let keys: Vec<serde_json::Value> = rows
                .into_iter()
                .map(
                    |(kid, kname, kprefix, kperms, klast, kexpires, kactive, kcreated)| {
                        serde_json::json!({
                            "id": kid,
                            "name": kname,
                            "key_prefix": kprefix,
                            "permissions": kperms,
                            "last_used_at": klast,
                            "expires_at": kexpires,
                            "is_active": kactive,
                            "created_at": kcreated,
                        })
                    },
                )
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(keys)));
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
// DELETE /api-keys/:id — revoke an API key
// ---------------------------------------------------------------------------

#[handler]
pub async fn revoke_api_key(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let key_id: String = req.param("id").unwrap_or_default();

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

    match diesel::delete(
        ak::api_keys
            .filter(ak::id.eq(&key_id))
            .filter(ak::user_id.eq(&uid)),
    )
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("API key not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "API key revoked" }),
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
// PUT /api-keys/:id — update name/permissions
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_api_key(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let key_id: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct UpdateApiKeyRequest {
        name: Option<String>,
        permissions: Option<String>,
    }

    let body: UpdateApiKeyRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.name.is_none() && body.permissions.is_none() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("No fields to update".into()).to_response(),
        )));
        return;
    }

    let name = match body.name.as_deref().map(validate_api_key_name).transpose() {
        Ok(value) => value,
        Err(message) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(message.into()).to_response(),
            )));
            return;
        }
    };
    let permissions = match body
        .permissions
        .as_deref()
        .map(normalize_api_key_permissions)
        .transpose()
    {
        Ok(value) => value,
        Err(message) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(message.into()).to_response(),
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

    let target = ak::api_keys
        .filter(ak::id.eq(&key_id))
        .filter(ak::user_id.eq(&uid));

    let result = match (name.as_deref(), permissions.as_deref()) {
        (Some(name), Some(permissions)) => {
            diesel::update(target)
                .set((ak::name.eq(name), ak::permissions.eq(permissions)))
                .execute(&mut conn)
                .await
        }
        (Some(name), None) => {
            diesel::update(target)
                .set(ak::name.eq(name))
                .execute(&mut conn)
                .await
        }
        (None, Some(permissions)) => {
            diesel::update(target)
                .set(ak::permissions.eq(permissions))
                .execute(&mut conn)
                .await
        }
        (None, None) => unreachable!("empty updates are rejected above"),
    };

    match result {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("API key not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "API key updated" }),
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
