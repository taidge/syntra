/// Obtain the shared application state without allowing a missing router
/// injection to panic the process. Handlers use this macro because Salvo
/// handlers return `()` and therefore cannot use `?` with a typed extractor.
#[macro_export]
macro_rules! require_app_state {
    ($depot:expr, $res:expr) => {{
        match $depot.obtain::<std::sync::Arc<$crate::state::AppState>>() {
            Ok(state) => state.clone(),
            Err(_) => {
                $res.status_code(salvo::http::StatusCode::INTERNAL_SERVER_ERROR);
                $res.render(salvo::prelude::Json(serde_json::json!({
                    "success": false,
                    "error": { "code": 500, "message": "Application state unavailable" }
                })));
                return;
            }
        }
    }};
}

pub mod access;
pub mod admin;
pub mod ai;
pub mod api_keys;
pub mod auth;
pub mod backup;
pub mod batch;
pub mod branding;
pub mod capabilities;
pub mod client_log;
pub mod comments;
pub mod convert;
pub mod documents;
pub mod favorites;
pub mod file_locks;
pub mod file_properties;
pub mod file_versions;
pub mod files;
pub mod folders;
pub mod groups;
pub mod health;
pub mod invitations;
pub mod ip_restrictions;
pub mod monitoring;
pub mod notification_prefs;
pub mod notifications;
pub mod operability;
pub mod password_policy;
pub mod pdf;
pub mod presence;
pub mod quota;
pub mod recent;
mod request_body;
pub mod rooms;
pub mod search;
pub mod sessions;
pub mod settings;
pub mod shares;
pub mod signatures;
pub mod smtp;
pub mod spa;
pub mod tags;
pub mod templates;
pub mod tenants;
pub mod totp;
pub mod trash;
pub mod upload;
pub mod watermark;
pub mod webhooks;
pub mod wopi;
pub mod ws;
pub mod ws_notifications;
