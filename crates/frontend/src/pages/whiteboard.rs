use dioxus::prelude::*;

use crate::components::navbar::Navbar;

/// Whiteboard / freeform canvas editor with drawing tools.
#[component]
pub fn Whiteboard(id: String) -> Element {
    let mut tool = use_signal(|| "pen".to_string());
    let mut color = use_signal(|| "#000000".to_string());
    let mut stroke_width = use_signal(|| 2.0f64);
    let mut doc_title = use_signal(|| "Untitled Whiteboard".to_string());
    let mut saved_status = use_signal(|| "All changes saved".to_string());
    let mut zoom = use_signal(|| 100u32);
    let mut elements = use_signal(|| Vec::<serde_json::Value>::new());
    let mut undo_stack = use_signal(|| Vec::<Vec<serde_json::Value>>::new());

    /// List of available drawing tools.
    let tools: Vec<(&str, &str)> = vec![
        ("pen", "Pen"),
        ("eraser", "Eraser"),
        ("rect", "Rectangle"),
        ("circle", "Circle"),
        ("line", "Line"),
        ("arrow", "Arrow"),
        ("text", "Text"),
        ("select", "Select"),
    ];

    rsx! {
        Navbar {}
        div { class: "editor-container",
            // Title bar
            div { class: "doc-title-bar",
                input {
                    class: "doc-title-input",
                    value: "{doc_title}",
                    oninput: move |e| doc_title.set(e.value()),
                    placeholder: "Untitled Whiteboard",
                }
                span { class: "save-status", "{saved_status}" }
            }

            // Drawing toolbar
            div { class: "whiteboard-toolbar",
                // Tool buttons
                div { class: "toolbar-group",
                    for (key, label) in tools.iter() {
                        {
                            let key_owned = key.to_string();
                            let key_for_class = key.to_string();
                            let label_owned = label.to_string();
                            rsx! {
                                button {
                                    class: if *tool.read() == key_for_class { "tool-btn active" } else { "tool-btn" },
                                    title: "{label_owned}",
                                    onclick: move |_| tool.set(key_owned.clone()),
                                    "{label_owned}"
                                }
                            }
                        }
                    }
                }

                div { class: "toolbar-separator" }

                // Color and stroke
                div { class: "toolbar-group",
                    label { class: "toolbar-label", "Color:" }
                    input {
                        r#type: "color",
                        class: "toolbar-color",
                        value: "{color}",
                        oninput: move |e| color.set(e.value()),
                    }
                    label { class: "toolbar-label", "Size:" }
                    input {
                        r#type: "range",
                        class: "toolbar-range",
                        min: "1",
                        max: "20",
                        value: "{stroke_width}",
                        oninput: move |e| {
                            if let Ok(v) = e.value().parse::<f64>() {
                                stroke_width.set(v);
                            }
                        },
                    }
                    span { class: "stroke-preview", "{stroke_width:.0}px" }
                }

                div { class: "toolbar-separator" }

                // Undo / Redo / Clear
                div { class: "toolbar-group",
                    button {
                        class: "tool-btn",
                        title: "Undo (Ctrl+Z)",
                        onclick: move |_| {
                            let current = elements.read().clone();
                            if !current.is_empty() {
                                undo_stack.write().push(current.clone());
                                elements.write().pop();
                                saved_status.set("Unsaved changes".to_string());
                            }
                        },
                        "Undo"
                    }
                    button {
                        class: "tool-btn",
                        title: "Redo (Ctrl+Y)",
                        onclick: move |_| {
                            if let Some(state) = undo_stack.write().pop() {
                                *elements.write() = state;
                                saved_status.set("Unsaved changes".to_string());
                            }
                        },
                        "Redo"
                    }
                    button {
                        class: "tool-btn danger",
                        title: "Clear canvas",
                        onclick: move |_| {
                            let current = elements.read().clone();
                            if !current.is_empty() {
                                undo_stack.write().push(current);
                            }
                            elements.write().clear();
                            saved_status.set("Unsaved changes".to_string());
                        },
                        "Clear"
                    }
                }

                div { class: "toolbar-separator" }

                // Zoom
                div { class: "toolbar-group",
                    button {
                        class: "tool-btn",
                        onclick: move |_| {
                            let z = *zoom.read();
                            if z > 25 { zoom.set(z - 25); }
                        },
                        "-"
                    }
                    span { class: "zoom-label", "{zoom}%" }
                    button {
                        class: "tool-btn",
                        onclick: move |_| {
                            let z = *zoom.read();
                            if z < 400 { zoom.set(z + 25); }
                        },
                        "+"
                    }
                }

                div { class: "toolbar-separator" }

                // Export
                div { class: "toolbar-group",
                    button {
                        class: "tool-btn",
                        title: "Export as PNG",
                        onclick: move |_| {
                            saved_status.set("Export PNG: coming soon".to_string());
                        },
                        "Export PNG"
                    }
                    button {
                        class: "tool-btn",
                        title: "Export as SVG",
                        onclick: move |_| {
                            saved_status.set("Export SVG: coming soon".to_string());
                        },
                        "Export SVG"
                    }
                }
            }

            // Canvas container
            div { class: "whiteboard-canvas-container",
                svg {
                    class: "whiteboard-canvas",
                    width: "100%",
                    height: "100%",
                    view_box: "0 0 1920 1080",
                    // Grid background pattern
                    defs {
                        pattern {
                            id: "grid",
                            width: "20",
                            height: "20",
                            pattern_units: "userSpaceOnUse",
                            path {
                                d: "M 20 0 L 0 0 0 20",
                                fill: "none",
                                stroke: "#e0e0e0",
                                stroke_width: "0.5",
                            }
                        }
                    }
                    rect {
                        width: "100%",
                        height: "100%",
                        fill: "url(#grid)",
                    }
                    // Drawing elements would be rendered here dynamically
                }
            }

            // Status bar
            div { class: "status-bar",
                span { "Tool: {tool}" }
                span { "Color: {color}" }
                span { "Stroke: {stroke_width:.0}px" }
                span { "Zoom: {zoom}%" }
                span { "ID: {id}" }
            }
        }
    }
}
