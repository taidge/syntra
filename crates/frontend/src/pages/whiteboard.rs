use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_timers::callback::Timeout;
use serde::Deserialize;
use crate::Route;
use crate::components::menubar::Menubar;
use crate::components::navbar::Navbar;
use crate::components::toast::{Toast, ToastKind, push_toast};
use crate::state::{AuthState, api_get, api_put, export_document};

const VIEW_W: f64 = 1920.0;
const VIEW_H: f64 = 1080.0;
const SVG_ID: &str = "whiteboard-svg";

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
    elements: Vec<serde_json::Value>,
    #[serde(default)]
    background_color: Option<String>,
}

#[derive(Deserialize)]
struct LayerLite {
    #[serde(default)]
    elements: Vec<serde_json::Value>,
}

#[derive(Clone, PartialEq)]
struct DraftStroke {
    tool: String,
    color: String,
    stroke_width: f64,
    points: Vec<(f64, f64)>,
    start: (f64, f64),
    end: (f64, f64),
}

fn svg_point(client_x: f64, client_y: f64) -> Option<(f64, f64)> {
    let doc = web_sys::window()?.document()?;
    let el = doc.get_element_by_id(SVG_ID)?;
    let rect = el.get_bounding_client_rect();
    let rw = rect.width();
    let rh = rect.height();
    if rw <= 0.0 || rh <= 0.0 {
        return None;
    }
    let x = (client_x - rect.left()) * VIEW_W / rw;
    let y = (client_y - rect.top()) * VIEW_H / rh;
    Some((x, y))
}

fn pen_path_d(points: &[(f64, f64)]) -> String {
    if points.is_empty() {
        return String::new();
    }
    let mut s = String::new();
    for (i, (x, y)) in points.iter().enumerate() {
        if i == 0 {
            s.push_str(&format!("M {x:.2} {y:.2}"));
        } else {
            s.push_str(&format!(" L {x:.2} {y:.2}"));
        }
    }
    s
}

fn element_bbox(e: &serde_json::Value) -> Option<(f64, f64, f64, f64)> {
    let t = e.get("type")?.as_str()?;
    match t {
        "rect" => {
            let x = e.get("x")?.as_f64()?;
            let y = e.get("y")?.as_f64()?;
            let w = e.get("width")?.as_f64()?;
            let h = e.get("height")?.as_f64()?;
            Some((x, y, x + w, y + h))
        }
        "circle" => {
            let cx = e.get("cx")?.as_f64()?;
            let cy = e.get("cy")?.as_f64()?;
            let rx = e.get("rx")?.as_f64()?;
            let ry = e.get("ry")?.as_f64()?;
            Some((cx - rx, cy - ry, cx + rx, cy + ry))
        }
        "line" => {
            let x1 = e.get("x1")?.as_f64()?;
            let y1 = e.get("y1")?.as_f64()?;
            let x2 = e.get("x2")?.as_f64()?;
            let y2 = e.get("y2")?.as_f64()?;
            Some((x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)))
        }
        "pen" => {
            let pts = e.get("points")?.as_array()?;
            let mut min_x = f64::INFINITY;
            let mut min_y = f64::INFINITY;
            let mut max_x = f64::NEG_INFINITY;
            let mut max_y = f64::NEG_INFINITY;
            for p in pts {
                if let Some(arr) = p.as_array() {
                    if arr.len() == 2 {
                        let x = arr[0].as_f64().unwrap_or(0.0);
                        let y = arr[1].as_f64().unwrap_or(0.0);
                        if x < min_x { min_x = x; }
                        if y < min_y { min_y = y; }
                        if x > max_x { max_x = x; }
                        if y > max_y { max_y = y; }
                    }
                }
            }
            if min_x.is_finite() {
                Some((min_x - 4.0, min_y - 4.0, max_x + 4.0, max_y + 4.0))
            } else {
                None
            }
        }
        "text" => {
            let x = e.get("x")?.as_f64()?;
            let y = e.get("y")?.as_f64()?;
            let s = e.get("size").and_then(|v| v.as_f64()).unwrap_or(16.0);
            let len = e.get("content").and_then(|v| v.as_str()).map(|c| c.chars().count()).unwrap_or(1) as f64;
            Some((x, y - s, x + s * 0.6 * len, y + 2.0))
        }
        _ => None,
    }
}

