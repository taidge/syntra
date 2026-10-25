use crate::{components::ui::*, state::AuthState};
use glory::prelude::*;
fn destination(key: &'static str, icon: &'static str, path: &'static str) -> impl Widget {
    let name = crate::i18n::t(key);
    link()
        .target(path)
        .name(name.clone())
        .class("mobile-bar-item")
        .fill((
            label(icon).kind(TextKind::Inline).class("mobile-bar-icon"),
            label(name).kind(TextKind::Inline).class("mobile-bar-label"),
        ))
}
#[allow(non_snake_case)]
pub fn MobileBar(_scope: Scope, _auth: Cage<AuthState>) -> impl Widget {
    view().class("mobile-bar").fill((
        destination("mobile.home", "🏠", "/dashboard"),
        destination("mobile.files", "📁", "/files"),
        action(crate::i18n::t("mobile.ai"), || {
            if let Some(window) = web_sys::window()
                && let Some(document) = window.document()
                && let Ok(event) = web_sys::CustomEvent::new("contrix:open-ai")
            {
                let _ = document.dispatch_event(&event);
            }
        })
        .unstyled()
        .class("mobile-bar-item")
        .name(crate::i18n::t("mobile.ai"))
        .label("")
        .fill((
            label("✨").kind(TextKind::Inline).class("mobile-bar-icon"),
            label(crate::i18n::t("mobile.ai"))
                .kind(TextKind::Inline)
                .class("mobile-bar-label"),
        )),
        destination("mobile.account", "👤", "/profile"),
    ))
}
