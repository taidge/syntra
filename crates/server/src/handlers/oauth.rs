use std::sync::Arc;

use salvo::prelude::*;

use crate::state::AppState;
use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;

/// GET /auth/oauth/:provider - Redirect to OAuth provider
#[handler]
pub async fn oauth_redirect(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let provider: String = req.param("provider").unwrap_or_default();

    let (client_id, auth_url) = match provider.as_str() {
        "google" => {
            let id = state.config.oauth.google_client_id.as_deref().unwrap_or("");
            (id, "https://accounts.google.com/o/oauth2/v2/auth")
        }
        "github" => {
            let id = state.config.oauth.github_client_id.as_deref().unwrap_or("");
            (id, "https://github.com/login/oauth/authorize")
        }
        _ => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(format!("Unknown OAuth provider: {provider}")).to_response(),
            )));
            return;
        }
    };

    if client_id.is_empty() {
        res.status_code(StatusCode::NOT_IMPLEMENTED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation(format!("OAuth provider {provider} not configured")).to_response(),
        )));
        return;
    }

    let redirect_uri = format!(
        "{}:{}/api/v1/auth/oauth/{}/callback",
        state.config.server.host, state.config.server.port, provider
    );
    let state_token = uuid::Uuid::new_v4().to_string();

    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&state={}&scope={}",
        auth_url,
        client_id,
        redirect_uri,
        state_token,
        if provider == "google" {
            "openid email profile"
        } else {
            "user:email"
        }
    );

    res.status_code(StatusCode::FOUND);
    res.headers_mut().insert("location", url.parse().unwrap());
}

/// GET /auth/oauth/:provider/callback - Handle OAuth callback
#[handler]
pub async fn oauth_callback(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let _state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let provider: String = req.param("provider").unwrap_or_default();
    let code: Option<String> = req.query("code");

    // The authorization code must never be echoed back to the caller (it is a
    // bearer-equivalent secret). Token exchange / user provisioning is not yet
    // implemented, so respond honestly with 501 rather than reflecting input or
    // pretending success. A complete implementation must also persist and
    // verify the `state` value issued in `oauth_redirect` to prevent CSRF.
    if code.is_none() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Missing authorization code".into()).to_response(),
        )));
        return;
    }

    res.status_code(StatusCode::NOT_IMPLEMENTED);
    res.render(Json(ApiResponse::<()>::err(
        AppError::Validation(format!("OAuth login via {provider} is not yet available"))
            .to_response(),
    )));
}