fn bbox_contains(b: (f64, f64, f64, f64), x: f64, y: f64) -> bool {
    x >= b.0 && x <= b.2 && y >= b.1 && y <= b.3
}

fn find_element_at(elements: &[serde_json::Value], x: f64, y: f64) -> Option<usize> {
    for (i, el) in elements.iter().enumerate().rev() {
        if let Some((x1, y1, x2, y2)) = element_bbox(el) {
            if x >= x1 - 4.0 && x <= x2 + 4.0 && y >= y1 - 4.0 && y <= y2 + 4.0 {
                return Some(i);
            }
        }
    }
    None
}

fn move_element(el: &mut serde_json::Value, dx: f64, dy: f64) {
    let ty = el.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
    let obj = match el.as_object_mut() { Some(o) => o, None => return };
    fn shift(v: &mut serde_json::Value, d: f64) {
        if let Some(n) = v.as_f64() { *v = serde_json::json!(n + d); }
    }
    match ty.as_str() {
        "rect" | "text" => {
            if let Some(x) = obj.get_mut("x") { shift(x, dx); }
            if let Some(y) = obj.get_mut("y") { shift(y, dy); }
        }
        "circle" => {
            if let Some(x) = obj.get_mut("cx") { shift(x, dx); }
            if let Some(y) = obj.get_mut("cy") { shift(y, dy); }
        }
        "line" => {
            if let Some(x) = obj.get_mut("x1") { shift(x, dx); }
            if let Some(y) = obj.get_mut("y1") { shift(y, dy); }
            if let Some(x) = obj.get_mut("x2") { shift(x, dx); }
            if let Some(y) = obj.get_mut("y2") { shift(y, dy); }
        }
        "pen" => {
            if let Some(points) = obj.get_mut("points").and_then(|p| p.as_array_mut()) {
                for p in points {
                    if let Some(arr) = p.as_array_mut() {
                        if let Some(x) = arr.get_mut(0) { shift(x, dx); }
                        if let Some(y) = arr.get_mut(1) { shift(y, dy); }
                    }
                }
            }
        }
        _ => {}
    }
}

fn xml_escape(input: &str) -> String {
    let mut s = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => s.push_str("&amp;"),
            '<' => s.push_str("&lt;"),
            '>' => s.push_str("&gt;"),
            '"' => s.push_str("&quot;"),
            '\'' => s.push_str("&apos;"),
            _ => s.push(ch),
        }
    }
    s
}

