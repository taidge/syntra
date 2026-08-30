use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, CreateFolderRequest, FileResponse};
use contrix_core::error::AppError;
use contrix_core::models::{File, NewFile};
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;

use crate::handlers::access::{
    authorize_file_access, check_room_access, load_accessible_folder, load_accessible_target_folder,
};
use crate::middleware::auth::get_user_id;

// ---------------------------------------------------------------------------
// POST /folders
// ---------------------------------------------------------------------------

fn document_id_for_file(conn: &mut diesel::SqliteConnection, file_id: &str) -> Option<Uuid> {
    d::documents
        .filter(d::file_id.eq(file_id))
        .select(d::id)
        .first::<String>(conn)
        .optional()
        .ok()
        .flatten()
        .and_then(|id| id.parse().ok())
}

#[handler]
pub async fn create_folder(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: CreateFolderRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let folder_id = Uuid::new_v4();
    let now = Utc::now();
    let parent_id_str = body.parent_id.map(|pid| pid.to_string());
    let requested_room_id = body.room_id.map(|rid| rid.to_string());
    let path_val = format!("/{}", body.name);

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

    let room_id_for_new_folder = if let Some(parent_id) = parent_id_str.as_deref() {
        match load_accessible_target_folder(&mut conn, parent_id, &uid, "editor") {
            Ok(Some(parent)) => {
                if let Some(requested_room_id) = requested_room_id.as_deref()
                    && parent.room_id.as_deref() != Some(requested_room_id)
                {
                    res.status_code(StatusCode::BAD_REQUEST);
                    res.render(Json(ApiResponse::<()>::err(
                        AppError::Validation("room_id must match the parent folder room".into())
                            .to_response(),
                    )));
                    return;
                }
                parent.room_id.clone()
            }
            Ok(None) => {
                res.status_code(StatusCode::NOT_FOUND);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::NotFound("Parent folder not found".into()).to_response(),
                )));
                return;
            }
            Err(e) => {
                res.status_code(StatusCode::FORBIDDEN);
                res.render(Json(ApiResponse::<()>::err(e.to_response())));
                return;
            }
        }
    } else if let Some(room_id) = requested_room_id.as_deref() {
        match check_room_access(&mut conn, &uid, room_id, "editor") {
            Ok(()) => Some(room_id.to_string()),
            Err(e) => {
                res.status_code(StatusCode::FORBIDDEN);
                res.render(Json(ApiResponse::<()>::err(e.to_response())));
                return;
            }
        }
    } else {
        None
    };

    let new_file = NewFile {
        id: folder_id.to_string(),
        name: body.name.clone(),
        path: path_val.clone(),
        mime_type: None,
        size: 0,
        owner_id: uid.clone(),
        parent_id: parent_id_str,
        is_folder: true,
        is_trashed: false,
        trashed_at: None,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        room_id: room_id_for_new_folder,
        thumbnail_path: None,
        sort_order: 0,
    };

    match diesel::insert_into(f::files)
        .values(&new_file)
        .execute(&mut conn)
    {
        Ok(_) => {
            let resp = FileResponse {
                id: folder_id,
                name: body.name,
                mime_type: None,
                size: 0,
                owner_id: uid.parse().unwrap_or_default(),
                parent_id: body.parent_id,
                is_folder: true,
                is_trashed: false,
                trashed_at: None,
                created_at: now,
                updated_at: now,
                shared_with_count: 0,
                document_id: None,
            };
            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(resp)));
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
// GET /folders/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_folder(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let folder_id_param: String = req.param("id").unwrap_or_default();

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

    match load_accessible_folder(&mut conn, &folder_id_param, &uid, "viewer") {
        Ok(Some(row)) => {
            let shared_with_count =
                crate::handlers::access::count_shares_for_file(&mut conn, &row.id);
            let resp = FileResponse {
                id: row.id.parse().unwrap_or_default(),
                name: row.name,
                mime_type: None,
                size: 0,
                owner_id: uid.parse().unwrap_or_default(),
                parent_id: row.parent_id.as_deref().and_then(|s| s.parse().ok()),
                is_folder: true,
                is_trashed: row.is_trashed,
                trashed_at: None,
                created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
                updated_at: row.updated_at.parse().unwrap_or_else(|_| Utc::now()),
                shared_with_count,
                document_id: None,
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(resp)));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Folder not found".into()).to_response(),
            )));
        }
        Err(e) => {
            let status = match e {
                AppError::Forbidden(_) => StatusCode::FORBIDDEN,
                AppError::NotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            res.status_code(status);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
        }
    }
}

