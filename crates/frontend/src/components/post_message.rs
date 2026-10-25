use glory::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};
#[cfg(target_arch = "wasm32")]
use web_sys::MessageEvent;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum HostMessage {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "save")]
    Save,
    #[serde(rename = "saved")]
    Saved { document_id: String },
    #[serde(rename = "load-document")]
    LoadDocument { document_id: String },
    #[serde(rename = "current-document")]
    CurrentDocument {
        document_id: Option<String>,
        document_type: Option<String>,
    },
    #[serde(rename = "request-state")]
    RequestState,
    #[serde(rename = "state")]
    State { route: String, authenticated: bool },
    #[serde(rename = "log")]
    Log { level: String, message: String },
}

pub fn post_to_host(msg: &HostMessage) {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = msg;
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(Some(parent)) = window.parent() else {
            return;
        };
        let Ok(payload) = serde_json::to_string(msg) else {
            return;
        };
        let _ = parent.post_message(&payload.into(), "*");
    }
}

#[allow(non_snake_case)]
pub fn PostMessageBridge(scope: Scope) -> impl Widget {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = scope;
    #[cfg(target_arch = "wasm32")]
    {
        let cb = Closure::<dyn FnMut(MessageEvent)>::new(move |evt: MessageEvent| {
            let raw = match evt.data().as_string() {
                Some(s) => s,
                None => return,
            };
            let msg: HostMessage = match serde_json::from_str(&raw) {
                Ok(m) => m,
                Err(_) => return,
            };
            match msg {
                HostMessage::Ping => post_to_host(&HostMessage::Pong),
                HostMessage::Save => {
                    if let Some(window) = web_sys::window()
                        && let Some(doc) = window.document()
                        && let Ok(event) = web_sys::CustomEvent::new("contrix:save")
                    {
                        let _ = doc.dispatch_event(&event);
                    }
                }
                HostMessage::LoadDocument { document_id } => {
                    if document_id.is_empty() {
                        return;
                    }
                    if let Some(window) = web_sys::window() {
                        let _ = window
                            .location()
                            .set_href(&format!("/documents/{document_id}"));
                    }
                }
                HostMessage::RequestState => {
                    let route = web_sys::window()
                        .and_then(|w| w.location().pathname().ok())
                        .unwrap_or_default();
                    let authenticated = web_sys::window()
                        .and_then(|w| w.local_storage().ok().flatten())
                        .and_then(|s| s.get_item("contrix.auth").ok().flatten())
                        .map(|s| !s.is_empty() && s != "{}")
                        .unwrap_or(false);
                    post_to_host(&HostMessage::State {
                        route,
                        authenticated,
                    });
                }
                _ => {}
            }
        });
        if let Some(window) = web_sys::window() {
            let _ = window.add_event_listener_with_callback("message", cb.as_ref().unchecked_ref());
        }
        let _ = scope.on_cleanup(move || {
            if let Some(window) = web_sys::window() {
                let _ = window
                    .remove_event_listener_with_callback("message", cb.as_ref().unchecked_ref());
            }
            drop(cb);
        });
    }
    group()
}
