use crate::{components::ui::*, state::AuthState};
use glory::prelude::*;

fn current_theme() -> String {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item("theme").ok().flatten())
        .unwrap_or_else(|| "light".into())
}
pub fn set_theme(value: &str) {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item("theme", value);
        }
        if let Some(root) = window
            .document()
            .and_then(|document| document.document_element())
        {
            let _ = root.set_attribute("data-theme", value);
        }
        let _ = window.location().reload();
    }
}
#[allow(non_snake_case)]
pub fn Navbar(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    row().class("navbar").fill((
        anchor("Contrix Office", "/dashboard"),
        anchor(crate::i18n::t("nav.dashboard"), "/dashboard"),
        anchor(crate::i18n::t("nav.files"), "/files"),
        anchor(crate::i18n::t("profile.title"), "/profile"),
        when(
            scope.bond(move || auth.get().user.is_some_and(|user| user.role == "admin")),
            || anchor("Admin", "/admin"),
        ),
        crate::components::notification_center::NotificationCenter(scope.clone(), auth),
        action("中文 / English", || {
            let next = if crate::i18n::current_locale() == "zh" {
                "en"
            } else {
                "zh"
            };
            crate::i18n::set_locale(next);
            if let Some(window) = web_sys::window() {
                let _ = window.location().reload();
            }
        }),
        action("Light / Dark", || {
            set_theme(if current_theme() == "dark" {
                "light"
            } else {
                "dark"
            });
        }),
        action(crate::i18n::t("nav.logout"), move || {
            crate::state::clear_auth_state();
            set(auth, AuthState::default());
            go("/login");
        }),
    ))
}
