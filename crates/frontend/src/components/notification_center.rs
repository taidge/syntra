use dioxus::prelude::*;
use serde::Serialize;

use crate::components::{Toast, ToastKind, push_toast};
use crate::notification_client::{
    NotificationConnectionState, NotificationEvent, NotificationItem, NotificationSocket,
    connect_notifications, merge_notification,
};
use crate::state::{AuthState, api_get, api_put};

#[derive(Serialize)]
struct EmptyBody {}

#[component]
pub fn NotificationCenter() -> Element {
    let auth = use_context::<Signal<AuthState>>();
    let toast_bus = use_context::<Signal<Vec<Toast>>>();
    let token = auth.read().token.clone().unwrap_or_default();
    let mut open = use_signal(|| false);
    let mut notifications = use_signal(Vec::<NotificationItem>::new);
    let mut refresh = use_signal(|| 0_u32);
    let mut connection = use_signal(|| NotificationConnectionState::Connecting);
    let mut socket = use_signal(|| None::<NotificationSocket>);

    let token_for_fetch = token.clone();
    let fetched = use_resource(move || {
        let token = token_for_fetch.clone();
        let _refresh = *refresh.read();
        async move {
            if token.is_empty() {
                Ok(Vec::new())
            } else {
                api_get::<Vec<NotificationItem>>("/notifications", &token).await
            }
        }
    });
    use_effect(move || {
        if let Some(Ok(items)) = fetched.read().as_ref() {
            let mut current = notifications.write();
            for item in items.iter().rev().cloned() {
                merge_notification(&mut current, item);
            }
        }
    });

    let token_for_socket = token.clone();
    use_effect(move || {
        socket.set(None);
        if token_for_socket.is_empty() {
            notifications.write().clear();
            return;
        }
        let bus = toast_bus;
        let handle = connect_notifications(
            token_for_socket.clone(),
            move |event| match event {
                NotificationEvent::Notification(item) => {
                    push_toast(bus, ToastKind::Info, item.title.clone());
                    merge_notification(&mut notifications.write(), item);
                }
                NotificationEvent::Control { event } if event == "resync" => refresh += 1,
                NotificationEvent::Control { .. } => {}
            },
            move |state| {
                connection.set(state);
                if state == NotificationConnectionState::Connected {
                    refresh += 1;
                }
            },
        );
        socket.set(Some(handle));
    });

    let unread = notifications
        .read()
        .iter()
        .filter(|notification| !notification.is_read)
        .count();
    let connection_label = match *connection.read() {
        NotificationConnectionState::Connected => crate::i18n::t("notifications.live"),
        NotificationConnectionState::Failed => crate::i18n::t("notifications.offline"),
        NotificationConnectionState::Connecting | NotificationConnectionState::Reconnecting(_) => {
            crate::i18n::t("notifications.reconnecting")
        }
    };

    rsx! {
        div { class: "notification-center",
            button {
                class: "notification-button",
                aria_label: crate::i18n::t("notifications.title"),
                title: crate::i18n::t("notifications.title"),
                onclick: move |_| open.toggle(),
                "🔔"
                if unread > 0 {
                    span { class: "notification-badge", "{unread}" }
                }
            }
            if *open.read() {
                div { class: "notification-panel",
                    header { class: "notification-header",
                        div {
                            strong { {crate::i18n::t("notifications.title")} }
                            span { class: "notification-connection", "{connection_label}" }
                        }
                        if unread > 0 {
                            button {
                                class: "btn-link",
                                onclick: {
                                    let token = token.clone();
                                    move |_| {
                                        let token = token.clone();
                                        wasm_bindgen_futures::spawn_local(async move {
                                            if api_put::<serde_json::Value, _>("/notifications/read-all", &EmptyBody {}, &token).await.is_ok() {
                                                for notification in notifications.write().iter_mut() {
                                                    notification.is_read = true;
                                                }
                                            }
                                        });
                                    }
                                },
                                {crate::i18n::t("notifications.mark_all_read")}
                            }
                        }
                    }
                    div { class: "notification-list",
                        if notifications.read().is_empty() {
                            p { class: "notification-empty", {crate::i18n::t("notifications.empty")} }
                        } else {
                            for item in notifications.read().iter().cloned() {
                                {
                                    let id = item.id.clone();
                                    let token = token.clone();
                                    rsx! {
                                        article {
                                            key: "{id}",
                                            class: if item.is_read { "notification-item" } else { "notification-item unread" },
                                            onclick: move |_| {
                                                if notifications.read().iter().any(|value| value.id == id && value.is_read) {
                                                    return;
                                                }
                                                let id_for_request = id.clone();
                                                let token = token.clone();
                                                wasm_bindgen_futures::spawn_local(async move {
                                                    if api_put::<serde_json::Value, _>(
                                                        &format!("/notifications/{id_for_request}/read"),
                                                        &EmptyBody {},
                                                        &token,
                                                    ).await.is_ok()
                                                        && let Some(item) = notifications.write().iter_mut().find(|item| item.id == id_for_request)
                                                    {
                                                        item.is_read = true;
                                                    }
                                                });
                                            },
                                            div { class: "notification-item-header",
                                                strong { "{item.title}" }
                                                span { "{item.created_at}" }
                                            }
                                            if !item.message.is_empty() {
                                                p { "{item.message}" }
                                            }
                                            if let Some(link) = item.link {
                                                a {
                                                    href: "{link}",
                                                    onclick: move |event| event.stop_propagation(),
                                                    {crate::i18n::t("notifications.open")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
