use dioxus::prelude::*;

use crate::Route;
use crate::components::navbar::Navbar;

fn route_for_type(document_type: &str, doc_id: &str) -> Option<Route> {
    match document_type {
        "writer" => Some(Route::Writer {
            id: doc_id.to_string(),
        }),
        "calc" => Some(Route::Calc {
            id: doc_id.to_string(),
        }),
        "slides" => Some(Route::Slides {
            id: doc_id.to_string(),
        }),
        "whiteboard" => Some(Route::Whiteboard {
            id: doc_id.to_string(),
        }),
        _ => None,
    }
}

/// Generic document editor that loads the document type from the API and
/// redirects to the appropriate specialised editor (Writer, Calc, Slides, or
/// Whiteboard).
///
/// While the document metadata is being fetched, a loading indicator is shown.
#[component]
pub fn DocumentEditor(id: String) -> Element {
    let _ = use_context::<Signal<String>>().read();
    let doc_id = id.clone();
    let auth = use_context::<Signal<crate::state::AuthState>>();
    let nav = use_navigator();

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

    // Determine the document type from the fetched metadata.
    let detected_type = match &*doc_meta.read() {
        Some(Some(meta)) => meta
            .get("document_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    };

    // Redirect automatically once the document type is known.
    let doc_id_for_redirect = doc_id.clone();
    use_effect(move || {
        let detected = match &*doc_meta.read() {
            Some(Some(meta)) => meta
                .get("document_type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            _ => None,
        };
        if let Some(route) = detected
            .as_deref()
            .and_then(|dt| route_for_type(dt, &doc_id_for_redirect))
        {
            nav.replace(route);
        }
    });

    let redirect_route = detected_type
        .as_deref()
        .and_then(|dt| route_for_type(dt, &doc_id));

    rsx! {
        Navbar {}
        div { class: "loading-page",
            div { class: "loading-card",
                if let Some(route) = redirect_route {
                    div { class: "loading-redirect",
                        p { {crate::i18n::t("doc_editor.redirecting")} }
                        Link { to: route, class: "btn btn-primary",
                            {crate::i18n::t("doc_editor.open")}
                        }
                    }
                } else {
                    div { class: "loading-spinner" }
                    h2 { {crate::i18n::t("editor.loading_document")} }
                }
                p { class: "loading-id", {crate::i18n::t("doc_editor.document_id").replace("{id}", &id)} }
                p { class: "loading-hint",
                    {crate::i18n::t("doc_editor.manual_hint")}
                }
                div { class: "loading-actions",
                    Link { to: Route::Writer { id: doc_id.clone() }, class: "btn btn-outline",
                        {crate::i18n::t("doc_editor.open_writer")}
                    }
                    Link { to: Route::Calc { id: doc_id.clone() }, class: "btn btn-outline",
                        {crate::i18n::t("doc_editor.open_calc")}
                    }
                    Link { to: Route::Slides { id: doc_id.clone() }, class: "btn btn-outline",
                        {crate::i18n::t("doc_editor.open_slides")}
                    }
                    Link { to: Route::Whiteboard { id: doc_id.clone() }, class: "btn btn-outline",
                        {crate::i18n::t("doc_editor.open_whiteboard")}
                    }
                }
            }
        }
    }
}
