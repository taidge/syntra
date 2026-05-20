use salvo::prelude::*;

use contrix_core::dto::{ApiResponse, HealthResponse};

#[handler]
pub async fn health_check(res: &mut Response) {
    let resp = HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: 0,
    };
    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(resp)));
}
