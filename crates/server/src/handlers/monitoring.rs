use diesel::prelude::*;
use salvo::prelude::*;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::schema::conversion_jobs::dsl as cj;
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::user_presence::dsl as up;
use contrix_core::schema::users::dsl as u;
use contrix_core::schema::webhook_deliveries::dsl as wd;

use crate::middleware::auth::get_user_role;

// ---------------------------------------------------------------------------
// GET /admin/monitoring  (Get current server metrics)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_monitoring(depot: &mut Depot, res: &mut Response) {
    if get_user_role(depot) != Some("admin".to_string()) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Admin required".into()).to_response(),
        )));
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

    let total_users = u::users.count().get_result::<i64>(&mut conn).unwrap_or(0);

    let total_documents = d::documents
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);

    let total_files = f::files
        .filter(f::is_trashed.eq(false))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);

    let active_users = up::user_presence
        .filter(up::is_active.eq(true))
        .select(up::user_id)
        .distinct()
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);

    let active_documents = up::user_presence
        .filter(up::is_active.eq(true))
        .select(up::document_id)
        .distinct()
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);

    // Try to read memory info from /proc/self/status on Linux
    let rss_bytes: Option<u64> = {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|s| {
                    s.lines()
                        .find(|line| line.starts_with("VmRSS:"))
                        .and_then(|line| {
                            line.split_whitespace()
                                .nth(1)
                                .and_then(|v| v.parse::<u64>().ok())
                                .map(|kb| kb * 1024) // Convert kB to bytes
                        })
                })
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    };

    let memory_info = serde_json::json!({
        "rss_bytes": rss_bytes,
    });

    // Compute uptime from AppState.started_at.
    let uptime_seconds = (chrono::Utc::now() - state.started_at).num_seconds();

    let metrics = serde_json::json!({
        "total_users": total_users,
        "total_documents": total_documents,
        "total_files": total_files,
        "active_users": active_users,
        "active_documents": active_documents,
        "memory": memory_info,
        "uptime_seconds": uptime_seconds,
    });

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(metrics)));
}

// ---------------------------------------------------------------------------
// GET /admin/monitoring/documents  (Get per-document stats)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_document_stats(depot: &mut Depot, res: &mut Response) {
    if get_user_role(depot) != Some("admin".to_string()) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Admin required".into()).to_response(),
        )));
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

    // Get all documents that have at least one active presence record.
    match up::user_presence
        .filter(up::is_active.eq(true))
        .select((up::document_id, up::user_id, up::last_seen_at))
        .load::<(String, String, String)>(&mut conn)
    {
        Ok(rows) => {
            // Group by document_id.
            let mut doc_map: std::collections::HashMap<String, Vec<serde_json::Value>> =
                std::collections::HashMap::new();

            for (doc_id, user_id, last_seen) in rows {
                doc_map.entry(doc_id).or_default().push(serde_json::json!({
                    "user_id": user_id,
                    "last_seen_at": last_seen,
                }));
            }

            let doc_stats: Vec<serde_json::Value> = doc_map
                .into_iter()
                .map(|(doc_id, sessions)| {
                    serde_json::json!({
                        "document_id": doc_id,
                        "active_sessions": sessions.len(),
                        "users": sessions,
                    })
                })
                .collect();

            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(doc_stats)));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}

/// Prometheus text exposition for scraping by standard monitoring stacks.
#[handler]
pub async fn prometheus_metrics(depot: &mut Depot, res: &mut Response) {
    if get_user_role(depot) != Some("admin".to_string()) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Admin required".into()).to_response(),
        )));
        return;
    }
    let state = crate::require_app_state!(depot, res);
    let mut conn = match state.db.get() {
        Ok(conn) => conn,
        Err(error) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(error.to_string()).to_response(),
            )));
            return;
        }
    };
    let users = u::users.count().get_result::<i64>(&mut conn).unwrap_or(0);
    let documents = d::documents
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);
    let active = up::user_presence
        .filter(up::is_active.eq(true))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);
    let pending = cj::conversion_jobs
        .filter(cj::status.eq_any(["pending", "processing"]))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);
    let failed = cj::conversion_jobs
        .filter(cj::status.eq("failed"))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);
    let webhook_failed = wd::webhook_deliveries
        .filter(wd::success.eq(false))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0);
    let uptime = (chrono::Utc::now() - state.started_at).num_seconds().max(0);
    let body = format!(
        "# HELP contrix_uptime_seconds Server uptime in seconds\n# TYPE contrix_uptime_seconds gauge\ncontrix_uptime_seconds {uptime}\n\
# HELP contrix_users_total Registered users\n# TYPE contrix_users_total gauge\ncontrix_users_total {users}\n\
# HELP contrix_documents_total Stored documents\n# TYPE contrix_documents_total gauge\ncontrix_documents_total {documents}\n\
# HELP contrix_active_presence Active collaborative presence records\n# TYPE contrix_active_presence gauge\ncontrix_active_presence {active}\n\
# HELP contrix_conversion_jobs Durable conversion jobs by status\n# TYPE contrix_conversion_jobs gauge\ncontrix_conversion_jobs{{status=\"pending\"}} {pending}\ncontrix_conversion_jobs{{status=\"failed\"}} {failed}\n\
# HELP contrix_webhook_deliveries_failed Failed webhook deliveries available for retry\n# TYPE contrix_webhook_deliveries_failed gauge\ncontrix_webhook_deliveries_failed {webhook_failed}\n"
    );
    res.status_code(StatusCode::OK);
    res.headers_mut().insert(
        "content-type",
        "text/plain; version=0.0.4; charset=utf-8"
            .parse()
            .expect("static content type"),
    );
    res.body(body);
}
