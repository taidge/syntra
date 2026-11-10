use contrix_core::dto::{ApiResponse, HealthResponse};
use contrix_core::error::AppError;
use salvo::prelude::*;

use crate::middleware::request_observability::{database_cause, record_failure};

#[handler]
pub async fn health_check(depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let response = HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        uptime_seconds: (chrono::Utc::now() - state.started_at).num_seconds().max(0) as u64,
    };
    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(response)));
}

#[handler]
pub async fn ready_check(depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let probe = async {
        use diesel::prelude::*;
        use horme::RunQueryDsl;
        let mut db = state.db.clone();
        let _: i32 = diesel::select(1.into_sql::<diesel::sql_types::Integer>())
            .get_result(&mut db)
            .await?;
        state.storage.check_ready().await?;
        Ok::<_, contrix_core::error::AppError>(())
    };
    let failure = match tokio::time::timeout(std::time::Duration::from_secs(2), probe).await {
        Ok(Ok(())) => None,
        Err(_) => Some("readiness_deadline"),
        Ok(Err(error @ AppError::Database(_))) => Some(database_cause(&error)),
        Ok(Err(_)) => Some("storage_probe_failed"),
    };
    if let Some(cause) = failure {
        record_failure("readiness", cause);
    }
    let ready = failure.is_none();
    res.status_code(if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    });
    res.render(Json(
        serde_json::json!({"success":ready,"data":{"status":if ready{"ready"}else{"unavailable"}}}),
    ));
}
