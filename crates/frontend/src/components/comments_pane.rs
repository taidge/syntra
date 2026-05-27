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

fn render_with_mentions(content: &str) -> Element {
    let mut out: Vec<(bool, String)> = Vec::new();
    let mut current = String::new();
    let mut in_mention = false;
    for ch in content.chars() {
        if !in_mention && ch == '@' {
            if !current.is_empty() {
                out.push((false, std::mem::take(&mut current)));
            }
            in_mention = true;
            current.push(ch);
        } else if in_mention && (ch.is_alphanumeric() || ch == '_') {
            current.push(ch);
        } else if in_mention {
            out.push((true, std::mem::take(&mut current)));
            in_mention = false;
            current.push(ch);
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        out.push((in_mention, current));
    }

    rsx! {
        p {
            for (i, (is_mention, text)) in out.into_iter().enumerate() {
                if is_mention {
                    span { key: "{i}", class: "mention", "{text}" }
                } else {
                    span { key: "{i}", "{text}" }
                }
            }
        }
    }
}

fn mention_prefix(s: &str) -> Option<String> {
    let at = s.rfind('@')?;
    let before_ok = at == 0
        || s[..at]
            .chars()
            .next_back()
            .map(|c| !c.is_alphanumeric() && c != '_')
            .unwrap_or(true);
    if !before_ok {
        return None;
    }
    let tail = &s[at + 1..];
    if tail.chars().any(|c| !c.is_alphanumeric() && c != '_') {
        return None;
    }
    Some(tail.to_string())
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
    let mut suggestions = use_signal(Vec::<String>::new);
    let mut selected_idx = use_signal(|| 0usize);
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
        let mut local_sugg = suggestions;
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
                    local_sugg.write().clear();
                    let cur = *tick.read();
                    tick.set(cur + 1);
                }
                Err(e) => push_toast(bus, ToastKind::Error, format!("Failed: {e}")),
            }
        });
    };

    let pool: Vec<String> = {
        let mut v: Vec<String> = vec![
            "alice".to_string(),
            "bob".to_string(),
            "charlie".to_string(),
            "demo".to_string(),
        ];
        if let Some(u) = auth.read().user.as_ref() {
            let me = u.username.clone();
            if !v.iter().any(|x| x == &me) {
                v.insert(0, me);
            }
        }
        v
    };

    let mut insert_mention = move |name: String| {
        let mut v = draft.read().clone();
        if let Some(idx) = v.rfind('@') {
            v.truncate(idx);
            v.push('@');
            v.push_str(&name);
            v.push(' ');
            draft.set(v);
        }
        suggestions.write().clear();
        selected_idx.set(0);
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
                                {render_with_mentions(&c.content)}
                            }
                        }
                    }
                }
            }
            div { class: "comments-compose",
                if !suggestions.read().is_empty() {
                    div { class: "mention-suggestions", role: "listbox",
                        for (i, s) in suggestions.read().iter().cloned().enumerate() {
                            button {
                                key: "{s}",
                                class: if i == *selected_idx.read() { "mention-suggestion active" } else { "mention-suggestion" },
                                r#type: "button",
                                role: "option",
                                onmousedown: move |e| e.prevent_default(),
                                onclick: {
                                    let name = s.clone();
                                    move |_| insert_mention(name.clone())
                                },
                                "@{s}"
                            }
                        }
                    }
                }
                textarea {
                    class: "form-input",
                    rows: 3,
                    placeholder: "Add a comment...",
                    aria_label: "New comment",
                    value: "{draft}",
                    oninput: {
                        let pool = pool.clone();
                        move |e: FormEvent| {
                            let v = e.value();
                            draft.set(v.clone());
                            if let Some(prefix) = mention_prefix(&v) {
                                let p = prefix.to_lowercase();
                                let matches: Vec<String> = pool
                                    .iter()
                                    .filter(|u| u.to_lowercase().starts_with(&p))
                                    .cloned()
                                    .collect();
                                suggestions.set(matches);
                                selected_idx.set(0);
                            } else {
                                suggestions.write().clear();
                                selected_idx.set(0);
                            }
                        }
                    },
                    onkeydown: move |e: KeyboardEvent| {
                        let has_sugg = !suggestions.read().is_empty();
                        if !has_sugg {
                            return;
                        }
                        match e.key() {
                            Key::ArrowDown => {
                                e.prevent_default();
                                let len = suggestions.read().len();
                                let cur = *selected_idx.read();
                                selected_idx.set((cur + 1) % len);
                            }
                            Key::ArrowUp => {
                                e.prevent_default();
                                let len = suggestions.read().len();
                                let cur = *selected_idx.read();
                                selected_idx.set((cur + len - 1) % len);
                            }
                            Key::Enter => {
                                e.prevent_default();
                                let idx = *selected_idx.read();
                                let name = suggestions.read().get(idx).cloned();
                                if let Some(n) = name {
                                    insert_mention(n);
                                }
                            }
                            Key::Escape => {
                                e.prevent_default();
                                suggestions.write().clear();
                                selected_idx.set(0);
                            }
                            _ => {}
                        }
                    },
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
