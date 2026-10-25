use crate::{
    components::ui::*,
    state::{self, AuthState},
};
use glory::prelude::*;
use serde::Deserialize;
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
    #[serde(default)]
    pub parent_id: Option<String>,
}
fn mention_prefix(value: &str) -> Option<String> {
    let at = value.rfind('@')?;
    if at > 0
        && value[..at]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_alphanumeric() || character == '_')
    {
        return None;
    }
    let tail = &value[at + 1..];
    if tail
        .chars()
        .all(|character| character.is_alphanumeric() || character == '_')
    {
        Some(tail.into())
    } else {
        None
    }
}
fn refresh(
    scope: &Scope,
    auth: Cage<AuthState>,
    id: String,
    items: Cage<Vec<CommentItem>>,
    feedback: Cage<String>,
) {
    let token = token(auth);
    let _ = scope.spawn(async move {
        match state::api_get::<Vec<CommentItem>>(&format!("/documents/{id}/comments"), &token).await
        {
            Ok(value) => set(items, value),
            Err(error) => set(feedback, error),
        }
    });
}
#[allow(non_snake_case)]
pub fn CommentsPane(
    scope: Scope,
    auth: Cage<AuthState>,
    doc_id: String,
    open: Cage<bool>,
) -> impl Widget {
    let items = scope.cage(Vec::<CommentItem>::new());
    let draft = scope.cage(String::new());
    let reply = scope.cage(None::<String>);
    let feedback = scope.cage(String::new());
    refresh(&scope, auth, doc_id.clone(), items, feedback);
    let weak = scope.downgrade();
    let suggestions = scope.bond(move || {
        let Some(prefix) = mention_prefix(&draft.get()) else {
            return Vec::new();
        };
        let mut names = items
            .get()
            .into_iter()
            .map(|item| item.username)
            .collect::<Vec<_>>();
        if let Some(user) = auth.get().user {
            names.push(user.username);
        }
        names.sort();
        names.dedup();
        names
            .into_iter()
            .filter(|name| name.starts_with(&prefix))
            .take(8)
            .collect::<Vec<_>>()
    });
    when(open, move || {
        let weak = weak.clone();
        let id = doc_id.clone();
        col().class("comments-pane").fill((
            row().class("comments-pane-header").fill((
                text()
                    .kind(TextKind::Heading(2))
                    .content(crate::i18n::t("comments.title")),
                action("×", move || set(open, false))
                    .name("Close comments")
                    .class("btn-icon"),
            )),
            col().class("comments-list").fill((
                when(items.map(|items| items.is_empty()), || {
                    label(crate::i18n::t("comments.empty")).class("pane-empty")
                }),
                each(
                    items,
                    |item| item.id.clone(),
                    move |item, _| {
                        let comment = item.get();
                        let parent = comment.id.clone();
                        col()
                            .class(item.map(|item| {
                                if item.parent_id.is_some() {
                                    "comment-item comment-reply".to_owned()
                                } else {
                                    "comment-item".to_owned()
                                }
                            }))
                            .fill((
                                label(format!(
                                    "{} · {}",
                                    comment.display_name.unwrap_or(comment.username),
                                    comment.created_at
                                ))
                                .class("comment-meta"),
                                label(comment.content).class("comment-body"),
                                action("Reply", move || set(reply, Some(parent.clone())))
                                    .class("btn-link comment-reply-btn"),
                            ))
                    },
                ),
            )),
            col().class("comments-compose").fill((
                text().content_fn(move || {
                    reply
                        .get()
                        .map(|id| format!("Reply to {id}"))
                        .unwrap_or_default()
                }),
                sapid::text_field()
                    .theme(theme())
                    .class("form-input")
                    .multiline(true)
                    .name(crate::i18n::t("comments.placeholder"))
                    .value(draft)
                    .on_value_change(move |value| set(draft, value)),
                row().fill(each(
                    suggestions,
                    |name| name.clone(),
                    move |name, _| {
                        action(format!("@{}", name.get()), move || {
                            let current = draft.get();
                            if let Some(at) = current.rfind('@') {
                                set(draft, format!("{}@{} ", &current[..at], name.get()));
                            }
                        })
                    },
                )),
                row().class("comment-compose-actions").fill((
                    when(reply.map(|reply| reply.is_some()), move || {
                        action("Cancel reply", move || set(reply, None)).class("btn-link")
                    }),
                    action(crate::i18n::t("comments.new_comment"), move || {
                        if draft.get().trim().is_empty() {
                            return;
                        }
                        let Some(scope) = weak.upgrade() else {
                            return;
                        };
                        let token = token(auth);
                        let id = id.clone();
                        let body =
                            serde_json::json!({"content":draft.get(),"parent_id":reply.get()});
                        let weak = scope.downgrade();
                        let _ = scope.spawn(async move {
                            match state::api_post::<serde_json::Value, _>(
                                &format!("/documents/{id}/comments"),
                                &body,
                                &token,
                            )
                            .await
                            {
                                Ok(_) => {
                                    set(draft, String::new());
                                    set(reply, None);
                                    if let Some(scope) = weak.upgrade() {
                                        refresh(&scope, auth, id, items, feedback);
                                    }
                                }
                                Err(error) => set(feedback, error),
                            }
                        });
                    })
                    .class("btn btn-primary btn-sm"),
                )),
                status(feedback),
            )),
        ))
    })
}
