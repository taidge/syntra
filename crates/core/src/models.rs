use diesel::prelude::*;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// User
// ---------------------------------------------------------------------------

/// Role assigned to a user, controlling access throughout the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    Editor,
    Viewer,
}

impl UserRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            UserRole::Admin => "admin",
            UserRole::Editor => "editor",
            UserRole::Viewer => "viewer",
        }
    }

    pub fn parse_lossy(s: &str) -> Self {
        match s {
            "admin" => UserRole::Admin,
            "viewer" => UserRole::Viewer,
            _ => UserRole::Editor,
        }
    }
}

/// A registered user of the Contrix Office platform.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::users)]
pub struct User {
    pub id: String,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: String,
    /// Bytes of storage currently consumed
    pub storage_used: i64,
    /// Maximum allowed storage in bytes
    pub storage_quota: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    pub last_login_at: Option<String>,
}

/// For inserting a new user.
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::users)]
pub struct NewUser {
    pub id: String,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: String,
    pub storage_used: i64,
    pub storage_quota: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
    pub last_login_at: Option<String>,
}

// ---------------------------------------------------------------------------
// File
// ---------------------------------------------------------------------------

/// A file or folder stored in the virtual filesystem.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::files)]
pub struct File {
    pub id: String,
    pub name: String,
    pub path: String,
    pub mime_type: Option<String>,
    /// Size in bytes (0 for folders)
    pub size: i64,
    pub owner_id: String,
    /// Parent folder id; `None` for root-level items
    pub parent_id: Option<String>,
    pub is_folder: bool,
    pub is_trashed: bool,
    pub trashed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub room_id: Option<String>,
    pub thumbnail_path: Option<String>,
    pub sort_order: i32,
}

/// For inserting a new file.
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::files)]
pub struct NewFile {
    pub id: String,
    pub name: String,
    pub path: String,
    pub mime_type: Option<String>,
    pub size: i64,
    pub owner_id: String,
    pub parent_id: Option<String>,
    pub is_folder: bool,
    pub is_trashed: bool,
    pub trashed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub room_id: Option<String>,
    pub thumbnail_path: Option<String>,
    pub sort_order: i32,
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// The kind of document managed by the doc-engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentType {
    Writer,
    Calc,
    Slides,
    Whiteboard,
}

impl DocumentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocumentType::Writer => "writer",
            DocumentType::Calc => "calc",
            DocumentType::Slides => "slides",
            DocumentType::Whiteboard => "whiteboard",
        }
    }

    pub fn parse_lossy(s: &str) -> Self {
        match s {
            "writer" => DocumentType::Writer,
            "calc" => DocumentType::Calc,
            "slides" => DocumentType::Slides,
            _ => DocumentType::Whiteboard,
        }
    }
}

/// A rich document backed by a JSON content tree.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::documents)]
pub struct Document {
    pub id: String,
    /// The [`File`] entry that represents this document on disk
    pub file_id: String,
    pub doc_type: String,
    pub content: Option<String>,
    /// Monotonically increasing version counter
    pub version: i32,
    pub created_at: String,
    pub updated_at: String,
}

/// For inserting a new document.
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::documents)]
pub struct NewDocument {
    pub id: String,
    pub file_id: String,
    pub doc_type: String,
    pub content: Option<String>,
    pub version: i32,
    pub created_at: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Version
// ---------------------------------------------------------------------------

/// An immutable snapshot of a document at a specific point in time.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::versions)]
pub struct Version {
    pub id: String,
    pub document_id: String,
    pub version_number: i32,
    pub content: Option<String>,
    pub created_by: String,
    /// Optional human-readable label (e.g., "Draft v2")
    pub label: Option<String>,
    pub created_at: String,
}

/// For inserting a new version.
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::versions)]
pub struct NewVersion {
    pub id: String,
    pub document_id: String,
    pub version_number: i32,
    pub content: Option<String>,
    pub created_by: String,
    pub label: Option<String>,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Share
// ---------------------------------------------------------------------------

/// Permission level granted via a share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SharePermission {
    Read,
    Write,
    Admin,
}

