use std::sync::Arc;

use salvo::prelude::*;
use salvo::websocket::{Message, WebSocket, WebSocketUpgrade};
use tokio::sync::broadcast;
use uuid::Uuid;

use contrix_collab::CollabMessage;
use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /ws/collab/:document_id  (WebSocket upgrade)
// ---------------------------------------------------------------------------

#[handler]
pub async fn collab_ws(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let user_id_str = match get_user_id(depot) {
        Some(id) => id,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let document_id_str: String = req.param("document_id").unwrap_or_default();

    let user_id: Uuid = match user_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Invalid user id".into()).to_response(),
            )));
            return;
        }
    };

    let document_id: Uuid = match document_id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("Invalid document id".into()).to_response(),
            )));
            return;
        }
    };

    // Fetch username for the collab session.
    let username = {
        use contrix_core::schema::users::dsl as u;
        use diesel::prelude::*;
        let uname = state.db.get().ok().and_then(|mut conn| {
            u::users
                .filter(u::id.eq(&user_id_str))
                .select(u::username)
                .first::<String>(&mut conn)
                .ok()
        });
        uname.unwrap_or_else(|| "anonymous".to_string())
    };

    let collab = state.collab.clone();
    let state_clone = state.clone();

    let _ = WebSocketUpgrade::new()
        .upgrade(req, res, move |ws| {
            handle_ws(ws, collab, state_clone, document_id, user_id, username)
        })
        .await;
}

async fn handle_ws(
    mut ws: WebSocket,
    collab: contrix_collab::CollabManager,
    state: Arc<AppState>,
    document_id: Uuid,
    user_id: Uuid,
    username: String,
) {
    let mut rx: broadcast::Receiver<CollabMessage> =
        match collab.join_session(document_id, user_id, username).await {
            Ok(r) => r,
            Err(_) => return,
        };

    loop {
        tokio::select! {
            // Broadcast channel -> WebSocket (send to client).
            msg = rx.recv() => {
                match msg {
                    Ok(collab_msg) => {
                        if let Ok(text) = serde_json::to_string(&collab_msg) {
                            if ws.send(Message::text(text)).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Missed some messages, continue.
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            // WebSocket -> broadcast channel (receive from client).
            msg = ws.recv() => {
                match msg {
                    Some(Ok(msg)) => {
                        if let Ok(text) = msg.as_str() {
                            if let Ok(collab_msg) = serde_json::from_str::<CollabMessage>(text) {
                                match collab_msg {
                                    CollabMessage::Operation(op) => {
                                        let version = collab.apply_operation(document_id, op.clone()).await;
                                        // Persist to DB
                                        if let Ok(ver) = &version {
                                            if let Ok(mut conn) = state.db.get() {
                                                use diesel::prelude::*;
                                                use contrix_core::schema::operation_log::dsl as ol;
                                                use contrix_core::models::NewOperationLogEntry;
                                                let entry = NewOperationLogEntry {
                                                    id: Uuid::new_v4().to_string(),
                                                    document_id: document_id.to_string(),
                                                    user_id: op.user_id.to_string(),
                                                    op_type: serde_json::to_string(&op.op_type).unwrap_or_default(),
                                                    op_data: serde_json::to_string(&op).unwrap_or_default(),
                                                    version: *ver as i32,
                                                    created_at: chrono::Utc::now().to_rfc3339(),
                                                };
                                                let _ = diesel::insert_into(ol::operation_log).values(&entry).execute(&mut conn);
                                            }
                                        }
                                    }
                                    CollabMessage::CursorUpdate(cursor) => {
                                        collab.update_cursor(document_id, cursor).await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Some(Err(_)) | None => {
                        // Client disconnected.
                        break;
                    }
                }
            }
        }
    }

    // Clean up: leave session.
    collab.leave_session(document_id, user_id).await;
}
