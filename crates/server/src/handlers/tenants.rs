use chrono::Utc;
use diesel::prelude::*;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use syntra_core::dto::ApiResponse;
use syntra_core::error::AppError;
use syntra_core::models::{NewTenant, Tenant};
use syntra_core::schema::tenants::dsl as t;

use crate::middleware::auth::get_user_role;

/// Verify the caller has admin (super-admin) role; returns `false` and writes
/// a 403 if not.
fn require_super_admin(depot: &Depot, res: &mut Response) -> bool {
    match get_user_role(depot) {
        Some(role) if role == "admin" => true,
        _ => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Forbidden("Super-admin access required".into()).to_response(),
            )));
            false
        }
    }
}

// ---------------------------------------------------------------------------
// POST /admin/tenants — create a tenant
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_tenant(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_super_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);

    #[derive(serde::Deserialize)]
    struct CreateTenantReq {
        name: String,
        domain: Option<String>,
        owner_id: Option<String>,
        plan: Option<String>,
        max_users: Option<i32>,
        max_storage_bytes: Option<i64>,
        settings: Option<serde_json::Value>,
    }

    let body: CreateTenantReq = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.name.trim().is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Tenant name is required".into()).to_response(),
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

    // Check domain uniqueness if provided.
    if let Some(ref domain) = body.domain {
        let existing: i64 = t::tenants
            .filter(t::domain.eq(domain))
            .count()
            .get_result(&mut conn)
            .await
            .unwrap_or(0);
        if existing > 0 {
            res.status_code(StatusCode::CONFLICT);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Conflict("Domain already in use".into()).to_response(),
            )));
            return;
        }
    }

    let now = Utc::now().to_rfc3339();
    let tenant_id = Uuid::new_v4().to_string();

    let new_tenant = NewTenant {
        id: tenant_id.clone(),
        name: body.name.clone(),
        domain: body.domain.clone(),
        owner_id: body.owner_id.clone(),
        plan: body.plan.unwrap_or_else(|| "free".to_string()),
        max_users: body.max_users.unwrap_or(10),
        max_storage_bytes: body.max_storage_bytes.unwrap_or(5_368_709_120),
        is_active: 1,
        settings: body.settings.map(|v| v.to_string()),
        created_at: now.clone(),
        updated_at: now,
    };

    match diesel::insert_into(t::tenants)
        .values(&new_tenant)
        .execute(&mut conn)
        .await
    {
        Ok(_) => {
            let tenant: Tenant = t::tenants
                .filter(t::id.eq(&tenant_id))
                .first(&mut conn)
                .await
                .unwrap();
            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(tenant_to_json(&tenant))));
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
// GET /admin/tenants — list all tenants
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_tenants(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_super_admin(depot, res) {
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

    match t::tenants
        .order(t::created_at.desc())
        .limit(limit_val)
        .offset(offset_val)
        .load::<Tenant>(&mut conn)
        .await
    {
        Ok(tenants) => {
            let list: Vec<serde_json::Value> = tenants.iter().map(tenant_to_json).collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(list)));
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
// GET /admin/tenants/:id — get a single tenant
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_tenant(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_super_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let tenant_id: String = req.param("id").unwrap_or_default();

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

    match t::tenants
        .filter(t::id.eq(&tenant_id))
        .first::<Tenant>(&mut conn)
        .await
    {
        Ok(tenant) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(tenant_to_json(&tenant))));
        }
        Err(horme::Error::Query(diesel::result::Error::NotFound)) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Tenant not found".into()).to_response(),
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
// PUT /admin/tenants/:id — update a tenant
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_tenant(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_super_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let tenant_id: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct UpdateTenantReq {
        name: Option<String>,
        domain: Option<String>,
        owner_id: Option<String>,
        plan: Option<String>,
        max_users: Option<i32>,
        max_storage_bytes: Option<i64>,
        is_active: Option<bool>,
        settings: Option<serde_json::Value>,
    }

    let body: UpdateTenantReq = match req.parse_json().await {
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

    // Apply each optional field.
    let target = t::tenants.filter(t::id.eq(&tenant_id));

    // Always update `updated_at`.
    let base_result = diesel::update(target)
        .set(t::updated_at.eq(&now))
        .execute(&mut conn)
        .await;
    if let Err(e) = base_result {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    // We need to re-create the target for each additional set call due to
    // Diesel's ownership semantics.
    if let Some(ref name) = body.name {
        let _ = diesel::update(t::tenants.filter(t::id.eq(&tenant_id)))
            .set(t::name.eq(name))
            .execute(&mut conn)
            .await;
    }
    if let Some(ref domain) = body.domain {
        let _ = diesel::update(t::tenants.filter(t::id.eq(&tenant_id)))
            .set(t::domain.eq(domain))
            .execute(&mut conn)
            .await;
    }
    if let Some(ref owner_id) = body.owner_id {
        let _ = diesel::update(t::tenants.filter(t::id.eq(&tenant_id)))
            .set(t::owner_id.eq(owner_id))
            .execute(&mut conn)
            .await;
    }
    if let Some(ref plan) = body.plan {
        let _ = diesel::update(t::tenants.filter(t::id.eq(&tenant_id)))
            .set(t::plan.eq(plan))
            .execute(&mut conn)
            .await;
    }
    if let Some(max_users) = body.max_users {
        let _ = diesel::update(t::tenants.filter(t::id.eq(&tenant_id)))
            .set(t::max_users.eq(max_users))
            .execute(&mut conn)
            .await;
    }
    if let Some(max_storage) = body.max_storage_bytes {
        let _ = diesel::update(t::tenants.filter(t::id.eq(&tenant_id)))
            .set(t::max_storage_bytes.eq(max_storage))
            .execute(&mut conn)
            .await;
    }
    if let Some(active) = body.is_active {
        let _ = diesel::update(t::tenants.filter(t::id.eq(&tenant_id)))
            .set(t::is_active.eq(if active { 1 } else { 0 }))
            .execute(&mut conn)
            .await;
    }
    if let Some(ref settings) = body.settings {
        let _ = diesel::update(t::tenants.filter(t::id.eq(&tenant_id)))
            .set(t::settings.eq(settings.to_string()))
            .execute(&mut conn)
            .await;
    }

    // Return the updated tenant.
    match t::tenants
        .filter(t::id.eq(&tenant_id))
        .first::<Tenant>(&mut conn)
        .await
    {
        Ok(tenant) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(tenant_to_json(&tenant))));
        }
        Err(horme::Error::Query(diesel::result::Error::NotFound)) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Tenant not found".into()).to_response(),
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
// DELETE /admin/tenants/:id — deactivate a tenant
// ---------------------------------------------------------------------------

#[handler]
pub async fn deactivate_tenant(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_super_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let tenant_id: String = req.param("id").unwrap_or_default();

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

    match diesel::update(t::tenants.filter(t::id.eq(&tenant_id)))
        .set((t::is_active.eq(0), t::updated_at.eq(&now)))
        .execute(&mut conn)
        .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Tenant not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Tenant deactivated" }),
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
// GET /admin/tenants/:id/users — list users belonging to a tenant
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_tenant_users(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_super_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let tenant_id: String = req.param("id").unwrap_or_default();

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

    // Verify tenant exists.
    let tenant_exists: i64 = t::tenants
        .filter(t::id.eq(&tenant_id))
        .count()
        .get_result(&mut conn)
        .await
        .unwrap_or(0);
    if tenant_exists == 0 {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Tenant not found".into()).to_response(),
        )));
        return;
    }

    // Use raw SQL because the `tenant_id` column was added via ALTER TABLE
    // and is not part of the original Diesel schema for `users`.
    #[derive(diesel::QueryableByName)]
    struct TenantUser {
        #[diesel(sql_type = diesel::sql_types::Text)]
        id: String,
        #[diesel(sql_type = diesel::sql_types::Text)]
        username: String,
        #[diesel(sql_type = diesel::sql_types::Text)]
        email: String,
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        display_name: Option<String>,
        #[diesel(sql_type = diesel::sql_types::Text)]
        role: String,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        storage_used: i64,
        #[diesel(sql_type = diesel::sql_types::Text)]
        created_at: String,
    }

    match diesel::sql_query(
        "SELECT id, username, email, display_name, role, storage_used, created_at \
         FROM users WHERE tenant_id = ?",
    )
    .bind::<diesel::sql_types::Text, _>(&tenant_id)
    .load::<TenantUser>(&mut conn)
    .await
    {
        Ok(users) => {
            let user_list: Vec<serde_json::Value> = users
                .into_iter()
                .map(|tu| {
                    serde_json::json!({
                        "id": tu.id,
                        "username": tu.username,
                        "email": tu.email,
                        "display_name": tu.display_name,
                        "role": tu.role,
                        "storage_used": tu.storage_used,
                        "created_at": tu.created_at,
                    })
                })
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
// GET /admin/tenants/:id/stats — tenant storage / user stats
// ---------------------------------------------------------------------------

#[handler]
pub async fn tenant_stats(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_super_admin(depot, res) {
        return;
    }

    let state = crate::require_app_state!(depot, res);
    let tenant_id: String = req.param("id").unwrap_or_default();

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

    // Verify tenant exists and fetch it.
    let tenant: Tenant = match t::tenants
        .filter(t::id.eq(&tenant_id))
        .first::<Tenant>(&mut conn)
        .await
    {
        Ok(ten) => ten,
        Err(horme::Error::Query(diesel::result::Error::NotFound)) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Tenant not found".into()).to_response(),
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

    // Count users and total storage via raw SQL (tenant_id not in Diesel schema).
    #[derive(diesel::QueryableByName)]
    struct TenantStatRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        user_count: i64,
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        total_storage: i64,
    }

    let stats = diesel::sql_query(
        "SELECT COUNT(*) AS user_count, COALESCE(SUM(storage_used), 0) AS total_storage \
         FROM users WHERE tenant_id = ?",
    )
    .bind::<diesel::sql_types::Text, _>(&tenant_id)
    .get_result::<TenantStatRow>(&mut conn)
    .await
    .ok();

    let (user_count, total_storage) = match stats {
        Some(row) => (row.user_count, row.total_storage),
        None => (0, 0),
    };

    let storage_percent = if tenant.max_storage_bytes > 0 {
        (total_storage as f64 / tenant.max_storage_bytes as f64) * 100.0
    } else {
        0.0
    };

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "tenant_id": tenant.id,
        "tenant_name": tenant.name,
        "plan": tenant.plan,
        "is_active": tenant.is_active != 0,
        "user_count": user_count,
        "max_users": tenant.max_users,
        "total_storage_used": total_storage,
        "max_storage_bytes": tenant.max_storage_bytes,
        "storage_usage_percent": (storage_percent * 100.0).round() / 100.0,
    }))));
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn tenant_to_json(tenant: &Tenant) -> serde_json::Value {
    let settings_val: serde_json::Value = tenant
        .settings
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "id": tenant.id,
        "name": tenant.name,
        "domain": tenant.domain,
        "owner_id": tenant.owner_id,
        "plan": tenant.plan,
        "max_users": tenant.max_users,
        "max_storage_bytes": tenant.max_storage_bytes,
        "is_active": tenant.is_active != 0,
        "settings": settings_val,
        "created_at": tenant.created_at,
        "updated_at": tenant.updated_at,
    })
}