impl SharePermission {
    pub fn as_str(&self) -> &'static str {
        match self {
            SharePermission::Read => "read",
            SharePermission::Write => "write",
            SharePermission::Admin => "admin",
        }
    }

    pub fn parse_lossy(s: &str) -> Self {
        match s {
            "write" => SharePermission::Write,
            "admin" => SharePermission::Admin,
            _ => SharePermission::Read,
        }
    }
}

/// A share record that grants access to a file (or folder) for another user
/// or via a public link.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::shares)]
pub struct Share {
    pub id: String,
    pub file_id: String,
    pub shared_by: String,
    /// `None` when the share is a public link rather than user-specific
    pub shared_with: Option<String>,
    pub permission: String,
    /// Random token for public-link access
    pub public_token: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub shared_with_group: Option<String>,
    pub password_hash: Option<String>,
}

/// For inserting a new share.
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::shares)]
pub struct NewShare {
    pub id: String,
    pub file_id: String,
    pub shared_by: String,
    pub shared_with: Option<String>,
    pub permission: String,
    pub public_token: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub shared_with_group: Option<String>,
    pub password_hash: Option<String>,
}

// ---------------------------------------------------------------------------
// Comment
// ---------------------------------------------------------------------------

/// A comment (or reply) attached to a document.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::comments)]
pub struct Comment {
    pub id: String,
    pub document_id: String,
    pub user_id: String,
    pub content: String,
    /// If set, this comment is a reply to another comment
    pub parent_id: Option<String>,
    pub is_resolved: bool,
    /// Editor-specific anchor (e.g., cell reference, text range)
    pub position: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// For inserting a new comment.
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::comments)]
pub struct NewComment {
    pub id: String,
    pub document_id: String,
    pub user_id: String,
    pub content: String,
    pub parent_id: Option<String>,
    pub is_resolved: bool,
    pub position: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Notification
// ---------------------------------------------------------------------------

/// Category of a notification delivered to a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationType {
    Share,
    Comment,
    Mention,
    System,
}

impl NotificationType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NotificationType::Share => "share",
            NotificationType::Comment => "comment",
            NotificationType::Mention => "mention",
            NotificationType::System => "system",
        }
    }

    pub fn parse_lossy(s: &str) -> Self {
        match s {
            "share" => NotificationType::Share,
            "comment" => NotificationType::Comment,
            "mention" => NotificationType::Mention,
            _ => NotificationType::System,
        }
    }
}

/// An in-app notification for a user.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::notifications)]
pub struct Notification {
    pub id: String,
    pub user_id: String,
    pub notification_type: String,
    pub title: String,
    pub message: Option<String>,
    /// Deep-link into the application (e.g., `/documents/<id>`)
    pub link: Option<String>,
    pub is_read: bool,
    pub created_at: String,
}

/// For inserting a new notification.
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::notifications)]
pub struct NewNotification {
    pub id: String,
    pub user_id: String,
    pub notification_type: String,
    pub title: String,
    pub message: Option<String>,
    pub link: Option<String>,
    pub is_read: bool,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// ActivityLog
// ---------------------------------------------------------------------------

/// An audit log entry recording a user action.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::activity_logs)]
pub struct ActivityLog {
    pub id: String,
    pub user_id: String,
    /// Verb describing the action (e.g., "create", "update", "delete", "share")
    pub action: String,
    /// Kind of resource acted upon (e.g., "document", "file", "user")
    pub resource_type: String,
    pub resource_id: String,
    /// Arbitrary extra context
    pub details: Option<String>,
    pub created_at: String,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

/// For inserting a new activity log entry.
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::activity_logs)]
pub struct NewActivityLog {
    pub id: String,
    pub user_id: String,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub details: Option<String>,
    pub created_at: String,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

// ---------------------------------------------------------------------------
// Room
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoomType {
    Collaboration,
    Custom,
    Public,
}

impl RoomType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RoomType::Collaboration => "collaboration",
            RoomType::Custom => "custom",
            RoomType::Public => "public",
        }
    }

    pub fn parse_lossy(s: &str) -> Self {
        match s {
            "custom" => RoomType::Custom,
            "public" => RoomType::Public,
            _ => RoomType::Collaboration,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::rooms)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub room_type: String,
    pub owner_id: String,
    pub is_archived: bool,
    pub created_at: String,
    pub updated_at: String,
    pub logo_url: Option<String>,
    pub color: Option<String>,
    pub storage_quota: Option<i64>,
    pub auto_delete_days: Option<i32>,
    pub auto_delete_enabled: bool,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::rooms)]
