use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ErrorResponse;
use crate::models::{DocumentType, NotificationType, RoomType, SharePermission, UserRole};

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

/// Payload for the `POST /auth/register` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

/// Payload for the `POST /auth/login` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    /// Required when the account has TOTP enabled. Accepts either a six-digit
    /// authenticator code or an unused eight-character backup code.
    #[serde(default)]
    pub totp_code: Option<String>,
}

/// Payload for the `POST /auth/forgot-password` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

/// Payload for the `POST /auth/reset-password` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub password: String,
}

/// Response returned after successful authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: UserProfile,
}

// ---------------------------------------------------------------------------
// User
// ---------------------------------------------------------------------------

/// Public-facing user profile (no sensitive fields like `password_hash`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: UserRole,
    pub storage_used: i64,
    pub storage_quota: i64,
}

/// Payload for updating the authenticated user's profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateUserRequest {
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

// ---------------------------------------------------------------------------
// File / Folder
// ---------------------------------------------------------------------------

/// Payload for creating a new folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFolderRequest {
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub room_id: Option<Uuid>,
}

/// Payload for moving a file or folder to another location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveFileRequest {
    pub target_folder_id: Uuid,
}

/// Enriched file metadata returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileResponse {
    pub id: Uuid,
    pub name: String,
    pub mime_type: Option<String>,
    pub size: i64,
    pub owner_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub is_folder: bool,
    pub is_trashed: bool,
    pub trashed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Number of active shares for this file
    pub shared_with_count: u32,
    pub document_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// Payload for creating a new document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDocumentRequest {
    pub name: String,
    pub document_type: DocumentType,
    pub parent_id: Option<Uuid>,
    pub room_id: Option<Uuid>,
}

/// Payload for saving document content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDocumentRequest {
    pub content: serde_json::Value,
}

/// Full document information returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentResponse {
    pub id: Uuid,
    pub file_id: Uuid,
    pub name: String,
    pub document_type: DocumentType,
    pub content: serde_json::Value,
    pub version: i32,
    pub owner_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Share
// ---------------------------------------------------------------------------

/// Payload for creating a new share.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateShareRequest {
    pub file_id: Uuid,
    pub shared_with_user_id: Option<Uuid>,
    pub permission: SharePermission,
    pub expires_at: Option<DateTime<Utc>>,
    pub shared_with_group_id: Option<Uuid>,
    pub password: Option<String>,
}

/// Share record returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareResponse {
    pub id: Uuid,
    pub file_id: Uuid,
    pub shared_by_user_id: Uuid,
    pub shared_with_user_id: Option<Uuid>,
    pub permission: SharePermission,
    pub public_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub shared_with_group_id: Option<Uuid>,
    pub is_password_protected: bool,
}

// ---------------------------------------------------------------------------
// Comment
// ---------------------------------------------------------------------------

/// Payload for creating a comment on a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCommentRequest {
    pub content: String,
    pub parent_id: Option<Uuid>,
    pub position: Option<serde_json::Value>,
}

/// Comment returned by the API (includes author information).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentResponse {
    pub id: Uuid,
    pub document_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub content: String,
    pub parent_id: Option<Uuid>,
    pub is_resolved: bool,
    pub position: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// Query parameters for the search endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub document_type: Option<DocumentType>,
    pub from_date: Option<DateTime<Utc>>,
    pub to_date: Option<DateTime<Utc>>,
    pub page: u32,
    pub per_page: u32,
}

/// A single search hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: Uuid,
    pub name: String,
    pub document_type: DocumentType,
    /// Highlighted text snippet from the matching content
    pub snippet: String,
    /// Relevance score assigned by the search engine
    pub score: f32,
    pub updated_at: DateTime<Utc>,
}

/// Paginated search response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

// ---------------------------------------------------------------------------
// Notification
// ---------------------------------------------------------------------------

/// Notification returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationResponse {
    pub id: Uuid,
    pub notification_type: NotificationType,
    pub title: String,
    pub message: String,
    pub link: Option<String>,
    pub is_read: bool,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Admin
// ---------------------------------------------------------------------------

/// Aggregate statistics for the admin dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminStats {
    pub total_users: u64,
    pub total_files: u64,
    pub total_documents: u64,
    /// Total storage used across all users, in bytes
    pub total_storage_used: i64,
}

