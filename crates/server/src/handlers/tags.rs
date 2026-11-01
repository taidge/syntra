use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{
    ApiResponse, CreateTagRequest, TagFileRequest, TagResponse, UpdateTagRequest,
};
use contrix_core::error::AppError;
use contrix_core::models::{NewFileTag, NewTag, Tag};
use contrix_core::schema::file_tags::dsl as ft;
use contrix_core::schema::tags::dsl as t;

use crate::handlers::access::load_accessible_file;
use crate::middleware::auth::get_user_id;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tag_model_to_response(row: Tag) -> TagResponse {
    TagResponse {
        id: row.id.parse().unwrap_or_default(),
        name: row.name,
        color: row.color,
        created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
    }
}

/// Verify the tag belongs to the user. Renders an error response and returns
/// false when it does not.
async fn require_owned_tag(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    tag_id: &str,
    uid: &str,
    res: &mut Response,
) -> bool {
    match t::tags
        .filter(t::id.eq(tag_id))
        .filter(t::owner_id.eq(uid))
        .select(t::id)
        .first::<String>(conn)
        .await
        .optional()
    {
        Ok(Some(_)) => true,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Tag not found".into()).to_response(),
            )));
            false
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            false
        }
    }
}

/// Verify the user can access the file with `required_role`. Renders an error
/// response and returns false when they cannot.
async fn require_accessible_file(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    file_id: &str,
    uid: &str,
    required_role: &str,
    res: &mut Response,
) -> bool {
    match load_accessible_file(conn, file_id, uid, required_role).await {
        Ok(Some(_)) => true,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
            false
        }
        Err(e) => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            false
        }
    }
}

// ---------------------------------------------------------------------------
// POST /tags
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_tag(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: CreateTagRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
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

    let tag_id_val = Uuid::new_v4();
    let now = Utc::now();

    let new_tag = NewTag {
        id: tag_id_val.to_string(),
        name: body.name.clone(),
        color: body.color.clone(),
        owner_id: uid.clone(),
        created_at: now.to_rfc3339(),
    };

    match diesel::insert_into(t::tags)
        .values(&new_tag)
        .execute(&mut conn)
        .await
    {
        Ok(_) => {
            let resp = TagResponse {
                id: tag_id_val,
                name: body.name,
                color: body.color,
                created_at: now,
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
// GET /tags
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_tags(depot: &mut Depot, res: &mut Response) {
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

    match t::tags
        .filter(t::owner_id.eq(&uid))
        .order(t::name.asc())
        .load::<Tag>(&mut conn)
        .await
    {
        Ok(rows) => {
            let tag_list: Vec<TagResponse> = rows.into_iter().map(tag_model_to_response).collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(tag_list)));
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
// PUT /tags/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_tag(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let tag_id_param: String = req.param("id").unwrap_or_default();

    let body: UpdateTagRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
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

    let result = match (&body.name, &body.color) {
        (Some(name), Some(color)) => {
            diesel::update(
                t::tags
                    .filter(t::id.eq(&tag_id_param))
                    .filter(t::owner_id.eq(&uid)),
            )
            .set((t::name.eq(name), t::color.eq(Some(color))))
            .execute(&mut conn)
            .await
        }
        (Some(name), None) => {
            diesel::update(
                t::tags
                    .filter(t::id.eq(&tag_id_param))
                    .filter(t::owner_id.eq(&uid)),
            )
            .set(t::name.eq(name))
            .execute(&mut conn)
            .await
        }
        (None, Some(color)) => {
            diesel::update(
                t::tags
                    .filter(t::id.eq(&tag_id_param))
                    .filter(t::owner_id.eq(&uid)),
            )
            .set(t::color.eq(Some(color)))
            .execute(&mut conn)
            .await
        }
        (None, None) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("No fields to update".into()).to_response(),
            )));
            return;
        }
    };

    match result {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Tag not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Tag updated" }),
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
// DELETE /tags/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_tag(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let tag_id_param: String = req.param("id").unwrap_or_default();

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

    // Delete associated file_tags first.
    let _ = diesel::delete(ft::file_tags.filter(ft::tag_id.eq(&tag_id_param)))
        .execute(&mut conn)
        .await;

    match diesel::delete(
        t::tags
            .filter(t::id.eq(&tag_id_param))
            .filter(t::owner_id.eq(&uid)),
    )
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Tag not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Tag deleted" }),
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
// POST /tags/assign
// ---------------------------------------------------------------------------

#[handler]
pub async fn assign_tag(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: TagFileRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
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

    if !require_owned_tag(&mut conn, &body.tag_id.to_string(), &uid, res).await {
        return;
    }
    if !require_accessible_file(&mut conn, &body.file_id.to_string(), &uid, "editor", res).await {
        return;
    }

    let file_tag_id = Uuid::new_v4();
    let now = Utc::now();

    let new_file_tag = NewFileTag {
        id: file_tag_id.to_string(),
        file_id: body.file_id.to_string(),
        tag_id: body.tag_id.to_string(),
        created_at: now.to_rfc3339(),
    };

    match diesel::insert_into(ft::file_tags)
        .values(&new_file_tag)
        .execute(&mut conn)
        .await
    {
        Ok(_) => {
            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Tag assigned" }),
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
// DELETE /tags/assign
// ---------------------------------------------------------------------------

#[handler]
pub async fn unassign_tag(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: Option<String> = req.query("file_id");
    let tag_id_param: Option<String> = req.query("tag_id");

    let (file_id_str, tag_id_str) = match (file_id_param, tag_id_param) {
        (Some(fid), Some(tid)) => (fid, tid),
        _ => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("file_id and tag_id query params required".into())
                    .to_response(),
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

    if !require_owned_tag(&mut conn, &tag_id_str, &uid, res).await {
        return;
    }
    if !require_accessible_file(&mut conn, &file_id_str, &uid, "editor", res).await {
        return;
    }

    match diesel::delete(
        ft::file_tags
            .filter(ft::file_id.eq(&file_id_str))
            .filter(ft::tag_id.eq(&tag_id_str)),
    )
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Tag assignment not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Tag unassigned" }),
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
// GET /files/:file_id/tags
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_file_tags(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("file_id").unwrap_or_default();

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

    if !require_accessible_file(&mut conn, &file_id_param, &uid, "viewer", res).await {
        return;
    }

    match ft::file_tags
        .inner_join(t::tags.on(ft::tag_id.eq(t::id)))
        .filter(ft::file_id.eq(&file_id_param))
        .select((t::id, t::name, t::color, t::created_at))
        .load::<(String, String, Option<String>, String)>(&mut conn)
        .await
    {
        Ok(rows) => {
            let tags: Vec<TagResponse> = rows
                .into_iter()
                .map(|(tid, tname, tcolor, cat)| TagResponse {
                    id: tid.parse().unwrap_or_default(),
                    name: tname,
                    color: tcolor,
                    created_at: cat.parse().unwrap_or_else(|_| Utc::now()),
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(tags)));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
