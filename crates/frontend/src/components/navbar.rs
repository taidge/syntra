use dioxus::prelude::*;

use crate::Route;
use crate::state::{self, AuthState};

fn current_theme() -> String {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| el.get_attribute("data-theme"))
        .unwrap_or_default()
}

fn set_theme(value: &str) {
    if let Some(window) = web_sys::window() {
        if let Some(el) = window.document().and_then(|d| d.document_element()) {
            let _ = el.set_attribute("data-theme", value);
        }
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item("theme", value);
        }
    }
}

/// Top navigation bar displayed on all authenticated pages.
#[component]
pub fn Navbar() -> Element {
    let mut auth = use_context::<Signal<AuthState>>();
    let nav = use_navigator();
    let mut theme = use_signal(current_theme);
    let mut locale = use_context::<Signal<String>>();

    let user_initial = {
        let state = auth.read();
        state
            .user
            .as_ref()
            .and_then(|u| u.username.chars().next())
            .unwrap_or('U')
            .to_uppercase()
            .to_string()
    };

    let other_locale: &'static str = if *locale.read() == "zh" { "en" } else { "zh" };
    let lang_label: &'static str = if *locale.read() == "zh" { "EN" } else { "中" };
    let theme_label = crate::i18n::t("nav.theme_toggle");
    let logout_label = crate::i18n::t("nav.logout");
    let search_placeholder = crate::i18n::t("nav.search_placeholder");
    let dashboard_label = crate::i18n::t("nav.dashboard");
    let files_label = crate::i18n::t("nav.files");
    let admin_label = crate::i18n::t("nav.admin");
    let profile_label = crate::i18n::t("nav.profile");

    rsx! {
        nav { class: "navbar",
            div { class: "navbar-brand",
                Link { to: Route::Dashboard {}, class: "brand-link",
                    span { class: "brand-icon", "\u{1F4CB}" }
                    span { class: "brand-text", "Contrix Office" }
                }
            }

            div { class: "navbar-menu",
                Link { to: Route::Dashboard {}, class: "navbar-item", "{dashboard_label}" }
                Link { to: Route::FileManager {}, class: "navbar-item", "{files_label}" }
                Link { to: Route::Admin {}, class: "navbar-item", "{admin_label}" }
            }

            div { class: "navbar-end",
                div { class: "navbar-search",
                    input {
                        r#type: "search",
                        placeholder: "{search_placeholder}",
                        class: "search-input",
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Enter {
                                nav.push(Route::FileManager {});
                            }
                        },
                    }
                }
                div { class: "navbar-user",
                    button {
                        class: "lang-toggle",
                        title: "{lang_label}",
                        aria_label: "Switch language",
                        onclick: move |_| {
                            crate::i18n::set_locale(other_locale);
                            locale.set(other_locale.to_string());
                        },
                        "{lang_label}"
                    }
                    button {
                        class: "theme-toggle",
                        title: "{theme_label}",
                        aria_label: "{theme_label}",
                        onclick: move |_| {
                            let next = if *theme.read() == "dark" { "light" } else { "dark" };
                            set_theme(next);
                            theme.set(next.to_string());
                        },
                        {if *theme.read() == "dark" { "\u{2600}" } else { "\u{1F319}" }}
                    }
                    Link { to: Route::Profile {}, class: "user-avatar", title: "{profile_label}", aria_label: "{profile_label}", "{user_initial}" }
                    button {
                        class: "btn btn-outline btn-sm",
                        aria_label: "{logout_label}",
                        onclick: move |_| {
                            state::clear_auth_state();
                            auth.set(AuthState::default());
                            nav.push(Route::Login {});
                        },
                        "{logout_label}"
                    }
                }
            }
        }
    }
}
