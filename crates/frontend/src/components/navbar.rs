use crate::{components::ui::*, state::AuthState};
use glory::prelude::*;

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
    }
}
#[allow(non_snake_case)]
pub fn Navbar(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    let query = scope.cage(String::new());
    let language = if crate::i18n::current_locale() == "zh" {
        "EN"
    } else {
        "中"
    };
    view().class("navbar").fill((
        view().class("navbar-brand").fill(
            link()
                .target("/dashboard")
                .name("Contrix Office")
                .class("brand-link")
                .fill((
                    view()
                        .class("brand-icon")
                        .fill(icon_sized(Icon::Office, 20.)),
                    label("Contrix Office").class("brand-text"),
                )),
        ),
        view().class("navbar-menu").fill((
            anchor(crate::i18n::t("nav.dashboard"), "/dashboard").class("navbar-item"),
            anchor(crate::i18n::t("nav.files"), "/files").class("navbar-item"),
            when(
                scope.bond(move || auth.get().user.is_some_and(|user| user.role == "admin")),
                || anchor(crate::i18n::t("nav.admin"), "/admin").class("navbar-item"),
            ),
        )),
        view().class("navbar-end").fill((
            view().class("navbar-search has-search-icon").fill((
                view().class("search-icon").fill(icon(Icon::Search)),
                sapid::text_field()
                    .unstyled()
                    .class("search-input")
                    .name(crate::i18n::t("nav.search_placeholder"))
                    .placeholder(crate::i18n::t("nav.search_placeholder"))
                    .purpose(Purpose::Search)
                    .value(query)
                    .on_value_change(move |value| set(query, value))
                    .on_commit(|_| go("/files")),
            )),
            crate::components::notification_center::NotificationCenter(scope.clone(), auth),
            view().class("navbar-user").fill((
                action("", || {
                    let next = if crate::i18n::current_locale() == "zh" {
                        "en"
                    } else {
                        "zh"
                    };
                    crate::i18n::set_locale(next);
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().reload();
                    }
                })
                .unstyled()
                .name("Switch language")
                .class("lang-toggle with-icon")
                .fill((icon(Icon::Globe), label(language).class("language-code"))),
                action("", move || {
                    let next_dark = current_theme() != "dark";
                    set_theme(if next_dark { "dark" } else { "light" });
                })
                .unstyled()
                .name(crate::i18n::t("nav.theme_toggle"))
                .class("theme-toggle icon-button")
                .fill((
                    icon(Icon::Sun).class("ui-icon theme-light-icon"),
                    icon(Icon::Moon).class("ui-icon theme-dark-icon"),
                )),
                link()
                    .target("/profile")
                    .name(crate::i18n::t("profile.title"))
                    .class("user-avatar")
                    .label(scope.bond(move || {
                        auth.get()
                            .user
                            .as_ref()
                            .and_then(|user| user.username.chars().next())
                            .unwrap_or('U')
                            .to_uppercase()
                            .to_string()
                    })),
                action("", move || {
                    crate::state::clear_auth_state();
                    set(auth, AuthState::default());
                    go("/login");
                })
                .unstyled()
                .name(crate::i18n::t("nav.logout"))
                .class("btn btn-outline btn-sm with-icon logout-button")
                .fill((icon(Icon::Logout), label(crate::i18n::t("nav.logout")))),
            )),
        )),
    ))
}