// ---------------------------------------------------------------------------
// Room
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoomRequest {
    pub name: String,
    pub description: Option<String>,
    pub room_type: Option<RoomType>,
    pub logo_url: Option<String>,
    pub color: Option<String>,
    pub storage_quota: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRoomRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomResponse {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub room_type: RoomType,
    pub owner_id: Uuid,
    pub is_archived: bool,
    pub member_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddRoomMemberRequest {
    pub user_id: Uuid,
    pub role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMemberResponse {
    pub id: Uuid,
    pub room_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Group
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateGroupRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupResponse {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_by: Uuid,
    pub member_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddGroupMemberRequest {
    pub user_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMemberResponse {
    pub id: Uuid,
    pub group_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub added_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Tag
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTagRequest {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTagRequest {
    pub name: Option<String>,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagResponse {
    pub id: Uuid,
    pub name: String,
    pub color: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagFileRequest {
    pub file_id: Uuid,
    pub tag_id: Uuid,
}

// ---------------------------------------------------------------------------
// Favorite / Recent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FavoriteRequest {
    pub file_id: Uuid,
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResponse {
    pub id: Uuid,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// TOTP
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpSetupRequest {
    pub current_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpSetupResponse {
    pub secret: String,
    pub qr_url: String,
    pub backup_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TotpVerifyRequest {
    pub code: String,
}

// ---------------------------------------------------------------------------
// Password Policy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordPolicyRequest {
    pub min_length: Option<i32>,
    pub require_uppercase: Option<bool>,
    pub require_lowercase: Option<bool>,
    pub require_digit: Option<bool>,
    pub require_special: Option<bool>,
    pub max_age_days: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordPolicyResponse {
    pub min_length: i32,
    pub require_uppercase: bool,
    pub require_lowercase: bool,
    pub require_digit: bool,
    pub require_special: bool,
    pub max_age_days: Option<i32>,
}

// ---------------------------------------------------------------------------
// IP Restriction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateIpRestrictionRequest {
    pub ip_range: String,
    pub restriction_type: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpRestrictionResponse {
    pub id: Uuid,
    pub ip_range: String,
    pub restriction_type: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Webhook
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWebhookRequest {
    pub name: String,
    pub url: String,
    pub secret: Option<String>,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateWebhookRequest {
    pub name: Option<String>,
    pub url: Option<String>,
    pub events: Option<Vec<String>>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookResponse {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub events: Vec<String>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDeliveryResponse {
    pub id: Uuid,
    pub event: String,
    pub response_status: Option<i32>,
    pub success: bool,
    pub delivered_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Branding
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateBrandingRequest {
    pub logo_url: Option<String>,
    pub favicon_url: Option<String>,
    pub app_name: Option<String>,
    pub primary_color: Option<String>,
    pub custom_css: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrandingResponse {
    pub logo_url: Option<String>,
    pub favicon_url: Option<String>,
    pub app_name: String,
    pub primary_color: Option<String>,
    pub custom_css: Option<String>,
}

// ---------------------------------------------------------------------------
// Backup
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupResponse {
    pub id: String,
    pub file_name: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Quota
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaResponse {
    pub user_id: Uuid,
    pub storage_used: i64,
    pub storage_quota: i64,
    pub usage_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateQuotaRequest {
    pub user_id: Uuid,
    pub storage_quota: i64,
}

// ---------------------------------------------------------------------------
// Template
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTemplateRequest {
    pub name: String,
    pub description: Option<String>,
    pub document_type: DocumentType,
    pub content: Option<serde_json::Value>,
    pub is_global: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTemplateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub content: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateResponse {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub document_type: DocumentType,
    pub content: Option<serde_json::Value>,
    pub thumbnail_url: Option<String>,
    pub is_global: bool,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Invitation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateInvitationRequest {
    pub email: String,
    pub role: Option<UserRole>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvitationResponse {
    pub id: Uuid,
    pub email: String,
    pub role: UserRole,
    pub invited_by: Uuid,
    pub token: String,
    pub message: Option<String>,
    pub is_accepted: bool,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Upload Session (Chunked Upload)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUploadSessionRequest {
    pub file_name: String,
    pub mime_type: Option<String>,
    pub total_size: i64,
    pub parent_id: Option<Uuid>,
    pub room_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSessionResponse {
    pub id: Uuid,
    pub file_name: String,
    pub total_size: i64,
    pub uploaded_size: i64,
    pub chunk_count: i32,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// SMTP
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpSettingsRequest {
    pub host: String,
    pub port: Option<i32>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_email: String,
    pub from_name: Option<String>,
    pub use_tls: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpSettingsResponse {
    pub host: String,
    pub port: i32,
    pub username: Option<String>,
    pub from_email: String,
    pub from_name: String,
    pub use_tls: bool,
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_seconds: u64,
}

// ---------------------------------------------------------------------------
// File Lock
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileLockResponse {
    pub id: Uuid,
    pub file_id: Uuid,
    pub locked_by: Uuid,
    pub lock_token: String,
    pub lock_type: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// User Presence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceRequest {
    pub cursor_position: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceResponse {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    pub cursor_position: Option<serde_json::Value>,
    pub is_active: bool,
    pub last_seen_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// User Settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettingRequest {
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSettingResponse {
    pub key: String,
    pub value: String,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Watermark
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatermarkRequest {
    pub text: Option<String>,
    pub opacity: Option<f32>,
    pub font_size: Option<i32>,
    pub rotation: Option<i32>,
    pub is_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatermarkResponse {
    pub text: Option<String>,
    pub opacity: f32,
    pub font_size: i32,
    pub rotation: i32,
    pub is_enabled: bool,
}

// ---------------------------------------------------------------------------
// Client Log
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientLogEntry {
    pub level: Option<String>,
    pub message: String,
    pub stack_trace: Option<String>,
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConversionRequest {
    pub file_id: Uuid,
    pub target_format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionJobResponse {
    pub id: Uuid,
    pub source_file_id: Uuid,
    pub target_format: String,
    pub status: String,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Monitoring
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringResponse {
    pub active_documents: u64,
    pub active_users: u64,
    pub total_users: u64,
    pub total_documents: u64,
    pub total_storage_bytes: i64,
    pub memory_usage_bytes: u64,
}

// ---------------------------------------------------------------------------
// Pagination & generic API wrapper
// ---------------------------------------------------------------------------

/// Common pagination query parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationParams {
    pub page: u32,
    pub per_page: u32,
}

/// Uniform JSON envelope used for all API responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorResponse>,
}

impl<T: Serialize> ApiResponse<T> {
    /// Create a successful response wrapping `data`.
    pub fn ok(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    /// Create an error response.
    pub fn err(error: ErrorResponse) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(error),
        }
    }
}
