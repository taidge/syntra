use crate::{
    components::ui::*,
    state::{self, AuthState},
};
use glory::prelude::*;
use serde::Deserialize;

#[derive(Deserialize)]
struct Capabilities {
    features: Features,
}
#[derive(Deserialize)]
struct Features {
    ai: bool,
}
#[derive(Deserialize)]
struct Answer {
    answer: String,
}

#[allow(non_snake_case)]
pub fn AiAssist(scope: Scope, auth: Cage<AuthState>, doc_id: String) -> impl Widget {
    let available = scope.cage(false);
    let open = scope.cage(false);
    let prompt = scope.cage(String::new());
    let mode = scope.cage("summarize".to_string());
    let answer = scope.cage(String::new());
    let busy = scope.cage(false);
    let weak = scope.downgrade();
    let _ = scope.spawn(async move {
        if let Ok(capabilities) = state::api_get::<Capabilities>("/capabilities", "").await {
            set(available, capabilities.features.ai);
        }
    });

    when(available, move || {
        let weak = weak.clone();
        let id = doc_id.clone();
        let panel = when(open, move || {
            let weak = weak.clone();
            let id = id.clone();
            let modes = ["summarize", "translate", "explain", "draft"]
                .into_iter()
                .map(|value| {
                    action(crate::i18n::t(&format!("ai.{value}")), move || {
                        set(mode, value.into());
                    })
                    .class(mode.map(move |mode| {
                        if mode == value {
                            "ai-mode active".to_owned()
                        } else {
                            "ai-mode".to_owned()
                        }
                    }))
                })
                .collect::<Vec<_>>();
            let send = sapid::button()
                .theme(theme())
                .class("btn btn-primary btn-sm")
                .label(crate::i18n::t("ai.send"))
                .loading(busy)
                .on_press(move || {
                    if busy.get() || prompt.get().trim().is_empty() {
                        return;
                    }
                    let Some(scope) = weak.upgrade() else {
                        return;
                    };
                    let token = token(auth);
                    let body = serde_json::json!({
                        "document_id": id,
                        "prompt": prompt.get(),
                        "mode": mode.get(),
                    });
                    set(busy, true);
                    let _ = scope.spawn(async move {
                        match state::api_post::<Answer, _>("/ai/assist", &body, &token).await {
                            Ok(value) => set(answer, value.answer),
                            Err(error) => set(answer, error),
                        }
                        set(busy, false);
                    });
                });
            col().class("ai-pane").fill((
                row().class("ai-pane-header").fill((
                    text()
                        .kind(TextKind::Heading(2))
                        .content(crate::i18n::t("ai.title")),
                    action("×", move || set(open, false))
                        .name("Close AI assistant")
                        .class("btn-icon"),
                )),
                row().class("ai-modes").fill(modes),
                sapid::text_field()
                    .theme(theme())
                    .class("form-input ai-prompt")
                    .name(crate::i18n::t("ai.placeholder"))
                    .multiline(true)
                    .value(prompt)
                    .on_value_change(move |value| set(prompt, value)),
                send,
                status(answer).class("ai-answer"),
            ))
        });
        group().fill((
            action("✦", move || set(open, !open.get()))
                .name(crate::i18n::t("ai.title"))
                .class("ai-fab"),
            panel,
        ))
    })
}
