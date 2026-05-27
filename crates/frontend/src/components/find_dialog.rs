use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct FindDialogProps {
    pub open: Signal<bool>,
    pub on_search: EventHandler<String>,
    pub on_next: EventHandler<()>,
    pub on_prev: EventHandler<()>,
    #[props(default)]
    pub on_replace: EventHandler<(String, String)>,
    #[props(default)]
    pub on_replace_all: EventHandler<(String, String)>,
    pub match_info: String,
}

#[component]
pub fn FindDialog(props: FindDialogProps) -> Element {
    if !*props.open.read() {
        return rsx! {};
    }
    let mut query = use_signal(String::new);
    let mut replace_text = use_signal(String::new);
    let mut replace_mode = use_signal(|| false);
    let mut open_sig = props.open;
    let dialog_class = if *replace_mode.read() {
        "find-dialog replace-mode"
    } else {
        "find-dialog"
    };
    rsx! {
        div { class: "{dialog_class}",
            div { class: "find-row",
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
                    onclick: move |_| {
                        let cur = *replace_mode.read();
                        replace_mode.set(!cur);
                    },
                    title: "Toggle Replace",
                    aria_label: "Toggle replace mode",
                    "Replace"
                }
                button {
                    class: "find-btn",
                    onclick: move |_| open_sig.set(false),
                    title: "Close (Esc)",
                    aria_label: "Close find dialog (Esc)",
                    "✕"
                }
            }
            if *replace_mode.read() {
                div { class: "find-row",
                    input {
                        r#type: "text",
                        class: "find-input",
                        placeholder: "Replace with...",
                        value: "{replace_text}",
                        oninput: move |e| replace_text.set(e.value()),
                        onkeydown: move |e: KeyboardEvent| {
                            if e.key() == Key::Escape {
                                e.prevent_default();
                                open_sig.set(false);
                            }
                        },
                    }
                    button {
                        class: "find-btn",
                        onclick: move |_| {
                            let q = query.read().clone();
                            let r = replace_text.read().clone();
                            if !q.is_empty() {
                                props.on_replace.call((q, r));
                            }
                        },
                        title: "Replace current match",
                        aria_label: "Replace current match",
                        "Replace"
                    }
                    button {
                        class: "find-btn",
                        onclick: move |_| {
                            let q = query.read().clone();
                            let r = replace_text.read().clone();
                            if !q.is_empty() {
                                props.on_replace_all.call((q, r));
                            }
                        },
                        title: "Replace all matches",
                        aria_label: "Replace all matches",
                        "Replace All"
                    }
                }
            }
        }
    }
}
