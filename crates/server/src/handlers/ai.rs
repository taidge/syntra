use std::sync::Arc;

use salvo::prelude::*;
use serde::{Deserialize, Serialize};

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

#[derive(Deserialize)]
struct AssistRequest {
    pub document_id: String,
    pub prompt: String,
    pub mode: String,
}

#[derive(Serialize)]
struct AssistResponse {
    pub answer: String,
}

#[handler]
pub async fn assist(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let _state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let uid = match get_user_id(depot) {
        Some(v) => v,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

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

    let answer = match body.mode.as_str() {
        "summarize" => "Stub: AI integration not configured. Replace this stub with a real LLM call (Anthropic / OpenAI / local) by reading the document content from state.db and posting to your LLM endpoint.".to_string(),
        "translate" => "Stub: translation not configured.".to_string(),
        "explain" => format!(
            "Stub: AI explanation for prompt '{}'.",
            body.prompt.chars().take(80).collect::<String>()
        ),
        "draft" => format!(
            "Stub: AI draft for user {uid}. Prompt was: {}.",
            body.prompt.chars().take(120).collect::<String>()
        ),
        _ => "Stub: unknown mode.".to_string(),
    };

    res.render(Json(ApiResponse::ok(AssistResponse { answer })));
}
