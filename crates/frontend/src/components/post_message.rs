use dioxus::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
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
    let Some(window) = web_sys::window() else { return; };
    let Ok(Some(parent)) = window.parent() else { return; };
    let Ok(payload) = serde_json::to_string(msg) else { return; };
    let _ = parent.post_message(&payload.into(), "*");
}

#[component]
pub fn PostMessageBridge() -> Element {
    use_hook(|| {
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
                    if let Some(window) = web_sys::window() {
                        if let Some(doc) = window.document() {
                            if let Ok(event) = web_sys::CustomEvent::new("contrix:save") {
                                let _ = doc.dispatch_event(&event);
                            }
                        }
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
            let _ = window
                .add_event_listener_with_callback("message", cb.as_ref().unchecked_ref());
        }
        cb.forget();
    });
    rsx! { Fragment {} }
}
