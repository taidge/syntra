use std::cell::RefCell;
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting(u32),
    Failed,
}

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

#[derive(Debug, Clone, Default)]
pub struct RemoteCursor {
    pub user_id: String,
    pub username: String,
    pub color: String,
    pub position: Option<u32>,
}

#[derive(Clone)]
pub struct CollabHandle {
    inner: Rc<RefCell<HandleInner>>,
}

struct HandleInner {
    ws: Option<WebSocket>,
    client_id: Uuid,
    counter: u64,
    version: u64,
    closed: bool,
    callbacks: Option<SocketCallbacks>,
    reconnect: Option<Timeout>,
}

struct SocketCallbacks {
    _message: Closure<dyn FnMut(MessageEvent)>,
    _open: Closure<dyn FnMut()>,
    _close: Closure<dyn FnMut(CloseEvent)>,
    _error: Closure<dyn FnMut(web_sys::Event)>,
}

impl Drop for HandleInner {
    fn drop(&mut self) {
        if let Some(socket) = self.ws.take() {
            socket.set_onclose(None);
            socket.set_onmessage(None);
            socket.set_onerror(None);
            socket.set_onopen(None);
            let _ = socket.close();
        }
    }
}

impl CollabHandle {
    pub fn current_ws(&self) -> Option<WebSocket> {
        self.inner.borrow().ws.clone()
    }

    /// Close explicitly without scheduling another connection after unmount.
    pub fn close(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.closed = true;
        inner.reconnect.take();
        if let Some(socket) = inner.ws.take() {
            socket.set_onclose(None);
            socket.set_onmessage(None);
            socket.set_onerror(None);
            socket.set_onopen(None);
            let _ = socket.close();
        }
        inner.callbacks.take();
    }

    /// Send a typed collaborative operation using this client's monotonic
    /// counter and the latest server version observed on the socket.
    pub fn send_operation(&self, user_id: &str, op_type: serde_json::Value) -> Result<(), String> {
        let mut inner = self.inner.borrow_mut();
        if inner.closed {
            return Err("collaboration session is closed".into());
        }
        let ws = inner.ws.clone().ok_or("collaboration socket unavailable")?;
        if ws.ready_state() != WebSocket::OPEN {
            return Err("collaboration socket is not open".into());
        }
        inner.counter = inner.counter.saturating_add(1);
        let message = serde_json::json!({
            "type": "Operation",
            "id": {
                "counter": inner.counter,
                "client_id": inner.client_id,
            },
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "user_id": user_id,
            "base_version": inner.version,
            "op_type": op_type,
        });
        let text = serde_json::to_string(&message).map_err(|e| e.to_string())?;
        ws.send_with_str(&text)
            .map_err(|e| format!("websocket send failed: {e:?}"))
    }
}

const BACKOFF_STEPS_MS: &[u32] = &[1000, 2000, 5000, 15000, 30000];
const MAX_ATTEMPTS: u32 = 12;

pub fn connect_with_reconnect(
    document_id: String,
    token: String,
    on_message: impl FnMut(CollabMessage) + 'static,
    on_state: impl FnMut(ConnectionState) + 'static,
) -> CollabHandle {
    let inner = Rc::new(RefCell::new(HandleInner {
        ws: None,
        client_id: Uuid::new_v4(),
        counter: 0,
        version: 0,
        closed: false,
        callbacks: None,
        reconnect: None,
    }));
    let on_message = Rc::new(RefCell::new(on_message));
    let on_state = Rc::new(RefCell::new(on_state));
    let attempt = Rc::new(RefCell::new(0u32));

    open_connection(
        document_id,
        token,
        inner.clone(),
        on_message,
        on_state,
        attempt,
    );

    CollabHandle { inner }
}

