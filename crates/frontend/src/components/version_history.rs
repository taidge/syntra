use dioxus::prelude::*;
use serde::Deserialize;

use crate::components::{Toast, ToastKind, push_toast};
use crate::state::{AuthState, api_post};

#[derive(Deserialize, Clone, PartialEq)]
pub struct VersionItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub version_number: i32,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Props, Clone, PartialEq)]
pub struct VersionHistoryProps {
    pub doc_id: String,
    pub open: Signal<bool>,
}

#[component]
pub fn VersionHistory(props: VersionHistoryProps) -> Element {
    let auth = use_context::<Signal<AuthState>>();
    let toast_bus = use_context::<Signal<Vec<Toast>>>();
    let token = auth.read().token.clone().unwrap_or_default();
    let doc_id_for_fetch = props.doc_id.clone();
    let token_for_fetch = token.clone();
    let mut refresh_tick = use_signal(|| 0u32);

    let versions = use_resource(move || {
        let _ = refresh_tick.read();
        let id = doc_id_for_fetch.clone();
        let t = token_for_fetch.clone();
        async move {
            crate::state::api_get::<Vec<VersionItem>>(
                &format!("/documents/{id}/versions"),
                &t,
            )
            .await
        }
    });

    let mut open_sig = props.open;
    if !*open_sig.read() {
        return rsx! { Fragment {} };
    }

    let restore_token = token.clone();
    let restore_id = props.doc_id.clone();

    rsx! {
        aside { class: "version-pane",
            header { class: "version-pane-header",
                strong { "Version history" }
                button {
                    class: "btn-icon",
                    aria_label: "Close version history",
                    onclick: move |_| open_sig.set(false),
                    "\u{2715}"
                }
            }
            div { class: "version-list",
                match versions.read().as_ref() {
                    None => rsx! { p { class: "muted", "Loading\u{2026}" } },
                    Some(Err(e)) => rsx! { p { class: "muted", "Failed: {e}" } },
                    Some(Ok(items)) if items.is_empty() => rsx! { p { class: "muted", "No versions yet." } },
                    Some(Ok(items)) => rsx! {
                        for v in items.iter().cloned() {
                            div { class: "version-item", key: "{v.id}",
                                div { class: "version-meta",
                                    strong { "v{v.version_number}" }
                                    span { class: "muted", " \u{00b7} {v.created_at}" }
                                    if let Some(label) = v.label.clone() {
                                        if !label.is_empty() {
                                            div { class: "version-label", "{label}" }
                                        }
                                    }
                                }
                                {
                                    let vid = v.id.clone();
                                    let did = restore_id.clone();
                                    let t = restore_token.clone();
                                    let bus = toast_bus;
                                    let mut tick = refresh_tick;
                                    rsx! {
                                        button {
                                            class: "btn btn-outline btn-sm",
                                            aria_label: "Restore version {v.version_number}",
                                            onclick: move |_| {
                                                let vid = vid.clone();
                                                let did = did.clone();
                                                let t = t.clone();
                                                let bus = bus;
                                                let mut tick = tick;
                                                wasm_bindgen_futures::spawn_local(async move {
                                                    match api_post::<serde_json::Value, _>(
                                                        &format!("/documents/{did}/restore/{vid}"),
                                                        &serde_json::json!({}),
                                                        &t,
                                                    ).await {
                                                        Ok(_) => {
                                                            push_toast(bus, ToastKind::Success, "Version restored");
                                                            let cur = *tick.read();
                                                            tick.set(cur + 1);
                                                        }
                                                        Err(e) => push_toast(bus, ToastKind::Error, format!("Restore failed: {e}")),
                                                    }
                                                });
                                            },
                                            "Restore"
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