pub struct NewRoom {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub room_type: String,
    pub owner_id: String,
    pub is_archived: bool,
    pub created_at: String,
    pub updated_at: String,
    pub logo_url: Option<String>,
    pub color: Option<String>,
    pub storage_quota: Option<i64>,
    pub auto_delete_days: Option<i32>,
    pub auto_delete_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::room_members)]
pub struct RoomMember {
    pub id: String,
    pub room_id: String,
    pub user_id: String,
    pub role: String,
    pub joined_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::room_members)]
pub struct NewRoomMember {
    pub id: String,
    pub room_id: String,
    pub user_id: String,
    pub role: String,
    pub joined_at: String,
}

// ---------------------------------------------------------------------------
// Group
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::groups)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::groups)]
pub struct NewGroup {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::group_members)]
pub struct GroupMember {
    pub id: String,
    pub group_id: String,
    pub user_id: String,
    pub added_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::group_members)]
pub struct NewGroupMember {
    pub id: String,
    pub group_id: String,
    pub user_id: String,
    pub added_at: String,
}

// ---------------------------------------------------------------------------
// Tag
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::tags)]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub owner_id: String,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::tags)]
pub struct NewTag {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub owner_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::file_tags)]
pub struct FileTag {
    pub id: String,
    pub file_id: String,
    pub tag_id: String,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::file_tags)]
pub struct NewFileTag {
    pub id: String,
    pub file_id: String,
    pub tag_id: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Favorite
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::favorites)]
pub struct Favorite {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::favorites)]
pub struct NewFavorite {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// RecentFile
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::recent_files)]
pub struct RecentFile {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub accessed_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::recent_files)]
pub struct NewRecentFile {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub accessed_at: String,
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::sessions)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub token_hash: String,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub is_active: bool,
    pub created_at: String,
    pub expires_at: String,
    pub last_activity_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::sessions)]
pub struct NewSession {
    pub id: String,
    pub user_id: String,
    pub token_hash: String,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub is_active: bool,
    pub created_at: String,
    pub expires_at: String,
    pub last_activity_at: String,
}

// ---------------------------------------------------------------------------
// TotpSecret
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::totp_secrets)]
pub struct TotpSecret {
    pub id: String,
    pub user_id: String,
    pub secret: String,
    pub is_enabled: bool,
    pub backup_codes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::totp_secrets)]
pub struct NewTotpSecret {
    pub id: String,
    pub user_id: String,
    pub secret: String,
    pub is_enabled: bool,
    pub backup_codes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// PasswordPolicy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::password_policies)]
pub struct PasswordPolicy {
    pub id: String,
    pub min_length: i32,
    pub require_uppercase: bool,
    pub require_lowercase: bool,
    pub require_digit: bool,
    pub require_special: bool,
    pub max_age_days: Option<i32>,
    pub updated_by: String,
    pub updated_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::password_policies)]
