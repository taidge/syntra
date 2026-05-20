use dioxus::prelude::*;

/// A reusable modal dialog overlay.
///
/// When `show` is `true` the modal renders on top of the page with a dimmed
/// backdrop.  Clicking the backdrop or the close button sets `show` to `false`.
#[component]
pub fn Modal(title: String, show: Signal<bool>, children: Element) -> Element {
    if !*show.read() {
        return rsx! {};
    }

    rsx! {
        div { class: "modal-overlay",
            onclick: move |_| show.set(false),

            div { class: "modal-container",
                // Stop click propagation so clicking inside the modal does
                // not dismiss it.
                onclick: move |evt| evt.stop_propagation(),

                div { class: "modal-header",
                    h3 { class: "modal-title", "{title}" }
                    button {
                        class: "modal-close",
                        onclick: move |_| show.set(false),
                        "x"
                    }
                }

                div { class: "modal-body",
                    {children}
                }
            }
        }
    }
}
