use dioxus::prelude::*;
use serde::Deserialize;

use crate::components::{Toast, ToastKind, push_toast};
use crate::state::{AuthState, api_post};

#[derive(Deserialize, Clone, PartialEq)]
pub struct CommentItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub is_resolved: bool,
}

#[derive(serde::Serialize)]
struct CreateComment {
    content: String,
}

#[derive(Props, Clone, PartialEq)]
pub struct CommentsPaneProps {
    pub doc_id: String,
    pub open: Signal<bool>,
}

#[component]
pub fn CommentsPane(props: CommentsPaneProps) -> Element {
    let auth = use_context::<Signal<AuthState>>();
    let toast_bus = use_context::<Signal<Vec<Toast>>>();
    let token = auth.read().token.clone().unwrap_or_default();
    let doc_id_for_fetch = props.doc_id.clone();
    let token_for_fetch = token.clone();
    let mut refresh_tick = use_signal(|| 0u32);

    let comments = use_resource(move || {
        let _ = refresh_tick.read();
        let id = doc_id_for_fetch.clone();
        let t = token_for_fetch.clone();
        async move {
            crate::state::api_get::<Vec<CommentItem>>(
                &format!("/documents/{id}/comments"),
                &t,
            )
            .await
        }
    });

    let mut draft = use_signal(String::new);
    let mut open_sig = props.open;
    if !*open_sig.read() {
        return rsx! { Fragment {} };
    }

    let post_token = token.clone();
    let post_id = props.doc_id.clone();
    let post = move |_| {
        let content = draft.read().clone();
        if content.is_empty() {
            return;
        }
        let id = post_id.clone();
        let t = post_token.clone();
        let bus = toast_bus;
        let mut tick = refresh_tick;
        let mut local_draft = draft;
        wasm_bindgen_futures::spawn_local(async move {
            match api_post::<serde_json::Value, _>(
                &format!("/documents/{id}/comments"),
                &CreateComment {
                    content: content.clone(),
                },
                &t,
            )
            .await
            {
                Ok(_) => {
                    push_toast(bus, ToastKind::Success, "Comment added");
                    local_draft.set(String::new());
                    let cur = *tick.read();
                    tick.set(cur + 1);
                }
                Err(e) => push_toast(bus, ToastKind::Error, format!("Failed: {e}")),
            }
        });
    };

    rsx! {
        aside { class: "comments-pane",
            header { class: "comments-pane-header",
                strong { "Comments" }
                button {
                    class: "btn-icon",
                    aria_label: "Close comments",
                    onclick: move |_| open_sig.set(false),
                    "\u{2715}"
                }
            }
            div { class: "comments-list",
                match comments.read().as_ref() {
                    None => rsx! { p { class: "muted", "Loading\u{2026}" } },
                    Some(Err(e)) => rsx! { p { class: "muted", "Failed: {e}" } },
                    Some(Ok(items)) if items.is_empty() => rsx! { p { class: "muted", "No comments yet." } },
                    Some(Ok(items)) => rsx! {
                        for c in items.iter().cloned() {
                            div { class: "comment-item", key: "{c.id}",
                                div { class: "comment-meta",
                                    strong {
                                        {
                                            c.display_name
                                                .clone()
                                                .unwrap_or_else(|| c.username.clone())
                                        }
                                    }
                                    span { class: "muted", " \u{00b7} {c.created_at}" }
                                }
                                p { "{c.content}" }
                            }
                        }
                    }
                }
            }
            div { class: "comments-compose",
                textarea {
                    class: "form-input",
                    rows: 3,
                    placeholder: "Add a comment...",
                    aria_label: "New comment",
                    value: "{draft}",
                    oninput: move |e| draft.set(e.value()),
                }
                button {
                    class: "btn btn-primary btn-sm",
                    aria_label: "Post comment",
                    onclick: post,
                    "Post"
                }
            }
        }
    }
}