pub struct NewPasswordPolicy {
    pub id: String,
    pub min_length: i32,
    pub require_uppercase: bool,
    pub require_lowercase: bool,
    pub require_digit: bool,
    pub require_special: bool,
    pub max_age_days: Option<i32>,
    pub updated_by: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// IpRestriction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::ip_restrictions)]
pub struct IpRestriction {
    pub id: String,
    pub ip_range: String,
    pub restriction_type: String,
    pub description: Option<String>,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::ip_restrictions)]
pub struct NewIpRestriction {
    pub id: String,
    pub ip_range: String,
    pub restriction_type: String,
    pub description: Option<String>,
    pub created_by: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Webhook
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::webhooks)]
pub struct Webhook {
    pub id: String,
    pub name: String,
    pub url: String,
    pub secret: Option<String>,
    pub events: String,
    pub is_active: bool,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::webhooks)]
pub struct NewWebhook {
    pub id: String,
    pub name: String,
    pub url: String,
    pub secret: Option<String>,
    pub events: String,
    pub is_active: bool,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::webhook_deliveries)]
pub struct WebhookDelivery {
    pub id: String,
    pub webhook_id: String,
    pub event: String,
    pub payload: String,
    pub response_status: Option<i32>,
    pub response_body: Option<String>,
    pub delivered_at: String,
    pub success: bool,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::webhook_deliveries)]
pub struct NewWebhookDelivery {
    pub id: String,
    pub webhook_id: String,
    pub event: String,
    pub payload: String,
    pub response_status: Option<i32>,
    pub response_body: Option<String>,
    pub delivered_at: String,
    pub success: bool,
}

// ---------------------------------------------------------------------------
// BrandingSettings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::branding_settings)]
pub struct BrandingSettings {
    pub id: String,
    pub logo_url: Option<String>,
    pub favicon_url: Option<String>,
    pub app_name: String,
    pub primary_color: Option<String>,
    pub custom_css: Option<String>,
    pub updated_by: String,
    pub updated_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::branding_settings)]
pub struct NewBrandingSettings {
    pub id: String,
    pub logo_url: Option<String>,
    pub favicon_url: Option<String>,
    pub app_name: String,
    pub primary_color: Option<String>,
    pub custom_css: Option<String>,
    pub updated_by: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// SmtpSettings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::smtp_settings)]
pub struct SmtpSettings {
    pub id: String,
    pub host: String,
    pub port: i32,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_email: String,
    pub from_name: String,
    pub use_tls: bool,
    pub updated_by: String,
    pub updated_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::smtp_settings)]
pub struct NewSmtpSettings {
    pub id: String,
    pub host: String,
    pub port: i32,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from_email: String,
    pub from_name: String,
    pub use_tls: bool,
    pub updated_by: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Template
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::templates)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub doc_type: String,
    pub content: Option<String>,
    pub thumbnail_url: Option<String>,
    pub is_global: bool,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::templates)]
pub struct NewTemplate {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub doc_type: String,
    pub content: Option<String>,
    pub thumbnail_url: Option<String>,
    pub is_global: bool,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Invitation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::invitations)]
pub struct Invitation {
    pub id: String,
    pub email: String,
    pub role: String,
    pub invited_by: String,
    pub token: String,
    pub message: Option<String>,
    pub is_accepted: bool,
    pub accepted_at: Option<String>,
    pub expires_at: String,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::invitations)]
pub struct NewInvitation {
    pub id: String,
    pub email: String,
    pub role: String,
    pub invited_by: String,
    pub token: String,
    pub message: Option<String>,
    pub is_accepted: bool,
    pub accepted_at: Option<String>,
    pub expires_at: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// UploadSession
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::upload_sessions)]
pub struct UploadSession {
    pub id: String,
    pub user_id: String,
    pub file_name: String,
    pub mime_type: Option<String>,
    pub total_size: i64,
    pub uploaded_size: i64,
    pub chunk_count: i32,
    pub parent_id: Option<String>,
    pub room_id: Option<String>,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::upload_sessions)]
