use dioxus::prelude::*;

use crate::Route;
use crate::components::navbar::Navbar;

/// Generic document editor that loads the document type from the API and
/// routes to the appropriate specialised editor (Writer, Calc, Slides, or
/// Whiteboard).
///
/// While the document metadata is being fetched, a loading indicator is shown.
#[component]
pub fn DocumentEditor(id: String) -> Element {
    let doc_id = id.clone();
    let auth = use_context::<Signal<crate::state::AuthState>>();

    // Fetch document metadata from the API to determine its type.
    let doc_id_for_resource = doc_id.clone();
    let doc_meta = use_resource(move || {
        let doc_id = doc_id_for_resource.clone();
        let token = auth.read().token.clone().unwrap_or_default();
        async move {
            crate::state::api_get::<serde_json::Value>(&format!("/documents/{}", doc_id), &token)
                .await
                .ok()
        }
    });

    // Determine the doc_type from the fetched metadata (if available).
    let detected_type = match &*doc_meta.read() {
        Some(Some(meta)) => meta
            .get("doc_type")
            .or_else(|| meta.get("mime_type"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    };

    // If we have detected the type, show a redirect link at the top.
    let redirect_route = detected_type.as_deref().and_then(|dt| match dt {
        "writer" | "document" | "application/vnd.contrix.writer" => {
            Some(Route::Writer { id: doc_id.clone() })
        }
        "calc" | "spreadsheet" | "application/vnd.contrix.calc" => {
            Some(Route::Calc { id: doc_id.clone() })
        }
        "slides" | "presentation" | "application/vnd.contrix.slides" => {
            Some(Route::Slides { id: doc_id.clone() })
        }
        "whiteboard" | "application/vnd.contrix.whiteboard" => {
            Some(Route::Whiteboard { id: doc_id.clone() })
        }
        _ => None,
    });

    rsx! {
        Navbar {}
        div { class: "loading-page",
            div { class: "loading-card",
                if let Some(route) = redirect_route {
                    div { class: "loading-redirect",
                        p { "Document type detected. Redirecting..." }
                        Link { to: route, class: "btn btn-primary",
                            "Open Document"
                        }
                    }
                } else {
                    div { class: "loading-spinner" }
                    h2 { "Loading document..." }
                }
                p { class: "loading-id", "Document ID: {id}" }
                p { class: "loading-hint",
                    "If the document does not load automatically, choose an editor below:"
                }
                div { class: "loading-actions",
                    Link { to: Route::Writer { id: doc_id.clone() }, class: "btn btn-outline",
                        "Open as Document"
                    }
                    Link { to: Route::Calc { id: doc_id.clone() }, class: "btn btn-outline",
                        "Open as Spreadsheet"
                    }
                    Link { to: Route::Slides { id: doc_id.clone() }, class: "btn btn-outline",
                        "Open as Presentation"
                    }
                    Link { to: Route::Whiteboard { id: doc_id.clone() }, class: "btn btn-outline",
                        "Open as Whiteboard"
                    }
                }
            }
        }
    }
}
