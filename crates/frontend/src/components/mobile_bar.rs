use dioxus::prelude::*;

use crate::Route;

#[component]
pub fn MobileBar() -> Element {
    let nav = use_navigator();
    rsx! {
        nav { class: "mobile-bar",
            button {
                class: "mobile-bar-item",
                onclick: move |_| { nav.push(Route::Dashboard {}); },
                aria_label: "Dashboard",
                span { class: "mobile-bar-icon", "\u{1F3E0}" }
                span { class: "mobile-bar-label", "Home" }
            }
            button {
                class: "mobile-bar-item",
                onclick: move |_| { nav.push(Route::FileManager {}); },
                aria_label: "Files",
                span { class: "mobile-bar-icon", "\u{1F4C1}" }
                span { class: "mobile-bar-label", "Files" }
            }
            button {
                class: "mobile-bar-item",
                onclick: move |_| {
                    if let Some(window) = web_sys::window() {
                        if let Some(doc) = window.document() {
                            if let Ok(event) = web_sys::CustomEvent::new("contrix:open-ai") {
                                let _ = doc.dispatch_event(&event);
                            }
                        }
                    }
                },
                aria_label: "AI",
                span { class: "mobile-bar-icon", "\u{2728}" }
                span { class: "mobile-bar-label", "AI" }
            }
            button {
                class: "mobile-bar-item",
                onclick: move |_| { nav.push(Route::Profile {}); },
                aria_label: "Account",
                span { class: "mobile-bar-icon", "\u{1F464}" }
                span { class: "mobile-bar-label", "Account" }
            }
        }
    }
}