// ---------------------------------------------------------------------------
// GET /folders/:id/children
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_folder_children(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let folder_id_param: String = req.param("id").unwrap_or_default();

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

    let folder = match load_accessible_folder(&mut conn, &folder_id_param, &uid, "viewer") {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Folder not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    let mut query = f::files
        .into_boxed()
        .filter(f::parent_id.eq(&folder_id_param))
        .filter(f::is_trashed.eq(false))
        .order((f::is_folder.desc(), f::name.asc()));

    if folder.room_id.is_none() {
        query = query.filter(f::owner_id.eq(&uid));
    }

    match query.load::<File>(&mut conn) {
        Ok(rows) => {
            let children: Vec<FileResponse> = rows
                .into_iter()
                .map(|row| {
                    let document_id = document_id_for_file(&mut conn, &row.id);
                    let shared_with_count =
                        crate::handlers::access::count_shares_for_file(&mut conn, &row.id);
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
                        shared_with_count,
                        document_id,
                    }
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(children)));
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
// PUT /folders/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_folder(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let folder_id_param: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct RenameFolderRequest {
        name: String,
    }

    let body: RenameFolderRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let now = Utc::now();

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

    let folder = match load_accessible_folder(&mut conn, &folder_id_param, &uid, "editor") {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Folder not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    match diesel::update(f::files.filter(f::id.eq(&folder.id)))
        .set((f::name.eq(&body.name), f::updated_at.eq(now.to_rfc3339())))
        .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Folder not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Folder renamed" }),
            )));
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
// DELETE /folders/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_folder(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let folder_id_param: String = req.param("id").unwrap_or_default();
    let now = Utc::now();

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

    let folder = match load_accessible_folder(&mut conn, &folder_id_param, &uid, "editor") {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Folder not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    // Soft-delete the folder's children.
    if folder.room_id.is_some() {
        let _ = diesel::update(f::files.filter(f::parent_id.eq(&folder_id_param)))
            .set((
                f::is_trashed.eq(true),
                f::trashed_at.eq(Some(now.to_rfc3339())),
                f::updated_at.eq(now.to_rfc3339()),
            ))
            .execute(&mut conn);
    } else {
        let _ = diesel::update(
            f::files
                .filter(f::parent_id.eq(&folder_id_param))
                .filter(f::owner_id.eq(&uid)),
        )
        .set((
            f::is_trashed.eq(true),
            f::trashed_at.eq(Some(now.to_rfc3339())),
            f::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn);
    }

    // Soft-delete the folder itself.
    match diesel::update(f::files.filter(f::id.eq(&folder.id)))
        .set((
            f::is_trashed.eq(true),
            f::trashed_at.eq(Some(now.to_rfc3339())),
            f::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Folder not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Folder trashed" }),
            )));
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
// GET /folders/:id/path
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_folder_path(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let folder_id_param: String = req.param("id").unwrap_or_default();

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

    let mut path = Vec::new();
    let mut current_id: Option<String> = Some(folder_id_param.clone());

    while let Some(ref cid) = current_id {
        let folder = f::files
            .filter(f::id.eq(cid))
            .filter(f::is_folder.eq(true))
            .first::<File>(&mut conn)
            .optional();

        match folder {
            Ok(Some(row)) => {
                if authorize_file_access(&mut conn, &row, &uid, "viewer").is_err() {
                    break;
                }
                let fid = row.id;
                let fname = row.name;
                let pid = row.parent_id;
                path.push(serde_json::json!({"id": fid, "name": fname}));
                current_id = pid;
            }
            _ => break,
        }
    }

    path.reverse();

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(path)));
}
