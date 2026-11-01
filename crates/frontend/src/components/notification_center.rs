#[cfg(target_arch = "wasm32")]
use crate::notification_client::{self, NotificationEvent};
use crate::{
    components::ui::*,
    notification_client::{NotificationConnectionState, NotificationItem},
    state::{self, AuthState},
};
use glory::prelude::*;
#[cfg(target_arch = "wasm32")]
fn refresh(
    scope: &Scope,
    auth: Cage<AuthState>,
    items: Cage<Vec<NotificationItem>>,
    feedback: Cage<String>,
) {
    let token = token(auth);
    let _ = scope.spawn(async move {
        match state::api_get::<Vec<NotificationItem>>("/notifications", &token).await {
            Ok(value) => {
                let _ = items.try_revise(|items| {
                    for item in value.into_iter().rev() {
                        notification_client::merge_notification(items, item);
                    }
                });
            }
            Err(error) => set(feedback, error),
        }
    });
}
#[allow(non_snake_case)]
pub fn NotificationCenter(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    let items = scope.cage(Vec::<NotificationItem>::new());
    let open = scope.cage(false);
    let connection = scope.cage(NotificationConnectionState::Connecting);
    let feedback = scope.cage(String::new());
    #[cfg(target_arch = "wasm32")]
    {
        let token_value = token(auth);
        if !token_value.is_empty() {
            refresh(&scope, auth, items, feedback);
            let events_weak = scope.downgrade();
            let state_weak = scope.downgrade();
            let socket = notification_client::connect_notifications(
                token_value,
                move |event| match event {
                    NotificationEvent::Notification(item) => {
                        let _ = items.try_revise(|items| {
                            notification_client::merge_notification(items, item)
                        });
                    }
                    NotificationEvent::Control { event } if event == "resync" => {
                        if let Some(scope) = events_weak.upgrade() {
                            refresh(&scope, auth, items, feedback);
                        }
                    }
                    NotificationEvent::Control { .. } => {}
                },
                move |value| {
                    set(connection, value);
                    if value == NotificationConnectionState::Connected
                        && let Some(scope) = state_weak.upgrade()
                    {
                        refresh(&scope, auth, items, feedback);
                    }
                },
            );
            let _ = scope.retain(socket);
        }
    }
    let weak = scope.downgrade();
    let read_weak = scope.downgrade();
    view().class("notification-center notifications").fill((
        sapid::button()
            .theme(theme())
            .class("notification-button")
            .name(scope.bond(move || {
                format!(
                    "{} ({})",
                    crate::i18n::t("notifications.title"),
                    items.get().iter().filter(|item| !item.is_read).count()
                )
            }))
            .label("🔔")
            .unstyled()
            .on_press(move || set(open, !open.get())),
        when(open, move || {
            let weak = weak.clone();
            let read_weak = read_weak.clone();
            col().class("notification-panel").fill((
                row().class("notification-header").fill((
                    col().fill((
                        text().content(crate::i18n::t("notifications.title")),
                        text()
                            .class("notification-connection")
                            .content_fn(move || format!("{:?}", connection.get())),
                    )),
                    action(crate::i18n::t("notifications.mark_all_read"), move || {
                        let Some(scope) = weak.upgrade() else {
                            return;
                        };
                        let token = token(auth);
                        let _ = scope.spawn(async move {
                            match state::api_put::<serde_json::Value, _>(
                                "/notifications/read-all",
                                &serde_json::json!({}),
                                &token,
                            )
                            .await
                            {
                                Ok(_) => {
                                    let _ = items.try_revise(|items| {
                                        for item in items {
                                            item.is_read = true;
                                        }
                                    });
                                }
                                Err(error) => set(feedback, error),
                            }
                        });
                    })
                    .class("btn-link"),
                )),
                status(feedback),
                when(items.map(|items| items.is_empty()), || {
                    label(crate::i18n::t("notifications.empty")).class("notification-empty")
                }),
                col().class("notification-list").fill(
                    each(items)
                        .key(|item| item.id.clone())
                        .children(move |item_row| {
                            let item = item_row.value;
                            let weak = read_weak.clone();
                            let notification = item.get();
                            let id = notification.id;
                            let target = notification.link.clone();
                            col()
                                .class(item.map(|item| {
                                    if item.is_read {
                                        "notification-item".to_owned()
                                    } else {
                                        "notification-item unread".to_owned()
                                    }
                                }))
                                .fill((
                                    label(notification.title).class("notification-item-header"),
                                    label(notification.message).class("notification-message"),
                                    action("Open / mark read", move || {
                                        let Some(scope) = weak.upgrade() else {
                                            return;
                                        };
                                        let token = token(auth);
                                        let id = id.clone();
                                        let target = target.clone();
                                        let _ = scope.spawn(async move {
                                            match state::api_put::<serde_json::Value, _>(
                                                &format!("/notifications/{id}/read"),
                                                &serde_json::json!({}),
                                                &token,
                                            )
                                            .await
                                            {
                                                Ok(_) => {
                                                    let _ = items.try_revise(|items| {
                                                        if let Some(item) = items
                                                            .iter_mut()
                                                            .find(|item| item.id == id)
                                                        {
                                                            item.is_read = true;
                                                        }
                                                    });
                                                    if let Some(target) = target {
                                                        go(&target);
                                                    }
                                                }
                                                Err(error) => set(feedback, error),
                                            }
                                        });
                                    })
                                    .class("btn-link"),
                                ))
                        }),
                ),
            ))
        }),
    ))
}
