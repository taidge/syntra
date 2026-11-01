//! Shared access-control helpers for file/folder/room authorization.
//!
//! Role hierarchy: admin > editor > viewer.

use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;

use contrix_core::error::AppError;
use contrix_core::models::File;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::room_members::dsl as rm;

/// Count the number of active (non-expired) share rows for a file. Used to
/// populate `FileResponse.shared_with_count` with a real value.
pub async fn count_shares_for_file(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    file_id: &str,
) -> u32 {
    use contrix_core::schema::shares::dsl as s;

    let now = chrono::Utc::now().to_rfc3339();
    s::shares
        .filter(s::file_id.eq(file_id))
        .filter(s::expires_at.is_null().or(s::expires_at.gt(now)))
        .count()
        .get_result::<i64>(conn)
        .await
        .unwrap_or(0)
        .max(0) as u32
}

/// Check if user has at least `required_role` access to a room.
pub async fn check_room_access(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    user_id: &str,
    room_id: &str,
    required_role: &str,
) -> Result<(), AppError> {
    let member = rm::room_members
        .filter(rm::room_id.eq(room_id))
        .filter(rm::user_id.eq(user_id))
        .select(rm::role)
        .first::<String>(conn)
        .await
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))?;

    match member {
        Some(role) => {
            let allowed = match required_role {
                "viewer" => true,
                "editor" => role == "owner" || role == "editor" || role == "admin",
                "admin" => role == "owner" || role == "admin",
                _ => false,
            };
            if allowed {
                Ok(())
            } else {
                Err(AppError::Forbidden("Insufficient room permissions".into()))
            }
        }
        None => Err(AppError::Forbidden("Not a member of this room".into())),
    }
}

/// Authorize access to an already-loaded file row: room membership wins,
/// otherwise the user must be the owner.
pub async fn authorize_file_access(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    row: &File,
    user_id: &str,
    required_role: &str,
) -> Result<(), AppError> {
    if let Some(room_id) = row.room_id.as_deref() {
        check_room_access(conn, user_id, room_id, required_role).await
    } else if row.owner_id == user_id {
        Ok(())
    } else {
        Err(AppError::NotFound("File not found".into()))
    }
}

/// Load a file by id and authorize the user for `required_role`.
pub async fn load_accessible_file(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    file_id: &str,
    user_id: &str,
    required_role: &str,
) -> Result<Option<File>, AppError> {
    let row = f::files
        .filter(f::id.eq(file_id))
        .first::<File>(conn)
        .await
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))?;

    match row {
        Some(row) => {
            authorize_file_access(conn, &row, user_id, required_role).await?;
            Ok(Some(row))
        }
        None => Ok(None),
    }
}

/// Load a folder by id (trashed or not) and authorize the user for
/// `required_role`. Returns a validation error if the id refers to a
/// non-folder file.
pub async fn load_accessible_folder(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    folder_id: &str,
    user_id: &str,
    required_role: &str,
) -> Result<Option<File>, AppError> {
    match load_accessible_file(conn, folder_id, user_id, required_role).await? {
        Some(row) if !row.is_folder => Err(AppError::Validation("Target is not a folder".into())),
        other => Ok(other),
    }
}

/// Resolve a `documents.id` to its backing [`File`] and authorize the user for
/// `required_role`. Returns `Ok(None)` if the document does not exist.
///
/// Used by document-scoped sub-resources (comments, presence, signatures) so
/// that access mirrors the underlying file's room/owner permissions instead of
/// being open to any authenticated user.
pub async fn load_accessible_document(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    document_id: &str,
    user_id: &str,
    required_role: &str,
) -> Result<Option<File>, AppError> {
    use contrix_core::schema::documents::dsl as d;

    let file_id = d::documents
        .filter(d::id.eq(document_id))
        .select(d::file_id)
        .first::<String>(conn)
        .await
        .optional()
        .map_err(|e| AppError::Database(e.to_string()))?;

    match file_id {
        Some(fid) => load_accessible_file(conn, &fid, user_id, required_role).await,
        None => Ok(None),
    }
}

/// Like [`load_accessible_folder`] but excludes trashed folders. Use when the
/// folder is the destination or parent of new content.
pub async fn load_accessible_target_folder(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    folder_id: &str,
    user_id: &str,
    required_role: &str,
) -> Result<Option<File>, AppError> {
    match load_accessible_folder(conn, folder_id, user_id, required_role).await? {
        Some(row) if row.is_trashed => Ok(None),
        other => Ok(other),
    }
}
