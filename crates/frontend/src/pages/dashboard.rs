use dioxus::prelude::*;

use crate::Route;
use crate::components::navbar::Navbar;

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

    let welcome = crate::i18n::t("nav.dashboard");
    let recent_label = crate::i18n::t("dashboard.recent");
    let new_writer = crate::i18n::t("dashboard.new_writer");
    let new_calc = crate::i18n::t("dashboard.new_calc");
    let new_slides = crate::i18n::t("dashboard.new_slides");
    let new_whiteboard = crate::i18n::t("dashboard.new_whiteboard");
    let template_label = crate::i18n::t("dashboard.from_template");
    let activity_label = crate::i18n::t("dashboard.activity");
    let storage_text = crate::i18n::t("dashboard.storage_text");
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
                        // Fallback: show placeholder cards while loading or if empty
                        _ => rsx! {
                            div { class: "doc-card",
                                div { class: "doc-card-preview writer-preview",
                                    span { "W" }
                                }
                                div { class: "doc-card-info",
                                    h4 { "Project Proposal" }
                                    p { class: "doc-meta", "Modified 2 hours ago" }
                                }
                            }
                            div { class: "doc-card",
                                div { class: "doc-card-preview calc-preview",
                                    span { "C" }
                                }
                                div { class: "doc-card-info",
                                    h4 { "Budget 2026" }
                                    p { class: "doc-meta", "Modified yesterday" }
                                }
                            }
                            div { class: "doc-card",
                                div { class: "doc-card-preview slides-preview",
                                    span { "S" }
                                }
                                div { class: "doc-card-info",
                                    h4 { "Q1 Review" }
                                    p { class: "doc-meta", "Modified 3 days ago" }
                                }
                            }
                            div { class: "doc-card",
                                div { class: "doc-card-preview whiteboard-preview",
                                    span { "B" }
                                }
                                div { class: "doc-card-info",
                                    h4 { "Architecture Diagram" }
                                    p { class: "doc-meta", "Modified last week" }
                                }
                            }
                        },
                    }
                }
            }

            // Storage usage
            div { class: "storage-info",
                h3 { "{storage_label}" }
                div { class: "storage-details",
                    div { class: "storage-bar-large",
                        div { class: "storage-fill", style: "width: 12%" }
                    }
                    p { class: "storage-text", "{storage_text}" }
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
                                div { class: "activity-item",
                                    span { class: "activity-dot" }
                                    span { "You edited " }
                                    strong { "{doc.name}" }
                                    span { class: "activity-time", " - {doc.updated_at}" }
                                }
                            }
                        },
                        // Fallback: show placeholder activity while loading
                        _ => rsx! {
                            div { class: "activity-item",
                                span { class: "activity-dot" }
                                span { "You edited " }
                                strong { "Project Proposal" }
                                span { class: "activity-time", " - 2 hours ago" }
                            }
                            div { class: "activity-item",
                                span { class: "activity-dot" }
                                span { "You created " }
                                strong { "Budget 2026" }
                                span { class: "activity-time", " - yesterday" }
                            }
                            div { class: "activity-item",
                                span { class: "activity-dot" }
                                span { "You shared " }
                                strong { "Q1 Review" }
                                span { " with 3 people" }
                                span { class: "activity-time", " - 3 days ago" }
                            }
                        },
                    }
                }
            }
        }
    }
}
