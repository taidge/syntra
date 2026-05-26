use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{File, FormData, Headers, Request, RequestInit, RequestMode, Response};

// ---------------------------------------------------------------------------
// Client-side auth state
// ---------------------------------------------------------------------------

/// Authentication state stored in a global signal.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AuthState {
    pub token: Option<String>,
    pub user: Option<UserInfo>,
}

impl AuthState {
    /// Returns `true` when the user is authenticated.
    pub fn is_authenticated(&self) -> bool {
        self.token.is_some() && self.user.is_some()
    }
}

/// Lightweight user info kept in client-side state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserInfo {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub display_name: Option<String>,
    pub role: String,
}

// ---------------------------------------------------------------------------
// DTO types (mirroring server-side contrix_core::dto for JSON de/serialization)
// ---------------------------------------------------------------------------

/// Envelope used by the backend for all JSON responses.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<ErrorBody>,
}

/// Error details returned inside an `ApiResponse`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ErrorBody {
    pub code: u16,
    pub message: String,
    pub details: Option<String>,
}

/// Request body for `POST /auth/login`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Request body for `POST /auth/register`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

/// Successful auth response (login or register).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AuthResponseData {
    pub token: String,
    pub user: UserProfile,
}

/// Public-facing user profile returned by the backend.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct UserProfile {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: String,
    pub storage_used: i64,
    pub storage_quota: i64,
}

/// Request body for `POST /folders`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CreateFolderRequest {
    pub name: String,
    pub parent_id: Option<Uuid>,
}

/// File/folder metadata returned by the backend.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct FileResponse {
    pub id: Uuid,
    pub name: String,
    pub path: String,
    pub mime_type: Option<String>,
    pub size: i64,
    pub owner_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub is_folder: bool,
    pub is_trashed: bool,
    pub trashed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub shared_with_count: u32,
    pub document_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// API client helpers
// ---------------------------------------------------------------------------

/// Base path for all backend API calls (proxied in dev via Dioxus config).
pub const API_BASE: &str = "/api/v1";
const AUTH_STORAGE_KEY: &str = "contrix.auth";

// ---------------------------------------------------------------------------
// Client-side auth persistence
// ---------------------------------------------------------------------------

/// Load the last known auth state from browser localStorage.
pub fn load_auth_state() -> AuthState {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(AUTH_STORAGE_KEY).ok().flatten())
        .and_then(|raw| serde_json::from_str::<AuthState>(&raw).ok())
        .unwrap_or_default()
}

/// Persist the auth state into browser localStorage.
pub fn persist_auth_state(state: &AuthState) {
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        if let Ok(serialized) = serde_json::to_string(state) {
            let _ = storage.set_item(AUTH_STORAGE_KEY, &serialized);
        }
    }
}

/// Remove any persisted auth state from browser localStorage.
pub fn clear_auth_state() {
    if let Some(storage) =
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    {
        let _ = storage.remove_item(AUTH_STORAGE_KEY);
    }
}

/// Low-level fetch wrapper using `web_sys::Request` + `window.fetch()`.
async fn fetch_with_opts(
    url: &str,
    method: &str,
    body: Option<String>,
    token: &str,
) -> Result<String, String> {
    let opts = RequestInit::new();
    opts.set_method(method);
    opts.set_mode(RequestMode::SameOrigin);

    let headers = Headers::new().map_err(|e| format!("{:?}", e))?;
    headers
        .set("Content-Type", "application/json")
        .map_err(|e| format!("{:?}", e))?;
    if !token.is_empty() {
        headers
            .set("Authorization", &format!("Bearer {}", token))
            .map_err(|e| format!("{:?}", e))?;
    }
    opts.set_headers(&headers);

    if let Some(b) = body {
        opts.set_body(&wasm_bindgen::JsValue::from_str(&b));
    }

    let request = Request::new_with_str_and_init(url, &opts).map_err(|e| format!("{:?}", e))?;

    let window = web_sys::window().ok_or_else(|| "no global window object".to_string())?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;

    let resp: Response = resp_value
        .dyn_into()
        .map_err(|_| "fetch did not return a Response".to_string())?;

    if !resp.ok() {
        // Try to extract a body for a better error message.
        if let Ok(text_promise) = resp.text() {
            if let Ok(text_val) = JsFuture::from(text_promise).await {
                if let Some(text) = text_val.as_string() {
                    // Attempt to parse the envelope error.
                    if let Ok(envelope) =
                        serde_json::from_str::<ApiResponse<serde_json::Value>>(&text)
                    {
                        if let Some(err) = envelope.error {
                            return Err(err.message);
                        }
                    }
                    return Err(format!("HTTP {}: {}", resp.status(), text));
                }
            }
        }
        return Err(format!("HTTP {}", resp.status()));
    }

    let text_promise = resp.text().map_err(|e| format!("{:?}", e))?;
    let text_val = JsFuture::from(text_promise)
        .await
        .map_err(|e| format!("{:?}", e))?;
    text_val
        .as_string()
        .ok_or_else(|| "response body is not a string".to_string())
}

