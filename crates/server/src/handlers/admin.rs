use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use chrono::Utc;
use contrix_core::dto::{AdminStats, ApiResponse};
use contrix_core::error::AppError;
use contrix_core::models::NewUser;
use contrix_core::schema::activity_logs::dsl as a;
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::sessions::dsl as sess;
use contrix_core::schema::users::dsl as u;

use crate::middleware::auth::{get_user_id, get_user_role};

/// Extract `(ip_address, user_agent)` from a request for audit logging.
fn request_context(req: &Request) -> (Option<String>, Option<String>) {
    let ip = req.remote_addr().ip().map(|i| i.to_string());
    let ua = req
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    (ip, ua)
}

/// Verify the caller has admin role; returns `false` and writes a 403 if not.
fn require_admin(depot: &Depot, res: &mut Response) -> bool {
    match get_user_role(depot) {
        Some(role) if role == "admin" => true,
        _ => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Forbidden("Admin access required".into()).to_response(),
            )));
            false
        }
    }
}

// ---------------------------------------------------------------------------
// GET /admin/stats
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_stats(depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);

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

    let total_users = u::users.count().get_result::<i64>(&mut conn).unwrap_or(0) as u64;

    let total_files = f::files
        .filter(f::is_trashed.eq(false))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0) as u64;

    let total_documents = d::documents
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0) as u64;

    // Use raw SQL for the SUM to avoid Numeric type mismatch with SQLite.
    let total_storage_used: i64 =
        diesel::sql_query("SELECT COALESCE(SUM(storage_used), 0) AS total FROM users")
            .get_result::<StorageSum>(&mut conn)
            .map(|r| r.total)
            .unwrap_or(0);

    let stats = AdminStats {
        total_users,
        total_files,
        total_documents,
        total_storage_used,
    };

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(stats)));
}

/// Helper struct for the raw SQL sum query.
#[derive(diesel::QueryableByName)]
struct StorageSum {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    total: i64,
}

