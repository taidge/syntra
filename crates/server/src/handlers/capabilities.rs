use salvo::prelude::*;

use syntra_core::dto::ApiResponse;

// ---------------------------------------------------------------------------
// GET /capabilities  (Return server capabilities)
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_capabilities(depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let capabilities = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "features": {
            "documents": ["writer", "calc", "slides", "whiteboard"],
            "collaboration": true,
            "comments": true,
            "versioning": true,
            "sharing": true,
            "rooms": true,
            "groups": true,
            "tags": true,
            "favorites": true,
            "templates": true,
            "chunked_upload": true,
            "file_locking": true,
            "presence": true,
            "webhooks": true,
            "wopi": true,
            "conversion": true,
            "api_key_authentication": true,
            "2fa": true,
            "invitations": true,
            "watermark": true,
            "ai": state.ai.is_some(),
        },
        "limits": {
            "max_upload_size_mb": state.config.storage.max_upload_size_mb,
        },
        // No third-party language integrations are currently implemented.
        "integrations": [],
    });
    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(capabilities)));
}

// ---------------------------------------------------------------------------
// GET /robots.txt
// ---------------------------------------------------------------------------

#[handler]
pub async fn robots_txt(res: &mut Response) {
    res.status_code(StatusCode::OK);
    res.headers_mut()
        .insert("content-type", "text/plain".parse().unwrap());
    res.render("User-agent: *\nDisallow: /api/\nAllow: /\n");
}
