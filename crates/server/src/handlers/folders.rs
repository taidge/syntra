use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use syntra_core::dto::{ApiResponse, CreateFolderRequest, FileResponse};
use syntra_core::error::AppError;
use syntra_core::models::{File, NewFile};
use syntra_core::schema::documents::dsl as d;
use syntra_core::schema::files::dsl as f;

use crate::handlers::access::{
    authorize_file_access, check_room_access, load_accessible_folder, load_accessible_target_folder,
};
use crate::middleware::auth::get_user_id;

fn render_error(res: &mut Response, error: AppError) {
    res.status_code(
        StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
    );
    res.render(Json(ApiResponse::<()>::err(error.to_response())));
}

fn descendant_ids(conn: &mut SqliteConnection, file_id: &str) -> Result<Vec<String>, AppError> {
    #[derive(diesel::QueryableByName)]
    struct Descendant {
        #[diesel(sql_type = diesel::sql_types::Text)]
        id: String,
    }
    // UNION rather than UNION ALL also terminates on a corrupt legacy cycle.
    let rows = diesel::RunQueryDsl::load::<Descendant>(diesel::sql_query(
        "WITH RECURSIVE subtree(id) AS (SELECT ? UNION SELECT files.id FROM files JOIN subtree ON files.parent_id = subtree.id) SELECT id FROM subtree"
    ).bind::<diesel::sql_types::Text, _>(file_id), conn)?;
    Ok(rows.into_iter().map(|row| row.id).collect())
}

fn load_existing_file(conn: &mut SqliteConnection, file_id: &str) -> Result<File, AppError> {
    diesel::OptionalExtension::optional(diesel::RunQueryDsl::first::<File>(
        f::files.find(file_id),
        conn,
    ))?
    .ok_or_else(|| AppError::NotFound("File not found".into()))
}

pub async fn trash_file_tree(
    db: &crate::state::DbPool,
    file_id: &str,
    user: &str,
    credential: &crate::middleware::auth::AuthenticatedCredential,
    method: &salvo::http::Method,
    request_path: &str,
) -> Result<usize, AppError> {
    let (file_id, user) = (file_id.to_owned(), user.to_owned());
    let (credential, method, request_path) =
        (credential.clone(), method.clone(), request_path.to_owned());
    db.run(move |conn| {
        conn.immediate_transaction::<_, AppError, _>(|conn| {
            crate::middleware::auth::validate_credential_in_transaction(
                conn,
                &credential,
                &method,
                &request_path,
            )?;
            let root = load_existing_file(conn, &file_id)?;
            crate::services::persistence::authorize(conn, &root, &user, "editor")?;
            let ids = descendant_ids(conn, &file_id)?;
            let mut active_ids = Vec::new();
            for id in &ids {
                let file = load_existing_file(conn, id)?;
                crate::services::persistence::authorize_including_trashed(
                    conn, &file, &user, "editor",
                )?;
                crate::services::persistence::check_file_lock(conn, id, &user, None)?;
                if !file.is_trashed {
                    active_ids.push(file.id);
                }
            }
            let batch = Uuid::new_v4().to_string();
            for id in &active_ids {
                diesel::RunQueryDsl::execute(
                    diesel::sql_query(
                        "INSERT INTO trash_batch_members(file_id,batch_id) VALUES (?,?) \
                         ON CONFLICT(file_id) DO UPDATE SET batch_id=excluded.batch_id",
                    )
                    .bind::<diesel::sql_types::Text, _>(id)
                    .bind::<diesel::sql_types::Text, _>(&batch),
                    conn,
                )?;
            }
            let now = Utc::now().to_rfc3339();
            Ok(diesel::RunQueryDsl::execute(
                diesel::update(
                    f::files
                        .filter(f::id.eq_any(active_ids))
                        .filter(f::is_trashed.eq(false)),
                )
                .set((
                    f::is_trashed.eq(true),
                    f::trashed_at.eq(Some(&now)),
                    f::updated_at.eq(&now),
                )),
                conn,
            )?)
        })
    })
    .await
}

