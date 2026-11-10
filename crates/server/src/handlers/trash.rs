use chrono::Utc;
use contrix_core::dto::{ApiResponse, FileResponse};
use contrix_core::error::AppError;
use contrix_core::models::File;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::room_members::dsl as rm;
use diesel::prelude::*;
use horme::RunQueryDsl;
use salvo::prelude::*;

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

    let file_id_param: String = req.param("id").unwrap_or_default();
    let result = state
        .db
        .run(move |conn| {
            use diesel::OptionalExtension;
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                let file = diesel::RunQueryDsl::first::<File>(f::files.find(&file_id_param), conn)
                    .optional()?
                    .ok_or_else(|| AppError::NotFound("Trashed file not found".into()))?;
                crate::services::persistence::authorize_including_trashed(
                    conn, &file, &uid, "admin",
                )?;
                if !file.is_trashed {
                    return Err(AppError::NotFound("Trashed file not found".into()));
                }
                diesel::RunQueryDsl::execute(
                    diesel::update(f::files.find(&file_id_param)).set((
                        f::is_trashed.eq(false),
                        f::trashed_at.eq(None::<String>),
                        f::updated_at.eq(Utc::now().to_rfc3339()),
                    )),
                    conn,
                )?;
                Ok(())
            })
        })
        .await;
    match result {
        Ok(()) => res.render(Json(ApiResponse::ok(
            serde_json::json!({"message":"File restored"}),
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

    match crate::services::persistence::purge_trashed(&state.db, &file_id_param, Some(&uid), None)
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
pub async fn empty_trash(depot: &mut Depot, res: &mut Response) {
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
        match crate::services::persistence::purge_trashed(&state.db, &file.id, Some(&uid), None)
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
