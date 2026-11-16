use crate::middleware::auth::{AuthenticatedCredential, get_credential};
use crate::middleware::request_observability::{observe_ws_operation, with_database_stage};
use crate::services::collaboration::{self, Snapshot};
use crate::state::AppState;
use contrix_collab::CollabMessage;
use contrix_core::{dto::ApiResponse, error::AppError};
use salvo::prelude::*;
use salvo::websocket::{Message, WebSocket, WebSocketUpgrade};
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

fn render_error(res: &mut Response, error: AppError) {
    res.status_code(
        StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
    );
    res.render(Json(ApiResponse::<()>::err(error.to_response())));
}
async fn authorized(
    state: &AppState,
    credential: &AuthenticatedCredential,
    id: Uuid,
) -> Result<(), AppError> {
    database_backpressure(id, || authorized_once(state, credential, id))
        .await
        .inspect_err(|error| {
            tracing::warn!(document=%id, user=%credential.user_id, %error, "Collaboration revalidation unavailable or denied");
        })
}

/// Wait on bounded database contention, never on a cached authorization result.
/// The caller retains at most one pending frame and sends no document content
/// or ACK while the full credential/ACL check is unavailable.
async fn database_backpressure<T, F, Work>(id: Uuid, mut work: F) -> Result<T, AppError>
where
    F: FnMut() -> Work,
    Work: std::future::Future<Output = Result<T, AppError>>,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut delay = std::time::Duration::from_millis(10);
    let mut deferred = false;
    loop {
        match tokio::time::timeout_at(deadline, work()).await {
            Ok(Err(error)) if error.is_transient_database() => {
                if !deferred {
                    tracing::debug!(document=%id, "Database backpressure; pausing collaboration until revalidation succeeds");
                    deferred = true;
                }
                tokio::time::sleep_until((tokio::time::Instant::now() + delay).min(deadline)).await;
                delay = (delay * 2).min(std::time::Duration::from_millis(100));
                if tokio::time::Instant::now() >= deadline {
                    return Err(error);
                }
            }
            Ok(result) => return result,
            Err(_) => {
                return Err(AppError::Database(
                    "collaboration database backpressure deadline exceeded".into(),
                ));
            }
        }
    }
}

async fn authorized_once(
    state: &AppState,
    credential: &AuthenticatedCredential,
    id: Uuid,
) -> Result<(), AppError> {
    use contrix_core::models::File;
    use contrix_core::schema::{documents::dsl as d, files::dsl as f};
    use diesel::prelude::*;

    let credential = credential.clone();
    let path = format!("/api/v1/ws/collab/{id}");
    let id = id.to_string();
    state
        .db
        .run(move |conn| {
            // One lease and one read snapshot cover this authorization attempt;
            // no credential or ACL decision survives into the next attempt.
            conn.transaction::<_, AppError, _>(|conn| {
                crate::middleware::auth::validate_credential_in_transaction(
                    conn,
                    &credential,
                    &salvo::http::Method::GET,
                    &path,
                )?;
                let file = d::documents
                    .inner_join(f::files.on(d::file_id.eq(f::id)))
                    .filter(d::id.eq(&id))
                    .select(File::as_select())
                    .first::<File>(conn)
                    .optional()?
                    .ok_or_else(|| AppError::NotFound("Document not found".into()))?;
                // The prior async ACL did not exclude trashed files. Current
                // room membership remains required even for a file's owner.
                crate::services::persistence::authorize_including_trashed(
                    conn,
                    &file,
                    &credential.user_id,
                    "editor",
                )
            })
        })
        .await
}
#[handler]
pub async fn collab_ws(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let Some(credential) = get_credential(depot) else {
        render_error(res, AppError::Auth("Not authenticated".into()));
        return;
    };
    let id = match req
        .param::<String>("document_id")
        .and_then(|id| id.parse::<Uuid>().ok())
    {
        Some(id) => id,
        None => {
            render_error(res, AppError::Validation("Invalid document id".into()));
            return;
        }
    };
    if let Err(error) = authorized(&state, &credential, id).await {
        render_error(res, error);
        return;
    }
    let user = match credential.user_id.parse::<Uuid>() {
        Ok(user) => user,
        Err(_) => {
            render_error(res, AppError::Auth("Invalid user".into()));
            return;
        }
    };
    let username = {
        use contrix_core::schema::users::dsl as u;
        use diesel::prelude::*;
        use horme::RunQueryDsl;
        let mut db = state.db.clone();
        match u::users
            .find(&credential.user_id)
            .select(u::username)
            .first::<String>(&mut db)
            .await
        {
            Ok(name) => name,
            Err(error) => {
                render_error(res, error.into());
                return;
            }
        }
    };
    let _ = WebSocketUpgrade::new()
        .max_message_size(collaboration::MAX_MESSAGE_BYTES)
        .max_frame_size(collaboration::MAX_MESSAGE_BYTES)
        .upgrade(req, res, move |socket| {
            handle_ws(socket, state, credential, id, user, username)
        })
        .await;
}
async fn send(ws: &mut WebSocket, message: &CollabMessage) -> bool {
    let Ok(text) = serde_json::to_string(message) else {
        return false;
    };
    matches!(
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            ws.send(Message::text(text))
        )
        .await,
        Ok(Ok(()))
    )
}

