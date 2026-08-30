use diesel::prelude::*;
use salvo::prelude::*;
use serde::{Deserialize, Serialize};

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::schema::documents::dsl as d;

use crate::handlers::access::load_accessible_document;
use crate::middleware::auth::get_user_id;
use crate::services::{AuditContext, AuditService};

#[derive(Deserialize)]
struct AssistRequest {
    document_id: String,
    prompt: String,
    mode: String,
}

#[derive(Serialize)]
struct AssistResponse {
    answer: String,
}

const MODES: &[&str] = &["summarize", "translate", "explain", "draft"];

#[handler]
pub async fn assist(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let user_id = match get_user_id(depot) {
        Some(user_id) => user_id,
        None => {
            render_error(
                res,
                StatusCode::UNAUTHORIZED,
                AppError::Auth("Not authenticated".into()),
            );
            return;
        }
    };
    let body: AssistRequest = match req.parse_json().await {
        Ok(body) => body,
        Err(error) => {
            render_error(
                res,
                StatusCode::BAD_REQUEST,
                AppError::Validation(error.to_string()),
            );
            return;
        }
    };
    let document_id = body.document_id.trim();
    let prompt = body.prompt.trim();
    if document_id.is_empty() || prompt.is_empty() {
        render_error(
            res,
            StatusCode::BAD_REQUEST,
            AppError::Validation("document_id and prompt are required".into()),
        );
        return;
    }
    if !MODES.contains(&body.mode.as_str()) {
        render_error(
            res,
            StatusCode::BAD_REQUEST,
            AppError::Validation("mode must be summarize, translate, explain, or draft".into()),
        );
        return;
    }

    // Authorize before revealing whether AI is configured. This prevents the
    // assistant endpoint from becoming a document-existence oracle.
    let document_content = {
        let mut conn = match state.db.get() {
            Ok(conn) => conn,
            Err(error) => {
                render_error(
                    res,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    AppError::Database(error.to_string()),
                );
                return;
            }
        };
        match load_accessible_document(&mut conn, document_id, &user_id, "viewer") {
            Ok(Some(_)) => {}
            Ok(None) | Err(AppError::NotFound(_)) | Err(AppError::Forbidden(_)) => {
                render_error(
                    res,
                    StatusCode::NOT_FOUND,
                    AppError::NotFound("Document not found".into()),
                );
                return;
            }
            Err(error) => {
                render_error(res, StatusCode::INTERNAL_SERVER_ERROR, error);
                return;
            }
        }
        match d::documents
            .filter(d::id.eq(document_id))
            .select(d::content)
            .first::<Option<String>>(&mut conn)
        {
            Ok(content) => content.unwrap_or_else(|| "{}".into()),
            Err(error) => {
                render_error(
                    res,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    AppError::Database(error.to_string()),
                );
                return;
            }
        }
    };

    let Some(ai) = state.ai.as_ref() else {
        render_error(
            res,
            StatusCode::SERVICE_UNAVAILABLE,
            AppError::Internal("AI assistant is disabled".into()),
        );
        return;
    };
    if prompt.chars().count() > ai.max_prompt_chars() {
        render_error(
            res,
            StatusCode::BAD_REQUEST,
            AppError::Validation(format!(
                "prompt must not exceed {} characters",
                ai.max_prompt_chars()
            )),
        );
        return;
    }

    let ip_address = req.remote_addr().to_string();
    let user_agent = req
        .headers()
        .get("user-agent")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let answer = match ai
        .complete(&user_id, &document_content, prompt, &body.mode)
        .await
    {
        Ok(answer) => answer,
        Err(error) if error.to_string().contains("quota exceeded") => {
            render_error(
                res,
                StatusCode::TOO_MANY_REQUESTS,
                AppError::RateLimit("AI request quota exceeded; try again later".into()),
            );
            return;
        }
        Err(error) => {
            tracing::error!(user_id, document_id, error = %error, "AI completion failed");
            render_error(
                res,
                StatusCode::BAD_GATEWAY,
                AppError::Internal("AI provider request failed".into()),
            );
            return;
        }
    };

    if let Ok(mut conn) = state.db.get() {
        let details = serde_json::json!({
            "mode": body.mode,
            "model": ai.model(),
            "prompt_chars": prompt.chars().count(),
            "answer_chars": answer.chars().count(),
        })
        .to_string();
        AuditService::log(
            &mut conn,
            &user_id,
            "ai.assist",
            "document",
            document_id,
            AuditContext {
                details: Some(&details),
                ip_address: Some(&ip_address),
                user_agent: user_agent.as_deref(),
            },
        );
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(AssistResponse { answer })));
}

fn render_error(res: &mut Response, status: StatusCode, error: AppError) {
    res.status_code(status);
    res.render(Json(ApiResponse::<()>::err(error.to_response())));
}
