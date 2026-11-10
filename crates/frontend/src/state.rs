use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{File, FormData, Headers, Request, RequestCache, RequestInit, RequestMode, Response};

// ---------------------------------------------------------------------------
// Client-side auth state
// ---------------------------------------------------------------------------

/// Authentication state stored in the application's Glory Cage.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AuthState {
    pub token: Option<String>,
    pub user: Option<UserInfo>,
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
    pub totp_code: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct UploadResponse {
    #[serde(flatten)]
    pub file: FileResponse,
    #[serde(default)]
    pub import_warnings: Vec<String>,
    pub original_download_url: Option<String>,
}

// ---------------------------------------------------------------------------
// API client helpers
// ---------------------------------------------------------------------------

/// Same-origin base path for all backend API calls.
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
        && let Ok(serialized) = serde_json::to_string(state)
    {
        let _ = storage.set_item(AUTH_STORAGE_KEY, &serialized);
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

struct FetchFailure {
    status: Option<u16>,
    message: String,
}

/// Keep the HTTP status for requests whose failure can leave a write in flight.
async fn fetch_response_text(
    url: &str,
    method: &str,
    body: Option<String>,
    token: &str,
    cache: RequestCache,
) -> Result<String, FetchFailure> {
    let opts = RequestInit::new();
    opts.set_method(method);
    opts.set_mode(RequestMode::SameOrigin);
    opts.set_cache(cache);

    let javascript_error = |error| FetchFailure {
        status: None,
        message: format!("{error:?}"),
    };
    let headers = Headers::new().map_err(javascript_error)?;
    headers
        .set("Content-Type", "application/json")
        .map_err(javascript_error)?;
    if !token.is_empty() {
        headers
            .set("Authorization", &format!("Bearer {}", token))
            .map_err(javascript_error)?;
    }
    opts.set_headers(&headers);

    if let Some(b) = body {
        opts.set_body(&wasm_bindgen::JsValue::from_str(&b));
    }

    let request = Request::new_with_str_and_init(url, &opts).map_err(javascript_error)?;

    let window = web_sys::window().ok_or_else(|| FetchFailure {
        status: None,
        message: "no global window object".into(),
    })?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(javascript_error)?;

    let resp: Response = resp_value.dyn_into().map_err(|_| FetchFailure {
        status: None,
        message: "fetch did not return a Response".into(),
    })?;

    if !resp.ok() {
        let failure = |message| FetchFailure {
            status: Some(resp.status()),
            message,
        };
        // Try to extract a body for a better error message.
        if let Ok(text_promise) = resp.text()
            && let Ok(text_val) = JsFuture::from(text_promise).await
            && let Some(text) = text_val.as_string()
        {
            // Attempt to parse the envelope error.
            if let Ok(envelope) = serde_json::from_str::<ApiResponse<serde_json::Value>>(&text)
                && let Some(err) = envelope.error
            {
                return Err(failure(err.message));
            }
            return Err(failure(format!("HTTP {}: {}", resp.status(), text)));
        }
        return Err(failure(format!("HTTP {}", resp.status())));
    }

    let text_promise = resp.text().map_err(javascript_error)?;
    let text_val = JsFuture::from(text_promise)
        .await
        .map_err(javascript_error)?;
    text_val.as_string().ok_or_else(|| FetchFailure {
        status: None,
        message: "response body is not a string".into(),
    })
}

async fn fetch_with_opts(
    url: &str,
    method: &str,
    body: Option<String>,
    token: &str,
) -> Result<String, String> {
    fetch_response_text(url, method, body, token, RequestCache::Default)
        .await
        .map_err(|error| error.message)
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

#[derive(Deserialize)]
pub(crate) struct DocumentSnapshot {
    pub version: i32,
    pub content: serde_json::Value,
}

pub(crate) enum DocumentSave {
    Saved(i32),
    Changed(DocumentSnapshot),
    Unconfirmed,
    Rejected { status: u16, message: String },
}

/// One CAS write, followed by a fresh read only if its result is ambiguous.
/// A newer snapshot confirms the submitted JSON value, including nested maps
/// whose keys may arrive in a different order.
pub(crate) async fn save_document(
    id: &str,
    content: &serde_json::Value,
    expected_version: i32,
    token: &str,
) -> DocumentSave {
    #[derive(Deserialize)]
    struct Receipt {
        version: i32,
    }

    let url = format!("{API_BASE}/documents/{id}");
    let body = serde_json::json!({"content": content, "expected_version": expected_version});
    match fetch_response_text(
        &url,
        "PUT",
        Some(body.to_string()),
        token,
        RequestCache::Default,
    )
    .await
    {
        Ok(text) => {
            if let Ok(receipt) = unwrap_envelope::<Receipt>(&text)
                && receipt.version > expected_version
            {
                return DocumentSave::Saved(receipt.version);
            }
        }
        Err(FetchFailure {
            status: Some(status),
            message,
        }) if !matches!(status, 502..=504) => {
            return DocumentSave::Rejected { status, message };
        }
        Err(_) => {}
    }

    let snapshot = fetch_response_text(&url, "GET", None, token, RequestCache::NoStore)
        .await
        .ok()
        .and_then(|text| unwrap_envelope::<DocumentSnapshot>(&text).ok());
    match snapshot {
        Some(snapshot) if snapshot.version > expected_version => {
            if &snapshot.content == content {
                DocumentSave::Saved(snapshot.version)
            } else {
                DocumentSave::Changed(snapshot)
            }
        }
        _ => DocumentSave::Unconfirmed,
    }
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

/// Trigger a browser download for short, generated text such as one-time
/// recovery codes. A data URL avoids sending sensitive contents elsewhere.
pub fn download_text_file(file_name: &str, contents: &str) -> Result<(), String> {
    use base64::Engine;

    let encoded = base64::engine::general_purpose::STANDARD.encode(contents);
    let href = format!("data:text/plain;charset=utf-8;base64,{encoded}");
    let window = web_sys::window().ok_or_else(|| "no global window object".to_string())?;
    let document = window
        .document()
        .ok_or_else(|| "no document object".to_string())?;
    let anchor: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .map_err(|error| format!("{error:?}"))?
        .dyn_into()
        .map_err(|_| "failed to create anchor element".to_string())?;
    anchor.set_href(&href);
    anchor.set_download(file_name);
    anchor.click();
    Ok(())
}

/// Export a document via the authenticated POST endpoint and trigger a
/// browser download of the returned bytes.
pub async fn export_document(doc_id: &str, format: &str, token: &str) -> Result<(), String> {
    authenticated_download(
        &format!("{API_BASE}/documents/{doc_id}/export/{format}"),
        "POST",
        &format!("export.{format}"),
        token,
    )
    .await
}

pub async fn download_original(url: &str, filename: &str, token: &str) -> Result<(), String> {
    let path = url.strip_suffix("?original=true").unwrap_or(url);
    if !path.starts_with("/api/v1/files/")
        || !path.ends_with("/download")
        || path.contains(['?', '#', '\\'])
    {
        return Err("Invalid original file download URL".into());
    }
    authenticated_download(url, "GET", filename, token).await
}

async fn authenticated_download(
    url: &str,
    method: &str,
    fallback_name: &str,
    token: &str,
) -> Result<(), String> {
    let opts = RequestInit::new();
    opts.set_method(method);
    opts.set_mode(RequestMode::SameOrigin);

    let headers = Headers::new().map_err(|e| format!("{:?}", e))?;
    if !token.is_empty() {
        headers
            .set("Authorization", &format!("Bearer {}", token))
            .map_err(|e| format!("{:?}", e))?;
    }
    opts.set_headers(&headers);

    let request = Request::new_with_str_and_init(url, &opts).map_err(|e| format!("{:?}", e))?;
    let window = web_sys::window().ok_or_else(|| "no global window object".to_string())?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("{:?}", e))?;
    let resp: Response = resp_value
        .dyn_into()
        .map_err(|_| "fetch did not return a Response".to_string())?;

    if !resp.ok() {
        if let Ok(text_promise) = resp.text()
            && let Ok(text_val) = JsFuture::from(text_promise).await
            && let Some(text) = text_val.as_string()
            && let Ok(envelope) = serde_json::from_str::<ApiResponse<serde_json::Value>>(&text)
            && let Some(err) = envelope.error
        {
            return Err(err.message);
        }
        return Err(format!("HTTP {}", resp.status()));
    }

    // Prefer the server-provided filename from Content-Disposition.
    let file_name = resp
        .headers()
        .get("content-disposition")
        .ok()
        .flatten()
        .and_then(|cd| {
            cd.split("filename=")
                .nth(1)
                .map(|part| part.trim_matches(|c| c == '"' || c == ' ').to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| fallback_name.to_owned());

    let blob_promise = resp.blob().map_err(|e| format!("{:?}", e))?;
    let blob_value = JsFuture::from(blob_promise)
        .await
        .map_err(|e| format!("{:?}", e))?;
    let blob: web_sys::Blob = blob_value
        .dyn_into()
        .map_err(|_| "response body is not a Blob".to_string())?;

    let object_url =
        web_sys::Url::create_object_url_with_blob(&blob).map_err(|e| format!("{:?}", e))?;
    let document = window
        .document()
        .ok_or_else(|| "no document object".to_string())?;
    let anchor: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .map_err(|e| format!("{:?}", e))?
        .dyn_into()
        .map_err(|_| "failed to create anchor element".to_string())?;
    anchor.set_href(&object_url);
    anchor.set_download(&file_name);
    anchor.click();
    let _ = web_sys::Url::revoke_object_url(&object_url);

    Ok(())
}

/// Upload a file with multipart/form-data to the backend.
pub async fn upload_file(
    file: File,
    parent_id: Option<Uuid>,
    token: &str,
) -> Result<UploadResponse, String> {
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
        if let Ok(envelope) = serde_json::from_str::<ApiResponse<serde_json::Value>>(&text)
            && let Some(err) = envelope.error
        {
            return Err(err.message);
        }
        return Err(format!("HTTP {}: {}", resp.status(), text));
    }

    unwrap_envelope(&text)
}
