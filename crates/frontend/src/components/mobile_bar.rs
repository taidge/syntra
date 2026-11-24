use crate::{components::ui::*, state::AuthState};
use glory::prelude::*;
fn destination(key: &'static str, kind: Icon, path: &'static str) -> impl Widget {
    let name = crate::i18n::t(key);
    link()
        .target(path)
        .name(name.clone())
        .class("mobile-bar-item")
        .fill((
            icon_sized(kind, 20.).class("ui-icon mobile-bar-icon"),
            label(name).kind(TextKind::Inline).class("mobile-bar-label"),
        ))
}
#[allow(non_snake_case)]
pub fn MobileBar(_scope: Scope, _auth: Cage<AuthState>) -> impl Widget {
    view().class("mobile-bar").fill((
        destination("mobile.home", Icon::Home, "/dashboard"),
        destination("mobile.files", Icon::Folder, "/files"),
        action(crate::i18n::t("mobile.ai"), || {
            if let Some(window) = web_sys::window()
                && let Some(document) = window.document()
                && let Ok(event) = web_sys::CustomEvent::new("syntra:open-ai")
            {
                let _ = document.dispatch_event(&event);
            }
        })
        .unstyled()
        .class("mobile-bar-item")
        .name(crate::i18n::t("mobile.ai"))
        .label("")
        .fill((
            icon_sized(Icon::Sparkles, 20.).class("ui-icon mobile-bar-icon"),
            label(crate::i18n::t("mobile.ai"))
                .kind(TextKind::Inline)
                .class("mobile-bar-label"),
        )),
        destination("mobile.account", Icon::User, "/profile"),
    ))
}
