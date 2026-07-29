use dioxus::prelude::*;

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

fn view_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("menu.view.zoom_in", "zoom-in", "Ctrl++"),
        MenuItem::new("menu.view.zoom_out", "zoom-out", "Ctrl+-"),
        MenuItem::sep("menu.view.zoom_reset", "zoom-reset", "Ctrl+0"),
        MenuItem::new("menu.view.toggle_sidebar", "toggle-sidebar", ""),
        MenuItem::new("menu.view.toggle_theme", "toggle-theme", ""),
    ]
}

fn insert_items(document_type: &str) -> Vec<MenuItem> {
    match document_type {
        "writer" => vec![
            MenuItem::new("menu.insert.image", "insert-image", ""),
            MenuItem::new("menu.insert.table", "insert-table", ""),
            MenuItem::new("menu.insert.link", "insert-link", ""),
            MenuItem::new("menu.insert.page_break", "insert-page-break", ""),
        ],
        "calc" => vec![
            MenuItem::new("menu.insert.row_above", "insert-row", ""),
            MenuItem::new("menu.insert.col_left", "insert-col", ""),
            MenuItem::sep("menu.insert.sheet", "insert-sheet", ""),
            MenuItem::new("menu.insert.chart", "insert-chart", ""),
            MenuItem::new("menu.insert.function", "insert-function", ""),
        ],
        "slides" => vec![
            MenuItem::new("menu.insert.slide", "insert-slide", ""),
            MenuItem::new("menu.insert.textbox", "insert-textbox", ""),
            MenuItem::new("menu.insert.image", "insert-image", ""),
            MenuItem::new("menu.insert.shape", "insert-shape", ""),
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

#[derive(Props, Clone, PartialEq)]
pub struct MenubarProps {
    pub document_type: String,
    pub on_command: EventHandler<String>,
}

#[component]
pub fn Menubar(props: MenubarProps) -> Element {
    let _ = use_context::<Signal<String>>().read();
    let mut open = use_signal(|| None::<String>);

    let document_type = props.document_type.clone();
    let menus: Vec<(&'static str, &'static str, Vec<MenuItem>)> = vec![
        ("File", "menubar.file", file_items(&document_type)),
        ("Edit", "menubar.edit", edit_items()),
        ("View", "menubar.view", view_items()),
        ("Insert", "menubar.insert", insert_items(&document_type)),
        ("Format", "menubar.format", format_items(&document_type)),
        ("Tools", "menubar.tools", tools_items(&document_type)),
        ("Help", "menubar.help", help_items()),
    ];

    rsx! {
        div {
            class: "menubar",
            onclick: move |e: MouseEvent| {
                e.stop_propagation();
            },
            for (name, label_key, items) in menus.into_iter() {
                {
                    let name_owned = name.to_string();
                    let display_label = crate::i18n::t(label_key);
                    let name_for_click = name.to_string();
                    let is_open = open.read().as_deref() == Some(name);
                    rsx! {
                        div {
                            class: "menubar-menu",
                            style: "position: relative; display: inline-block;",
                            button {
                                class: if is_open { "menubar-item active" } else { "menubar-item" },
                                onclick: move |e: MouseEvent| {
                                    e.stop_propagation();
                                    let cur = open.read().clone();
                                    if cur.as_deref() == Some(name_for_click.as_str()) {
                                        open.set(None);
                                    } else {
                                        open.set(Some(name_for_click.clone()));
                                    }
                                },
                                "{display_label}"
                            }
                            if is_open {
                                div { class: "menubar-dropdown",
                                    for (idx, item) in items.iter().enumerate() {
                                        {
                                            let cmd = item.cmd.to_string();
                                            let label = crate::i18n::t(item.label_key);
                                            let shortcut = item.shortcut;
                                            let sep = item.separator_after;
                                            let on_cmd = props.on_command;
                                            rsx! {
                                                button {
                                                    key: "{name_owned}-{idx}",
                                                    onclick: move |e: MouseEvent| {
                                                        e.stop_propagation();
                                                        on_cmd.call(cmd.clone());
                                                        open.set(None);
                                                    },
                                                    span { "{label}" }
                                                    if !shortcut.is_empty() {
                                                        span { class: "menubar-shortcut", "{shortcut}" }
                                                    }
                                                }
                                                if sep {
                                                    div { class: "menubar-sep" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
