use crate::{components::ui::*, state::FileResponse};
use glory::prelude::*;
#[allow(non_snake_case)]
pub fn FileList(
    files: Cage<Vec<FileResponse>>,
    on_open: impl Fn(FileResponse) + 'static,
) -> impl Widget {
    let on_open = std::rc::Rc::new(on_open);
    col().class("file-list").fill(each(
        files,
        |file| file.id,
        move |file, _| {
            let on_open = on_open.clone();
            action(file.get().name, move || on_open(file.get()))
        },
    ))
}
