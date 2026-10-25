use crate::components::ui::{set, theme};
use glory::prelude::*;
#[allow(non_snake_case)]
pub fn Modal<W: Widget>(
    key: String,
    title: String,
    show: Cage<bool>,
    mut content: impl FnMut() -> W + 'static,
) -> impl Widget {
    sapid::dialog(key)
        .theme(theme())
        .class("office-modal")
        .title(title)
        .close_label(crate::i18n::t("common.close"))
        .close_content(|| text().kind(TextKind::Inline).content("×"))
        .show_trigger(false)
        .open(show)
        .on_open_change(move |value| set(show, value))
        .content(move || col().class("modal-body").fill(content()))
}
