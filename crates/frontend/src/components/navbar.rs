use dioxus::prelude::*;

use crate::Route;
use crate::state::{self, AuthState};

/// Top navigation bar displayed on all authenticated pages.
#[component]
pub fn Navbar() -> Element {
    let mut auth = use_context::<Signal<AuthState>>();
    let nav = use_navigator();

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
                    span { class: "user-avatar", "{user_initial}" }
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