pub struct NewUploadSession {
    pub id: String,
    pub user_id: String,
    pub file_name: String,
    pub mime_type: Option<String>,
    pub total_size: i64,
    pub uploaded_size: i64,
    pub chunk_count: i32,
    pub parent_id: Option<String>,
    pub room_id: Option<String>,
    pub status: String,
    pub created_at: String,
    pub expires_at: String,
}

// ---------------------------------------------------------------------------
// FileLock
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::file_locks)]
pub struct FileLock {
    pub id: String,
    pub file_id: String,
    pub locked_by: String,
    pub lock_token: String,
    pub lock_type: String,
    pub expires_at: String,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::file_locks)]
pub struct NewFileLock {
    pub id: String,
    pub file_id: String,
    pub locked_by: String,
    pub lock_token: String,
    pub lock_type: String,
    pub expires_at: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// UserPresence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::user_presence)]
pub struct UserPresence {
    pub id: String,
    pub document_id: String,
    pub user_id: String,
    pub session_id: Option<String>,
    pub cursor_position: Option<String>,
    pub is_active: bool,
    pub last_seen_at: String,
    pub joined_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::user_presence)]
pub struct NewUserPresence {
    pub id: String,
    pub document_id: String,
    pub user_id: String,
    pub session_id: Option<String>,
    pub cursor_position: Option<String>,
    pub is_active: bool,
    pub last_seen_at: String,
    pub joined_at: String,
}

// ---------------------------------------------------------------------------
// UserSetting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::user_settings)]
pub struct UserSetting {
    pub id: String,
    pub user_id: String,
    pub setting_key: String,
    pub setting_value: String,
    pub updated_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::user_settings)]
pub struct NewUserSetting {
    pub id: String,
    pub user_id: String,
    pub setting_key: String,
    pub setting_value: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// WatermarkSettings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::watermark_settings)]
pub struct WatermarkSettings {
    pub id: String,
    pub text: Option<String>,
    pub opacity: f32,
    pub font_size: i32,
    pub rotation: i32,
    pub is_enabled: bool,
    pub updated_by: String,
    pub updated_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::watermark_settings)]
pub struct NewWatermarkSettings {
    pub id: String,
    pub text: Option<String>,
    pub opacity: f32,
    pub font_size: i32,
    pub rotation: i32,
    pub is_enabled: bool,
    pub updated_by: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// ClientLog
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::client_logs)]
pub struct ClientLog {
    pub id: String,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub level: String,
    pub message: String,
    pub stack_trace: Option<String>,
    pub user_agent: Option<String>,
    pub url: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::client_logs)]
pub struct NewClientLog {
    pub id: String,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub level: String,
    pub message: String,
    pub stack_trace: Option<String>,
    pub user_agent: Option<String>,
    pub url: Option<String>,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// IntegrationSettings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::integration_settings)]
pub struct IntegrationSetting {
    pub id: String,
    pub service_name: String,
    pub is_enabled: bool,
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub extra_config: Option<String>,
    pub updated_by: String,
    pub updated_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::integration_settings)]
pub struct NewIntegrationSetting {
    pub id: String,
    pub service_name: String,
    pub is_enabled: bool,
    pub api_url: Option<String>,
    pub api_key: Option<String>,
    pub extra_config: Option<String>,
    pub updated_by: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// ConversionJob
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::conversion_jobs)]
pub struct ConversionJob {
    pub id: String,
    pub user_id: String,
    pub source_file_id: String,
    pub target_format: String,
    pub status: String,
    pub output_path: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::conversion_jobs)]
pub struct NewConversionJob {
    pub id: String,
    pub user_id: String,
    pub source_file_id: String,
    pub target_format: String,
    pub status: String,
    pub output_path: Option<String>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

// ---------------------------------------------------------------------------
// PasswordResetToken
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::password_reset_tokens)]
pub struct PasswordResetToken {
    pub id: String,
    pub user_id: String,
    pub token: String,
    pub expires_at: String,
    pub used: bool,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::password_reset_tokens)]
