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
        if let Some(el) = window
            .document()
            .and_then(|d| d.document_element())
        {
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

    // Derive a display initial from the current user (fallback "U").
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

    rsx! {
        nav { class: "navbar",
            div { class: "navbar-brand",
                Link { to: Route::Dashboard {}, class: "brand-link",
                    span { class: "brand-icon", "\u{1F4CB}" }
                    span { class: "brand-text", "Contrix Office" }
                }
            }

            div { class: "navbar-menu",
                Link { to: Route::Dashboard {}, class: "navbar-item", "Dashboard" }
                Link { to: Route::FileManager {}, class: "navbar-item", "Files" }
                Link { to: Route::Admin {}, class: "navbar-item", "Admin" }
            }

            div { class: "navbar-end",
                div { class: "navbar-search",
                    input {
                        r#type: "search",
                        placeholder: "Search documents...",
                        class: "search-input",
                    }
                }
                div { class: "navbar-user",
                    button {
                        class: "theme-toggle",
                        title: "Toggle theme",
                        onclick: move |_| {
                            let next = if *theme.read() == "dark" { "light" } else { "dark" };
                            set_theme(next);
                            theme.set(next.to_string());
                        },
                        {if *theme.read() == "dark" { "\u{2600}" } else { "\u{1F319}" }}
                    }
                    Link { to: Route::Profile {}, class: "user-avatar", title: "Profile", "{user_initial}" }
                    button {
                        class: "btn btn-outline btn-sm",
                        onclick: move |_| {
                            state::clear_auth_state();
                            auth.set(AuthState::default());
                            nav.push(Route::Login {});
                        },
                        "Logout"
                    }
                }
            }
        }
    }
}
