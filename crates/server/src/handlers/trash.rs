use chrono::Utc;
use diesel::prelude::*;
use horme::RunQueryDsl;
use salvo::prelude::*;
use syntra_core::dto::{ApiResponse, FileResponse};
use syntra_core::error::AppError;
use syntra_core::models::File;
use syntra_core::schema::files::dsl as f;
use syntra_core::schema::room_members::dsl as rm;

use crate::middleware::auth::get_user_id;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn file_model_to_response(row: File) -> FileResponse {
    FileResponse {
        id: row.id.parse().unwrap_or_default(),
        name: row.name,
        mime_type: row.mime_type,
        size: row.size,
        owner_id: row.owner_id.parse().unwrap_or_default(),
        parent_id: row.parent_id.as_deref().and_then(|s| s.parse().ok()),
        is_folder: row.is_folder,
        is_trashed: row.is_trashed,
        trashed_at: row
            .trashed_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc)),
        created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
        updated_at: row.updated_at.parse().unwrap_or_else(|_| Utc::now()),
        shared_with_count: 0,
        document_id: None,
    }
}

// ---------------------------------------------------------------------------
// GET /trash
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_trashed(depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    match f::files
        .filter(f::owner_id.eq(&uid))
        .filter(
            f::room_id.is_null().or(f::room_id.eq_any(
                rm::room_members
                    .filter(rm::user_id.eq(&uid))
                    .select(rm::room_id.nullable()),
            )),
        )
        .filter(f::is_trashed.eq(true))
        .order(f::trashed_at.desc())
        .load::<File>(&mut conn)
        .await
    {
        Ok(rows) => {
            let mut file_list: Vec<FileResponse> =
                rows.into_iter().map(file_model_to_response).collect();
            for r in &mut file_list {
                r.shared_with_count =
                    crate::handlers::access::count_shares_for_file(&mut conn, &r.id.to_string())
                        .await;
            }
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(file_list)));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}

// ---------------------------------------------------------------------------
// POST /trash/:id/restore
// ---------------------------------------------------------------------------

fn restore_trashed_tree(
    conn: &mut SqliteConnection,
    file_id: &str,
    user: &str,
) -> Result<usize, AppError> {
    use diesel::OptionalExtension;

    let root = diesel::RunQueryDsl::first::<File>(f::files.find(file_id), conn)
        .optional()?
        .ok_or_else(|| AppError::NotFound("Trashed file not found".into()))?;
    if root.room_id.is_none() && root.owner_id != user {
        return Err(AppError::NotFound("Trashed file not found".into()));
    }
    crate::services::persistence::authorize_including_trashed(conn, &root, user, "admin")?;
    if !root.is_trashed {
        return Err(AppError::NotFound("Trashed file not found".into()));
    }

    #[derive(diesel::QueryableByName)]
    struct Member {
        #[diesel(sql_type = diesel::sql_types::Text)]
        id: String,
    }
    // Restore only this subtree's reachable members of the deletion action.
    // An older trashed ancestor is a barrier: leave its descendants' membership
    // intact so they can be restored after that ancestor. Older rows have no
    // batch; their timestamps cannot prove membership.
    let members = diesel::RunQueryDsl::load::<Member>(
        diesel::sql_query(
            "WITH RECURSIVE deletion_batch(batch_id) AS \
             (SELECT batch_id FROM trash_batch_members WHERE file_id = ?), subtree(id) AS \
             (SELECT ? UNION SELECT files.id FROM files JOIN subtree ON files.parent_id = subtree.id \
              WHERE files.is_trashed = 0 OR files.id IN \
              (SELECT file_id FROM trash_batch_members WHERE batch_id IN (SELECT batch_id FROM deletion_batch))) \
             SELECT members.file_id AS id FROM trash_batch_members members \
             JOIN files ON files.id = members.file_id \
             WHERE members.batch_id IN (SELECT batch_id FROM deletion_batch) \
             AND files.is_trashed = 1 AND files.id IN (SELECT id FROM subtree)",
        )
        .bind::<diesel::sql_types::Text, _>(file_id)
        .bind::<diesel::sql_types::Text, _>(file_id),
        conn,
    )?;
    let ids: Vec<String> = if members.is_empty() {
        vec![root.id]
    } else {
        members.into_iter().map(|member| member.id).collect()
    };
    let restoring: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
    let rows = diesel::RunQueryDsl::load::<File>(f::files.filter(f::id.eq_any(&ids)), conn)?;
    let mut names = std::collections::HashSet::new();
    for file in &rows {
        crate::services::persistence::authorize_including_trashed(conn, file, user, "admin")?;
        if let Some(parent_id) = &file.parent_id {
            let parent = diesel::RunQueryDsl::first::<File>(f::files.find(parent_id), conn)
                .optional()?
                .ok_or_else(|| AppError::Conflict("Parent folder no longer exists".into()))?;
            crate::services::persistence::authorize_including_trashed(
                conn, &parent, user, "admin",
            )?;
            if !parent.is_folder || (parent.is_trashed && !restoring.contains(parent_id.as_str())) {
                return Err(AppError::Conflict("Restore the parent folder first".into()));
            }
        }
        crate::handlers::folders::ensure_available_name(
            conn,
            &file.owner_id,
            file.parent_id.as_deref(),
            file.room_id.as_deref(),
            &file.name,
            Some(&file.id),
        )?;
        // Also detect conflicting names among the rows about to become active.
        let namespace_owner = file.room_id.is_none().then_some(file.owner_id.as_str());
        if !names.insert((
            file.parent_id.as_deref(),
            file.room_id.as_deref(),
            namespace_owner,
            file.name.as_str(),
        )) {
            return Err(AppError::Conflict(
                "A sibling item with that name already exists".into(),
            ));
        }
    }
    let restored = diesel::RunQueryDsl::execute(
        diesel::update(
            f::files
                .filter(f::id.eq_any(&ids))
                .filter(f::is_trashed.eq(true)),
        )
        .set((
            f::is_trashed.eq(false),
            f::trashed_at.eq(None::<String>),
            f::updated_at.eq(Utc::now().to_rfc3339()),
        )),
        conn,
    )?;
    for id in &ids {
        diesel::RunQueryDsl::execute(
            diesel::sql_query("DELETE FROM trash_batch_members WHERE file_id = ?")
                .bind::<diesel::sql_types::Text, _>(id),
            conn,
        )?;
    }
    Ok(restored)
}

