use std::sync::Arc;

use chrono::Utc;
use contrix_core::dto::{ApiResponse, SessionResponse};
use contrix_core::error::AppError;
use contrix_core::schema::sessions::dsl as s;
use diesel::prelude::*;
use salvo::prelude::*;

use crate::middleware::auth::{get_user_id, hash_token};
use crate::state::AppState;

/// Try to resolve the current session id from the Authorization header.
/// Falls back to the most-recently-active session for the user.
fn resolve_current_session_id(
    req: &Request,
    uid: &str,
    conn: &mut diesel::SqliteConnection,
) -> Option<String> {
    let token = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or_default();

    let token_hash_val = hash_token(token);

    // First try an exact match on the hashed token.
    if let Ok(Some(id)) = s::sessions
        .filter(s::user_id.eq(uid))
        .filter(s::is_active.eq(true))
        .filter(s::token_hash.eq(&token_hash_val))
        .select(s::id)
        .first::<String>(conn)
        .optional()
    {
        return Some(id);
    }

    // Fallback: most recent active session.
    s::sessions
        .filter(s::user_id.eq(uid))
        .filter(s::is_active.eq(true))
        .order(s::last_activity_at.desc())
        .select(s::id)
        .first::<String>(conn)
        .optional()
        .unwrap_or(None)
}

// ---------------------------------------------------------------------------
// GET /sessions
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_sessions(depot: &mut Depot, res: &mut Response) {
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

    match s::sessions
        .filter(s::user_id.eq(&uid))
        .filter(s::is_active.eq(true))
        .order(s::last_activity_at.desc())
        .select((
            s::id,
            s::ip_address,
            s::user_agent,
            s::is_active,
            s::created_at,
            s::last_activity_at,
        ))
        .load::<(String, Option<String>, Option<String>, bool, String, String)>(&mut conn)
    {
        Ok(rows) => {
            let sessions: Vec<SessionResponse> = rows
                .into_iter()
                .map(
                    |(sid, sip, sua, sactive, screated, slast)| SessionResponse {
                        id: sid.parse().unwrap_or_default(),
                        ip_address: sip,
                        user_agent: sua,
                        is_active: sactive,
                        created_at: screated.parse().unwrap_or_else(|_| Utc::now()),
                        last_activity_at: slast.parse().unwrap_or_else(|_| Utc::now()),
                    },
                )
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(sessions)));
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
// GET /sessions/current
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_current_session(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let current_id = match resolve_current_session_id(req, &uid, &mut conn) {
        Some(id) => id,
        None => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("No active session found".into()).to_response(),
            )));
            return;
        }
    };

    match s::sessions
        .filter(s::id.eq(&current_id))
        .select((
            s::id,
            s::ip_address,
            s::user_agent,
            s::is_active,
            s::created_at,
            s::last_activity_at,
        ))
        .first::<(String, Option<String>, Option<String>, bool, String, String)>(&mut conn)
        .optional()
    {
        Ok(Some((sid, sip, sua, sactive, screated, slast))) => {
            let resp = SessionResponse {
                id: sid.parse().unwrap_or_default(),
                ip_address: sip,
                user_agent: sua,
                is_active: sactive,
                created_at: screated.parse().unwrap_or_else(|_| Utc::now()),
                last_activity_at: slast.parse().unwrap_or_else(|_| Utc::now()),
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(resp)));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Session not found".into()).to_response(),
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
// DELETE /sessions/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn revoke_session(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let session_id_param: String = req.param("id").unwrap_or_default();

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

    match diesel::update(
        s::sessions
            .filter(s::id.eq(&session_id_param))
            .filter(s::user_id.eq(&uid)),
    )
    .set(s::is_active.eq(false))
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Session not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Session revoked" }),
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
// DELETE /sessions
// ---------------------------------------------------------------------------

#[handler]
pub async fn revoke_all_sessions(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let current_session_id = resolve_current_session_id(req, &uid, &mut conn);

    let result = match &current_session_id {
        Some(current_id) => diesel::update(
            s::sessions
                .filter(s::user_id.eq(&uid))
                .filter(s::is_active.eq(true))
                .filter(s::id.ne(current_id)),
        )
        .set(s::is_active.eq(false))
        .execute(&mut conn),
        None => {
            // Cannot identify current session — revoke all active sessions.
            diesel::update(
                s::sessions
                    .filter(s::user_id.eq(&uid))
                    .filter(s::is_active.eq(true)),
            )
            .set(s::is_active.eq(false))
            .execute(&mut conn)
        }
    };

    match result {
        Ok(count) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "message": "All other sessions revoked",
                "count": count
            }))));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
