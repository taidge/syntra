use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct FindDialogProps {
    pub open: Signal<bool>,
    pub on_search: EventHandler<String>,
    pub on_next: EventHandler<()>,
    pub on_prev: EventHandler<()>,
    pub match_info: String,
}

#[component]
pub fn FindDialog(props: FindDialogProps) -> Element {
    if !*props.open.read() {
        return rsx! {};
    }
    let mut query = use_signal(String::new);
    let mut open_sig = props.open;
    rsx! {
        div { class: "find-dialog",
            input {
                r#type: "text",
                class: "find-input",
                placeholder: "Find...",
                value: "{query}",
                autofocus: true,
                oninput: move |e| {
                    query.set(e.value());
                    props.on_search.call(e.value());
                },
                onkeydown: move |e: KeyboardEvent| {
                    match e.key() {
                        Key::Enter => {
                            e.prevent_default();
                            if e.modifiers().contains(Modifiers::SHIFT) {
                                props.on_prev.call(());
                            } else {
                                props.on_next.call(());
                            }
                        }
                        Key::Escape => {
                            e.prevent_default();
                            open_sig.set(false);
                        }
                        _ => {}
                    }
                },
            }
            span { class: "find-info", "{props.match_info}" }
            button {
                class: "find-btn",
                onclick: move |_| props.on_prev.call(()),
                title: "Previous (Shift+Enter)",
                aria_label: "Previous match (Shift+Enter)",
                "↑"
            }
            button {
                class: "find-btn",
                onclick: move |_| props.on_next.call(()),
                title: "Next (Enter)",
                aria_label: "Next match (Enter)",
                "↓"
            }
            button {
                class: "find-btn",
                onclick: move |_| open_sig.set(false),
                title: "Close (Esc)",
                aria_label: "Close find dialog (Esc)",
                "✕"
            }
        }
    }
}
