use crate::components::ui::*;
use glory::prelude::*;
pub struct FindCallbacks {
    pub search: std::rc::Rc<dyn Fn(String)>,
    pub next: std::rc::Rc<dyn Fn()>,
    pub previous: std::rc::Rc<dyn Fn()>,
    pub replace: std::rc::Rc<dyn Fn(String, String)>,
    pub replace_all: std::rc::Rc<dyn Fn(String, String)>,
}
#[allow(non_snake_case)]
pub fn FindDialog(scope: Scope, open: Cage<bool>, callbacks: FindCallbacks) -> impl Widget {
    let query = scope.cage(String::new());
    let replacement = scope.cage(String::new());
    let search = callbacks.search;
    when(open, move || {
        let search = search.clone();
        let next = callbacks.next.clone();
        let previous = callbacks.previous.clone();
        let replace = callbacks.replace.clone();
        let replace_all = callbacks.replace_all.clone();
        col().class("find-dialog").fill((
            sapid::text_field()
                .theme(theme())
                .name(crate::i18n::t("find.placeholder"))
                .value(query)
                .on_value_change(move |value| {
                    set(query, value.clone());
                    search(value);
                }),
            field(
                crate::i18n::t("find.replace_placeholder"),
                replacement,
                Purpose::Text,
            ),
            row().fill((
                action("Previous", move || previous()),
                action("Next", move || next()),
                action("Replace", move || replace(query.get(), replacement.get())),
                action("Replace all", move || {
                    replace_all(query.get(), replacement.get())
                }),
                action("Close", move || set(open, false)),
            )),
        ))
    })
}
