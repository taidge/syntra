use crate::components::ui::theme;
use glory::prelude::*;

#[derive(Clone, PartialEq)]
struct MenuItem {
    label_key: &'static str,
    cmd: &'static str,
    shortcut: &'static str,
    separator_after: bool,
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

#[allow(non_snake_case)]
pub fn Menubar(
    _scope: Scope,
    document_type: String,
    on_command: impl Fn(String) + 'static,
) -> impl Widget {
    let command = std::rc::Rc::new(on_command);
    let menus = vec![
        ("file", "menubar.file", file_items(&document_type)),
        ("edit", "menubar.edit", edit_items()),
        ("view", "menubar.view", view_items(&document_type)),
        ("insert", "menubar.insert", insert_items(&document_type)),
        ("format", "menubar.format", format_items(&document_type)),
        ("tools", "menubar.tools", tools_items(&document_type)),
        ("help", "menubar.help", help_items()),
    ];
    let widgets = menus
        .into_iter()
        .map(|(key, label, items)| {
            let mut menu = sapid::menu(format!("syntra-{key}"))
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