pub async fn move_file_tree(
    db: &crate::state::DbPool,
    file_id: &str,
    target_id: &str,
    user: &str,
    credential: &crate::middleware::auth::AuthenticatedCredential,
    method: &salvo::http::Method,
    request_path: &str,
) -> Result<usize, AppError> {
    let (file_id, target_id, user) = (file_id.to_owned(), target_id.to_owned(), user.to_owned());
    let (credential, method, request_path) =
        (credential.clone(), method.clone(), request_path.to_owned());
    db.run(move |conn| {
        conn.immediate_transaction::<_, AppError, _>(|conn| {
            crate::middleware::auth::validate_credential_in_transaction(
                conn,
                &credential,
                &method,
                &request_path,
            )?;
            let source = load_existing_file(conn, &file_id)?;
            let target = load_existing_file(conn, &target_id)?;
            if !target.is_folder || target.is_trashed {
                return Err(AppError::Validation(
                    "Target must be an active folder".into(),
                ));
            }
            crate::services::persistence::authorize(conn, &target, &user, "editor")?;
            let role = if source.room_id == target.room_id {
                "editor"
            } else {
                "admin"
            };
            crate::services::persistence::authorize(conn, &source, &user, role)?;
            let ids = descendant_ids(conn, &file_id)?;
            if ids.contains(&target_id) {
                return Err(AppError::Validation(
                    "Cannot move a folder into itself or its descendants".into(),
                ));
            }
            for id in &ids {
                let file = load_existing_file(conn, id)?;
                crate::services::persistence::authorize_including_trashed(
                    conn, &file, &user, role,
                )?;
                crate::services::persistence::check_file_lock(conn, id, &user, None)?;
                if target.room_id.is_none() && file.owner_id != user {
                    return Err(AppError::Forbidden(
                        "Cannot move another owner's content into private storage".into(),
                    ));
                }
            }
            ensure_available_name(
                conn,
                &source.owner_id,
                Some(&target_id),
                target.room_id.as_deref(),
                &source.name,
                Some(&file_id),
            )?;
            let now = Utc::now().to_rfc3339();
            diesel::RunQueryDsl::execute(
                diesel::update(f::files.filter(f::id.eq_any(&ids)))
                    .set((f::room_id.eq(target.room_id), f::updated_at.eq(&now))),
                conn,
            )?;
            Ok(diesel::RunQueryDsl::execute(
                diesel::update(f::files.find(file_id)).set(f::parent_id.eq(Some(target_id))),
                conn,
            )?)
        })
    })
    .await
}

