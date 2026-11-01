use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, CommentResponse, CreateCommentRequest};
use contrix_core::error::AppError;
use contrix_core::models::NewComment;
use contrix_core::schema::comments::dsl as c;
use contrix_core::schema::users::dsl as u;

use crate::handlers::access::load_accessible_document;
use crate::middleware::auth::get_user_id;

/// Authorize the caller for `required_role` on the document, rendering the
/// appropriate error response and returning `false` when access is denied.
async fn ensure_document_access(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    document_id: &str,
    user_id: &str,
    required_role: &str,
    res: &mut Response,
) -> bool {
    match load_accessible_document(conn, document_id, user_id, required_role).await {
        Ok(Some(_)) => true,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            false
        }
        Err(e) => {
            let code = match &e {
                AppError::Forbidden(_) => StatusCode::FORBIDDEN,
                AppError::NotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            res.status_code(code);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            false
        }
    }
}

// ---------------------------------------------------------------------------
// GET /documents/:id/comments
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_comments(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

    let mut conn = match state.db.get() {
        Ok(conn_val) => conn_val,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if !ensure_document_access(&mut conn, &doc_id_param, &uid, "viewer", res).await {
        return;
    }

    match c::comments
        .inner_join(u::users.on(c::user_id.eq(u::id)))
        .filter(c::document_id.eq(&doc_id_param))
        .order(c::created_at.asc())
        .select((
            c::id,
            c::document_id,
            c::user_id,
            u::username,
            u::display_name,
            u::avatar_url,
            c::content,
            c::parent_id,
            c::is_resolved,
            c::position,
            c::created_at,
            c::updated_at,
        ))
        .load::<(
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
            bool,
            Option<String>,
            String,
            String,
        )>(&mut conn)
        .await
    {
        Ok(rows) => {
            let comments: Vec<CommentResponse> = rows
                .into_iter()
                .map(
                    |(cid, did, cuid, uname, dname, avatar, cnt, pid, resolved, pos, cat, uat)| {
                        CommentResponse {
                            id: cid.parse().unwrap_or_default(),
                            document_id: did.parse().unwrap_or_default(),
                            user_id: cuid.parse().unwrap_or_default(),
                            username: uname,
                            display_name: dname,
                            avatar_url: avatar,
                            content: cnt,
                            parent_id: pid.as_deref().and_then(|v| v.parse().ok()),
                            is_resolved: resolved,
                            position: pos.as_deref().and_then(|v| serde_json::from_str(v).ok()),
                            created_at: cat.parse().unwrap_or_else(|_| Utc::now()),
                            updated_at: uat.parse().unwrap_or_else(|_| Utc::now()),
                        }
                    },
                )
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(comments)));
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
// POST /documents/:id/comments
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_comment(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

    let body: CreateCommentRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let comment_id_val = Uuid::new_v4();
    let now = Utc::now();
    let parent_id_str = body.parent_id.map(|pid| pid.to_string());
    let position_str = body
        .position
        .as_ref()
        .map(|p| serde_json::to_string(p).unwrap_or_default());

    let mut conn = match state.db.get() {
        Ok(conn_val) => conn_val,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if !ensure_document_access(&mut conn, &doc_id_param, &uid, "viewer", res).await {
        return;
    }

    let new_comment = NewComment {
        id: comment_id_val.to_string(),
        document_id: doc_id_param.clone(),
        user_id: uid.clone(),
        content: body.content.clone(),
        parent_id: parent_id_str,
        is_resolved: false,
        position: position_str,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };

    if let Err(e) = diesel::insert_into(c::comments)
        .values(&new_comment)
        .execute(&mut conn)
        .await
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    crate::services::WebhookService::fire_event(
        &state.db,
        "comment.created",
        &serde_json::json!({
            "id": comment_id_val.to_string(),
            "document_id": &doc_id_param,
            "user_id": &uid,
            "content": &body.content,
            "parent_id": body.parent_id.map(|p| p.to_string()),
            "created_at": now.to_rfc3339(),
        }),
    )
    .await;

    {
        use contrix_core::schema::documents::dsl as d;
        use contrix_core::schema::files::dsl as f;
        let owner: Option<String> = d::documents
            .inner_join(f::files.on(d::file_id.eq(f::id)))
            .filter(d::id.eq(&doc_id_param))
            .select(f::owner_id)
            .first::<String>(&mut conn)
            .await
            .optional()
            .unwrap_or(None);
        if let Some(owner_id) = owner
            && owner_id != uid
        {
            crate::services::NotificationService::create(
                &state.db,
                &state.notifications,
                &owner_id,
                "comment",
                "New comment",
                "Someone commented on your document",
                Some(&format!("/documents/{doc_id_param}")),
            )
            .await;
        }
    }

    // Fetch username for response.
    let (uname, dname, avatar) = u::users
        .filter(u::id.eq(&uid))
        .select((u::username, u::display_name, u::avatar_url))
        .first::<(String, Option<String>, Option<String>)>(&mut conn)
        .await
        .unwrap_or(("unknown".to_string(), None, None));

    let resp = CommentResponse {
        id: comment_id_val,
        document_id: doc_id_param.parse().unwrap_or_default(),
        user_id: uid.parse().unwrap_or_default(),
        username: uname,
        display_name: dname,
        avatar_url: avatar,
        content: body.content,
        parent_id: body.parent_id,
        is_resolved: false,
        position: body.position,
        created_at: now,
        updated_at: now,
    };
    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(resp)));
}

// ---------------------------------------------------------------------------
// PUT /comments/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_comment(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let comment_id_param: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct UpdateCommentReq {
        content: String,
    }

    let body: UpdateCommentReq = match req.parse_json().await {
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
        Ok(conn_val) => conn_val,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    match diesel::update(
        c::comments
            .filter(c::id.eq(&comment_id_param))
            .filter(c::user_id.eq(&uid)),
    )
    .set((
        c::content.eq(&body.content),
        c::updated_at.eq(now.to_rfc3339()),
    ))
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Comment not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Comment updated" }),
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
// DELETE /comments/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_comment(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let comment_id_param: String = req.param("id").unwrap_or_default();

    let mut conn = match state.db.get() {
        Ok(conn_val) => conn_val,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    match diesel::delete(
        c::comments
            .filter(c::id.eq(&comment_id_param))
            .filter(c::user_id.eq(&uid)),
    )
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Comment not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Comment deleted" }),
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
// POST /comments/:id/resolve
// ---------------------------------------------------------------------------

#[handler]
pub async fn resolve_comment(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let comment_id_param: String = req.param("id").unwrap_or_default();
    let now = Utc::now();

    let mut conn = match state.db.get() {
        Ok(conn_val) => conn_val,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    // Resolve the comment's owning document and authorize the caller, so a
    // user cannot mark comments on documents they cannot access.
    let comment_doc_id: Option<String> = c::comments
        .filter(c::id.eq(&comment_id_param))
        .select(c::document_id)
        .first::<String>(&mut conn)
        .await
        .optional()
        .unwrap_or(None);
    let comment_doc_id = match comment_doc_id {
        Some(id) => id,
        None => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Comment not found".into()).to_response(),
            )));
            return;
        }
    };
    if !ensure_document_access(&mut conn, &comment_doc_id, &uid, "viewer", res).await {
        return;
    }

    match diesel::update(c::comments.filter(c::id.eq(&comment_id_param)))
        .set((c::is_resolved.eq(true), c::updated_at.eq(now.to_rfc3339())))
        .execute(&mut conn)
        .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Comment not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Comment resolved" }),
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
