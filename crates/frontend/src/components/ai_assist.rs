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
    let _locale = use_context::<Signal<String>>();
    let mut open = use_signal(|| false);
    let mut prompt = use_signal(|| crate::i18n::t("ai.placeholder"));
    let mut mode = use_signal(|| String::from("summarize"));
    let mut answer = use_signal(String::new);
    let mut loading = use_signal(|| false);
    let doc_id = props.doc_id.clone();

    let ai_title = crate::i18n::t("ai.title");
    let send_label = crate::i18n::t("ai.send");
    let thinking_label = crate::i18n::t("ai.thinking");

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
            aria_label: "{ai_title}",
            title: "{ai_title}",
            "\u{2728}"
        }
        if *open.read() {
            aside { class: "ai-pane",
                header { class: "ai-pane-header",
                    strong { "{ai_title}" }
                    button { class: "btn-icon", onclick: move |_| open.set(false), "\u{2715}" }
                }
                div { class: "ai-modes",
                    for m in ["summarize", "translate", "explain", "draft"] {
                        button {
                            class: if *mode.read() == m { "ai-mode active" } else { "ai-mode" },
                            onclick: move |_| mode.set(m.to_string()),
                            { crate::i18n::t(&format!("ai.{m}")) }
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
                    {if *loading.read() { thinking_label.clone() } else { send_label.clone() }}
                }
                if !answer.read().is_empty() {
                    div { class: "ai-answer", "{answer}" }
                }
            }
        }
    }
}
