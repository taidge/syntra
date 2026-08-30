use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{NewUserPresence, UserPresence};
use contrix_core::schema::user_presence::dsl as up;
use contrix_core::schema::users::dsl as u;

use crate::handlers::access::load_accessible_document;
use crate::middleware::auth::get_user_id;

/// Authorize the caller for `required_role` on the document, rendering the
/// appropriate error and returning `false` when access is denied.
fn ensure_document_access(
    conn: &mut diesel::SqliteConnection,
    document_id: &str,
    user_id: &str,
    required_role: &str,
    res: &mut Response,
) -> bool {
    match load_accessible_document(conn, document_id, user_id, required_role) {
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
// GET /documents/:doc_id/presence  (List active users)
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_presence(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("doc_id").unwrap_or_default();

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

    if !ensure_document_access(&mut conn, &doc_id_param, &uid, "viewer", res) {
        return;
    }

    match up::user_presence
        .inner_join(u::users.on(up::user_id.eq(u::id)))
        .filter(up::document_id.eq(&doc_id_param))
        .filter(up::is_active.eq(true))
        .select((
            up::id,
            up::document_id,
            up::user_id,
            u::username,
            u::display_name,
            up::session_id,
            up::cursor_position,
            up::is_active,
            up::last_seen_at,
            up::joined_at,
        ))
        .load::<(
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            bool,
            String,
            String,
        )>(&mut conn)
    {
        Ok(rows) => {
            let presence_list: Vec<serde_json::Value> = rows
                .into_iter()
                .map(
                    |(pid, did, puid, uname, dname, sid, cursor, active, seen, joined)| {
                        serde_json::json!({
                            "id": pid,
                            "document_id": did,
                            "user_id": puid,
                            "username": uname,
                            "display_name": dname,
                            "session_id": sid,
                            "cursor_position": cursor.as_deref()
                                .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok()),
                            "is_active": active,
                            "last_seen_at": seen,
                            "joined_at": joined,
                        })
                    },
                )
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(presence_list)));
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
// POST /documents/:doc_id/presence  (Join / update presence)
// ---------------------------------------------------------------------------

#[handler]
pub async fn join_presence(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("doc_id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct JoinPresenceRequest {
        session_id: Option<String>,
        cursor_position: Option<serde_json::Value>,
    }

    let body: JoinPresenceRequest = match req.parse_json().await {
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
    let cursor_str = body
        .cursor_position
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default());

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

    if !ensure_document_access(&mut conn, &doc_id_param, &uid, "viewer", res) {
        return;
    }

    // Check for existing presence for this user + document.
    let existing = up::user_presence
        .filter(up::document_id.eq(&doc_id_param))
        .filter(up::user_id.eq(&uid))
        .first::<UserPresence>(&mut conn)
        .optional();

    match existing {
        Ok(Some(record)) => {
            // Update existing presence record.
            if let Err(e) = diesel::update(up::user_presence.filter(up::id.eq(&record.id)))
                .set((
                    up::cursor_position.eq(&cursor_str),
                    up::is_active.eq(true),
                    up::last_seen_at.eq(now.to_rfc3339()),
                    up::session_id.eq(&body.session_id),
                ))
                .execute(&mut conn)
            {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Database(e.to_string()).to_response(),
                )));
                return;
            }

            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "id": record.id,
                "document_id": doc_id_param,
                "user_id": uid,
                "session_id": body.session_id,
                "cursor_position": body.cursor_position,
                "is_active": true,
                "last_seen_at": now.to_rfc3339(),
                "joined_at": record.joined_at,
            }))));
        }
        Ok(None) => {
            // Create new presence record.
            let presence_id = Uuid::new_v4().to_string();
            let new_presence = NewUserPresence {
                id: presence_id.clone(),
                document_id: doc_id_param.clone(),
                user_id: uid.clone(),
                session_id: body.session_id.clone(),
                cursor_position: cursor_str,
                is_active: true,
                last_seen_at: now.to_rfc3339(),
                joined_at: now.to_rfc3339(),
            };

            if let Err(e) = diesel::insert_into(up::user_presence)
                .values(&new_presence)
                .execute(&mut conn)
            {
                res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Database(e.to_string()).to_response(),
                )));
                return;
            }

            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "id": presence_id,
                "document_id": doc_id_param,
                "user_id": uid,
                "session_id": body.session_id,
                "cursor_position": body.cursor_position,
                "is_active": true,
                "last_seen_at": now.to_rfc3339(),
                "joined_at": now.to_rfc3339(),
            }))));
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
// DELETE /documents/:doc_id/presence  (Leave document)
// ---------------------------------------------------------------------------

#[handler]
pub async fn leave_presence(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("doc_id").unwrap_or_default();
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

    match diesel::update(
        up::user_presence
            .filter(up::document_id.eq(&doc_id_param))
            .filter(up::user_id.eq(&uid)),
    )
    .set((
        up::is_active.eq(false),
        up::last_seen_at.eq(now.to_rfc3339()),
    ))
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("No presence record found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Left document" }),
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
// POST /documents/:doc_id/presence/heartbeat  (Update last_seen_at)
// ---------------------------------------------------------------------------

#[handler]
pub async fn heartbeat(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("doc_id").unwrap_or_default();
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

    match diesel::update(
        up::user_presence
            .filter(up::document_id.eq(&doc_id_param))
            .filter(up::user_id.eq(&uid))
            .filter(up::is_active.eq(true)),
    )
    .set(up::last_seen_at.eq(now.to_rfc3339()))
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("No active presence record found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "message": "Heartbeat received",
                "last_seen_at": now.to_rfc3339(),
            }))));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