fn open_connection(
    document_id: String,
    token: String,
    inner: Rc<RefCell<HandleInner>>,
    on_message: Rc<RefCell<dyn FnMut(CollabMessage)>>,
    on_state: Rc<RefCell<dyn FnMut(ConnectionState)>>,
    attempt: Rc<RefCell<u32>>,
) {
    if inner.borrow().closed {
        return;
    }
    let current_attempt = *attempt.borrow();
    if current_attempt == 0 {
        (on_state.borrow_mut())(ConnectionState::Connecting);
    } else {
        (on_state.borrow_mut())(ConnectionState::Reconnecting(current_attempt));
    }

    let ws = match connect_collab(&document_id, &token) {
        Ok(ws) => ws,
        Err(_) => {
            schedule_reconnect(document_id, token, inner, on_message, on_state, attempt);
            return;
        }
    };

    let onmessage = {
        let on_message = on_message.clone();
        let inner_for_message = Rc::downgrade(&inner);
        let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |evt: MessageEvent| {
            let Some(inner_for_message) = inner_for_message.upgrade() else {
                return;
            };
            if let Some(text) = evt.data().as_string()
                && let Ok(msg) = serde_json::from_str::<CollabMessage>(&text)
            {
                {
                    let mut state = inner_for_message.borrow_mut();
                    match &msg {
                        CollabMessage::DocumentState { version, .. } => {
                            state.version = *version;
                        }
                        CollabMessage::Operation(_) => {
                            state.version = state.version.saturating_add(1);
                        }
                        _ => {}
                    }
                }
                (on_message.borrow_mut())(msg);
            }
        });
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage
    };

    let onopen = {
        let on_state = on_state.clone();
        let attempt_open = attempt.clone();
        let onopen = Closure::<dyn FnMut()>::new(move || {
            *attempt_open.borrow_mut() = 0;
            (on_state.borrow_mut())(ConnectionState::Connected);
        });
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        onopen
    };

    let onclose = {
        let document_id_cl = document_id.clone();
        let token_cl = token.clone();
        let inner_cl = Rc::downgrade(&inner);
        let on_message_cl = on_message.clone();
        let on_state_cl = on_state.clone();
        let attempt_cl = attempt.clone();
        let onclose = Closure::<dyn FnMut(CloseEvent)>::new(move |_evt: CloseEvent| {
            let Some(inner_cl) = inner_cl.upgrade() else {
                return;
            };
            schedule_reconnect(
                document_id_cl.clone(),
                token_cl.clone(),
                inner_cl.clone(),
                on_message_cl.clone(),
                on_state_cl.clone(),
                attempt_cl.clone(),
            );
        });
        ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        onclose
    };

    let onerror = {
        // Reconnection is driven by onclose (which always follows onerror);
        // here we only surface the failure for debugging.
        let onerror = Closure::<dyn FnMut(web_sys::Event)>::new(move |_evt: web_sys::Event| {
            web_sys::console::warn_1(&"collab websocket error".into());
        });
        ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror
    };

    let mut state = inner.borrow_mut();
    state.ws = Some(ws);
    state.callbacks = Some(SocketCallbacks {
        _message: onmessage,
        _open: onopen,
        _close: onclose,
        _error: onerror,
    });
}

fn schedule_reconnect(
    document_id: String,
    token: String,
    inner: Rc<RefCell<HandleInner>>,
    on_message: Rc<RefCell<dyn FnMut(CollabMessage)>>,
    on_state: Rc<RefCell<dyn FnMut(ConnectionState)>>,
    attempt: Rc<RefCell<u32>>,
) {
    if inner.borrow().closed {
        return;
    }
    let next = {
        let mut a = attempt.borrow_mut();
        *a += 1;
        *a
    };
    if next > MAX_ATTEMPTS {
        (on_state.borrow_mut())(ConnectionState::Failed);
        inner.borrow_mut().ws = None;
        return;
    }
    let delay = reconnect_delay(next);
    (on_state.borrow_mut())(ConnectionState::Reconnecting(next));
    inner.borrow_mut().ws = None;

    let document_id_cl = document_id;
    let token_cl = token;
    let inner_cl = Rc::downgrade(&inner);
    let on_message_cl = on_message;
    let on_state_cl = on_state;
    let attempt_cl = attempt;
    let timeout = Timeout::new(delay, move || {
        let Some(inner_cl) = inner_cl.upgrade() else {
            return;
        };
        open_connection(
            document_id_cl,
            token_cl,
            inner_cl,
            on_message_cl,
            on_state_cl,
            attempt_cl,
        );
    });
    inner.borrow_mut().reconnect = Some(timeout);
}

fn reconnect_delay(attempt: u32) -> u32 {
    let index = ((attempt as usize).saturating_sub(1)).min(BACKOFF_STEPS_MS.len() - 1);
    BACKOFF_STEPS_MS[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(reconnect_delay(1), 1000);
        assert_eq!(reconnect_delay(3), 5000);
        assert_eq!(reconnect_delay(99), 30000);
    }

    #[test]
    fn closed_session_rejects_operations() {
        let handle = CollabHandle {
            inner: Rc::new(RefCell::new(HandleInner {
                ws: None,
                client_id: Uuid::new_v4(),
                counter: 0,
                version: 0,
                closed: false,
                callbacks: None,
                reconnect: None,
            })),
        };
        handle.close();
        assert!(
            handle
                .send_operation("user", serde_json::json!({}))
                .unwrap_err()
                .contains("closed")
        );
    }
}
