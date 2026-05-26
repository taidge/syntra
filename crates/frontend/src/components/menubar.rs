use dioxus::prelude::*;

#[derive(Clone, PartialEq)]
struct MenuItem {
    label: &'static str,
    cmd: &'static str,
    shortcut: &'static str,
    separator_after: bool,
}

impl MenuItem {
    const fn new(label: &'static str, cmd: &'static str, shortcut: &'static str) -> Self {
        Self { label, cmd, shortcut, separator_after: false }
    }
    const fn sep(label: &'static str, cmd: &'static str, shortcut: &'static str) -> Self {
        Self { label, cmd, shortcut, separator_after: true }
    }
}

fn file_items(doc_type: &str) -> Vec<MenuItem> {
    let export_label = match doc_type {
        "writer" => "Export DOCX",
        "calc" => "Export XLSX",
        "slides" => "Export PPTX",
        _ => "Export HTML",
    };
    let export_cmd = match doc_type {
        "writer" => "export-docx",
        "calc" => "export-xlsx",
        "slides" => "export-pptx",
        _ => "export-html",
    };
    vec![
        MenuItem::new("New", "new", ""),
        MenuItem::sep("Open…", "back", ""),
        MenuItem::new("Save", "save", "Ctrl+S"),
        MenuItem::sep("Save As…", "save-as", "Ctrl+Shift+S"),
        MenuItem::new("Export PDF", "export-pdf", ""),
        MenuItem { label: export_label, cmd: export_cmd, shortcut: "", separator_after: false },
        MenuItem::sep("Export HTML", "export-html", ""),
        MenuItem::new("Print", "print", "Ctrl+P"),
        MenuItem::new("Close", "back", ""),
    ]
}

fn edit_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("Undo", "undo", "Ctrl+Z"),
        MenuItem::sep("Redo", "redo", "Ctrl+Y"),
        MenuItem::new("Cut", "cut", "Ctrl+X"),
        MenuItem::new("Copy", "copy", "Ctrl+C"),
        MenuItem::sep("Paste", "paste", "Ctrl+V"),
        MenuItem::new("Find", "find", "Ctrl+F"),
        MenuItem::new("Replace", "replace", "Ctrl+H"),
    ]
}

fn view_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("Zoom In", "zoom-in", "Ctrl++"),
        MenuItem::new("Zoom Out", "zoom-out", "Ctrl+-"),
        MenuItem::sep("Reset Zoom", "zoom-reset", "Ctrl+0"),
        MenuItem::new("Toggle Sidebar", "toggle-sidebar", ""),
        MenuItem::new("Toggle Theme", "toggle-theme", ""),
    ]
}

fn insert_items(doc_type: &str) -> Vec<MenuItem> {
    match doc_type {
        "writer" => vec![
            MenuItem::new("Image", "insert-image", ""),
            MenuItem::new("Table", "insert-table", ""),
            MenuItem::new("Link", "insert-link", ""),
            MenuItem::new("Page Break", "insert-page-break", ""),
        ],
        "calc" => vec![
            MenuItem::new("Row Above", "insert-row", ""),
            MenuItem::new("Column Left", "insert-col", ""),
            MenuItem::sep("Sheet", "insert-sheet", ""),
            MenuItem::new("Chart", "insert-chart", ""),
            MenuItem::new("Function", "insert-function", ""),
        ],
        "slides" => vec![
            MenuItem::new("Slide", "insert-slide", ""),
            MenuItem::new("Text Box", "insert-textbox", ""),
            MenuItem::new("Image", "insert-image", ""),
            MenuItem::new("Shape", "insert-shape", ""),
        ],
        _ => vec![
            MenuItem::new("Text", "insert-text", ""),
            MenuItem::new("Image", "insert-image", ""),
            MenuItem::new("Shape", "insert-shape", ""),
        ],
    }
}

fn format_items(doc_type: &str) -> Vec<MenuItem> {
    match doc_type {
        "writer" => vec![
            MenuItem::new("Bold", "format-bold", "Ctrl+B"),
            MenuItem::new("Italic", "format-italic", "Ctrl+I"),
            MenuItem::sep("Underline", "format-underline", "Ctrl+U"),
            MenuItem::new("Paragraph", "format-paragraph", ""),
            MenuItem::new("Styles", "format-styles", ""),
        ],
        "calc" => vec![
            MenuItem::new("Cells", "format-cells", ""),
            MenuItem::new("Number", "format-number", ""),
            MenuItem::sep("Conditional Formatting", "format-conditional", ""),
            MenuItem::new("Merge Cells", "format-merge", ""),
        ],
        "slides" => vec![
            MenuItem::new("Background", "format-background", ""),
            MenuItem::new("Slide Layout", "format-layout", ""),
            MenuItem::new("Transition", "format-transition", ""),
        ],
        _ => vec![
            MenuItem::new("Stroke", "format-stroke", ""),
            MenuItem::new("Fill", "format-fill", ""),
        ],
    }
}

fn tools_items(doc_type: &str) -> Vec<MenuItem> {
    let mut v = Vec::new();
    if doc_type == "writer" {
        v.push(MenuItem::new("Word Count", "word-count", ""));
        v.push(MenuItem::new("Spell Check", "spell-check", ""));
    }
    v.push(MenuItem::sep("Statistics", "statistics", ""));
    v.push(MenuItem::new("Comments", "toggle-comments", ""));
    v.push(MenuItem::new("Version History", "version-history", ""));
    v
}

fn help_items() -> Vec<MenuItem> {
    vec![
        MenuItem::new("Keyboard Shortcuts", "help-shortcuts", ""),
        MenuItem::new("About Contrix Office", "about", ""),
    ]
}

#[derive(Props, Clone, PartialEq)]
pub struct MenubarProps {
    pub doc_type: String,
    pub on_command: EventHandler<String>,
}

#[component]
pub fn Menubar(props: MenubarProps) -> Element {
    let mut open = use_signal(|| None::<String>);

    let doc_type = props.doc_type.clone();
    let menus: Vec<(&'static str, Vec<MenuItem>)> = vec![
        ("File", file_items(&doc_type)),
        ("Edit", edit_items()),
        ("View", view_items()),
        ("Insert", insert_items(&doc_type)),
        ("Format", format_items(&doc_type)),
        ("Tools", tools_items(&doc_type)),
        ("Help", help_items()),
    ];

    rsx! {
        div {
            class: "menubar",
            onclick: move |e: MouseEvent| {
                e.stop_propagation();
            },
            for (name, items) in menus.into_iter() {
                {
                    let name_owned = name.to_string();
                    let name_for_class = name.to_string();
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
                                "{name_for_class}"
                            }
                            if is_open {
                                div { class: "menubar-dropdown",
                                    for (idx, item) in items.iter().enumerate() {
                                        {
                                            let cmd = item.cmd.to_string();
                                            let label = item.label;
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
