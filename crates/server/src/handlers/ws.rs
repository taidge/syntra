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
    let state = crate::require_app_state!(depot, res);
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

    // Authorize: the user must have editor access to the underlying file before
    // they may join the realtime session (read others' edits / write their own).
    {
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
        match crate::handlers::access::load_accessible_document(
            &mut conn,
            &document_id_str,
            &user_id_str,
            "editor",
        )
        .await
        {
            Ok(Some(_)) => {}
            Ok(None) => {
                res.status_code(StatusCode::NOT_FOUND);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::NotFound("Document not found".into()).to_response(),
                )));
                return;
            }
            Err(e) => {
                let code = match &e {
                    AppError::Forbidden(_) => StatusCode::FORBIDDEN,
                    AppError::NotFound(_) => StatusCode::NOT_FOUND,
                    _ => StatusCode::INTERNAL_SERVER_ERROR,
                };
                res.status_code(code);
                res.render(Json(ApiResponse::<()>::err(e.to_response())));
                return;
            }
        }
    }

    // Fetch username for the collab session.
    let username = {
        use contrix_core::schema::users::dsl as u;
        use diesel::prelude::*;
        use horme::RunQueryDsl;
        let mut conn = state.db.clone();
        let uname = {
            u::users
                .filter(u::id.eq(&user_id_str))
                .select(u::username)
                .first::<String>(&mut conn)
                .await
                .ok()
        };
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

    // Establish both the version baseline and durable document snapshot.
    // Reconnecting editors must not keep a stale pre-disconnect HTTP snapshot.
    let version = collab.session_version(document_id).await.unwrap_or(0);
    if !send_snapshot(&mut ws, &state, document_id, version).await {
        collab.leave_session(document_id, user_id).await;
        return;
    }

    loop {
        tokio::select! {
                    // Broadcast channel -> WebSocket (send to client).
                    msg = rx.recv() => {
                        match msg {
                            Ok(collab_msg) => {
                                if let Ok(text) = serde_json::to_string(&collab_msg)
                                    && ws.send(Message::text(text)).await.is_err() {
                                        break;
                                    }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                let version = collab.session_version(document_id).await.unwrap_or(0);
                                if !send_snapshot(&mut ws, &state, document_id, version).await { break; }
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
                                if let Ok(text) = msg.as_str()
                                    && let Ok(collab_msg) = serde_json::from_str::<CollabMessage>(text) {
                                        match collab_msg {
                                            CollabMessage::Operation(mut op) => {
                                                // Never trust the client-supplied author;
                                                // bind the operation to the authenticated user.
                                                op.user_id = user_id;
                                                match collab.apply_operation(document_id, op).await {
                                                    Ok((version, applied_op)) => {
                                                        // Persist the transformed operation that the
                                                        // server actually broadcast/applied, not the
                                                        // client's pre-transform payload.
                                                        if let Ok(mut conn) = state.db.get() {
                                                            use contrix_core::models::NewOperationLogEntry;
                                                            use contrix_core::schema::operation_log::dsl as ol;

        use horme::RunQueryDsl;
                                                            let entry = NewOperationLogEntry {
                                                                id: Uuid::new_v4().to_string(),
                                                                document_id: document_id.to_string(),
                                                                user_id: applied_op.user_id.to_string(),
                                                                op_type: serde_json::to_string(&applied_op.op_type).unwrap_or_default(),
                                                                op_data: serde_json::to_string(&applied_op).unwrap_or_default(),
                                                                version: version as i32,
                                                                created_at: chrono::Utc::now().to_rfc3339(),
                                                            };
                                                            if let Err(error) = diesel::insert_into(ol::operation_log)
                                                                .values(&entry)
                                                                .execute(&mut conn).await
                                                            {
                                                                tracing::error!(
                                                                    document_id = %document_id,
                                                                    version,
                                                                    "failed to persist collaboration operation: {error}"
                                                                );
                                                            }
                                                        }
                                                    }
                                                    Err(error) => {
                                                        let response = CollabMessage::Error {
                                                            message: error,
                                                        };
                                                        if let Ok(text) = serde_json::to_string(&response)
                                                            && ws.send(Message::text(text)).await.is_err()
                                                        {
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                            CollabMessage::CursorUpdate(mut cursor) => {
                                                // Bind the cursor to the authenticated
                                                // user so peers can't be impersonated.
                                                cursor.user_id = user_id;
                                                collab.update_cursor(document_id, cursor).await;
                                            }
                                            _ => {}
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

async fn send_snapshot(
    ws: &mut WebSocket,
    state: &AppState,
    document_id: Uuid,
    version: u64,
) -> bool {
    let (message, available) = match durable_snapshot(&state.db, document_id).await {
        Ok(content) => (CollabMessage::DocumentState { content, version }, true),
        Err(error) => {
            tracing::warn!(%document_id, %error, "Collaboration snapshot unavailable");
            (
                CollabMessage::Error {
                    message: "Document temporarily unavailable; reconnect to retry".into(),
                },
                false,
            )
        }
    };
    let Ok(text) = serde_json::to_string(&message) else {
        return false;
    };
    let sent = ws.send(Message::text(text)).await.is_ok();
    available && sent
}

async fn durable_snapshot(
    database: &crate::state::DbPool,
    document_id: Uuid,
) -> Result<serde_json::Value, contrix_core::error::AppError> {
    use contrix_core::schema::documents::dsl as documents;
    use diesel::prelude::*;
    use horme::RunQueryDsl;
    let mut conn = database.clone();
    let content = documents::documents
        .filter(documents::id.eq(document_id.to_string()))
        .select(documents::content)
        .first::<Option<String>>(&mut conn)
        .await?;
    match content {
        Some(content) => serde_json::from_str(&content).map_err(|error| {
            contrix_core::error::AppError::Internal(format!("Invalid stored document: {error}"))
        }),
        None => Ok(serde_json::Value::Null),
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use horme::{GetConnection, RunQueryDsl};

    #[tokio::test]
    async fn database_failure_never_becomes_an_empty_document_snapshot() {
        let directory = tempfile::tempdir().unwrap();
        let database = crate::state::DbPool::with_timeouts(
            &directory.path().join("snapshot.sqlite").to_string_lossy(),
            1,
            std::time::Duration::from_millis(250),
            std::time::Duration::from_secs(3),
            std::time::Duration::from_millis(30),
        )
        .unwrap();
        let mut provider = database.clone();
        diesel::sql_query("CREATE TABLE documents(id TEXT PRIMARY KEY, content TEXT)")
            .execute(&mut provider)
            .await
            .unwrap();
        let id = Uuid::new_v4();
        diesel::sql_query("INSERT INTO documents(id, content) VALUES (?, ?)")
            .bind::<diesel::sql_types::Text, _>(id.to_string())
            .bind::<diesel::sql_types::Text, _>(r#"{"cells":{"A1":"preserve"}}"#)
            .execute(&mut provider)
            .await
            .unwrap();
        let expected = serde_json::json!({"cells":{"A1":"preserve"}});
        assert_eq!(durable_snapshot(&database, id).await.unwrap(), expected);
        let lease = provider.get_mut_conn().await.unwrap();
        assert!(durable_snapshot(&database, id).await.is_err());
        drop(lease);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while database.state().admitted_operations != 0 {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("cancelled checkout and lease cleanup retire");
        assert_eq!(durable_snapshot(&database, id).await.unwrap(), expected);
        assert!(durable_snapshot(&database, Uuid::new_v4()).await.is_err());
    }
}