// ---------------------------------------------------------------------------
// GET /admin/users
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_users(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let page: u32 = req.query("page").unwrap_or(1);
    let per_page: u32 = req.query("per_page").unwrap_or(50);
    let offset_val = ((page.saturating_sub(1)) * per_page) as i64;
    let limit_val = per_page as i64;

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
        .order(u::created_at.desc())
        .limit(limit_val)
        .offset(offset_val)
        .select((
            u::id,
            u::username,
            u::email,
            u::display_name,
            u::role,
            u::storage_used,
            u::storage_quota,
            u::created_at,
            u::last_login_at,
        ))
        .load::<(
            String,
            String,
            String,
            Option<String>,
            String,
            i64,
            Option<i64>,
            String,
            Option<String>,
        )>(&mut conn)
    {
        Ok(rows) => {
            let user_list: Vec<serde_json::Value> = rows
                .into_iter()
                .map(
                    |(uid, uname, uemail, udisplay, urole, uused, uquota, ucreated, ulast)| {
                        serde_json::json!({
                            "id": uid,
                            "username": uname,
                            "email": uemail,
                            "display_name": udisplay,
                            "role": urole,
                            "storage_used": uused,
                            "storage_quota": uquota.unwrap_or(0),
                            "created_at": ucreated,
                            "last_login_at": ulast,
                        })
                    },
                )
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(user_list)));
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
// GET /admin/activity
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_activity(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let page: u32 = req.query("page").unwrap_or(1);
    let per_page: u32 = req.query("per_page").unwrap_or(50);
    let offset_val = ((page.saturating_sub(1)) * per_page) as i64;
    let limit_val = per_page as i64;

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

    match a::activity_logs
        .order(a::created_at.desc())
        .limit(limit_val)
        .offset(offset_val)
        .select((
            a::id,
            a::user_id,
            a::action,
            a::resource_type,
            a::resource_id,
            a::details,
            a::created_at,
        ))
        .load::<(
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            String,
        )>(&mut conn)
    {
        Ok(rows) => {
            let logs: Vec<serde_json::Value> = rows
                .into_iter()
                .map(
                    |(aid, auser, aaction, arestype, aresid, adetails, acreated)| {
                        let details_val: serde_json::Value = adetails
                            .as_deref()
                            .and_then(|v| serde_json::from_str(v).ok())
                            .unwrap_or(serde_json::Value::Null);
                        serde_json::json!({
                            "id": aid,
                            "user_id": auser,
                            "action": aaction,
                            "resource_type": arestype,
                            "resource_id": aresid,
                            "details": details_val,
                            "created_at": acreated,
                        })
                    },
                )
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(logs)));
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
// PUT /admin/users/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_user(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let user_id_param: String = req.param("id").unwrap_or_default();
    let actor = get_user_id(depot).unwrap_or_default();
    let (audit_ip, audit_ua) = request_context(req);

    #[derive(serde::Deserialize)]
    struct UpdateUserReq {
        role: Option<String>,
        display_name: Option<String>,
    }

    let body: UpdateUserReq = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.role.is_none() && body.display_name.is_none() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("No fields to update".into()).to_response(),
        )));
        return;
    }

    // Restrict role to a known set so arbitrary strings can't be written.
    const VALID_ROLES: &[&str] = &["admin", "editor", "viewer", "disabled"];
    if let Some(role) = body.role.as_deref()
        && !VALID_ROLES.contains(&role)
    {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Invalid role".into()).to_response(),
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

    // Prevent removing the last remaining admin (downgrade or disable), which
    // would otherwise lock everyone out of admin functions.
    if let Some(new_role) = body.role.as_deref()
        && new_role != "admin"
    {
        let is_target_admin = u::users
            .filter(u::id.eq(&user_id_param))
            .filter(u::role.eq("admin"))
            .count()
            .get_result::<i64>(&mut conn)
            .unwrap_or(0)
            > 0;
        if is_target_admin {
            let admin_count = u::users
                .filter(u::role.eq("admin"))
                .count()
                .get_result::<i64>(&mut conn)
                .unwrap_or(0);
            if admin_count <= 1 {
                res.status_code(StatusCode::BAD_REQUEST);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Validation("Cannot remove the last administrator".into())
                        .to_response(),
                )));
                return;
            }
        }
    }

    let now = Utc::now().to_rfc3339();

    let result = if body.role.is_some() && body.display_name.is_some() {
        diesel::update(u::users.filter(u::id.eq(&user_id_param)))
            .set((
                u::role.eq(body.role.as_ref().unwrap()),
                u::display_name.eq(body.display_name.as_ref()),
                u::updated_at.eq(&now),
            ))
            .execute(&mut conn)
    } else if body.role.is_some() {
        diesel::update(u::users.filter(u::id.eq(&user_id_param)))
            .set((
                u::role.eq(body.role.as_ref().unwrap()),
                u::updated_at.eq(&now),
            ))
            .execute(&mut conn)
    } else {
        diesel::update(u::users.filter(u::id.eq(&user_id_param)))
            .set((
                u::display_name.eq(body.display_name.as_ref()),
                u::updated_at.eq(&now),
            ))
            .execute(&mut conn)
    };

    match result {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("User not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            let details = body.role.as_deref().map(|r| format!("role={r}"));
            crate::services::AuditService::log(
                &mut conn,
                &actor,
                "user.update",
                "user",
                &user_id_param,
                crate::services::AuditContext {
                    details: details.as_deref(),
                    ip_address: audit_ip.as_deref(),
                    user_agent: audit_ua.as_deref(),
                },
            );
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "User updated" }),
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
// POST /admin/users/:id/force-logout
// ---------------------------------------------------------------------------

