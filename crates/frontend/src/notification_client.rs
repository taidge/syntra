use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gloo_timers::callback::Timeout;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{CloseEvent, MessageEvent, WebSocket};

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct NotificationItem {
    pub id: String,
    pub notification_type: String,
    pub title: String,
    pub message: String,
    pub link: Option<String>,
    pub is_read: bool,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum NotificationEvent {
    Notification(NotificationItem),
    Control { event: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationConnectionState {
    Connecting,
    Connected,
    Reconnecting(u32),
    Failed,
}

pub struct NotificationSocket {
    inner: Rc<SocketInner>,
}

struct SocketInner {
    active: Cell<bool>,
    socket: RefCell<Option<WebSocket>>,
    callbacks: RefCell<Option<SocketCallbacks>>,
}

struct SocketCallbacks {
    _open: Closure<dyn FnMut()>,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _close: Closure<dyn FnMut(CloseEvent)>,
    _error: Closure<dyn FnMut(web_sys::Event)>,
}

impl Drop for NotificationSocket {
    fn drop(&mut self) {
        self.inner.active.set(false);
        if let Some(socket) = self.inner.socket.borrow_mut().take() {
            socket.set_onopen(None);
            socket.set_onmessage(None);
            socket.set_onclose(None);
            socket.set_onerror(None);
            let _ = socket.close();
        }
        self.inner.callbacks.borrow_mut().take();
    }
}

const BACKOFF_MS: &[u32] = &[1_000, 2_000, 5_000, 15_000, 30_000];
const MAX_ATTEMPTS: u32 = 12;

pub fn connect_notifications(
    token: String,
    on_event: impl FnMut(NotificationEvent) + 'static,
    on_state: impl FnMut(NotificationConnectionState) + 'static,
) -> NotificationSocket {
    let inner = Rc::new(SocketInner {
        active: Cell::new(true),
        socket: RefCell::new(None),
        callbacks: RefCell::new(None),
    });
    open_connection(
        token,
        inner.clone(),
        Rc::new(RefCell::new(on_event)),
        Rc::new(RefCell::new(on_state)),
        Rc::new(Cell::new(0)),
    );
    NotificationSocket { inner }
}

fn open_connection(
    token: String,
    inner: Rc<SocketInner>,
    on_event: Rc<RefCell<dyn FnMut(NotificationEvent)>>,
    on_state: Rc<RefCell<dyn FnMut(NotificationConnectionState)>>,
    attempt: Rc<Cell<u32>>,
) {
    if !inner.active.get() {
        return;
    }
    let current_attempt = attempt.get();
    (on_state.borrow_mut())(if current_attempt == 0 {
        NotificationConnectionState::Connecting
    } else {
        NotificationConnectionState::Reconnecting(current_attempt)
    });

    let socket = match notification_socket(&token) {
        Ok(socket) => socket,
        Err(_) => {
            schedule_reconnect(token, inner, on_event, on_state, attempt);
            return;
        }
    };
    let onmessage = {
        let on_event = on_event.clone();
        let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            if let Some(text) = event.data().as_string()
                && let Ok(event) = serde_json::from_str::<NotificationEvent>(&text)
            {
                (on_event.borrow_mut())(event);
            }
        });
        socket.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage
    };
    let onopen = {
        let on_state = on_state.clone();
        let attempt = attempt.clone();
        let onopen = Closure::<dyn FnMut()>::new(move || {
            attempt.set(0);
            (on_state.borrow_mut())(NotificationConnectionState::Connected);
        });
        socket.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        onopen
    };
    let onclose = {
        let token = token.clone();
        let weak_inner = Rc::downgrade(&inner);
        let on_event = on_event.clone();
        let on_state = on_state.clone();
        let attempt = attempt.clone();
        let onclose = Closure::<dyn FnMut(CloseEvent)>::new(move |_| {
            if let Some(inner) = weak_inner.upgrade() {
                schedule_reconnect(
                    token.clone(),
                    inner,
                    on_event.clone(),
                    on_state.clone(),
                    attempt.clone(),
                );
            }
        });
        socket.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        onclose
    };
    let onerror = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {});
    socket.set_onerror(Some(onerror.as_ref().unchecked_ref()));
    inner.callbacks.replace(Some(SocketCallbacks {
        _open: onopen,
        _message: onmessage,
        _close: onclose,
        _error: onerror,
    }));
    inner.socket.replace(Some(socket));
}

fn notification_socket(token: &str) -> Result<WebSocket, String> {
    let window = web_sys::window().ok_or("no window")?;
    let location = window.location();
    let protocol = location.protocol().unwrap_or_else(|_| "http:".into());
    let host = location.host().unwrap_or_else(|_| "localhost:3000".into());
    let ws_protocol = if protocol == "https:" { "wss:" } else { "ws:" };
    WebSocket::new(&format!(
        "{ws_protocol}//{host}/api/v1/ws/notifications?token={token}"
    ))
    .map_err(|error| format!("{error:?}"))
}

fn schedule_reconnect(
    token: String,
    inner: Rc<SocketInner>,
    on_event: Rc<RefCell<dyn FnMut(NotificationEvent)>>,
    on_state: Rc<RefCell<dyn FnMut(NotificationConnectionState)>>,
    attempt: Rc<Cell<u32>>,
) {
    if !inner.active.get() {
        return;
    }
    let next = attempt.get().saturating_add(1);
    attempt.set(next);
    if let Some(socket) = inner.socket.borrow_mut().take() {
        socket.set_onclose(None);
    }
    if next > MAX_ATTEMPTS {
        (on_state.borrow_mut())(NotificationConnectionState::Failed);
        return;
    }
    (on_state.borrow_mut())(NotificationConnectionState::Reconnecting(next));
    let delay = reconnect_delay(next);
    Timeout::new(delay, move || {
        open_connection(token, inner, on_event, on_state, attempt);
    })
    .forget();
}

fn reconnect_delay(attempt: u32) -> u32 {
    BACKOFF_MS[(attempt.saturating_sub(1) as usize).min(BACKOFF_MS.len() - 1)]
}

pub fn merge_notification(items: &mut Vec<NotificationItem>, incoming: NotificationItem) {
    items.retain(|item| item.id != incoming.id);
    items.insert(0, incoming);
    items.truncate(100);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, title: &str) -> NotificationItem {
        NotificationItem {
            id: id.into(),
            notification_type: "comment".into(),
            title: title.into(),
            message: String::new(),
            link: None,
            is_read: false,
            created_at: String::new(),
        }
    }

    #[test]
    fn realtime_notifications_are_deduplicated_and_moved_first() {
        let mut items = vec![item("1", "old"), item("2", "second")];
        merge_notification(&mut items, item("1", "new"));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "new");
    }

    #[test]
    fn reconnect_backoff_is_bounded() {
        assert_eq!(reconnect_delay(1), 1_000);
        assert_eq!(reconnect_delay(3), 5_000);
        assert_eq!(reconnect_delay(99), 30_000);
    }
}
