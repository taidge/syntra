use std::sync::Arc;
use std::time::Duration;

use salvo::prelude::*;
use salvo::websocket::{Message, WebSocket, WebSocketUpgrade};
use tokio::sync::broadcast;
use tokio::time::interval;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;

use crate::middleware::auth::{
    AuthenticatedCredential, get_credential, get_user_id, validate_credential,
};
use crate::state::AppState;

const HEARTBEAT_INTERVAL_SECS: u64 = 30;
const MAX_CLIENT_MESSAGE_BYTES: usize = 64 * 1024;

async fn send(ws: &mut WebSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(5), ws.send(message)).await,
        Ok(Ok(()))
    )
}

#[handler]
pub async fn notifications_ws(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
    let user_id = match get_user_id(depot) {
        Some(id) => id,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let Some(credential) = get_credential(depot) else {
        res.status_code(StatusCode::UNAUTHORIZED);
        return;
    };
    let notifications = state.notifications.clone();

    let _ = WebSocketUpgrade::new()
        .max_message_size(MAX_CLIENT_MESSAGE_BYTES)
        .max_frame_size(MAX_CLIENT_MESSAGE_BYTES)
        .upgrade(req, res, move |ws| {
            handle_ws(ws, notifications, user_id, state, credential)
        })
        .await;
}

async fn handle_ws(
    mut ws: WebSocket,
    notifications: Arc<crate::services::NotificationBus>,
    user_id: String,
    state: Arc<AppState>,
    credential: AuthenticatedCredential,
) {
    let mut rx: broadcast::Receiver<String> = notifications.subscribe(&user_id).await;
    let mut heartbeat = interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    heartbeat.tick().await;
    let mut revalidate = interval(Duration::from_secs(2));

    loop {
        tokio::select! {
            _ = revalidate.tick() => {
                if validate_credential(&state,&credential,"/api/v1/ws/notifications").await.is_err(){break;}
            }
            msg = rx.recv() => {
                if validate_credential(&state,&credential,"/api/v1/ws/notifications").await.is_err(){break;}
                match msg {
                    Ok(payload) => {
                        if !send(&mut ws, Message::text(payload)).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if !send(&mut ws, Message::text(r#"{"event":"resync"}"#)).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = ws.recv() => {
                if validate_credential(&state,&credential,"/api/v1/ws/notifications").await.is_err(){break;}
                match msg {
                    Some(Ok(message)) if !message.is_close() => {}
                    Some(Ok(_)) => break,
                    Some(Err(_)) | None => break,
                }
            }
            _ = heartbeat.tick() => {
                if validate_credential(&state,&credential,"/api/v1/ws/notifications").await.is_err(){break;}
                if !send(&mut ws, Message::ping(Vec::new())).await {
                    break;
                }
            }
        }
    }
}
