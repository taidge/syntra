use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{ClientLog, NewClientLog};
use contrix_core::schema::client_logs::dsl as cl;

use crate::middleware::auth::{get_user_id, get_user_role};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// POST /client-log  (Submit client log entries)
// ---------------------------------------------------------------------------

#[handler]
pub async fn submit_client_logs(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    // User may or may not be authenticated for client logs.
    let uid = get_user_id(depot);

    #[derive(serde::Deserialize)]
    struct ClientLogEntry {
        level: String,
        message: String,
        stack_trace: Option<String>,
        url: Option<String>,
        session_id: Option<String>,
    }

    let entries: Vec<ClientLogEntry> = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if entries.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("No log entries provided".into()).to_response(),
        )));
        return;
    }

    let now = Utc::now();
    let user_agent = req
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

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

    let mut inserted_count = 0u64;

    for entry in &entries {
        let new_log = NewClientLog {
            id: Uuid::new_v4().to_string(),
            user_id: uid.clone(),
            session_id: entry.session_id.clone(),
            level: entry.level.clone(),
            message: entry.message.clone(),
            stack_trace: entry.stack_trace.clone(),
            user_agent: user_agent.clone(),
            url: entry.url.clone(),
            created_at: now.to_rfc3339(),
        };

        if diesel::insert_into(cl::client_logs)
            .values(&new_log)
            .execute(&mut conn)
            .is_ok()
        {
            inserted_count += 1;
        }
    }

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Client logs recorded",
        "count": inserted_count,
    }))));
}

// ---------------------------------------------------------------------------
// GET /admin/client-logs  (List client logs — admin only, paginated)
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_client_logs(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if get_user_role(depot) != Some("admin".to_string()) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Admin required".into()).to_response(),
        )));
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let page: u32 = req.query("page").unwrap_or(1);
    let per_page: u32 = req.query("per_page").unwrap_or(50);
    let offset_val = ((page.saturating_sub(1)) * per_page) as i64;
    let limit_val = per_page as i64;
    let level_filter: Option<String> = req.query("level");

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

    let result = if let Some(level) = level_filter {
        cl::client_logs
            .filter(cl::level.eq(&level))
            .order(cl::created_at.desc())
            .limit(limit_val)
            .offset(offset_val)
            .load::<ClientLog>(&mut conn)
    } else {
        cl::client_logs
            .order(cl::created_at.desc())
            .limit(limit_val)
            .offset(offset_val)
            .load::<ClientLog>(&mut conn)
    };

    match result {
        Ok(rows) => {
            let logs: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|log| {
                    serde_json::json!({
                        "id": log.id,
                        "user_id": log.user_id,
                        "session_id": log.session_id,
                        "level": log.level,
                        "message": log.message,
                        "stack_trace": log.stack_trace,
                        "user_agent": log.user_agent,
                        "url": log.url,
                        "created_at": log.created_at,
                    })
                })
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
// DELETE /admin/client-logs  (Clear old client logs — admin only)
// ---------------------------------------------------------------------------

#[handler]
pub async fn clear_client_logs(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if get_user_role(depot) != Some("admin".to_string()) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Admin required".into()).to_response(),
        )));
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();

    // Optional: delete logs older than N days (default: delete all).
    let older_than_days: Option<u32> = req.query("older_than_days");

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

    let result = if let Some(days) = older_than_days {
        let cutoff = (Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
        diesel::delete(cl::client_logs.filter(cl::created_at.lt(&cutoff))).execute(&mut conn)
    } else {
        diesel::delete(cl::client_logs).execute(&mut conn)
    };

    match result {
        Ok(count) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "message": "Client logs cleared",
                "deleted_count": count,
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
