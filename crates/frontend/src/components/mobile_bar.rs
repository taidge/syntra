use dioxus::prelude::*;

use crate::Route;

#[component]
pub fn MobileBar() -> Element {
    let _ = use_context::<Signal<String>>().read();
    let nav = use_navigator();
    let home_label = crate::i18n::t("mobile.home");
    let files_label = crate::i18n::t("mobile.files");
    let ai_label = crate::i18n::t("mobile.ai");
    let account_label = crate::i18n::t("mobile.account");
    rsx! {
        nav { class: "mobile-bar",
            button {
                class: "mobile-bar-item",
                onclick: move |_| { nav.push(Route::Dashboard {}); },
                aria_label: "{home_label}",
                span { class: "mobile-bar-icon", "\u{1F3E0}" }
                span { class: "mobile-bar-label", "{home_label}" }
            }
            button {
                class: "mobile-bar-item",
                onclick: move |_| { nav.push(Route::FileManager {}); },
                aria_label: "{files_label}",
                span { class: "mobile-bar-icon", "\u{1F4C1}" }
                span { class: "mobile-bar-label", "{files_label}" }
            }
            button {
                class: "mobile-bar-item",
                onclick: move |_| {
                    if let Some(window) = web_sys::window()
                        && let Some(doc) = window.document()
                            && let Ok(event) = web_sys::CustomEvent::new("contrix:open-ai") {
                                let _ = doc.dispatch_event(&event);
                            }
                },
                aria_label: "{ai_label}",
                span { class: "mobile-bar-icon", "\u{2728}" }
                span { class: "mobile-bar-label", "{ai_label}" }
            }
            button {
                class: "mobile-bar-item",
                onclick: move |_| { nav.push(Route::Profile {}); },
                aria_label: "{account_label}",
                span { class: "mobile-bar-icon", "\u{1F464}" }
                span { class: "mobile-bar-label", "{account_label}" }
            }
        }
    }
}
