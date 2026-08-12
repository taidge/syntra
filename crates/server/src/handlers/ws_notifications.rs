use std::sync::Arc;
use std::time::Duration;

use salvo::prelude::*;
use salvo::websocket::{Message, WebSocket, WebSocketUpgrade};
use tokio::sync::broadcast;
use tokio::time::interval;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

const HEARTBEAT_INTERVAL_SECS: u64 = 30;

#[handler]
pub async fn notifications_ws(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
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

    let notifications = state.notifications.clone();

    let _ = WebSocketUpgrade::new()
        .upgrade(req, res, move |ws| handle_ws(ws, notifications, user_id))
        .await;
}

async fn handle_ws(
    mut ws: WebSocket,
    notifications: Arc<crate::services::NotificationBus>,
    user_id: String,
) {
    let mut rx: broadcast::Receiver<String> = notifications.subscribe(&user_id).await;
    let mut heartbeat = interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    heartbeat.tick().await;

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(payload) => {
                        if ws.send(Message::text(payload)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if ws.send(Message::text(r#"{"event":"resync"}"#)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = ws.recv() => {
                match msg {
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
            _ = heartbeat.tick() => {
                if ws.send(Message::ping(Vec::new())).await.is_err() {
                    break;
                }
            }
        }
    }
}
