use crate::components::ui::theme;
use glory::prelude::*;

#[allow(non_snake_case)]
pub fn Menubar(
    _scope: Scope,
    document_type: String,
    on_command: impl Fn(String) + 'static,
) -> impl Widget {
    let command = std::rc::Rc::new(on_command);
    let widgets = syntra_office_ui::menus(&document_type)
        .into_iter()
        .map(|(key, label, items)| {
            let mut menu = sapid::dropdown_menu(format!("syntra-{key}"))
                .theme(theme())
                .label(crate::i18n::t(label));
            for (index, item) in items.into_iter().enumerate() {
                let command = command.clone();
                let cmd = item.cmd;
                let label = format!("{} {}", crate::i18n::t(item.label_key), item.shortcut);
                menu = menu.item(
                    sapid::menu_item(format!("{key}-{index}"))
                        .label(label)
                        .on_activate(move || command(cmd.to_string())),
                );
                if item.separator_after {
                    menu = menu.separator(format!("{key}-separator-{index}"));
                }
            }
            menu
        })
        .collect::<Vec<_>>();
    row().class("menubar").fill(widgets)
}
