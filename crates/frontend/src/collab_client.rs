use serde::{Deserialize, Serialize};
use web_sys::WebSocket;

/// Collaboration message types (mirrors server-side CollabMessage).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CollabMessage {
    Operation(serde_json::Value),
    CursorUpdate(serde_json::Value),
    UserJoined {
        user_id: String,
        username: String,
        color: String,
    },
    UserLeft {
        user_id: String,
    },
    DocumentState {
        content: serde_json::Value,
        version: u64,
    },
    Ack {
        op_id: serde_json::Value,
    },
    SyncRequest {
        from_version: u64,
    },
    Error {
        message: String,
    },
    Undo {
        user_id: String,
    },
    Redo {
        user_id: String,
    },
}

/// Create a WebSocket connection to the collaboration server.
///
/// Returns the WebSocket instance. The caller sets up onmessage/onclose handlers.
pub fn connect_collab(document_id: &str, token: &str) -> Result<WebSocket, String> {
    let window = web_sys::window().ok_or("no window")?;
    let location = window.location();
    let protocol = location.protocol().unwrap_or_else(|_| "http:".into());
    let host = location.host().unwrap_or_else(|_| "localhost:3000".into());

    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    let url = format!(
        "{}//{}/api/v1/ws/collab/{}?token={}",
        ws_protocol, host, document_id, token
    );

    let ws = WebSocket::new(&url).map_err(|e| format!("{:?}", e))?;
    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

    Ok(ws)
}

/// Send a CollabMessage over the WebSocket.
pub fn send_message(ws: &WebSocket, msg: &CollabMessage) -> Result<(), String> {
    let json = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    ws.send_with_str(&json).map_err(|e| format!("{:?}", e))
}

/// Represents a remote user's cursor position for visualization.
#[derive(Debug, Clone, Default)]
pub struct RemoteCursor {
    pub user_id: String,
    pub username: String,
    pub color: String,
    pub is_active: bool,
    pub position: Option<u32>,
}
