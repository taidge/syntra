use dioxus::prelude::*;

/// Document editing toolbar whose buttons adapt to the document type.
///
/// `doc_type` must be one of `"writer"`, `"calc"`, or `"slides"`.
#[component]
pub fn Toolbar(doc_type: String) -> Element {
    match doc_type.as_str() {
        "writer" => rsx! { WriterToolbar {} },
        "calc" => rsx! { CalcToolbar {} },
        "slides" => rsx! { SlidesToolbar {} },
        _ => rsx! { WriterToolbar {} },
    }
}

// ---------------------------------------------------------------------------
// Writer toolbar
// ---------------------------------------------------------------------------

#[component]
fn WriterToolbar() -> Element {
    rsx! {
        div { class: "toolbar",
            // Font controls
            div { class: "toolbar-group",
                select { class: "toolbar-select font-family",
                    option { "Arial" }
                    option { "Times New Roman" }
                    option { "Courier New" }
                    option { "Georgia" }
                    option { "Verdana" }
                }
                select { class: "toolbar-select font-size",
                    option { value: "10", "10" }
                    option { value: "11", "11" }
                    option { value: "12", selected: true, "12" }
                    option { value: "14", "14" }
                    option { value: "16", "16" }
                    option { value: "18", "18" }
                    option { value: "24", "24" }
                    option { value: "36", "36" }
                }
            }

            div { class: "toolbar-separator" }

            // Text formatting
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Bold (Ctrl+B)", "B" }
                button { class: "toolbar-btn italic", title: "Italic (Ctrl+I)", "I" }
                button { class: "toolbar-btn underline", title: "Underline (Ctrl+U)", "U" }
                button { class: "toolbar-btn strikethrough", title: "Strikethrough", "S" }
            }

            div { class: "toolbar-separator" }

            // Headings
            div { class: "toolbar-group",
                select { class: "toolbar-select",
                    option { "Normal" }
                    option { "Heading 1" }
                    option { "Heading 2" }
                    option { "Heading 3" }
                    option { "Heading 4" }
                }
            }

            div { class: "toolbar-separator" }

            // Lists
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Bulleted List", "* List" }
                button { class: "toolbar-btn", title: "Numbered List", "1. List" }
                button { class: "toolbar-btn", title: "Indent", ">>" }
                button { class: "toolbar-btn", title: "Outdent", "<<" }
            }

            div { class: "toolbar-separator" }

            // Alignment
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Align Left", "L" }
                button { class: "toolbar-btn", title: "Align Center", "C" }
                button { class: "toolbar-btn", title: "Align Right", "R" }
                button { class: "toolbar-btn", title: "Justify", "J" }
            }

            div { class: "toolbar-separator" }

            // Insert
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Insert Image", "Img" }
                button { class: "toolbar-btn", title: "Insert Table", "Table" }
                button { class: "toolbar-btn", title: "Insert Link", "Link" }
            }

            div { class: "toolbar-separator" }

            // Colors
            div { class: "toolbar-group",
                input { r#type: "color", class: "toolbar-color", title: "Text Color", value: "#000000" }
                input { r#type: "color", class: "toolbar-color", title: "Highlight Color", value: "#ffffff" }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Calc toolbar
// ---------------------------------------------------------------------------

#[component]
fn CalcToolbar() -> Element {
    rsx! {
        div { class: "toolbar",
            // Font
            div { class: "toolbar-group",
                select { class: "toolbar-select font-family",
                    option { "Arial" }
                    option { "Times New Roman" }
                    option { "Courier New" }
                }
                select { class: "toolbar-select font-size",
                    option { value: "10", "10" }
                    option { value: "11", selected: true, "11" }
                    option { value: "12", "12" }
                    option { value: "14", "14" }
                }
            }

            div { class: "toolbar-separator" }

            // Text formatting
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Bold", "B" }
                button { class: "toolbar-btn italic", title: "Italic", "I" }
                button { class: "toolbar-btn underline", title: "Underline", "U" }
            }

            div { class: "toolbar-separator" }

            // Number formatting
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Currency", "$" }
                button { class: "toolbar-btn", title: "Percent", "%" }
                button { class: "toolbar-btn", title: "Decrease Decimal", ".0" }
                button { class: "toolbar-btn", title: "Increase Decimal", ".00" }
            }

            div { class: "toolbar-separator" }

            // Cell operations
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Merge Cells", "Merge" }
                button { class: "toolbar-btn", title: "Cell Borders", "Borders" }
                input { r#type: "color", class: "toolbar-color", title: "Fill Color", value: "#ffffff" }
                input { r#type: "color", class: "toolbar-color", title: "Text Color", value: "#000000" }
            }

            div { class: "toolbar-separator" }

            // Alignment
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Align Left", "L" }
                button { class: "toolbar-btn", title: "Align Center", "C" }
                button { class: "toolbar-btn", title: "Align Right", "R" }
            }

            div { class: "toolbar-separator" }

            // Data
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Sort A-Z", "Sort" }
                button { class: "toolbar-btn", title: "Filter", "Filter" }
                button { class: "toolbar-btn", title: "Insert Function", "f(x)" }
                button { class: "toolbar-btn", title: "Insert Chart", "Chart" }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Slides toolbar
// ---------------------------------------------------------------------------

#[component]
fn SlidesToolbar() -> Element {
    rsx! {
        div { class: "toolbar",
            // Insert
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Insert Text Box", "Text" }
                button { class: "toolbar-btn", title: "Insert Image", "Image" }
                button { class: "toolbar-btn", title: "Insert Shape", "Shape" }
                button { class: "toolbar-btn", title: "Insert Table", "Table" }
                button { class: "toolbar-btn", title: "Insert Video", "Video" }
            }

            div { class: "toolbar-separator" }

            // Text formatting
            div { class: "toolbar-group",
                select { class: "toolbar-select font-family",
                    option { "Arial" }
                    option { "Times New Roman" }
                    option { "Georgia" }
                }
                select { class: "toolbar-select font-size",
                    option { value: "14", "14" }
                    option { value: "18", "18" }
                    option { value: "24", selected: true, "24" }
                    option { value: "36", "36" }
                    option { value: "48", "48" }
                }
                button { class: "toolbar-btn", title: "Bold", "B" }
                button { class: "toolbar-btn italic", title: "Italic", "I" }
            }

            div { class: "toolbar-separator" }

            // Colors
            div { class: "toolbar-group",
                input { r#type: "color", class: "toolbar-color", title: "Text Color", value: "#000000" }
                input { r#type: "color", class: "toolbar-color", title: "Fill Color", value: "#ffffff" }
            }

            div { class: "toolbar-separator" }

            // Slide actions
            div { class: "toolbar-group",
                button { class: "toolbar-btn", title: "Transition", "Transition" }
                button { class: "toolbar-btn", title: "Animation", "Animate" }
            }

            div { class: "toolbar-separator" }

            // Present
            div { class: "toolbar-group",
                button { class: "toolbar-btn btn-present", title: "Start Presentation", "Present" }
            }
        }
    }
}