fn close_policy(error: &AppError) -> (u16, &'static str) {
    match error {
        error if error.is_transient_database() => {
            (1013u16, "Database busy; reconnect to resynchronize")
        }
        AppError::Auth(_) | AppError::Forbidden(_) | AppError::NotFound(_) => {
            (1008u16, "Document access is no longer available")
        }
        _ => (
            1011u16,
            "Collaboration unavailable; reconnect to resynchronize",
        ),
    }
}

async fn close_with_error(ws: &mut WebSocket, error: &AppError) {
    let (code, reason) = close_policy(error);
    send(
        ws,
        &CollabMessage::Error {
            message: reason.into(),
        },
    )
    .await;
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        ws.send(Message::close_with(code, reason)),
    )
    .await;
}
async fn send_snapshot(
    ws: &mut WebSocket,
    state: &AppState,
    credential: &AuthenticatedCredential,
    id: Uuid,
) -> Option<u64> {
    match database_backpressure(id, || collaboration::snapshot(&state.db, id)).await {
        Ok(Snapshot {
            content, version, ..
        }) => {
            if let Err(error) = authorized(state, credential, id).await {
                close_with_error(ws, &error).await;
                return None;
            }
            let version = u64::try_from(version).ok()?;
            send(ws, &CollabMessage::DocumentState { content, version })
                .await
                .then_some(version)
        }
        Err(error) => {
            tracing::warn!(%id,%error,"Collaboration snapshot unavailable");
            close_with_error(ws, &error).await;
            None
        }
    }
}
async fn handle_ws(
    mut ws: WebSocket,
    state: Arc<AppState>,
    credential: AuthenticatedCredential,
    id: Uuid,
    user: Uuid,
    username: String,
) {
    // Subscribe before loading the snapshot. Queued state messages are filtered
    // by the snapshot's durable version, eliminating the subscribe/read race.
    let Ok(mut receiver) = state.collab.join_session(id, user, username).await else {
        return;
    };
    // The handshake and upgrade have async gaps. A credential revoked while
    // upgrading must not receive even its first document body.
    if let Err(error) = authorized(&state, &credential, id).await {
        close_with_error(&mut ws, &error).await;
        state.collab.leave_session(id, user).await;
        return;
    }
    let Some(mut observed) = send_snapshot(&mut ws, &state, &credential, id).await else {
        state.collab.leave_session(id, user).await;
        return;
    };
    let mut revalidate = tokio::time::interval(std::time::Duration::from_secs(2));
    let mut close_error = None;
    loop {
        tokio::select! {
            _=revalidate.tick()=>{
                if let Err(error)=authorized(&state,&credential,id).await {close_error=Some(error);break;}
                // Also detect commits made by another server process. A durable
                // snapshot removes dependence on in-memory broadcast delivery.
                match database_backpressure(id,||collaboration::snapshot(&state.db,id)).await {
                    Ok(current) if current.version as u64>observed=>{
                        if let Err(error)=authorized(&state,&credential,id).await {close_error=Some(error);break;}
                        observed=current.version as u64;
                        if !send(&mut ws,&CollabMessage::DocumentState{content:current.content,version:observed}).await{break;}
                    }
                    Err(error)=>{close_error=Some(error);break;},
                    _=>{}
                }
            }
            received=receiver.recv()=>{
                if let Err(error)=authorized(&state,&credential,id).await {close_error=Some(error);break;}
                match received{
                    Ok(CollabMessage::DocumentState{content,version})=>{
                        if version<=observed{continue;}
                        observed=version;
                        if !send(&mut ws,&CollabMessage::DocumentState{content,version}).await{break;}
                    }
                    Ok(message)=>{
                        if !send(&mut ws,&message).await{break;}
                    }
                    Err(broadcast::error::RecvError::Lagged(_))=>{
                        // Discard all buffered state covered by the new snapshot.
                        receiver=receiver.resubscribe();
                        let Some(version)=send_snapshot(&mut ws,&state,&credential,id).await else{break;};
                        observed=version;
                    }
                    Err(broadcast::error::RecvError::Closed)=>break,
                }
            }
            received=ws.recv()=>{
                let Some(Ok(message))=received else{break;};
                if message.is_close(){break;}
                if let Err(error)=authorized(&state,&credential,id).await {close_error=Some(error);break;}
                let Ok(text)=message.as_str() else{continue;};
                let response=match serde_json::from_str::<CollabMessage>(text){
                    Ok(CollabMessage::Operation(mut operation))=>{
                        operation.user_id=user;
                        let op_id=operation.id.clone();
                        let mut close_on_error = false;
                        let result = observe_ws_operation(
                            id,
                            op_id.client_id,
                            op_id.counter,
                            async {
                                let current = collaboration::commit_operation_with_credential(
                                    &state, id, operation, &credential,
                                ).await?;
                                close_on_error = true;
                                // This operation's observer owns the failure warning.
                                // Keep the same revalidation/deadline without authorized's
                                // separate warning, and retain its close-on-error policy.
                                with_database_stage(
                                    "ws_revalidation",
                                    database_backpressure(
                                        id, || authorized_once(&state, &credential, id),
                                    ),
                                ).await?;
                                Ok(current)
                            },
                        ).await;
                        match result{
                            Ok(current)=>{
                                let version=current.version as u64;
                                // Only the committing sender receives an ACK.
                                // Both document and operation record are durable.
                                if version>observed{
                                    observed=version;
                                    if !send(&mut ws,&CollabMessage::DocumentState{content:current.content,version}).await{break;}
                                }
                                Some(CollabMessage::Ack{op_id})
                            }
                            Err(error) if close_on_error => {
                                close_error = Some(error);
                                break;
                            },
                            Err(error)=>Some(CollabMessage::Error{message:error.to_string()}),
                        }
                    }
                    Ok(CollabMessage::CursorUpdate(mut cursor))=>{
                        cursor.user_id=user;
                        state.collab.update_cursor(id,cursor).await;None
                    }
                    Ok(CollabMessage::SyncRequest{..})=>{
                        let Some(version)=send_snapshot(&mut ws,&state,&credential,id).await else{break;};
                        observed=version;None
                    }
                    Ok(_)=>Some(CollabMessage::Error{message:"Unsupported client message".into()}),
                    Err(_)=>Some(CollabMessage::Error{message:"Invalid collaboration message".into()}),
                };
                if let Some(response)=response
                    && !send(&mut ws,&response).await{break;}
            }
        }
    }
    if let Some(error) = close_error {
        close_with_error(&mut ws, &error).await;
    } else {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), ws.send(Message::close()))
            .await;
    }
    state.collab.leave_session(id, user).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn revalidation_deadline_has_its_own_cause_and_keeps_retryable_close_policy() {
        let error =
            database_backpressure(Uuid::nil(), std::future::pending::<Result<(), AppError>>)
                .await
                .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Database error: collaboration database backpressure deadline exceeded"
        );
        assert_eq!(error.status_code(), 503);
        assert_eq!(
            crate::middleware::request_observability::database_cause(&error),
            "ws_backpressure_deadline"
        );
        assert_eq!(
            close_policy(&error),
            (1013, "Database busy; reconnect to resynchronize")
        );
        let worker =
            AppError::Database("database operation timed out; cleanup remains admitted".into());
        assert_eq!(
            crate::middleware::request_observability::database_cause(&worker),
            "operation_timeout"
        );
        assert_eq!(close_policy(&worker), close_policy(&error));
    }
}