#[handler]
pub async fn restore_file(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Auth("Not authenticated".into()).to_response(),
        )));
        return;
    };
    let method = req.method().clone();
    let request_path = req.uri().path().to_owned();
    let file_id_param: String = req.param("id").unwrap_or_default();
    let result = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                crate::middleware::auth::validate_credential_in_transaction(
                    conn,
                    &credential,
                    &method,
                    &request_path,
                )?;
                restore_trashed_tree(conn, &file_id_param, &uid)
            })
        })
        .await;
    match result {
        Ok(restored_count) => res.render(Json(ApiResponse::ok(
            serde_json::json!({"message":"File restored", "restored_count":restored_count}),
        ))),
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
        }
    }
}

// ---------------------------------------------------------------------------
// DELETE /trash/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn permanently_delete(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let file_id_param: String = req.param("id").unwrap_or_default();
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Auth("Not authenticated".into()).to_response(),
        )));
        return;
    };
    let method = req.method().clone();
    let request_path = req.uri().path().to_owned();
    match crate::services::persistence::purge_trashed_with_credential(
        &state.db,
        &file_id_param,
        Some(&uid),
        None,
        &credential,
        &method,
        &request_path,
    )
    .await
    {
        Ok(count) => {
            crate::services::persistence::request_gc(&state.db, &state.storage);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({"message":"File permanently deleted","count":count}),
            )));
        }
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
        }
    }
}

// ---------------------------------------------------------------------------
// DELETE /trash
// ---------------------------------------------------------------------------

#[handler]
pub async fn empty_trash(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        res.status_code(StatusCode::UNAUTHORIZED);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Auth("Not authenticated".into()).to_response(),
        )));
        return;
    };
    let method = req.method().clone();
    let request_path = req.uri().path().to_owned();
    if let Err(error) = crate::middleware::auth::validate_credential_for_request(
        &state,
        &credential,
        &method,
        &request_path,
    )
    .await
    {
        res.status_code(
            StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        );
        res.render(Json(ApiResponse::<()>::err(error.to_response())));
        return;
    }

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    // Fetch all trashed files to get paths and sizes.
    let trashed_files: Vec<File> = match f::files
        .filter(f::owner_id.eq(&uid))
        .filter(
            f::room_id.is_null().or(f::room_id.eq_any(
                rm::room_members
                    .filter(rm::user_id.eq(&uid))
                    .select(rm::room_id.nullable()),
            )),
        )
        .filter(f::is_trashed.eq(true))
        .load::<File>(&mut conn)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let mut count = 0;
    for file in trashed_files {
        match crate::services::persistence::purge_trashed_with_credential(
            &state.db,
            &file.id,
            Some(&uid),
            None,
            &credential,
            &method,
            &request_path,
        )
        .await
        {
            Ok(deleted) => count += deleted,
            Err(AppError::NotFound(_)) => {} // Already removed as part of a subtree.
            Err(error) => {
                res.status_code(
                    StatusCode::from_u16(error.status_code())
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                );
                res.render(Json(ApiResponse::<()>::err(error.to_response())));
                return;
            }
        }
    }
    crate::services::persistence::request_gc(&state.db, &state.storage);
    res.render(Json(ApiResponse::ok(
        serde_json::json!({"message":"Trash emptied","count":count}),
    )));
}

// ---------------------------------------------------------------------------
// GET /trash/count
// ---------------------------------------------------------------------------

#[handler]
pub async fn trash_count(depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    match f::files
        .filter(f::owner_id.eq(&uid))
        .filter(
            f::room_id.is_null().or(f::room_id.eq_any(
                rm::room_members
                    .filter(rm::user_id.eq(&uid))
                    .select(rm::room_id.nullable()),
            )),
        )
        .filter(f::is_trashed.eq(true))
        .count()
        .get_result::<i64>(&mut conn)
        .await
    {
        Ok(count) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({ "count": count }))));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
