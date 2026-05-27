use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Toast, ToastKind, push_toast};
use crate::state::{AuthState, api_post};

#[derive(Serialize)]
struct AssistRequest {
    pub document_id: String,
    pub prompt: String,
    pub mode: String,
}

#[derive(Deserialize)]
struct AssistResponse {
    pub answer: String,
}

#[derive(Props, Clone, PartialEq)]
pub struct AiAssistProps {
    pub doc_id: String,
}

#[component]
pub fn AiAssist(props: AiAssistProps) -> Element {
    let auth = use_context::<Signal<AuthState>>();
    let toast_bus = use_context::<Signal<Vec<Toast>>>();
    let mut open = use_signal(|| false);
    let mut prompt = use_signal(|| String::from("Summarize this document in 3 bullet points."));
    let mut mode = use_signal(|| String::from("summarize"));
    let mut answer = use_signal(String::new);
    let mut loading = use_signal(|| false);
    let doc_id = props.doc_id.clone();

    let send = move |_| {
        if *loading.read() {
            return;
        }
        let token = auth.read().token.clone().unwrap_or_default();
        let body = AssistRequest {
            document_id: doc_id.clone(),
            prompt: prompt.read().clone(),
            mode: mode.read().clone(),
        };
        let bus = toast_bus;
        loading.set(true);
        wasm_bindgen_futures::spawn_local(async move {
            match api_post::<AssistResponse, _>("/ai/assist", &body, &token).await {
                Ok(r) => {
                    answer.set(r.answer);
                    push_toast(bus, ToastKind::Success, "AI response received");
                }
                Err(e) => push_toast(bus, ToastKind::Error, format!("AI: {e}")),
            }
            loading.set(false);
        });
    };

    rsx! {
        button {
            class: "ai-fab",
            onclick: move |_| open.toggle(),
            aria_label: "AI assistant",
            title: "AI assistant",
            "\u{2728}"
        }
        if *open.read() {
            aside { class: "ai-pane",
                header { class: "ai-pane-header",
                    strong { "AI assistant" }
                    button { class: "btn-icon", onclick: move |_| open.set(false), "\u{2715}" }
                }
                div { class: "ai-modes",
                    for m in ["summarize", "translate", "explain", "draft"] {
                        button {
                            class: if *mode.read() == m { "ai-mode active" } else { "ai-mode" },
                            onclick: move |_| mode.set(m.to_string()),
                            "{m}"
                        }
                    }
                }
                textarea {
                    class: "form-input ai-prompt",
                    rows: 3,
                    value: "{prompt}",
                    oninput: move |e| prompt.set(e.value()),
                }
                button {
                    class: "btn btn-primary btn-sm",
                    disabled: *loading.read(),
                    onclick: send,
                    {if *loading.read() { "Thinking..." } else { "Send" }}
                }
                if !answer.read().is_empty() {
                    div { class: "ai-answer", "{answer}" }
                }
            }
        }
    }
}
