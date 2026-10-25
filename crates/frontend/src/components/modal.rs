use crate::components::ui::{set, theme};
use glory::prelude::*;
#[allow(non_snake_case)]
pub fn Modal<W: Widget>(
    key: String,
    title: String,
    show: Cage<bool>,
    content: impl FnMut() -> W + 'static,
) -> impl Widget {
    sapid::dialog(key)
        .theme(theme())
        .title(title)
        .show_trigger(false)
        .open(show)
        .on_open_change(move |value| set(show, value))
        .content(content)
}
