use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, CreateShareRequest, ShareResponse};
use contrix_core::error::AppError;
use contrix_core::models::{File, NewShare, SharePermission};
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::shares::dsl as s;

use crate::middleware::auth::get_user_id;
use crate::services::StorageService;

async fn hash_share_password(password: &str) -> Result<String, AppError> {
    crate::services::password_crypto::hash_secret(
        password.to_owned(),
        "Failed to hash share password",
    )
    .await
}

// ---------------------------------------------------------------------------
// POST /shares
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_share(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: CreateShareRequest = match req.parse_json().await {
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

    // Verify file ownership.
    let file_count: i64 = match f::files
        .filter(f::id.eq(body.file_id.to_string()))
        .filter(f::owner_id.eq(&uid))
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

    if file_count == 0 {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("File not found or not owned by you".into()).to_response(),
        )));
        return;
    }

    let share_id_val = Uuid::new_v4();
    let now = Utc::now();
    let shared_with_str = body.shared_with_user_id.map(|sw| sw.to_string());
    let pub_token = if body.shared_with_user_id.is_none() {
        Some(Uuid::new_v4().to_string().replace('-', ""))
    } else {
        None
    };
    let expires_at_str = body.expires_at.map(|dt| dt.to_rfc3339());

    let shared_with_group_str = body.shared_with_group_id.map(|g| g.to_string());

    let password_hash_val = match body.password.as_deref() {
        Some(password) if !password.is_empty() => match hash_share_password(password).await {
            Ok(hash) => Some(hash),
            Err(e) => {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(e.to_response())));
                return;
            }
        },
        _ => None,
    };

    let new_share = NewShare {
        id: share_id_val.to_string(),
        file_id: body.file_id.to_string(),
        shared_by: uid.clone(),
        shared_with: shared_with_str,
        permission: body.permission.as_str().to_string(),
        public_token: pub_token.clone(),
        expires_at: expires_at_str,
        created_at: now.to_rfc3339(),
        shared_with_group: shared_with_group_str,
        password_hash: password_hash_val.clone(),
    };

    match diesel::insert_into(s::shares)
        .values(&new_share)
        .execute(&mut conn)
        .await
    {
        Ok(_) => {
            crate::services::WebhookService::fire_event(
                &state.db,
                "share.created",
                &serde_json::json!({
                    "id": share_id_val.to_string(),
                    "file_id": body.file_id.to_string(),
                    "shared_by_user_id": &uid,
                    "shared_with_user_id": body.shared_with_user_id.map(|u| u.to_string()),
                    "shared_with_group_id": body.shared_with_group_id.map(|g| g.to_string()),
                    "permission": body.permission.as_str(),
                    "public_token": &pub_token,
                    "created_at": now.to_rfc3339(),
                }),
            )
            .await;

            if let Some(target) = body.shared_with_user_id.map(|u| u.to_string())
                && target != uid
            {
                let from_username = contrix_core::schema::users::dsl::users
                    .filter(contrix_core::schema::users::dsl::id.eq(&uid))
                    .select(contrix_core::schema::users::dsl::username)
                    .first::<String>(&mut conn)
                    .await
                    .unwrap_or_else(|_| "Someone".to_string());
                let file_id = body.file_id.to_string();
                crate::services::NotificationService::create(
                    &state.db,
                    &state.notifications,
                    &target,
                    "share",
                    "New share",
                    &format!("{from_username} shared a file with you"),
                    Some(&format!("/files/{file_id}")),
                )
                .await;
            }

            let resp = ShareResponse {
                id: share_id_val,
                file_id: body.file_id,
                shared_by_user_id: uid.parse().unwrap_or_default(),
                shared_with_user_id: body.shared_with_user_id,
                permission: body.permission,
                public_token: pub_token,
                expires_at: body.expires_at,
                created_at: now,
                shared_with_group_id: body.shared_with_group_id,
                is_password_protected: password_hash_val.is_some(),
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
// GET /shares/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_share(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let share_id_param: String = req.param("id").unwrap_or_default();

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

    match s::shares
        .filter(s::id.eq(&share_id_param))
        .filter(s::shared_by.eq(&uid))
        .select((
            s::id,
            s::file_id,
            s::shared_by,
            s::shared_with,
            s::permission,
            s::public_token,
            s::expires_at,
            s::created_at,
            s::shared_with_group,
            s::password_hash,
        ))
        .first::<(
            String,
            String,
            String,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
        )>(&mut conn)
        .await
        .optional()
    {
        Ok(Some((sid, fid, sby, sw, perm, pt, exp, cat, swg, pwh))) => {
            let resp = ShareResponse {
                id: sid.parse().unwrap_or_default(),
                file_id: fid.parse().unwrap_or_default(),
                shared_by_user_id: sby.parse().unwrap_or_default(),
                shared_with_user_id: sw.as_deref().and_then(|v| v.parse().ok()),
                permission: SharePermission::parse_lossy(&perm),
                public_token: pt,
                expires_at: exp
                    .as_deref()
                    .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                    .map(|d| d.with_timezone(&Utc)),
                created_at: cat.parse().unwrap_or_else(|_| Utc::now()),
                shared_with_group_id: swg.as_deref().and_then(|v| v.parse().ok()),
                is_password_protected: pwh.is_some(),
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(resp)));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Share not found".into()).to_response(),
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
// PUT /shares/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_share(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let share_id_param: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct UpdateShareReq {
        permission: Option<String>,
        expires_at: Option<String>,
    }

    let body: UpdateShareReq = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.permission.is_none() && body.expires_at.is_none() {
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

    // Handle the dynamic update by branching on which fields are present.
    let result = if body.permission.is_some() && body.expires_at.is_some() {
        diesel::update(
            s::shares
                .filter(s::id.eq(&share_id_param))
                .filter(s::shared_by.eq(&uid)),
        )
        .set((
            s::permission.eq(body.permission.as_ref().unwrap()),
            s::expires_at.eq(body.expires_at.as_ref()),
        ))
        .execute(&mut conn)
        .await
    } else if body.permission.is_some() {
        diesel::update(
            s::shares
                .filter(s::id.eq(&share_id_param))
                .filter(s::shared_by.eq(&uid)),
        )
        .set(s::permission.eq(body.permission.as_ref().unwrap()))
        .execute(&mut conn)
        .await
    } else {
        diesel::update(
            s::shares
                .filter(s::id.eq(&share_id_param))
                .filter(s::shared_by.eq(&uid)),
        )
        .set(s::expires_at.eq(body.expires_at.as_ref()))
        .execute(&mut conn)
        .await
    };

    match result {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Share not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Share updated" }),
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
// DELETE /shares/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_share(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let share_id_param: String = req.param("id").unwrap_or_default();

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
        s::shares
            .filter(s::id.eq(&share_id_param))
            .filter(s::shared_by.eq(&uid)),
    )
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Share not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Share deleted" }),
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
// GET /shares/file/:file_id
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_file_shares(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("file_id").unwrap_or_default();

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

    match s::shares
        .filter(s::file_id.eq(&file_id_param))
        .filter(s::shared_by.eq(&uid))
        .order(s::created_at.desc())
        .select((
            s::id,
            s::file_id,
            s::shared_by,
            s::shared_with,
            s::permission,
            s::public_token,
            s::expires_at,
            s::created_at,
            s::shared_with_group,
            s::password_hash,
        ))
        .load::<(
            String,
            String,
            String,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
        )>(&mut conn)
        .await
    {
        Ok(rows) => {
            let share_list: Vec<ShareResponse> = rows
                .into_iter()
                .map(
                    |(sid, fid, sby, sw, perm, pt, exp, cat, swg, pwh)| ShareResponse {
                        id: sid.parse().unwrap_or_default(),
                        file_id: fid.parse().unwrap_or_default(),
                        shared_by_user_id: sby.parse().unwrap_or_default(),
                        shared_with_user_id: sw.as_deref().and_then(|v| v.parse().ok()),
                        permission: SharePermission::parse_lossy(&perm),
                        public_token: pt,
                        expires_at: exp
                            .as_deref()
                            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                            .map(|d| d.with_timezone(&Utc)),
                        created_at: cat.parse().unwrap_or_else(|_| Utc::now()),
                        shared_with_group_id: swg.as_deref().and_then(|v| v.parse().ok()),
                        is_password_protected: pwh.is_some(),
                    },
                )
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(share_list)));
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
// GET /public/shares/{token}  (Public, unauthenticated share access)
// ---------------------------------------------------------------------------

/// Download a file via a public share token. No authentication is required, but
/// the share must exist, be unexpired, and — if password-protected — the
/// correct password must be supplied via the `password` query parameter.
#[handler]
pub async fn access_public_share(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let token: String = req.param("token").unwrap_or_default();
    let supplied_password: Option<String> = req.query("password");

    if token.is_empty() {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Share not found".into()).to_response(),
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

    // Look up the share by its public token.
    let share = s::shares
        .filter(s::public_token.eq(&token))
        .select((s::file_id, s::expires_at, s::password_hash))
        .first::<(String, Option<String>, Option<String>)>(&mut conn)
        .await
        .optional();

    let (file_id, expires_at, password_hash) = match share {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Share not found".into()).to_response(),
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

    // Reject expired shares.
    if let Some(exp) = expires_at
        && let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&exp)
        && dt.with_timezone(&Utc) < Utc::now()
    {
        res.status_code(StatusCode::GONE);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("This share has expired".into()).to_response(),
        )));
        return;
    }

    // Enforce the share password when one is set.
    if let Some(ref hash) = password_hash {
        let ok = match supplied_password.as_deref() {
            Some(password) => {
                crate::services::password_crypto::verify_password(password.to_owned(), hash.clone())
                    .await
                    .unwrap_or(false)
            }
            None => false,
        };
        if !ok {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("A valid password is required for this share".into()).to_response(),
            )));
            return;
        }
    }

    // Load the underlying file and stream its content.
    let file_row = match f::files
        .filter(f::id.eq(&file_id))
        .filter(f::is_trashed.eq(false))
        .first::<File>(&mut conn)
        .await
        .optional()
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Shared file no longer exists".into()).to_response(),
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

    let data = match state.storage.read_file(&file_row.path).await {
        Ok(d) => d,
        Err(_) => {
            // Documents are stored as JSON blobs; fall back to the document row.
            use contrix_core::schema::documents::dsl as d;
            let content = d::documents
                .filter(d::file_id.eq(&file_row.id))
                .select(d::content)
                .first::<Option<String>>(&mut conn)
                .await
                .optional()
                .ok()
                .flatten()
                .flatten();
            match content {
                Some(c) => c.into_bytes(),
                None => {
                    res.status_code(StatusCode::NOT_FOUND);
                    res.render(Json(ApiResponse::<()>::err(
                        AppError::NotFound("Shared file content is unavailable".into())
                            .to_response(),
                    )));
                    return;
                }
            }
        }
    };

    let ct = file_row
        .mime_type
        .unwrap_or_else(|| "application/octet-stream".to_string());
    res.status_code(StatusCode::OK);
    if let Ok(ct_val) = ct.parse() {
        res.headers_mut().insert("content-type", ct_val);
    }
    if let Ok(cd) = format!(
        "attachment; filename=\"{}\"",
        StorageService::sanitize_file_name(&file_row.name)
    )
    .parse()
    {
        res.headers_mut().insert("content-disposition", cd);
    }
    res.body(salvo::http::body::ResBody::Once(bytes::Bytes::from(data)));
}
