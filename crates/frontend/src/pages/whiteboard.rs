use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_timers::callback::Timeout;
use serde::Deserialize;

use crate::components::navbar::Navbar;
use crate::state::{AuthState, api_get, api_put};

#[derive(Clone, Deserialize)]
struct DocumentResponse {
    name: String,
    content: serde_json::Value,
}

#[derive(Deserialize)]
struct WhiteboardLite {
    #[serde(default)]
    layers: Vec<LayerLite>,
    #[serde(default)]
    background_color: Option<String>,
}

#[derive(Deserialize)]
struct LayerLite {
    #[serde(default)]
    elements: Vec<serde_json::Value>,
}

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
    let mut loaded = use_signal(|| false);

    let auth = use_context::<Signal<AuthState>>();
    let id_for_resource = id.clone();
    let doc_resource = use_resource(move || {
        let id = id_for_resource.clone();
        let token = auth.read().token.clone().unwrap_or_default();
        async move {
            api_get::<DocumentResponse>(&format!("/documents/{id}"), &token).await
        }
    });

    use_effect(move || {
        if *loaded.read() {
            return;
        }
        if let Some(Ok(doc)) = doc_resource.read().as_ref() {
            doc_title.set(doc.name.clone());
            if let Ok(wb) = serde_json::from_value::<WhiteboardLite>(doc.content.clone()) {
                let mut flat: Vec<serde_json::Value> = Vec::new();
                for layer in wb.layers {
                    flat.extend(layer.elements);
                }
                elements.set(flat);
                if let Some(bg) = wb.background_color {
                    // Background hint preserved; canvas background colour
                    // wiring is part of a later task.
                    let _ = bg;
                }
            }
            let _ = undo_stack;
            loaded.set(true);
        }
    });

    let save_timer: Rc<RefCell<Option<Timeout>>> = use_hook(|| Rc::new(RefCell::new(None)));

    let serialize_whiteboard = move || -> serde_json::Value {
        serde_json::json!({ "elements": elements.read().clone() })
    };

    let perform_save = {
        let id = id.clone();
        move |body: serde_json::Value, mut status: Signal<String>, token: String| {
            let id = id.clone();
            status.set("Saving…".to_string());
            wasm_bindgen_futures::spawn_local(async move {
                let req = serde_json::json!({ "content": body });
                match api_put::<serde_json::Value, _>(&format!("/documents/{id}"), &req, &token).await {
                    Ok(_) => status.set("All changes saved".to_string()),
                    Err(e) => status.set(format!("Save failed: {e}")),
                }
            });
        }
    };

    let schedule_save = {
        let save_timer = save_timer.clone();
        let perform_save = perform_save.clone();
        let serialize_whiteboard = serialize_whiteboard.clone();
        move || {
            if !*loaded.read() {
                return;
            }
            saved_status.set("Unsaved changes".to_string());
            let body = serialize_whiteboard();
            let token = auth.read().token.clone().unwrap_or_default();
            let status = saved_status;
            let perform_save = perform_save.clone();
            let new_timer = Timeout::new(1500, move || {
                perform_save(body, status, token);
            });
            *save_timer.borrow_mut() = Some(new_timer);
        }
    };

    let flush_save = {
        let save_timer = save_timer.clone();
        let perform_save = perform_save.clone();
        let serialize_whiteboard = serialize_whiteboard.clone();
        move || {
            if !*loaded.read() {
                return;
            }
            save_timer.borrow_mut().take();
            let body = serialize_whiteboard();
            let token = auth.read().token.clone().unwrap_or_default();
            perform_save(body, saved_status, token);
        }
    };

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
        div {
            class: "editor-container",
            tabindex: "0",
            onkeydown: {
                let mut flush_save = flush_save.clone();
                move |evt: KeyboardEvent| {
                    if evt.modifiers().contains(Modifiers::CONTROL) {
                        if evt.key() == Key::Character("s".to_string()) {
                            evt.prevent_default();
                            flush_save();
                        }
                    }
                }
            },
            // Title bar
            div { class: "doc-title-bar",
                input {
                    class: "doc-title-input",
                    value: "{doc_title}",
                    oninput: {
                        let mut schedule_save = schedule_save.clone();
                        move |e: FormEvent| {
                            doc_title.set(e.value());
                            schedule_save();
                        }
                    },
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
                        onclick: {
                            let mut schedule_save = schedule_save.clone();
                            move |_| {
                                let current = elements.read().clone();
                                if !current.is_empty() {
                                    undo_stack.write().push(current.clone());
                                    elements.write().pop();
                                    schedule_save();
                                }
                            }
                        },
                        "Undo"
                    }
                    button {
                        class: "tool-btn",
                        title: "Redo (Ctrl+Y)",
                        onclick: {
                            let mut schedule_save = schedule_save.clone();
                            move |_| {
                                if let Some(state) = undo_stack.write().pop() {
                                    *elements.write() = state;
                                    schedule_save();
                                }
                            }
                        },
                        "Redo"
                    }
                    button {
                        class: "tool-btn danger",
                        title: "Clear canvas",
                        onclick: {
                            let mut schedule_save = schedule_save.clone();
                            move |_| {
                                let current = elements.read().clone();
                                if !current.is_empty() {
                                    undo_stack.write().push(current);
                                }
                                elements.write().clear();
                                schedule_save();
                            }
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

            // Loading / error overlay
            match doc_resource.read().as_ref() {
                None => rsx! { div { class: "editor-loading", "Loading document..." } },
                Some(Err(e)) => rsx! { div { class: "editor-error", "Failed to load: {e}" } },
                Some(Ok(_)) => rsx! {},
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
