use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, CreateIpRestrictionRequest, IpRestrictionResponse};
use contrix_core::error::AppError;
use contrix_core::models::NewIpRestriction;
use contrix_core::schema::ip_restrictions::dsl as ip;

use crate::middleware::auth::{get_user_id, get_user_role};
use crate::state::AppState;

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
// POST /admin/ip-restrictions
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_ip_restriction(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

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

    let body: CreateIpRestrictionRequest = match req.parse_json().await {
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

    // Validate restriction_type.
    if body.restriction_type != "allow" && body.restriction_type != "deny" {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("restriction_type must be 'allow' or 'deny'".into()).to_response(),
        )));
        return;
    }

    let restriction_id = Uuid::new_v4();
    let now = Utc::now();

    let new_restriction = NewIpRestriction {
        id: restriction_id.to_string(),
        ip_range: body.ip_range.clone(),
        restriction_type: body.restriction_type.clone(),
        description: body.description.clone(),
        created_by: uid.clone(),
        created_at: now.to_rfc3339(),
    };

    match diesel::insert_into(ip::ip_restrictions)
        .values(&new_restriction)
        .execute(&mut conn)
    {
        Ok(_) => {
            let resp = IpRestrictionResponse {
                id: restriction_id,
                ip_range: body.ip_range,
                restriction_type: body.restriction_type,
                description: body.description,
                created_at: now,
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
// GET /admin/ip-restrictions
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_ip_restrictions(depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();

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

    match ip::ip_restrictions
        .order(ip::created_at.desc())
        .select((
            ip::id,
            ip::ip_range,
            ip::restriction_type,
            ip::description,
            ip::created_at,
        ))
        .load::<(String, String, String, Option<String>, String)>(&mut conn)
    {
        Ok(rows) => {
            let restrictions: Vec<IpRestrictionResponse> = rows
                .into_iter()
                .map(|(rid, rip, rtype, rdesc, rcreated)| IpRestrictionResponse {
                    id: rid.parse().unwrap_or_default(),
                    ip_range: rip,
                    restriction_type: rtype,
                    description: rdesc,
                    created_at: rcreated.parse().unwrap_or_else(|_| Utc::now()),
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(restrictions)));
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
// GET /admin/ip-restrictions/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_ip_restriction(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let restriction_id_param: String = req.param("id").unwrap_or_default();

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

    match ip::ip_restrictions
        .filter(ip::id.eq(&restriction_id_param))
        .select((
            ip::id,
            ip::ip_range,
            ip::restriction_type,
            ip::description,
            ip::created_at,
        ))
        .first::<(String, String, String, Option<String>, String)>(&mut conn)
        .optional()
    {
        Ok(Some((rid, rip, rtype, rdesc, rcreated))) => {
            let resp = IpRestrictionResponse {
                id: rid.parse().unwrap_or_default(),
                ip_range: rip,
                restriction_type: rtype,
                description: rdesc,
                created_at: rcreated.parse().unwrap_or_else(|_| Utc::now()),
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(resp)));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("IP restriction not found".into()).to_response(),
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
// DELETE /admin/ip-restrictions/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_ip_restriction(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let restriction_id_param: String = req.param("id").unwrap_or_default();

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

    match diesel::delete(ip::ip_restrictions.filter(ip::id.eq(&restriction_id_param)))
        .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("IP restriction not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "IP restriction deleted" }),
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