fn build_svg_string(elements: &[serde_json::Value]) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1920 1080\" width=\"1920\" height=\"1080\">",
    );
    s.push_str(r#"<rect width="100%" height="100%" fill="white"/>"#);
    for el in elements {
        let ty = el.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let stroke = el.get("stroke").and_then(|v| v.as_str()).unwrap_or("#000000");
        let sw = el.get("stroke_width").and_then(|v| v.as_f64()).unwrap_or(2.0);
        match ty {
            "pen" => {
                let points = el.get("points").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                if points.is_empty() {
                    continue;
                }
                s.push_str(&format!(
                    r#"<path fill="none" stroke="{stroke}" stroke-width="{sw}" stroke-linecap="round" stroke-linejoin="round" d=""#
                ));
                for (i, p) in points.iter().enumerate() {
                    let arr = p.as_array().cloned().unwrap_or_default();
                    let x = arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let y = arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
                    if i == 0 {
                        s.push_str(&format!("M {x:.2} {y:.2}"));
                    } else {
                        s.push_str(&format!(" L {x:.2} {y:.2}"));
                    }
                }
                s.push_str(r#""/>"#);
            }
            "rect" => {
                let x = el.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = el.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let w = el.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let h = el.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
                s.push_str(&format!(
                    r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="none" stroke="{stroke}" stroke-width="{sw}"/>"#
                ));
            }
            "circle" => {
                let cx = el.get("cx").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let cy = el.get("cy").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let rx = el.get("rx").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let ry = el.get("ry").and_then(|v| v.as_f64()).unwrap_or(0.0);
                s.push_str(&format!(
                    r#"<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="none" stroke="{stroke}" stroke-width="{sw}"/>"#
                ));
            }
            "line" => {
                let x1 = el.get("x1").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y1 = el.get("y1").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let x2 = el.get("x2").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y2 = el.get("y2").and_then(|v| v.as_f64()).unwrap_or(0.0);
                s.push_str(&format!(
                    r#"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{stroke}" stroke-width="{sw}" stroke-linecap="round"/>"#
                ));
            }
            "text" => {
                let x = el.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = el.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let content = el.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let col = el.get("color").and_then(|v| v.as_str()).unwrap_or("#000000");
                let size = el.get("size").and_then(|v| v.as_f64()).unwrap_or(16.0);
                s.push_str(&format!(
                    r#"<text x="{x}" y="{y}" fill="{col}" font-size="{size}" font-family="sans-serif">{}</text>"#,
                    xml_escape(content)
                ));
            }
            _ => {}
        }
    }
    s.push_str("</svg>");
    s
}

fn trigger_download(filename: &str, mime: &str, data: &[u8]) {
    use wasm_bindgen::JsCast;
    let Some(window) = web_sys::window() else { return };
    let Some(document) = window.document() else { return };
    let array = js_sys::Uint8Array::from(data);
    let parts = js_sys::Array::new();
    parts.push(&array.buffer());
    let blob_props = web_sys::BlobPropertyBag::new();
    blob_props.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_buffer_source_sequence_and_options(&parts, &blob_props) else { return };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else { return };
    let Ok(anchor_el) = document.create_element("a") else {
        let _ = web_sys::Url::revoke_object_url(&url);
        return;
    };
    let Ok(anchor) = anchor_el.dyn_into::<web_sys::HtmlAnchorElement>() else {
        let _ = web_sys::Url::revoke_object_url(&url);
        return;
    };
    anchor.set_href(&url);
    anchor.set_download(filename);
    anchor.click();
    let _ = web_sys::Url::revoke_object_url(&url);
}

fn svg_to_png_and_download(svg_str: &str, filename: &str) {
    use base64::Engine;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;
    use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement};
    let Some(window) = web_sys::window() else { return };
    let Some(document) = window.document() else { return };
    let Ok(img_el) = document.create_element("img") else { return };
    let Ok(img) = img_el.dyn_into::<HtmlImageElement>() else { return };
    let Ok(canvas_el) = document.create_element("canvas") else { return };
    let Ok(canvas) = canvas_el.dyn_into::<HtmlCanvasElement>() else { return };
    canvas.set_width(1920);
    canvas.set_height(1080);
    let svg_b64 = base64::engine::general_purpose::STANDARD.encode(svg_str.as_bytes());
    let data_url = format!("data:image/svg+xml;base64,{svg_b64}");
    let filename = filename.to_string();
    let canvas_for_load = canvas.clone();
    let img_for_load = img.clone();
    let onload = Closure::once_into_js(move || {
        let Ok(Some(ctx_raw)) = canvas_for_load.get_context("2d") else { return };
        let Ok(ctx) = ctx_raw.dyn_into::<CanvasRenderingContext2d>() else { return };
        let _ = ctx.draw_image_with_html_image_element(&img_for_load, 0.0, 0.0);
        let Ok(url) = canvas_for_load.to_data_url_with_type("image/png") else { return };
        let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
        let Ok(anchor_el) = document.create_element("a") else { return };
        let Ok(anchor) = anchor_el.dyn_into::<web_sys::HtmlAnchorElement>() else { return };
        anchor.set_href(&url);
        anchor.set_download(&filename);
        anchor.click();
    });
    img.set_onload(Some(onload.as_ref().unchecked_ref()));
    img.set_src(&data_url);
}

