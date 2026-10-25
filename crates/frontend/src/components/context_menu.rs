use crate::components::ui::*;
use glory::prelude::*;
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
#[allow(non_snake_case)]
pub fn ContextMenu(
    scope: Scope,
    state: Cage<Option<(f64, f64, Vec<MenuEntry>)>>,
    on_command: impl Fn(String) + 'static,
) -> impl Widget {
    let command = std::rc::Rc::new(on_command);
    let entries = scope.bond(move || {
        state
            .get()
            .map(|(_, _, entries)| entries)
            .unwrap_or_default()
    });
    when(scope.bond(move || state.get().is_some()), move || {
        let command = command.clone();
        col().class("context-menu").fill((
            each(
                entries,
                |entry| entry.command.clone(),
                move |entry, _| {
                    let command = command.clone();
                    sapid::button()
                        .theme(theme())
                        .label(entry.get().label)
                        .enabled(!entry.get().disabled)
                        .on_press(move || {
                            command(entry.get().command);
                            set(state, None);
                        })
                },
            ),
            action("Close", move || set(state, None)),
        ))
    })
}
