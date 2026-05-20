use dioxus::prelude::*;

use crate::Route;

/// Left sidebar with navigation tree and quick-access links.
#[component]
pub fn Sidebar() -> Element {
    let mut expanded_sections = use_signal(|| vec![true, false, false]);

    rsx! {
        aside { class: "sidebar",
            // Quick-create section
            div { class: "sidebar-section",
                h4 { class: "sidebar-heading", "Create New" }
                div { class: "sidebar-actions",
                    Link { to: Route::Writer { id: "new".to_string() }, class: "sidebar-action",
                        span { class: "action-icon", "+" }
                        span { "Document" }
                    }
                    Link { to: Route::Calc { id: "new".to_string() }, class: "sidebar-action",
                        span { class: "action-icon", "+" }
                        span { "Spreadsheet" }
                    }
                    Link { to: Route::Slides { id: "new".to_string() }, class: "sidebar-action",
                        span { class: "action-icon", "+" }
                        span { "Presentation" }
                    }
                    Link { to: Route::Whiteboard { id: "new".to_string() }, class: "sidebar-action",
                        span { class: "action-icon", "+" }
                        span { "Whiteboard" }
                    }
                }
            }

            // File tree section
            div { class: "sidebar-section",
                button {
                    class: "sidebar-heading sidebar-toggle",
                    onclick: move |_| {
                        let mut v = expanded_sections.read().clone();
                        v[0] = !v[0];
                        expanded_sections.set(v);
                    },
                    span { if expanded_sections.read()[0] { "v " } else { "> " } }
                    span { "My Files" }
                }
                if expanded_sections.read()[0] {
                    div { class: "sidebar-tree",
                        div { class: "tree-item",
                            span { class: "tree-icon folder-icon", "" }
                            span { "Documents" }
                        }
                        div { class: "tree-item",
                            span { class: "tree-icon folder-icon", "" }
                            span { "Spreadsheets" }
                        }
                        div { class: "tree-item",
                            span { class: "tree-icon folder-icon", "" }
                            span { "Presentations" }
                        }
                        div { class: "tree-item",
                            span { class: "tree-icon folder-icon", "" }
                            span { "Whiteboards" }
                        }
                    }
                }
            }

            // Shared section
            div { class: "sidebar-section",
                button {
                    class: "sidebar-heading sidebar-toggle",
                    onclick: move |_| {
                        let mut v = expanded_sections.read().clone();
                        v[1] = !v[1];
                        expanded_sections.set(v);
                    },
                    span { if expanded_sections.read()[1] { "v " } else { "> " } }
                    span { "Shared with me" }
                }
                if expanded_sections.read()[1] {
                    div { class: "sidebar-tree",
                        p { class: "tree-empty", "No shared items" }
                    }
                }
            }

            // Trash section
            div { class: "sidebar-section",
                button {
                    class: "sidebar-heading sidebar-toggle",
                    onclick: move |_| {
                        let mut v = expanded_sections.read().clone();
                        v[2] = !v[2];
                        expanded_sections.set(v);
                    },
                    span { if expanded_sections.read()[2] { "v " } else { "> " } }
                    span { "Trash" }
                }
                if expanded_sections.read()[2] {
                    div { class: "sidebar-tree",
                        p { class: "tree-empty", "Trash is empty" }
                    }
                }
            }

            // Storage indicator
            div { class: "sidebar-storage",
                div { class: "storage-label",
                    span { "Storage" }
                    span { "0 MB / 1 GB" }
                }
                div { class: "storage-bar",
                    div { class: "storage-fill", style: "width: 0%" }
                }
            }
        }
    }
}