pub(crate) fn ensure_available_name(
    conn: &mut SqliteConnection,
    owner: &str,
    parent: Option<&str>,
    room: Option<&str>,
    name: &str,
    exclude: Option<&str>,
) -> Result<(), AppError> {
    if name.trim().is_empty() || name.chars().count() > 255 {
        return Err(AppError::Validation(
            "A name of 1–255 characters is required".into(),
        ));
    }
    let mut query = f::files
        .into_boxed()
        .filter(f::name.eq(name))
        .filter(f::is_trashed.eq(false));
    // All members share a room/folder namespace; private roots are per-owner.
    query = if let Some(parent) = parent {
        query.filter(f::parent_id.eq(parent))
    } else {
        query.filter(f::parent_id.is_null())
    };
    query = if let Some(room) = room {
        query.filter(f::room_id.eq(room))
    } else {
        query
            .filter(f::room_id.is_null())
            .filter(f::owner_id.eq(owner))
    };
    if let Some(exclude) = exclude {
        query = query.filter(f::id.ne(exclude));
    }
    if diesel::RunQueryDsl::get_result::<i64>(query.count(), conn)? > 0 {
        return Err(AppError::Conflict(
            "A sibling item with that name already exists".into(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// POST /folders
// ---------------------------------------------------------------------------

async fn document_id_for_file(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    file_id: &str,
) -> Option<Uuid> {
    d::documents
        .filter(d::file_id.eq(file_id))
        .select(d::id)
        .first::<String>(conn)
        .await
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
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
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
        render_error(res, error);
        return;
    }

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
        match load_accessible_target_folder(&mut conn, parent_id, &uid, "editor").await {
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
        match check_room_access(&mut conn, &uid, room_id, "editor").await {
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

    let inserted = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                crate::middleware::auth::validate_credential_in_transaction(
                    conn,
                    &credential,
                    &method,
                    &request_path,
                )?;
                // Revalidate the target under the same write lock as insertion.
                if let Some(parent) = &new_file.parent_id {
                    let parent = diesel::RunQueryDsl::first::<File>(f::files.find(parent), conn)?;
                    if !parent.is_folder || parent.room_id != new_file.room_id {
                        return Err(AppError::Conflict(
                            "Parent folder changed; reload before creating".into(),
                        ));
                    }
                    crate::services::persistence::authorize(
                        conn,
                        &parent,
                        &new_file.owner_id,
                        "editor",
                    )?;
                } else if new_file.room_id.is_some() {
                    crate::services::persistence::authorize_room(
                        conn,
                        new_file.room_id.as_deref().expect("room checked"),
                        &new_file.owner_id,
                        "editor",
                    )?;
                }
                ensure_available_name(
                    conn,
                    &new_file.owner_id,
                    new_file.parent_id.as_deref(),
                    new_file.room_id.as_deref(),
                    &new_file.name,
                    None,
                )?;
                Ok(diesel::RunQueryDsl::execute(
                    diesel::insert_into(f::files).values(&new_file),
                    conn,
                )?)
            })
        })
        .await;
    match inserted {
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
        Err(error) => render_error(res, error),
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

    match load_accessible_folder(&mut conn, &folder_id_param, &uid, "viewer").await {
        Ok(Some(row)) => {
            let shared_with_count =
                crate::handlers::access::count_shares_for_file(&mut conn, &row.id).await;
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

    let folder = match load_accessible_folder(&mut conn, &folder_id_param, &uid, "viewer").await {
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

    match query.load::<File>(&mut conn).await {
        Ok(rows) => {
            let mut children = Vec::with_capacity(rows.len());
            for row in rows {
                if authorize_file_access(&mut conn, &row, &uid, "viewer")
                    .await
                    .is_err()
                {
                    continue;
                }
                let document_id = document_id_for_file(&mut conn, &row.id).await;
                let shared_with_count =
                    crate::handlers::access::count_shares_for_file(&mut conn, &row.id).await;
                children.push(FileResponse {
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
                });
            }
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
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
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
        render_error(res, error);
        return;
    }

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

    let folder = match load_accessible_folder(&mut conn, &folder_id_param, &uid, "editor").await {
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

    let actor = uid.clone();
    let renamed = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                crate::middleware::auth::validate_credential_in_transaction(
                    conn,
                    &credential,
                    &method,
                    &request_path,
                )?;
                let current = diesel::RunQueryDsl::first::<File>(f::files.find(&folder.id), conn)?;
                crate::services::persistence::authorize(conn, &current, &actor, "editor")?;
                ensure_available_name(
                    conn,
                    &current.owner_id,
                    current.parent_id.as_deref(),
                    current.room_id.as_deref(),
                    &body.name,
                    Some(&current.id),
                )?;
                Ok(diesel::RunQueryDsl::execute(
                    diesel::update(f::files.find(&current.id))
                        .set((f::name.eq(&body.name), f::updated_at.eq(now.to_rfc3339()))),
                    conn,
                )?)
            })
        })
        .await;
    match renamed {
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
        Err(error) => render_error(res, error),
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
    let Some(credential) = crate::middleware::auth::get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
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
        render_error(res, error);
        return;
    }

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

    let folder = match load_accessible_folder(&mut conn, &folder_id_param, &uid, "editor").await {
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

    match trash_file_tree(
        &state.db,
        &folder.id,
        &uid,
        &credential,
        &method,
        &request_path,
    )
    .await
    {
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({"message":"Folder trashed"}),
            )));
        }
        Err(error) => render_error(res, error),
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
    let mut visited = std::collections::HashSet::new();

    while let Some(ref cid) = current_id {
        if !visited.insert(cid.clone()) || visited.len() > 1024 {
            render_error(
                res,
                AppError::Conflict("Folder hierarchy contains a cycle or is too deep".into()),
            );
            return;
        }
        let folder = f::files
            .filter(f::id.eq(cid))
            .filter(f::is_folder.eq(true))
            .first::<File>(&mut conn)
            .await
            .optional();

        match folder {
            Ok(Some(row)) => {
                if authorize_file_access(&mut conn, &row, &uid, "viewer")
                    .await
                    .is_err()
                {
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

    if path.is_empty() {
        render_error(res, AppError::NotFound("Folder not found".into()));
        return;
    }

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(path)));
}
