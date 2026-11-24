//! Menu definitions shared by the web frontend and offline hosts.
pub const EMBEDDED_WRITER_JS: &[u8] = include_bytes!("../assets/embedded-writer.js");
pub const READER_JS: &str = include_str!("../assets/reader.js");
pub const COMMAND_BAR_JS: &str = include_str!("../assets/command-bar.js");
pub const COMMAND_BAR_CSS: &str = include_str!("../assets/command-bar.css");
pub const WRITER_SURFACE_JS: &[u8] = include_bytes!("../assets/writer-surface.js");

#[cfg(target_arch = "wasm32")]
pub mod writer;

#[derive(Clone, PartialEq)]
pub struct MenuItem {
    pub label_key: &'static str,
    pub cmd: &'static str,
    pub shortcut: &'static str,
    pub separator_after: bool,
}

impl MenuItem {
    const fn new(label_key: &'static str, cmd: &'static str, shortcut: &'static str) -> Self {
        Self {
            label_key,
            cmd,
            shortcut,
            separator_after: false,
        }
    }
    const fn sep(label_key: &'static str, cmd: &'static str, shortcut: &'static str) -> Self {
        Self {
            label_key,
            cmd,
            shortcut,
            separator_after: true,
        }
    }
}

fn file_items(document_type: &str) -> Vec<MenuItem> {
    let export_key = match document_type {
        "writer" => "menu.file.export_docx",
        "calc" => "menu.file.export_xlsx",
        "slides" => "menu.file.export_pptx",
        _ => "menu.file.export_html",
    };
    let export_cmd = match document_type {
        "writer" => "export-docx",
        "calc" => "export-xlsx",
        "slides" => "export-pptx",
        _ => "export-html",
    };
    vec![
        MenuItem::new("menu.file.new", "new", ""),
        MenuItem::sep("menu.file.open", "back", ""),
        MenuItem::new("menu.file.save", "save", "Ctrl+S"),
        MenuItem::sep("menu.file.save_as", "save-as", "Ctrl+Shift+S"),
        MenuItem::new("menu.file.export_pdf", "export-pdf", ""),
        MenuItem {
            label_key: export_key,
            cmd: export_cmd,
            shortcut: "",
            separator_after: false,
        },
        MenuItem::sep("menu.file.export_html", "export-html", ""),
        MenuItem::new("menu.file.print", "print", "Ctrl+P"),
        MenuItem::new("menu.file.close", "back", ""),
    ]
}

fn edit_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("menu.edit.undo", "undo", "Ctrl+Z"),
        MenuItem::sep("menu.edit.redo", "redo", "Ctrl+Y"),
        MenuItem::new("menu.edit.cut", "cut", "Ctrl+X"),
        MenuItem::new("menu.edit.copy", "copy", "Ctrl+C"),
        MenuItem::sep("menu.edit.paste", "paste", "Ctrl+V"),
        MenuItem::new("menu.edit.find", "find", "Ctrl+F"),
        MenuItem::new("menu.edit.replace", "replace", "Ctrl+H"),
    ]
}

fn view_items(document_type: &str) -> Vec<MenuItem> {
    let mut items = vec![
        MenuItem::new("menu.view.zoom_in", "zoom-in", "Ctrl++"),
        MenuItem::new("menu.view.zoom_out", "zoom-out", "Ctrl+-"),
        MenuItem::sep("menu.view.zoom_reset", "zoom-reset", "Ctrl+0"),
        MenuItem::new("menu.view.toggle_sidebar", "toggle-sidebar", ""),
        MenuItem::new("menu.view.toggle_theme", "toggle-theme", ""),
    ];
    if document_type == "calc" {
        items.push(MenuItem::new("menu.view.freeze_rows", "freeze-rows", ""));
        items.push(MenuItem::new("menu.view.freeze_cols", "freeze-cols", ""));
    }
    if document_type == "slides" {
        items.push(MenuItem::new("menu.view.presenter", "presenter-view", ""));
    }
    items
}

