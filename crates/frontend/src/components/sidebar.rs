use dioxus::prelude::*;
use serde::Deserialize;

use crate::Route;
use crate::state::{AuthState, api_get};

#[derive(Clone, Deserialize, PartialEq)]
struct StorageInfo {
    storage_used: i64,
    storage_quota: i64,
}

fn format_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Left sidebar with quick-create links and a live storage indicator.
#[component]
pub fn Sidebar() -> Element {
    let _ = use_context::<Signal<String>>().read();
    let auth = use_context::<Signal<AuthState>>();

    let me = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        async move { api_get::<StorageInfo>("/auth/me", &token).await.ok() }
    });

    rsx! {
        aside { class: "sidebar",
            // Quick-create section
            div { class: "sidebar-section",
                h4 { class: "sidebar-heading", {crate::i18n::t("sidebar.create_new")} }
                div { class: "sidebar-actions",
                    Link { to: Route::Writer { id: "new".to_string() }, class: "sidebar-action",
                        span { class: "action-icon", "+" }
                        span { {crate::i18n::t("sidebar.document")} }
                    }
                    Link { to: Route::Calc { id: "new".to_string() }, class: "sidebar-action",
                        span { class: "action-icon", "+" }
                        span { {crate::i18n::t("sidebar.spreadsheet")} }
                    }
                    Link { to: Route::Slides { id: "new".to_string() }, class: "sidebar-action",
                        span { class: "action-icon", "+" }
                        span { {crate::i18n::t("sidebar.presentation")} }
                    }
                    Link { to: Route::Whiteboard { id: "new".to_string() }, class: "sidebar-action",
                        span { class: "action-icon", "+" }
                        span { {crate::i18n::t("sidebar.whiteboard")} }
                    }
                }
            }

            // Storage indicator backed by live profile data
            if let Some(Some(info)) = me.read().as_ref() {
                {
                    let pct = if info.storage_quota > 0 {
                        ((info.storage_used as f64 / info.storage_quota as f64) * 100.0).min(100.0)
                    } else {
                        0.0
                    };
                    let used = format_size(info.storage_used);
                    let quota = format_size(info.storage_quota);
                    rsx! {
                        div { class: "sidebar-storage",
                            div { class: "storage-label",
                                span { {crate::i18n::t("sidebar.storage")} }
                                span { "{used} / {quota}" }
                            }
                            div { class: "storage-bar",
                                div { class: "storage-fill", style: "width: {pct}%" }
                            }
                        }
                    }
                }
            }
        }
    }
}