/// Helper to extract `data` from a successful `ApiResponse<T>`.
fn unwrap_envelope<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    let envelope: ApiResponse<T> =
        serde_json::from_str(text).map_err(|e| format!("JSON parse error: {}", e))?;
    if !envelope.success {
        let msg = envelope
            .error
            .map(|e| e.message)
            .unwrap_or_else(|| "Unknown error".into());
        return Err(msg);
    }
    envelope
        .data
        .ok_or_else(|| "Response contained no data".into())
}

/// Perform a GET request against the backend and unwrap the envelope.
pub async fn api_get<T: serde::de::DeserializeOwned>(path: &str, token: &str) -> Result<T, String> {
    let url = format!("{API_BASE}{path}");
    let text = fetch_with_opts(&url, "GET", None, token).await?;
    unwrap_envelope(&text)
}

/// Perform a POST request with a JSON body and unwrap the envelope.
pub async fn api_post<T: serde::de::DeserializeOwned, B: Serialize>(
    path: &str,
    body: &B,
    token: &str,
) -> Result<T, String> {
    let url = format!("{API_BASE}{path}");
    let json = serde_json::to_string(body).map_err(|e| e.to_string())?;
    let text = fetch_with_opts(&url, "POST", Some(json), token).await?;
    unwrap_envelope(&text)
}

/// Perform a PUT request with a JSON body and unwrap the envelope.
pub async fn api_put<T: serde::de::DeserializeOwned, B: Serialize>(
    path: &str,
    body: &B,
    token: &str,
) -> Result<T, String> {
    let url = format!("{API_BASE}{path}");
    let json = serde_json::to_string(body).map_err(|e| e.to_string())?;
    let text = fetch_with_opts(&url, "PUT", Some(json), token).await?;
    unwrap_envelope(&text)
}

/// Perform a DELETE request and check for success.
pub async fn api_delete(path: &str, token: &str) -> Result<(), String> {
    let url = format!("{API_BASE}{path}");
    let text = fetch_with_opts(&url, "DELETE", None, token).await?;
    // Just verify the envelope reports success; discard data.
    let envelope: ApiResponse<serde_json::Value> =
        serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {}", e))?;
    if !envelope.success {
        let msg = envelope
            .error
            .map(|e| e.message)
            .unwrap_or_else(|| "Unknown error".into());
        return Err(msg);
    }
    Ok(())
}

/// Trigger a document export by opening the export URL in a new tab.
pub async fn export_document(doc_id: &str, format: &str, _token: &str) -> Result<(), String> {
    let url = format!("{API_BASE}/documents/{doc_id}/export/{format}");
    if let Some(window) = web_sys::window() {
        let _ = window.open_with_url(&url);
    }
    Ok(())
}

/// Upload a file with multipart/form-data to the backend.
pub async fn upload_file(
    file: File,
    parent_id: Option<Uuid>,
    token: &str,
) -> Result<FileResponse, String> {
    let url = format!("{API_BASE}/files/upload");
    let form = FormData::new().map_err(|e| format!("{:?}", e))?;
    form.append_with_blob_and_filename("file", &file, &file.name())
        .map_err(|e| format!("{:?}", e))?;
    if let Some(parent_id) = parent_id {
        form.append_with_str("parent_id", &parent_id.to_string())
            .map_err(|e| format!("{:?}", e))?;
    }

    let opts = RequestInit::new();
    opts.set_method("POST");
    opts.set_mode(RequestMode::SameOrigin);
    opts.set_body(form.as_ref());

    let headers = Headers::new().map_err(|e| format!("{:?}", e))?;
    if !token.is_empty() {
        headers
            .set("Authorization", &format!("Bearer {}", token))
            .map_err(|e| format!("{:?}", e))?;
    }
    opts.set_headers(&headers);

    let request = Request::new_with_str_and_init(&url, &opts).map_err(|e| format!("{:?}", e))?;
    let window = web_sys::window().ok_or_else(|| "no global window object".to_string())?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;
    let resp: Response = resp_value
        .dyn_into()
        .map_err(|_| "fetch did not return a Response".to_string())?;

    let text_promise = resp.text().map_err(|e| format!("{:?}", e))?;
    let text_val = JsFuture::from(text_promise)
        .await
        .map_err(|e| format!("{:?}", e))?;
    let text = text_val
        .as_string()
        .ok_or_else(|| "response body is not a string".to_string())?;

    if !resp.ok() {
        if let Ok(envelope) = serde_json::from_str::<ApiResponse<serde_json::Value>>(&text) {
            if let Some(err) = envelope.error {
                return Err(err.message);
            }
        }
        return Err(format!("HTTP {}: {}", resp.status(), text));
    }

    unwrap_envelope(&text)
}
