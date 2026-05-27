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
    let _ = use_context::<Signal<String>>().read();
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
    let find_placeholder = crate::i18n::t("find.placeholder");
    let replace_placeholder = crate::i18n::t("find.replace_placeholder");
    let prev_title = crate::i18n::t("find.prev");
    let next_title = crate::i18n::t("find.next");
    let toggle_replace = crate::i18n::t("find.toggle_replace");
    let close_title = crate::i18n::t("find.close");
    let replace_label = crate::i18n::t("find.replace");
    let replace_all_label = crate::i18n::t("find.replace_all");
    rsx! {
        div { class: "{dialog_class}",
            div { class: "find-row",
                input {
                    r#type: "text",
                    class: "find-input",
                    placeholder: "{find_placeholder}",
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
                    title: "{prev_title}",
                    aria_label: "{prev_title}",
                    "↑"
                }
                button {
                    class: "find-btn",
                    onclick: move |_| props.on_next.call(()),
                    title: "{next_title}",
                    aria_label: "{next_title}",
                    "↓"
                }
                button {
                    class: "find-btn",
                    onclick: move |_| {
                        let cur = *replace_mode.read();
                        replace_mode.set(!cur);
                    },
                    title: "{toggle_replace}",
                    aria_label: "{toggle_replace}",
                    "{replace_label}"
                }
                button {
                    class: "find-btn",
                    onclick: move |_| open_sig.set(false),
                    title: "{close_title}",
                    aria_label: "{close_title}",
                    "✕"
                }
            }
            if *replace_mode.read() {
                div { class: "find-row",
                    input {
                        r#type: "text",
                        class: "find-input",
                        placeholder: "{replace_placeholder}",
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
                        title: "{replace_label}",
                        aria_label: "{replace_label}",
                        "{replace_label}"
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
                        title: "{replace_all_label}",
                        aria_label: "{replace_all_label}",
                        "{replace_all_label}"
                    }
                }
            }
        }
    }
}