#[handler]
pub async fn force_logout_user(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let user_id_param: String = req.param("id").unwrap_or_default();

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

    match diesel::delete(sess::sessions.filter(sess::user_id.eq(&user_id_param))).execute(&mut conn)
    {
        Ok(count) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Sessions revoked", "count": count }),
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
// PUT /admin/users/:id/disable
// ---------------------------------------------------------------------------

#[handler]
pub async fn disable_user(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let user_id_param: String = req.param("id").unwrap_or_default();
    let actor = get_user_id(depot).unwrap_or_default();
    let (ip, ua) = request_context(req);

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

    match diesel::update(u::users.filter(u::id.eq(&user_id_param)))
        .set((u::role.eq("disabled"), u::updated_at.eq(&now)))
        .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("User not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            // Revoke active sessions so the disabled user cannot keep using a
            // server-side session, and record the action in the audit log.
            let _ = diesel::delete(sess::sessions.filter(sess::user_id.eq(&user_id_param)))
                .execute(&mut conn);
            crate::services::AuditService::log(
                &mut conn,
                &actor,
                "user.disable",
                "user",
                &user_id_param,
                crate::services::AuditContext {
                    details: None,
                    ip_address: ip.as_deref(),
                    user_agent: ua.as_deref(),
                },
            );
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "User disabled" }),
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
// PUT /admin/users/:id/activate — re-enable a disabled user
// ---------------------------------------------------------------------------

#[handler]
pub async fn activate_user(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let user_id_param: String = req.param("id").unwrap_or_default();

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

    match diesel::update(u::users.filter(u::id.eq(&user_id_param)))
        .set((u::role.eq("editor"), u::updated_at.eq(&now)))
        .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("User not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "User activated" }),
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
// GET /admin/users/disabled — list disabled users
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_disabled_users(depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);

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
        .filter(u::role.eq("disabled"))
        .order(u::created_at.desc())
        .select((
            u::id,
            u::username,
            u::email,
            u::display_name,
            u::role,
            u::storage_used,
            u::storage_quota,
            u::created_at,
            u::last_login_at,
        ))
        .load::<(
            String,
            String,
            String,
            Option<String>,
            String,
            i64,
            Option<i64>,
            String,
            Option<String>,
        )>(&mut conn)
    {
        Ok(rows) => {
            let user_list: Vec<serde_json::Value> = rows
                .into_iter()
                .map(
                    |(uid, uname, uemail, udisplay, urole, uused, uquota, ucreated, ulast)| {
                        serde_json::json!({
                            "id": uid,
                            "username": uname,
                            "email": uemail,
                            "display_name": udisplay,
                            "role": urole,
                            "storage_used": uused,
                            "storage_quota": uquota.unwrap_or(0),
                            "created_at": ucreated,
                            "last_login_at": ulast,
                        })
                    },
                )
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(user_list)));
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
// GET /admin/users/stats — count by status
// ---------------------------------------------------------------------------

#[handler]
pub async fn user_status_stats(depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);

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

    let active_count = u::users
        .filter(u::role.ne("disabled").and(u::role.ne("admin")))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);

    let disabled_count = u::users
        .filter(u::role.eq("disabled"))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);

    let admin_count = u::users
        .filter(u::role.eq("admin"))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "active": active_count,
        "disabled": disabled_count,
        "admin": admin_count,
    }))));
}

// ---------------------------------------------------------------------------
// POST /admin/users/import
// ---------------------------------------------------------------------------

#[handler]
pub async fn import_users(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);

    #[derive(serde::Deserialize)]
    struct ImportUserEntry {
        username: String,
        email: String,
        password: String,
        role: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct ImportUsersRequest {
        users: Vec<ImportUserEntry>,
    }

    let body: ImportUsersRequest = match req.parse_json().await {
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

    let now = Utc::now();
    let mut created = 0u32;
    let mut failed = 0u32;
    let mut errors: Vec<serde_json::Value> = Vec::new();
    let default_quota = (state.config.storage.default_quota_mb as i64) * 1_048_576;

    for entry in &body.users {
        // Check uniqueness by username.
        let username_exists: i64 = u::users
            .filter(u::username.eq(&entry.username))
            .count()
            .get_result(&mut conn)
            .unwrap_or(0);

        if username_exists > 0 {
            failed += 1;
            errors.push(serde_json::json!({
                "username": entry.username,
                "error": "Username already exists"
            }));
            continue;
        }

        // Check uniqueness by email.
        let email_exists: i64 = u::users
            .filter(u::email.eq(&entry.email))
            .count()
            .get_result(&mut conn)
            .unwrap_or(0);

        if email_exists > 0 {
            failed += 1;
            errors.push(serde_json::json!({
                "email": entry.email,
                "error": "Email already exists"
            }));
            continue;
        }

        // Hash password with Argon2.
        let salt = SaltString::generate(&mut OsRng);
        let password_hash = match Argon2::default().hash_password(entry.password.as_bytes(), &salt)
        {
            Ok(hash) => hash.to_string(),
            Err(e) => {
                failed += 1;
                errors.push(serde_json::json!({
                    "username": entry.username,
                    "error": format!("Password hashing failed: {e}")
                }));
                continue;
            }
        };

        let role_val = entry.role.as_deref().unwrap_or("editor").to_string();

        let new_user = NewUser {
            id: Uuid::new_v4().to_string(),
            username: entry.username.clone(),
            email: entry.email.clone(),
            password_hash,
            display_name: None,
            avatar_url: None,
            role: role_val,
            storage_used: 0,
            storage_quota: Some(default_quota),
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
            last_login_at: None,
        };

        match diesel::insert_into(u::users)
            .values(&new_user)
            .execute(&mut conn)
        {
            Ok(_) => {
                created += 1;
            }
            Err(e) => {
                failed += 1;
                errors.push(serde_json::json!({
                    "username": entry.username,
                    "error": e.to_string()
                }));
            }
        }
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "created": created,
        "failed": failed,
        "errors": errors,
    }))));
}
