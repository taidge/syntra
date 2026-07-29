use dioxus::prelude::*;

use crate::Route;
use crate::components::navbar::Navbar;
use crate::state::{UserProfile, api_get};

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

/// Main dashboard displayed after the user logs in.
#[component]
pub fn Dashboard() -> Element {
    let _ = use_context::<Signal<String>>().read();
    let auth = use_context::<Signal<crate::state::AuthState>>();

    // Fetch recent documents from the API.
    let recent_docs = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        async move {
            crate::state::api_get::<Vec<crate::state::FileResponse>>("/recent", &token)
                .await
                .unwrap_or_default()
        }
    });

    let me = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        async move { api_get::<UserProfile>("/auth/me", &token).await }
    });

    let welcome = crate::i18n::t("nav.dashboard");
    let recent_label = crate::i18n::t("dashboard.recent");
    let loading_label = crate::i18n::t("common.loading");
    let empty_recent_label = crate::i18n::t("dashboard.recent_empty");
    let new_writer = crate::i18n::t("dashboard.new_writer");
    let new_calc = crate::i18n::t("dashboard.new_calc");
    let new_slides = crate::i18n::t("dashboard.new_slides");
    let new_whiteboard = crate::i18n::t("dashboard.new_whiteboard");
    let template_label = crate::i18n::t("dashboard.from_template");
    let activity_label = crate::i18n::t("dashboard.activity");
    let storage_label = crate::i18n::t("profile.storage");

    rsx! {
        Navbar {}
        div { class: "dashboard",
            // Header with quick-create actions
            div { class: "dashboard-header",
                h1 { "{welcome}" }
                div { class: "quick-actions",
                    Link { to: Route::Writer { id: "new".to_string() }, class: "quick-action-btn writer",
                        span { class: "qa-icon", "W" }
                        span { "{new_writer}" }
                    }
                    Link { to: Route::Calc { id: "new".to_string() }, class: "quick-action-btn calc",
                        span { class: "qa-icon", "C" }
                        span { "{new_calc}" }
                    }
                    Link { to: Route::Slides { id: "new".to_string() }, class: "quick-action-btn slides",
                        span { class: "qa-icon", "S" }
                        span { "{new_slides}" }
                    }
                    Link { to: Route::Whiteboard { id: "new".to_string() }, class: "quick-action-btn whiteboard",
                        span { class: "qa-icon", "B" }
                        span { "{new_whiteboard}" }
                    }
                    Link { to: Route::FileManager {}, class: "quick-action-btn",
                        span { class: "qa-icon", "T" }
                        span { "{template_label}" }
                    }
                }
            }

            // Recent documents
            div { class: "recent-documents",
                h2 { "{recent_label}" }
                div { class: "doc-grid",
                    // Render API-fetched recent documents if available
                    match &*recent_docs.read() {
                        Some(docs) if !docs.is_empty() => rsx! {
                            for doc in docs.iter() {
                                {
                                    let doc_id = doc.id.to_string();
                                    let preview_class = if doc.is_folder {
                                        "doc-card-preview"
                                    } else {
                                        match doc.mime_type.as_deref() {
                                            Some(m) if m.contains("spreadsheet") || m.contains("calc") => "doc-card-preview calc-preview",
                                            Some(m) if m.contains("presentation") || m.contains("slides") => "doc-card-preview slides-preview",
                                            Some(m) if m.contains("whiteboard") => "doc-card-preview whiteboard-preview",
                                            _ => "doc-card-preview writer-preview",
                                        }
                                    };
                                    let icon = if doc.is_folder {
                                        "F"
                                    } else {
                                        match doc.mime_type.as_deref() {
                                            Some(m) if m.contains("spreadsheet") || m.contains("calc") => "C",
                                            Some(m) if m.contains("presentation") || m.contains("slides") => "S",
                                            Some(m) if m.contains("whiteboard") => "B",
                                            _ => "W",
                                        }
                                    };
                                    rsx! {
                                        Link {
                                            to: Route::DocumentEditor { id: doc_id },
                                            class: "doc-card",
                                            div { class: "{preview_class}",
                                                span { "{icon}" }
                                            }
                                            div { class: "doc-card-info",
                                                h4 { "{doc.name}" }
                                                p { class: "doc-meta", "Modified {doc.updated_at}" }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        // Still loading: show a neutral loading hint.
                        None => rsx! {
                            p { class: "doc-meta", "{loading_label}" }
                        },
                        // Loaded but empty: honest empty state, no fabricated docs.
                        _ => rsx! {
                            p { class: "doc-meta", "{empty_recent_label}" }
                        },
                    }
                }
            }

            // Storage usage
            div { class: "storage-info",
                h3 { "{storage_label}" }
                match me.read().as_ref() {
                    None => rsx! {
                        p { class: "storage-text", "{loading_label}" }
                    },
                    Some(Err(_)) => rsx! {
                        p { class: "storage-text", {crate::i18n::t("dashboard.storage_unavailable")} }
                    },
                    Some(Ok(profile)) => {
                        let pct = if profile.storage_quota > 0 {
                            ((profile.storage_used as f64 / profile.storage_quota as f64) * 100.0)
                                .clamp(0.0, 100.0)
                        } else {
                            0.0
                        };
                        let used = format_size(profile.storage_used);
                        let quota = format_size(profile.storage_quota);
                        let usage = crate::i18n::t("dashboard.storage_usage")
                            .replace("{used}", &used)
                            .replace("{quota}", &quota);
                        rsx! {
                            div { class: "storage-details",
                                div { class: "storage-bar-large",
                                    div { class: "storage-fill", style: "width: {pct}%" }
                                }
                                p { class: "storage-text", "{usage}" }
                            }
                        }
                    },
                }
            }

            // Activity feed
            div { class: "activity-feed",
                h3 { "{activity_label}" }
                div { class: "activity-list",
                    // Render activity from fetched recent docs if available
                    match &*recent_docs.read() {
                        Some(docs) if !docs.is_empty() => rsx! {
                            for doc in docs.iter().take(5) {
                                {
                                    let edited = crate::i18n::t("dashboard.activity_edited")
                                        .replace("{name}", &doc.name);
                                    let modified = crate::i18n::t("dashboard.modified")
                                        .replace("{when}", &doc.updated_at);
                                    rsx! {
                                        div { class: "activity-item",
                                            span { class: "activity-dot" }
                                            span { "{edited}" }
                                            span { class: "activity-time", "{modified}" }
                                        }
                                    }
                                }
                            }
                        },
                        None => rsx! {
                            p { class: "doc-meta", "{loading_label}" }
                        },
                        _ => rsx! {
                            p { class: "doc-meta", {crate::i18n::t("dashboard.activity_empty")} }
                        },
                    }
                }
            }
        }
    }
}
