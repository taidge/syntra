use dioxus::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{MessageEvent, WebSocket};

use crate::collab_client::{CollabMessage, RemoteCursor, connect_collab};
use crate::components::navbar::Navbar;
use crate::components::sidebar::Sidebar;
use crate::components::toolbar::Toolbar;
use crate::state::{AuthState, api_get};

#[derive(Clone, Deserialize)]
struct DocumentResponse {
    name: String,
    content: serde_json::Value,
}

#[derive(Deserialize)]
struct WriterDocLite {
    #[serde(default)]
    body: Vec<serde_json::Value>,
}

fn extract_run_text(runs: &[serde_json::Value]) -> String {
    runs.iter()
        .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
        .collect::<String>()
}

fn writer_content_to_plain(content: &serde_json::Value) -> String {
    let doc: WriterDocLite = match serde_json::from_value(content.clone()) {
        Ok(d) => d,
        Err(_) => return String::new(),
    };
    let mut out = String::new();
    for block in &doc.body {
        let runs = block
            .get("runs")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        if !runs.is_empty() {
            out.push_str(&extract_run_text(&runs));
            out.push('\n');
        }
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Rich-text document editor (word processor).
#[component]
pub fn Writer(id: String) -> Element {
    let mut doc_title = use_signal(|| "Untitled Document".to_string());
    let mut word_count = use_signal(|| 0u32);
    let mut char_count = use_signal(|| 0u32);
    let mut page_count = use_signal(|| 1u32);
    let mut saved_status = use_signal(|| "All changes saved".to_string());
    let mut show_sidebar = use_signal(|| true);
    let mut doc_body_text = use_signal(String::new);
    let mut loaded = use_signal(|| false);

    let auth = use_context::<Signal<AuthState>>();
    let presence = use_signal(Vec::<RemoteCursor>::new);
    let mut ws_handle = use_signal(|| None::<WebSocket>);

    let id_for_resource = id.clone();
    let doc_resource = use_resource(move || {
        let id = id_for_resource.clone();
        let token = auth.read().token.clone().unwrap_or_default();
        async move {
            api_get::<DocumentResponse>(&format!("/documents/{id}"), &token).await
        }
    });

    use_effect(move || {
        if *loaded.read() {
            return;
        }
        if let Some(Ok(doc)) = doc_resource.read().as_ref() {
            doc_title.set(doc.name.clone());
            let text = writer_content_to_plain(&doc.content);
            let wc = text.split_whitespace().count() as u32;
            let cc = text.len() as u32;
            doc_body_text.set(text);
            word_count.set(wc);
            char_count.set(cc);
            let _ = page_count;
            loaded.set(true);
        }
    });

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
                    match doc_resource.read().as_ref() {
                        None => rsx! { div { class: "editor-loading", "Loading document..." } },
                        Some(Err(e)) => rsx! { div { class: "editor-error", "Failed to load: {e}" } },
                        Some(Ok(_)) => rsx! {
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
                                    "{doc_body_text}"
                                }
                            }
                        },
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
