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
                })
                .collect::<Vec<_>>();
            let send = sapid::button()
                .theme(theme())
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
                heading(crate::i18n::t("ai.title")),
                row().fill(modes),
                sapid::text_field()
                    .theme(theme())
                    .name(crate::i18n::t("ai.placeholder"))
                    .multiline(true)
                    .value(prompt)
                    .on_value_change(move |value| set(prompt, value)),
                send,
                status(answer),
            ))
        });
        col().fill((
            action(crate::i18n::t("ai.title"), move || set(open, !open.get())),
            panel,
        ))
    })
}
