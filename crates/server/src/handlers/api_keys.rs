use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use rand::Rng;
use salvo::prelude::*;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::NewApiKey;
use contrix_core::schema::api_keys::dsl as ak;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// POST /api-keys — create a new API key
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_api_key(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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
    let full_key: String = raw_key.iter().map(|b| format!("{:02x}", b)).collect();

    // Store first 8 chars as prefix
    let key_prefix = full_key[..8].to_string();

    // Hash the full key with SHA-256
    let mut hasher = Sha256::new();
    hasher.update(full_key.as_bytes());
    let key_hash = format!("{:x}", hasher.finalize());

    let now = Utc::now();
    let new_key = NewApiKey {
        id: Uuid::new_v4().to_string(),
        user_id: uid,
        name: body.name.clone(),
        key_hash,
        key_prefix: key_prefix.clone(),
        permissions: body.permissions.unwrap_or_else(|| "*".to_string()),
        last_used_at: None,
        expires_at: body.expires_at.clone(),
        is_active: true,
        created_at: now.to_rfc3339(),
    };

    let key_id = new_key.id.clone();
    if let Err(e) = diesel::insert_into(ak::api_keys)
        .values(&new_key)
        .execute(&mut conn)
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
        "name": body.name,
        "key": full_key,
        "key_prefix": key_prefix,
        "permissions": new_key.permissions,
        "expires_at": body.expires_at,
        "created_at": now.to_rfc3339(),
    }))));
}

// ---------------------------------------------------------------------------
// GET /api-keys — list user's API keys
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_api_keys(depot: &mut Depot, res: &mut Response) {
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

    let result = if body.name.is_some() && body.permissions.is_some() {
        diesel::update(target)
            .set((
                ak::name.eq(body.name.as_ref().unwrap()),
                ak::permissions.eq(body.permissions.as_ref().unwrap()),
            ))
            .execute(&mut conn)
    } else if body.name.is_some() {
        diesel::update(target)
            .set(ak::name.eq(body.name.as_ref().unwrap()))
            .execute(&mut conn)
    } else {
        diesel::update(target)
            .set(ak::permissions.eq(body.permissions.as_ref().unwrap()))
            .execute(&mut conn)
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