fn insert_items(document_type: &str) -> Vec<MenuItem> {
    match document_type {
        "writer" => vec![
            MenuItem::new("menu.insert.image", "insert-image", ""),
            MenuItem::new("menu.insert.table", "insert-table", ""),
            MenuItem::new("menu.insert.link", "insert-link", ""),
            MenuItem::new("menu.insert.page_break", "insert-page-break", ""),
            MenuItem::new("menu.insert.section_break", "insert-section-break", ""),
            MenuItem::new("menu.insert.header", "insert-header", ""),
            MenuItem::new("menu.insert.footer", "insert-footer", ""),
            MenuItem::new("menu.insert.footnote", "insert-footnote", ""),
            MenuItem::new("menu.insert.toc", "insert-toc", ""),
            MenuItem::new("menu.insert.citation", "insert-citation", ""),
            MenuItem::new("menu.insert.merge_field", "insert-merge-field", ""),
        ],
        "calc" => vec![
            MenuItem::new("menu.insert.row_above", "insert-row", ""),
            MenuItem::new("menu.insert.col_left", "insert-col", ""),
            MenuItem::sep("menu.insert.sheet", "insert-sheet", ""),
            MenuItem::new("menu.insert.chart", "insert-chart", ""),
            MenuItem::new("menu.insert.function", "insert-function", ""),
            MenuItem::new("menu.insert.validation", "insert-validation", ""),
            MenuItem::new("menu.insert.named_range", "insert-named-range", ""),
            MenuItem::new("menu.insert.pivot", "insert-pivot", ""),
        ],
        "slides" => vec![
            MenuItem::new("menu.insert.slide", "insert-slide", ""),
            MenuItem::new("menu.insert.textbox", "insert-textbox", ""),
            MenuItem::new("menu.insert.image", "insert-image", ""),
            MenuItem::new("menu.insert.shape", "insert-shape", ""),
            MenuItem::new("menu.insert.animation", "insert-animation", ""),
            MenuItem::new("menu.insert.chart", "insert-chart", ""),
        ],
        _ => vec![
            MenuItem::new("menu.insert.text", "insert-text", ""),
            MenuItem::new("menu.insert.image", "insert-image", ""),
            MenuItem::new("menu.insert.shape", "insert-shape", ""),
        ],
    }
}

fn format_items(document_type: &str) -> Vec<MenuItem> {
    match document_type {
        "writer" => vec![
            MenuItem::new("menu.format.bold", "format-bold", "Ctrl+B"),
            MenuItem::new("menu.format.italic", "format-italic", "Ctrl+I"),
            MenuItem::sep("menu.format.underline", "format-underline", "Ctrl+U"),
            MenuItem::new("menu.format.paragraph", "format-paragraph", ""),
            MenuItem::new("menu.format.styles", "format-styles", ""),
        ],
        "calc" => vec![
            MenuItem::new("menu.format.cells", "format-cells", ""),
            MenuItem::new("menu.format.number", "format-number", ""),
            MenuItem::sep("menu.format.conditional", "format-conditional", ""),
            MenuItem::new("menu.format.merge", "format-merge", ""),
        ],
        "slides" => vec![
            MenuItem::new("menu.format.background", "format-background", ""),
            MenuItem::new("menu.format.layout", "format-layout", ""),
            MenuItem::new("menu.format.transition", "format-transition", ""),
        ],
        _ => vec![
            MenuItem::new("menu.format.stroke", "format-stroke", ""),
            MenuItem::new("menu.format.fill", "format-fill", ""),
        ],
    }
}

fn tools_items(document_type: &str) -> Vec<MenuItem> {
    let mut v = Vec::new();
    if document_type == "writer" {
        v.push(MenuItem::new("menu.tools.word_count", "word-count", ""));
        v.push(MenuItem::new("menu.tools.spell_check", "spell-check", ""));
    }
    v.push(MenuItem::sep("menu.tools.statistics", "statistics", ""));
    v.push(MenuItem::new("menu.tools.comments", "toggle-comments", ""));
    v.push(MenuItem::new(
        "menu.tools.version_history",
        "version-history",
        "",
    ));
    v
}

fn help_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("menu.help.shortcuts", "help-shortcuts", ""),
        MenuItem::new("menu.help.about", "about", ""),
    ]
}

pub fn menus(document_type: &str) -> Vec<(&'static str, &'static str, Vec<MenuItem>)> {
    vec![
        ("file", "menubar.file", file_items(document_type)),
        ("edit", "menubar.edit", edit_items()),
        ("view", "menubar.view", view_items(document_type)),
        ("insert", "menubar.insert", insert_items(document_type)),
        ("format", "menubar.format", format_items(document_type)),
        ("tools", "menubar.tools", tools_items(document_type)),
        ("help", "menubar.help", help_items()),
    ]
}

/// Grouped action identifiers for the embedded browser command bar. The host
/// supplies handlers for its supported commands; other entries stay hidden.
pub fn embedded_menus(document_type: &str) -> Vec<(&'static str, Vec<&'static str>)> {
    menus(document_type)
        .into_iter()
        .map(|(group, _, items)| {
            (
                group,
                items
                    .into_iter()
                    .map(|item| item.label_key.rsplit('.').next().unwrap_or(item.label_key))
                    .collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_commands_track_the_web_menu_for_each_format() {
        for (kind, export) in [
            ("writer", "export_docx"),
            ("calc", "export_xlsx"),
            ("slides", "export_pptx"),
        ] {
            let model = embedded_menus(kind);
            let group = |id| &model.iter().find(|(key, _)| *key == id).unwrap().1;
            assert!(group("file").contains(&export));
            assert!(group("edit").contains(&"undo"));
            assert!(group("view").contains(&"zoom_reset"));
            assert_eq!(group("format").contains(&"bold"), kind == "writer");
        }
    }
}
