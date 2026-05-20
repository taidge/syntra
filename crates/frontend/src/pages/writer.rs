use dioxus::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{MessageEvent, WebSocket};

use crate::collab_client::{CollabMessage, RemoteCursor, connect_collab};
use crate::components::navbar::Navbar;
use crate::components::sidebar::Sidebar;
use crate::components::toolbar::Toolbar;
use crate::state::AuthState;

/// Rich-text document editor (word processor).
#[component]
pub fn Writer(id: String) -> Element {
    let mut doc_title = use_signal(|| "Untitled Document".to_string());
    let mut word_count = use_signal(|| 0u32);
    let mut char_count = use_signal(|| 0u32);
    let mut page_count = use_signal(|| 1u32);
    let mut saved_status = use_signal(|| "All changes saved".to_string());
    let mut show_sidebar = use_signal(|| true);

    let auth = use_context::<Signal<AuthState>>();
    let presence = use_signal(Vec::<RemoteCursor>::new);
    let mut ws_handle = use_signal(|| None::<WebSocket>);

    let doc_id_for_effect = id.clone();
    use_effect(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        let doc_id = doc_id_for_effect.clone();
        match connect_collab(&doc_id, &token) {
            Ok(ws) => {
                let mut presence_for_msg = presence;
                let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(
                    move |evt: MessageEvent| {
                        if let Some(text) = evt.data().as_string() {
                            if let Ok(msg) = serde_json::from_str::<CollabMessage>(&text) {
                                match msg {
                                    CollabMessage::UserJoined { user_id, username, color } => {
                                        let mut list = presence_for_msg.write();
                                        if !list.iter().any(|c| c.user_id == user_id) {
                                            list.push(RemoteCursor {
                                                user_id,
                                                username,
                                                color,
                                                is_active: true,
                                            });
                                        }
                                    }
                                    CollabMessage::UserLeft { user_id } => {
                                        let mut list = presence_for_msg.write();
                                        list.retain(|c| c.user_id != user_id);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    },
                );
                ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
                onmessage.forget();
                ws_handle.set(Some(ws));
            }
            Err(_) => {
                ws_handle.set(None);
            }
        }
    });

    use_drop(move || {
        if let Some(ws) = ws_handle.write().take() {
            let _ = ws.close();
        }
    });

    rsx! {
        Navbar {}
        div {
            class: "editor-container",
            tabindex: "0",
            onkeydown: move |evt: KeyboardEvent| {
                if evt.modifiers().contains(Modifiers::CONTROL) {
                    if evt.key() == Key::Character("s".to_string()) {
                        evt.prevent_default();
                        saved_status.set("All changes saved".to_string());
                    }
                }
            },
            // Document title bar
            div { class: "doc-title-bar",
                input {
                    class: "doc-title-input",
                    value: "{doc_title}",
                    oninput: move |e| doc_title.set(e.value()),
                    placeholder: "Untitled Document",
                }
                span { class: "save-status", "{saved_status}" }
                if !presence.read().is_empty() {
                    div {
                        class: "presence-list",
                        style: "display: inline-flex; gap: 4px; margin-left: 12px; align-items: center;",
                        for cursor in presence.read().iter() {
                            span {
                                key: "{cursor.user_id}",
                                class: "presence-pill",
                                title: "{cursor.username}",
                                style: "background: {cursor.color}; color: #fff; border-radius: 999px; padding: 2px 8px; font-size: 12px; font-weight: 600; line-height: 1;",
                                {cursor.username.chars().take(2).collect::<String>().to_uppercase()}
                            }
                        }
                    }
                }
                button {
                    class: "btn btn-outline btn-sm",
                    onclick: move |_| {
                        let current = *show_sidebar.read();
                        show_sidebar.set(!current);
                    },
                    if *show_sidebar.read() { "Hide Sidebar" } else { "Show Sidebar" }
                }
            }

            // Toolbar
            Toolbar { doc_type: "writer".to_string() }

            // Main editor area
            div { class: "editor-main",
                if *show_sidebar.read() {
                    Sidebar {}
                }

                div { class: "editor-canvas writer-canvas",
                    // Paper-like page
                    div { class: "page",
                        div {
                            contenteditable: "true",
                            class: "writer-content",
                            oninput: move |evt: FormEvent| {
                                let text = evt.value();
                                let wc = text.split_whitespace().count() as u32;
                                let cc = text.len() as u32;
                                word_count.set(wc);
                                char_count.set(cc);
                                saved_status.set("Unsaved changes".to_string());
                            },
                        }
                    }
                }
            }

            // Status bar
            div { class: "status-bar",
                span { "Page {page_count} of {page_count}" }
                span { "Words: {word_count}" }
                span { "Characters: {char_count}" }
                span { "Document ID: {id}" }
            }
        }
    }
}
