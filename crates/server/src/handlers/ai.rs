use std::sync::Arc;

use salvo::prelude::*;
use serde::Deserialize;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

#[derive(Deserialize)]
struct AssistRequest {
    pub document_id: String,
    #[allow(dead_code)]
    pub prompt: String,
    #[allow(dead_code)]
    pub mode: String,
}

#[handler]
pub async fn assist(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let _state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    if get_user_id(depot).is_none() {
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Auth("Not authenticated".into()).to_response(),
        )));
        return;
    }

    let body: AssistRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if body.document_id.trim().is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("document_id is required".into()).to_response(),
        )));
        return;
    }

    // No LLM backend is configured yet; report that honestly instead of
    // returning canned placeholder answers.
    res.status_code(StatusCode::NOT_IMPLEMENTED);
    res.render(Json(ApiResponse::<()>::err(
        AppError::Internal("AI assistant is not configured on this server".into()).to_response(),
    )));
}
