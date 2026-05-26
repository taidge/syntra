use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
pub struct MenuEntry {
    pub label: String,
    pub command: String,
    pub disabled: bool,
}

impl MenuEntry {
    pub fn new(label: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            command: command.into(),
            disabled: false,
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct ContextMenuProps {
    pub state: Signal<Option<(f64, f64, Vec<MenuEntry>)>>,
    pub on_command: EventHandler<String>,
}

#[component]
pub fn ContextMenu(props: ContextMenuProps) -> Element {
    let mut state = props.state;
    let entries = state.read().as_ref().map(|(_, _, e)| e.clone());
    let pos = state.read().as_ref().map(|(x, y, _)| (*x, *y));
    let (Some(entries), Some((x, y))) = (entries, pos) else {
        return rsx! { Fragment {} };
    };
    rsx! {
        div {
            class: "context-menu-backdrop",
            onclick: move |_| state.set(None),
            oncontextmenu: move |evt: MouseEvent| {
                evt.prevent_default();
                state.set(None);
            },
        }
        div {
            class: "context-menu",
            style: "left: {x}px; top: {y}px;",
            onclick: move |evt: MouseEvent| evt.stop_propagation(),
            for entry in entries.iter() {
                {
                    let cmd = entry.command.clone();
                    let label = entry.label.clone();
                    let disabled = entry.disabled;
                    rsx! {
                        button {
                            class: "context-menu-item",
                            disabled,
                            onclick: move |_| {
                                props.on_command.call(cmd.clone());
                                state.set(None);
                            },
                            "{label}"
                        }
                    }
                }
            }
        }
    }
}
