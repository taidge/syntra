use std::cell::RefCell;
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use serde::{Deserialize, Serialize};
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

pub fn send_message(ws: &WebSocket, msg: &CollabMessage) -> Result<(), String> {
    let json = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    ws.send_with_str(&json).map_err(|e| format!("{:?}", e))
}

#[derive(Debug, Clone, Default)]
pub struct RemoteCursor {
    pub user_id: String,
    pub username: String,
    pub color: String,
    pub is_active: bool,
    pub position: Option<u32>,
}

#[derive(Clone)]
pub struct CollabHandle {
    inner: Rc<RefCell<HandleInner>>,
}

struct HandleInner {
    ws: Option<WebSocket>,
}

impl CollabHandle {
    pub fn current_ws(&self) -> Option<WebSocket> {
        self.inner.borrow().ws.clone()
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
    let inner = Rc::new(RefCell::new(HandleInner { ws: None }));
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

    {
        let on_message = on_message.clone();
        let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |evt: MessageEvent| {
            if let Some(text) = evt.data().as_string() {
                if let Ok(msg) = serde_json::from_str::<CollabMessage>(&text) {
                    (on_message.borrow_mut())(msg);
                }
            }
        });
        ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
    }

    {
        let on_state = on_state.clone();
        let attempt_open = attempt.clone();
        let onopen = Closure::<dyn FnMut()>::new(move || {
            *attempt_open.borrow_mut() = 0;
            (on_state.borrow_mut())(ConnectionState::Connected);
        });
        ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        onopen.forget();
    }

    {
        let document_id_cl = document_id.clone();
        let token_cl = token.clone();
        let inner_cl = inner.clone();
        let on_message_cl = on_message.clone();
        let on_state_cl = on_state.clone();
        let attempt_cl = attempt.clone();
        let onclose = Closure::<dyn FnMut(CloseEvent)>::new(move |_evt: CloseEvent| {
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
        onclose.forget();
    }

    {
        let onerror = Closure::<dyn FnMut(web_sys::Event)>::new(move |_evt: web_sys::Event| {});
        ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror.forget();
    }

    inner.borrow_mut().ws = Some(ws);
}

fn schedule_reconnect(
    document_id: String,
    token: String,
    inner: Rc<RefCell<HandleInner>>,
    on_message: Rc<RefCell<dyn FnMut(CollabMessage)>>,
    on_state: Rc<RefCell<dyn FnMut(ConnectionState)>>,
    attempt: Rc<RefCell<u32>>,
) {
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
    let delay_idx = ((next as usize).saturating_sub(1)).min(BACKOFF_STEPS_MS.len() - 1);
    let delay = BACKOFF_STEPS_MS[delay_idx];
    (on_state.borrow_mut())(ConnectionState::Reconnecting(next));
    inner.borrow_mut().ws = None;

    let document_id_cl = document_id;
    let token_cl = token;
    let inner_cl = inner;
    let on_message_cl = on_message;
    let on_state_cl = on_state;
    let attempt_cl = attempt;
    let timeout = Timeout::new(delay, move || {
        open_connection(
            document_id_cl,
            token_cl,
            inner_cl,
            on_message_cl,
            on_state_cl,
            attempt_cl,
        );
    });
    timeout.forget();
}