fn sanitize_filename(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' })
        .collect();
    let trimmed = s.trim();
    if trimmed.is_empty() { "whiteboard".to_string() } else { trimmed.to_string() }
}

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
    let mut draft = use_signal(|| None::<DraftStroke>);
    let mut is_drawing = use_signal(|| false);
    let mut text_input = use_signal(|| None::<((f64, f64), String)>);
    let mut selected_index = use_signal(|| None::<usize>);
    let mut drag_last = use_signal(|| None::<(f64, f64)>);
    let mut drag_active = use_signal(|| false);

    let auth = use_context::<Signal<AuthState>>();
    let toast_bus = use_context::<Signal<Vec<Toast>>>();
    let nav = use_navigator();
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
                flat.extend(wb.elements);
                elements.set(flat);
                if let Some(bg) = wb.background_color {
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

    let push_undo_snapshot = move || {
        let snap = elements.read().clone();
        undo_stack.write().push(snap);
    };

    let tools: Vec<(&str, &str)> = vec![
        ("pen", "Pen"),
        ("eraser", "Eraser"),
        ("rect", "Rectangle"),
        ("circle", "Circle"),
        ("line", "Line"),
        ("text", "Text"),
        ("select", "Select"),
    ];

    let canvas_cursor_class = {
        let t = tool.read().clone();
        match t.as_str() {
            "eraser" => "whiteboard-canvas tool-eraser",
            "text" => "whiteboard-canvas tool-text",
            "select" => "whiteboard-canvas tool-select",
            _ => "whiteboard-canvas",
        }
    };

    rsx! {
        Navbar {}
        div {
            class: "editor-container",
            tabindex: "0",
            onkeydown: {
                let flush_save = flush_save.clone();
                let mut schedule_save = schedule_save.clone();
                move |evt: KeyboardEvent| {
                    if evt.modifiers().contains(Modifiers::CONTROL) {
                        if evt.key() == Key::Character("s".to_string()) {
                            evt.prevent_default();
                            flush_save();
                        }
                        return;
                    }
                    match evt.key() {
                        Key::Delete | Key::Backspace => {
                            if text_input.read().is_some() { return; }
                            let sel = *selected_index.read();
                            if let Some(idx) = sel {
                                evt.prevent_default();
                                let snap = elements.read().clone();
                                if idx < snap.len() {
                                    undo_stack.write().push(snap);
                                    elements.write().remove(idx);
                                    selected_index.set(None);
                                    schedule_save();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            },
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

            {
                let mut flush_for_menu = flush_save.clone();
                let mut schedule_for_menu = schedule_save.clone();
                let id_for_menu = id.clone();
                let mut bus_for_menu = toast_bus;
                let nav_for_menu = nav;
                rsx! {
                    Menubar {
                        doc_type: "whiteboard".to_string(),
                        on_command: move |cmd: String| {
                            let mut bus = bus_for_menu;
                            match cmd.as_str() {
                                "save" => flush_for_menu(),
                                "back" => { nav_for_menu.push(Route::Dashboard {}); }
                                "undo" => {
                                    if let Some(state) = undo_stack.write().pop() {
                                        *elements.write() = state;
                                        schedule_for_menu();
                                    }
                                }
                                "toggle-theme" => {
                                    if let Some(window) = web_sys::window() {
                                        if let Some(el) = window.document().and_then(|d| d.document_element()) {
                                            let cur = el.get_attribute("data-theme").unwrap_or_default();
                                            let next = if cur == "dark" { "light" } else { "dark" };
                                            let _ = el.set_attribute("data-theme", next);
                                            if let Ok(Some(storage)) = window.local_storage() {
                                                let _ = storage.set_item("theme", next);
                                            }
                                        }
                                    }
                                }
                                "print" => {
                                    if let Some(w) = web_sys::window() { let _ = w.print(); }
                                }
                                "export-pdf" | "export-html" => {
                                    let format = match cmd.as_str() {
                                        "export-pdf" => "pdf",
                                        _ => "html",
                                    };
                                    let id = id_for_menu.clone();
                                    let token = auth.read().token.clone().unwrap_or_default();
                                    let mut bus2 = bus;
                                    let fmt_owned = format.to_string();
                                    wasm_bindgen_futures::spawn_local(async move {
                                        match export_document(&id, &fmt_owned, &token).await {
                                            Ok(_) => push_toast(bus2, ToastKind::Success, format!("Exported as {}", fmt_owned.to_uppercase())),
                                            Err(e) => push_toast(bus2, ToastKind::Error, format!("Export failed: {e}")),
                                        }
                                    });
                                }
                                other => push_toast(bus, ToastKind::Info, format!("Coming soon: {other}")),
                            }
                        },
                    }
                }
            }

            div { class: "whiteboard-toolbar",
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

                div { class: "toolbar-group",
                    button {
                        class: "tool-btn",
                        title: "Export as PNG",
                        onclick: move |_| {
                            let els = elements.read().clone();
                            let svg = build_svg_string(&els);
                            let base = sanitize_filename(&doc_title.read());
                            let filename = format!("{base}.png");
                            svg_to_png_and_download(&svg, &filename);
                            push_toast(toast_bus, ToastKind::Success, "Exported PNG".to_string());
                        },
                        "Export PNG"
                    }
                    button {
                        class: "tool-btn",
                        title: "Export as SVG",
                        onclick: move |_| {
                            let els = elements.read().clone();
                            let svg = build_svg_string(&els);
                            let base = sanitize_filename(&doc_title.read());
                            let filename = format!("{base}.svg");
                            trigger_download(&filename, "image/svg+xml", svg.as_bytes());
                            push_toast(toast_bus, ToastKind::Success, "Exported SVG".to_string());
                        },
                        "Export SVG"
                    }
                }
            }

            match doc_resource.read().as_ref() {
                None => rsx! { div { class: "editor-loading", "Loading document..." } },
                Some(Err(e)) => rsx! { div { class: "editor-error", "Failed to load: {e}" } },
                Some(Ok(_)) => rsx! {},
            }

            div { class: "whiteboard-canvas-container",
                svg {
                    id: "{SVG_ID}",
                    class: "{canvas_cursor_class}",
                    width: "100%",
                    height: "100%",
                    view_box: "0 0 1920 1080",
                    preserve_aspect_ratio: "xMidYMid meet",
                    onpointerdown: {
                        let mut schedule_save = schedule_save.clone();
                        let mut push_undo_snapshot = push_undo_snapshot;
                        move |evt: PointerEvent| {
                            let c = evt.client_coordinates();
                            let Some((x, y)) = svg_point(c.x, c.y) else { return; };
                            let t = tool.read().clone();
                            match t.as_str() {
                                "pen" | "rect" | "circle" | "line" => {
                                    is_drawing.set(true);
                                    draft.set(Some(DraftStroke {
                                        tool: t.clone(),
                                        color: color.read().clone(),
                                        stroke_width: *stroke_width.read(),
                                        points: vec![(x, y)],
                                        start: (x, y),
                                        end: (x, y),
                                    }));
                                }
                                "eraser" => {
                                    is_drawing.set(true);
                                    let current = elements.read().clone();
                                    let kept: Vec<serde_json::Value> = current.iter().filter(|e| {
                                        match element_bbox(e) {
                                            Some(b) => !bbox_contains(b, x, y),
                                            None => true,
                                        }
                                    }).cloned().collect();
                                    if kept.len() != current.len() {
                                        push_undo_snapshot();
                                        elements.set(kept);
                                        schedule_save();
                                    }
                                }
                                "text" => {
                                    text_input.set(Some(((x, y), String::new())));
                                }
                                "select" => {
                                    let hit = find_element_at(&elements.read(), x, y);
                                    selected_index.set(hit);
                                    if hit.is_some() {
                                        push_undo_snapshot();
                                        drag_last.set(Some((x, y)));
                                        drag_active.set(true);
                                    } else {
                                        drag_last.set(None);
                                        drag_active.set(false);
                                    }
                                }
                                _ => {}
                            }
                        }
                    },
                    onpointermove: move |evt: PointerEvent| {
                        let t = tool.read().clone();
                        if t == "select" {
                            if !*drag_active.read() { return; }
                            let Some(idx) = *selected_index.read() else { return; };
                            let c = evt.client_coordinates();
                            let Some((x, y)) = svg_point(c.x, c.y) else { return; };
                            let last = *drag_last.read();
                            if let Some((lx, ly)) = last {
                                let dx = x - lx;
                                let dy = y - ly;
                                if let Some(el) = elements.write().get_mut(idx) {
                                    move_element(el, dx, dy);
                                }
                                drag_last.set(Some((x, y)));
                            }
                            return;
                        }
                        if !*is_drawing.read() { return; }
                        let c = evt.client_coordinates();
                        let Some((x, y)) = svg_point(c.x, c.y) else { return; };
                        if t == "eraser" {
                            let current = elements.read().clone();
                            let kept: Vec<serde_json::Value> = current.iter().filter(|e| {
                                match element_bbox(e) {
                                    Some(b) => !bbox_contains(b, x, y),
                                    None => true,
                                }
                            }).cloned().collect();
                            if kept.len() != current.len() {
                                elements.set(kept);
                            }
                            return;
                        }
                        let mut d_opt = draft.write();
                        if let Some(d) = d_opt.as_mut() {
                            d.end = (x, y);
                            if d.tool == "pen" {
                                d.points.push((x, y));
                            }
                        }
                    },
                    onpointerup: {
                        let mut schedule_save = schedule_save.clone();
                        let mut push_undo_snapshot = push_undo_snapshot;
                        move |_evt: PointerEvent| {
                            let t = tool.read().clone();
                            if t == "select" {
                                if *drag_active.read() {
                                    drag_active.set(false);
                                    drag_last.set(None);
                                    schedule_save();
                                }
                                return;
                            }
                            if !*is_drawing.read() { return; }
                            is_drawing.set(false);
                            if t == "eraser" {
                                schedule_save();
                                return;
                            }
                            let d_opt = draft.write().take();
                            let Some(d) = d_opt else { return; };
                            let element = match d.tool.as_str() {
                                "pen" => {
                                    if d.points.len() < 2 { return; }
                                    let pts: Vec<serde_json::Value> = d.points.iter().map(|(x, y)| {
                                        serde_json::json!([x, y])
                                    }).collect();
                                    serde_json::json!({
                                        "type": "pen",
                                        "stroke": d.color,
                                        "stroke_width": d.stroke_width,
                                        "points": pts,
                                    })
                                }
                                "rect" => {
                                    let (sx, sy) = d.start;
                                    let (ex, ey) = d.end;
                                    let x = sx.min(ex);
                                    let y = sy.min(ey);
                                    let w = (ex - sx).abs();
                                    let h = (ey - sy).abs();
                                    if w < 1.0 && h < 1.0 { return; }
                                    serde_json::json!({
                                        "type": "rect",
                                        "stroke": d.color,
                                        "stroke_width": d.stroke_width,
                                        "x": x, "y": y, "width": w, "height": h,
                                    })
                                }
                                "circle" => {
                                    let (sx, sy) = d.start;
                                    let (ex, ey) = d.end;
                                    let cx = (sx + ex) / 2.0;
                                    let cy = (sy + ey) / 2.0;
                                    let rx = (ex - sx).abs() / 2.0;
                                    let ry = (ey - sy).abs() / 2.0;
                                    if rx < 1.0 && ry < 1.0 { return; }
                                    serde_json::json!({
                                        "type": "circle",
                                        "stroke": d.color,
                                        "stroke_width": d.stroke_width,
                                        "cx": cx, "cy": cy, "rx": rx, "ry": ry,
                                    })
                                }
                                "line" => {
                                    let (sx, sy) = d.start;
                                    let (ex, ey) = d.end;
                                    if (ex - sx).abs() < 1.0 && (ey - sy).abs() < 1.0 { return; }
                                    serde_json::json!({
                                        "type": "line",
                                        "stroke": d.color,
                                        "stroke_width": d.stroke_width,
                                        "x1": sx, "y1": sy, "x2": ex, "y2": ey,
                                    })
                                }
                                _ => return,
                            };
                            push_undo_snapshot();
                            elements.write().push(element);
                            schedule_save();
                        }
                    },
                    onpointerleave: move |_evt: PointerEvent| {
                        if *is_drawing.read() {
                            is_drawing.set(false);
                            draft.set(None);
                        }
                    },
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

                    for (idx, e) in elements.read().iter().enumerate() {
                        {
                            let t = e.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            let stroke = e.get("stroke").and_then(|v| v.as_str()).unwrap_or("#000").to_string();
                            let sw = e.get("stroke_width").and_then(|v| v.as_f64()).unwrap_or(2.0);
                            let key_str = format!("el-{idx}");
                            match t {
                                "pen" => {
                                    let pts: Vec<(f64, f64)> = e.get("points")
                                        .and_then(|v| v.as_array())
                                        .map(|a| a.iter().filter_map(|p| {
                                            let arr = p.as_array()?;
                                            Some((arr.get(0)?.as_f64()?, arr.get(1)?.as_f64()?))
                                        }).collect())
                                        .unwrap_or_default();
                                    let d = pen_path_d(&pts);
                                    rsx! {
                                        path {
                                            key: "{key_str}",
                                            d: "{d}",
                                            fill: "none",
                                            stroke: "{stroke}",
                                            stroke_width: "{sw}",
                                            stroke_linecap: "round",
                                            stroke_linejoin: "round",
                                        }
                                    }
                                }
                                "rect" => {
                                    let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let w = e.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let h = e.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    rsx! {
                                        rect {
                                            key: "{key_str}",
                                            x: "{x}",
                                            y: "{y}",
                                            width: "{w}",
                                            height: "{h}",
                                            fill: "none",
                                            stroke: "{stroke}",
                                            stroke_width: "{sw}",
                                        }
                                    }
                                }
                                "circle" => {
                                    let cx = e.get("cx").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let cy = e.get("cy").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let rx = e.get("rx").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let ry = e.get("ry").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    rsx! {
                                        ellipse {
                                            key: "{key_str}",
                                            cx: "{cx}",
                                            cy: "{cy}",
                                            rx: "{rx}",
                                            ry: "{ry}",
                                            fill: "none",
                                            stroke: "{stroke}",
                                            stroke_width: "{sw}",
                                        }
                                    }
                                }
                                "line" => {
                                    let x1 = e.get("x1").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let y1 = e.get("y1").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let x2 = e.get("x2").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let y2 = e.get("y2").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    rsx! {
                                        line {
                                            key: "{key_str}",
                                            x1: "{x1}",
                                            y1: "{y1}",
                                            x2: "{x2}",
                                            y2: "{y2}",
                                            stroke: "{stroke}",
                                            stroke_width: "{sw}",
                                            stroke_linecap: "round",
                                        }
                                    }
                                }
                                "text" => {
                                    let x = e.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let y = e.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let content = e.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let col = e.get("color").and_then(|v| v.as_str()).unwrap_or("#000").to_string();
                                    let size = e.get("size").and_then(|v| v.as_f64()).unwrap_or(16.0);
                                    rsx! {
                                        text {
                                            key: "{key_str}",
                                            x: "{x}",
                                            y: "{y}",
                                            fill: "{col}",
                                            font_size: "{size}",
                                            font_family: "sans-serif",
                                            "{content}"
                                        }
                                    }
                                }
                                _ => rsx! {},
                            }
                        }
                    }

                    {
                        let sel = *selected_index.read();
                        let bbox_opt = sel.and_then(|idx| elements.read().get(idx).and_then(element_bbox));
                        if let Some((x1, y1, x2, y2)) = bbox_opt {
                            let sx = x1 - 4.0;
                            let sy = y1 - 4.0;
                            let sw = (x2 - x1) + 8.0;
                            let sh = (y2 - y1) + 8.0;
                            rsx! {
                                rect {
                                    x: "{sx}",
                                    y: "{sy}",
                                    width: "{sw}",
                                    height: "{sh}",
                                    fill: "none",
                                    stroke: "#2563eb",
                                    stroke_width: "1.5",
                                    stroke_dasharray: "5,3",
                                    pointer_events: "none",
                                }
                            }
                        } else {
                            rsx! {}
                        }
                    }

                    if let Some(d) = draft.read().as_ref() {
                        {
                            let stroke = d.color.clone();
                            let sw = d.stroke_width;
                            match d.tool.as_str() {
                                "pen" => {
                                    let path_d = pen_path_d(&d.points);
                                    rsx! {
                                        path {
                                            d: "{path_d}",
                                            fill: "none",
                                            stroke: "{stroke}",
                                            stroke_width: "{sw}",
                                            stroke_linecap: "round",
                                            stroke_linejoin: "round",
                                            opacity: "0.7",
                                        }
                                    }
                                }
                                "rect" => {
                                    let (sx, sy) = d.start;
                                    let (ex, ey) = d.end;
                                    let x = sx.min(ex);
                                    let y = sy.min(ey);
                                    let w = (ex - sx).abs();
                                    let h = (ey - sy).abs();
                                    rsx! {
                                        rect {
                                            x: "{x}", y: "{y}",
                                            width: "{w}", height: "{h}",
                                            fill: "none",
                                            stroke: "{stroke}",
                                            stroke_width: "{sw}",
                                            opacity: "0.7",
                                        }
                                    }
                                }
                                "circle" => {
                                    let (sx, sy) = d.start;
                                    let (ex, ey) = d.end;
                                    let cx = (sx + ex) / 2.0;
                                    let cy = (sy + ey) / 2.0;
                                    let rx = (ex - sx).abs() / 2.0;
                                    let ry = (ey - sy).abs() / 2.0;
                                    rsx! {
                                        ellipse {
                                            cx: "{cx}", cy: "{cy}",
                                            rx: "{rx}", ry: "{ry}",
                                            fill: "none",
                                            stroke: "{stroke}",
                                            stroke_width: "{sw}",
                                            opacity: "0.7",
                                        }
                                    }
                                }
                                "line" => {
                                    let (sx, sy) = d.start;
                                    let (ex, ey) = d.end;
                                    rsx! {
                                        line {
                                            x1: "{sx}", y1: "{sy}",
                                            x2: "{ex}", y2: "{ey}",
                                            stroke: "{stroke}",
                                            stroke_width: "{sw}",
                                            opacity: "0.7",
                                            stroke_linecap: "round",
                                        }
                                    }
                                }
                                _ => rsx! {},
                            }
                        }
                    }
                }

                if let Some((pos, content)) = text_input.read().clone() {
                    {
                        let (tx, ty) = pos;
                        let screen_pos = web_sys::window()
                            .and_then(|w| w.document())
                            .and_then(|d| d.get_element_by_id(SVG_ID))
                            .map(|el| {
                                let rect = el.get_bounding_client_rect();
                                let sx = rect.left() + tx * rect.width() / VIEW_W;
                                let sy = rect.top() + ty * rect.height() / VIEW_H;
                                (sx, sy)
                            })
                            .unwrap_or((tx, ty));
                        let style = format!("left:{:.0}px; top:{:.0}px;", screen_pos.0, screen_pos.1);
                        rsx! {
                            input {
                                class: "whiteboard-text-input",
                                style: "{style}",
                                value: "{content}",
                                autofocus: true,
                                oninput: move |e: FormEvent| {
                                    let snapshot = text_input.read().clone();
                                    if let Some((p, _)) = snapshot {
                                        text_input.set(Some((p, e.value())));
                                    }
                                },
                                onkeydown: {
                                    let mut schedule_save = schedule_save.clone();
                                    let mut push_undo_snapshot = push_undo_snapshot;
                                    move |evt: KeyboardEvent| {
                                        if evt.key() == Key::Enter {
                                            evt.prevent_default();
                                            if let Some((p, c)) = text_input.read().clone() {
                                                if !c.is_empty() {
                                                    push_undo_snapshot();
                                                    elements.write().push(serde_json::json!({
                                                        "type": "text",
                                                        "x": p.0,
                                                        "y": p.1,
                                                        "content": c,
                                                        "color": color.read().clone(),
                                                        "size": 16,
                                                    }));
                                                    schedule_save();
                                                }
                                            }
                                            text_input.set(None);
                                        } else if evt.key() == Key::Escape {
                                            evt.prevent_default();
                                            text_input.set(None);
                                        }
                                    }
                                },
                                onblur: move |_| {
                                    if let Some((p, c)) = text_input.read().clone() {
                                        if !c.is_empty() {
                                            elements.write().push(serde_json::json!({
                                                "type": "text",
                                                "x": p.0,
                                                "y": p.1,
                                                "content": c,
                                                "color": color.read().clone(),
                                                "size": 16,
                                            }));
                                        }
                                    }
                                    text_input.set(None);
                                },
                            }
                        }
                    }
                }
            }

            div { class: "status-bar",
                span { "Tool: {tool}" }
                span { "Color: {color}" }
                span { "Stroke: {stroke_width:.0}px" }
                span { "Zoom: {zoom}%" }
                span { "Elements: {elements.read().len()}" }
                span { "ID: {id}" }
            }
            crate::components::ai_assist::AiAssist { doc_id: id.clone() }
        }
    }
}