pub struct NewPasswordResetToken {
    pub id: String,
    pub user_id: String,
    pub token: String,
    pub expires_at: String,
    pub used: bool,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// NotificationPreference
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::notification_preferences)]
pub struct NotificationPreference {
    pub id: String,
    pub user_id: String,
    pub event_type: String,
    pub enabled: bool,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::notification_preferences)]
pub struct NewNotificationPreference {
    pub id: String,
    pub user_id: String,
    pub event_type: String,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// OperationLogEntry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::operation_log)]
pub struct OperationLogEntry {
    pub id: String,
    pub document_id: String,
    pub user_id: String,
    pub op_type: String,
    pub op_data: String,
    pub version: i32,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::operation_log)]
pub struct NewOperationLogEntry {
    pub id: String,
    pub document_id: String,
    pub user_id: String,
    pub op_type: String,
    pub op_data: String,
    pub version: i32,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// ApiKey
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::api_keys)]
pub struct ApiKey {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub permissions: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub is_active: bool,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::api_keys)]
pub struct NewApiKey {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub key_hash: String,
    pub key_prefix: String,
    pub permissions: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub is_active: bool,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// LoginAttempt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::login_attempts)]
pub struct LoginAttempt {
    pub id: String,
    pub ip_address: String,
    pub email: String,
    pub success: bool,
    pub attempted_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::login_attempts)]
pub struct NewLoginAttempt {
    pub id: String,
    pub ip_address: String,
    pub email: String,
    pub success: bool,
    pub attempted_at: String,
}

// ---------------------------------------------------------------------------
// FileVersion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::file_versions)]
pub struct FileVersion {
    pub id: String,
    pub file_id: String,
    pub version_number: i32,
    pub path: String,
    pub size: i64,
    pub created_by: String,
    pub created_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::file_versions)]
pub struct NewFileVersion {
    pub id: String,
    pub file_id: String,
    pub version_number: i32,
    pub path: String,
    pub size: i64,
    pub created_by: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// FileProperty
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::file_properties)]
pub struct FileProperty {
    pub id: String,
    pub file_id: String,
    pub prop_key: String,
    pub prop_value: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::file_properties)]
pub struct NewFileProperty {
    pub id: String,
    pub file_id: String,
    pub prop_key: String,
    pub prop_value: String,
}

// ---------------------------------------------------------------------------
// FileReadStatus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::file_read_status)]
pub struct FileReadStatus {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub read_at: String,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::file_read_status)]
pub struct NewFileReadStatus {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub read_at: String,
}

// ---------------------------------------------------------------------------
// DocumentSignature
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::document_signatures)]
pub struct DocumentSignature {
    pub id: String,
    pub document_id: String,
    pub user_id: String,
    pub signature_data: String,
    pub signed_hash: String,
    pub signed_at: String,
    pub is_valid: bool,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::document_signatures)]
pub struct NewDocumentSignature {
    pub id: String,
    pub document_id: String,
    pub user_id: String,
    pub signature_data: String,
    pub signed_hash: String,
    pub signed_at: String,
    pub is_valid: bool,
}

// ---------------------------------------------------------------------------
// Tenant (Multi-Tenant)
// ---------------------------------------------------------------------------

/// A tenant in the multi-tenant system.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::tenants)]
pub struct Tenant {
    pub id: String,
    pub name: String,
    pub domain: Option<String>,
    pub owner_id: Option<String>,
    pub plan: String,
    pub max_users: i32,
    pub max_storage_bytes: i64,
    pub is_active: i32,
    pub settings: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// For inserting a new tenant.
#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::tenants)]
pub struct NewTenant {
    pub id: String,
    pub name: String,
    pub domain: Option<String>,
    pub owner_id: Option<String>,
    pub plan: String,
    pub max_users: i32,
    pub max_storage_bytes: i64,
    pub is_active: i32,
    pub settings: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}
