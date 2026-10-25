use super::document_editor::{EditorSession, editor_header};
use crate::state::AuthState;
use glory::prelude::*;

use serde_json::{Value, json};
const VIEW_W: f64 = 1920.0;
const VIEW_H: f64 = 1080.0;
fn canonical_to_ui(element: &serde_json::Value) -> serde_json::Value {
    let kind = element
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let projected = match kind {
        "Path" => serde_json::json!({"type":"pen","id":element.get("id"),
            "points":element.get("points").and_then(|v| v.as_array()).into_iter().flatten()
                .map(|point| serde_json::json!([point.get("x"), point.get("y")])).collect::<Vec<_>>(),
            "stroke":element.get("stroke_color"),"stroke_width":element.get("stroke_width")}),
        "Rectangle" => {
            serde_json::json!({"type":"rect","id":element.get("id"),"x":element.get("x"),"y":element.get("y"),
            "width":element.get("width"),"height":element.get("height"),"stroke":element.get("stroke_color"),"stroke_width":element.get("stroke_width")})
        }
        "Ellipse" => {
            serde_json::json!({"type":"circle","id":element.get("id"),"cx":element.get("x"),"cy":element.get("y"),
            "rx":element.get("rx"),"ry":element.get("ry"),"stroke":element.get("stroke_color"),"stroke_width":element.get("stroke_width")})
        }
        "Line" => {
            serde_json::json!({"type":"line","id":element.get("id"),"x1":element.pointer("/start/x"),"y1":element.pointer("/start/y"),
            "x2":element.pointer("/end/x"),"y2":element.pointer("/end/y"),"stroke":element.get("stroke_color"),"stroke_width":element.get("stroke_width")})
        }
        "Text" => {
            serde_json::json!({"type":"text","id":element.get("id"),"x":element.get("x"),"y":element.get("y"),
            "content":element.get("text"),"size":element.get("font_size"),"color":element.get("color")})
        }
        _ => element.clone(),
    };
    let mut result = element.clone();
    if let (Some(target), Some(projected)) = (result.as_object_mut(), projected.as_object()) {
        target.extend(
            projected
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    result
}

fn ui_to_canonical(element: &serde_json::Value) -> serde_json::Value {
    let id = element
        .get("id")
        .cloned()
        .unwrap_or_else(|| serde_json::json!(uuid::Uuid::new_v4()));
    let color = element
        .get("stroke")
        .cloned()
        .unwrap_or_else(|| serde_json::json!("#000000"));
    let width = element
        .get("stroke_width")
        .cloned()
        .unwrap_or_else(|| serde_json::json!(2.0));
    match element
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("")
    {
        "pen" => {
            serde_json::json!({"type":"Path","id":id,"points":element.get("points").and_then(|v| v.as_array()).into_iter().flatten()
            .map(|point| serde_json::json!({"x":point.get(0),"y":point.get(1),"pressure":null})).collect::<Vec<_>>(),
            "stroke_color":color,"stroke_width":width,"fill_color":null,"closed":false})
        }
        "rect" => {
            serde_json::json!({"type":"Rectangle","id":id,"x":element.get("x"),"y":element.get("y"),"width":element.get("width"),
            "height":element.get("height"),"rotation":0.0,"fill_color":null,"stroke_color":color,"stroke_width":width,"corner_radius":0.0})
        }
        "circle" => {
            serde_json::json!({"type":"Ellipse","id":id,"x":element.get("cx"),"y":element.get("cy"),"rx":element.get("rx"),
            "ry":element.get("ry"),"rotation":0.0,"fill_color":null,"stroke_color":color,"stroke_width":width})
        }
        "line" => {
            serde_json::json!({"type":"Line","id":id,"start":{"x":element.get("x1"),"y":element.get("y1"),"pressure":null},
            "end":{"x":element.get("x2"),"y":element.get("y2"),"pressure":null},"stroke_color":color,"stroke_width":width,"arrow_start":false,"arrow_end":false})
        }
        "text" => {
            serde_json::json!({"type":"Text","id":id,"x":element.get("x"),"y":element.get("y"),"text":element.get("content"),
            "font_size":element.get("size").cloned().unwrap_or_else(|| serde_json::json!(16.0)),"font_family":"Arial",
            "color":element.get("color").cloned().unwrap_or_else(|| serde_json::json!("#000000")),"bold":false,"italic":false})
        }
        _ => element.clone(),
    }
}

fn merge_whiteboard(
    base: &serde_json::Value,
    elements: &[serde_json::Value],
    background: &str,
    title: &str,
) -> serde_json::Value {
    let mut result = base.clone();
    if !result.is_object() {
        result = serde_json::json!({});
    }
    let object = result.as_object_mut().expect("object initialized above");
    object
        .entry("id")
        .or_insert_with(|| serde_json::json!(uuid::Uuid::new_v4()));
    object.insert("title".into(), serde_json::json!(title));
    object
        .entry("canvas_width")
        .or_insert_with(|| serde_json::json!(VIEW_W));
    object
        .entry("canvas_height")
        .or_insert_with(|| serde_json::json!(VIEW_H));
    object.insert("background_color".into(), serde_json::json!(background));
    let mut layers = base["layers"].as_array().cloned().unwrap_or_default();
    let mut remaining = elements.iter().map(ui_to_canonical).collect::<Vec<_>>();
    for layer in &mut layers {
        let mut retained = Vec::new();
        for original in layer["elements"].as_array().into_iter().flatten() {
            if let Some(index) = remaining
                .iter()
                .position(|element| element["id"] == original["id"])
            {
                let edited = remaining.remove(index);
                let editable_original = ui_to_canonical(&canonical_to_ui(original));
                let mut merged = original.clone();
                if let (Some(target), Some(source)) = (merged.as_object_mut(), edited.as_object()) {
                    for (key, value) in source {
                        if editable_original.get(key) != Some(value) {
                            target.insert(key.clone(), value.clone());
                        }
                    }
                }
                retained.push(merged);
            }
        }
        layer["elements"] = serde_json::Value::Array(retained);
    }
    if layers.is_empty() {
        layers.push(serde_json::json!({"id":uuid::Uuid::new_v4(),"name":"Layer 1","visible":true,"locked":false,"opacity":1.0,"elements":[]}));
    }
    if !remaining.is_empty() {
        let destination = layers.iter().position(|layer| layer["locked"] != true && layer["visible"] != false).unwrap_or_else(|| {
            layers.push(serde_json::json!({"id":uuid::Uuid::new_v4(),"name":"Layer 2","visible":true,"locked":false,"opacity":1.0,"elements":[]}));
            layers.len() - 1
        });
        if let Some(first) = layers[destination]["elements"].as_array_mut() {
            first.extend(remaining);
        }
    }
    object.insert("layers".into(), serde_json::Value::Array(layers));
    object.remove("elements");
    result
}

#[cfg(test)]
fn whiteboard_replace_from_operation(
    payload: &serde_json::Value,
) -> Option<(String, serde_json::Value)> {
    let user = payload.get("user_id")?.as_str()?.to_string();
    let operation = payload.get("op_type")?;
    if operation.get("type")?.as_str()? != "Replace" {
        return None;
    }
    let path = operation.get("path")?.as_array()?;
    if path.len() != 1 || path[0].as_str()? != "whiteboard" {
        return None;
    }
    Some((user, operation.get("new_value")?.clone()))
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
    let el = doc.query_selector(".whiteboard-canvas").ok().flatten()?;
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
                if let Some(arr) = p.as_array()
                    && arr.len() == 2
                {
                    let x = arr[0].as_f64().unwrap_or(0.0);
                    let y = arr[1].as_f64().unwrap_or(0.0);
                    if x < min_x {
                        min_x = x;
                    }
                    if y < min_y {
                        min_y = y;
                    }
                    if x > max_x {
                        max_x = x;
                    }
                    if y > max_y {
                        max_y = y;
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
            let len = e
                .get("content")
                .and_then(|v| v.as_str())
                .map(|c| c.chars().count())
                .unwrap_or(1) as f64;
            Some((x, y - s, x + s * 0.6 * len, y + 2.0))
        }
        "Image" => {
            let x = e["x"].as_f64()?;
            let y = e["y"].as_f64()?;
            Some((x, y, x + e["width"].as_f64()?, y + e["height"].as_f64()?))
        }
        _ => None,
    }
}

fn bbox_contains(b: (f64, f64, f64, f64), x: f64, y: f64) -> bool {
    x >= b.0 && x <= b.2 && y >= b.1 && y <= b.3
}

fn move_element(el: &mut serde_json::Value, dx: f64, dy: f64) {
    let ty = el
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let obj = match el.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    fn shift(v: &mut serde_json::Value, d: f64) {
        if let Some(n) = v.as_f64() {
            *v = serde_json::json!(n + d);
        }
    }
    match ty.as_str() {
        "rect" | "text" | "Image" => {
            if let Some(x) = obj.get_mut("x") {
                shift(x, dx);
            }
            if let Some(y) = obj.get_mut("y") {
                shift(y, dy);
            }
        }
        "circle" => {
            if let Some(x) = obj.get_mut("cx") {
                shift(x, dx);
            }
            if let Some(y) = obj.get_mut("cy") {
                shift(y, dy);
            }
        }
        "line" => {
            if let Some(x) = obj.get_mut("x1") {
                shift(x, dx);
            }
            if let Some(y) = obj.get_mut("y1") {
                shift(y, dy);
            }
            if let Some(x) = obj.get_mut("x2") {
                shift(x, dx);
            }
            if let Some(y) = obj.get_mut("y2") {
                shift(y, dy);
            }
        }
        "pen" => {
            if let Some(points) = obj.get_mut("points").and_then(|p| p.as_array_mut()) {
                for p in points {
                    if let Some(arr) = p.as_array_mut() {
                        if let Some(x) = arr.get_mut(0) {
                            shift(x, dx);
                        }
                        if let Some(y) = arr.get_mut(1) {
                            shift(y, dy);
                        }
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

fn build_svg_string(elements: &[serde_json::Value], background_color: &str) -> String {
    let background_color = xml_escape(background_color);
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1920 1080\" width=\"1920\" height=\"1080\">",
    );
    s.push_str(&format!(
        r#"<rect width="100%" height="100%" fill="{background_color}"/>"#
    ));
    for el in elements {
        let ty = el.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let stroke = xml_escape(
            el.get("stroke")
                .and_then(|v| v.as_str())
                .unwrap_or("#000000"),
        );
        let sw = el
            .get("stroke_width")
            .and_then(|v| v.as_f64())
            .unwrap_or(2.0);
        let fill = xml_escape(el["fill_color"].as_str().unwrap_or("none"));
        match ty {
            "pen" => {
                let points = el
                    .get("points")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                if points.is_empty() {
                    continue;
                }
                s.push_str(&format!(
                    r#"<path fill="{fill}" stroke="{stroke}" stroke-width="{sw}" stroke-linecap="round" stroke-linejoin="round" d=""#
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
                    r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{fill}" stroke="{stroke}" stroke-width="{sw}"/>"#
                ));
            }
            "circle" => {
                let cx = el.get("cx").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let cy = el.get("cy").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let rx = el.get("rx").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let ry = el.get("ry").and_then(|v| v.as_f64()).unwrap_or(0.0);
                s.push_str(&format!(
                    r#"<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}" fill="{fill}" stroke="{stroke}" stroke-width="{sw}"/>"#
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
                let col = xml_escape(
                    el.get("color")
                        .and_then(|v| v.as_str())
                        .unwrap_or("#000000"),
                );
                let size = el.get("size").and_then(|v| v.as_f64()).unwrap_or(16.0);
                s.push_str(&format!(
                    r#"<text x="{x}" y="{y}" fill="{col}" font-size="{size}" font-family="sans-serif">{}</text>"#,
                    xml_escape(content)
                ));
            }
            "Image" => {
                let x = el["x"].as_f64().unwrap_or(0.0);
                let y = el["y"].as_f64().unwrap_or(0.0);
                let width = el["width"].as_f64().unwrap_or(100.0);
                let height = el["height"].as_f64().unwrap_or(100.0);
                let url = xml_escape(el["url"].as_str().unwrap_or_default());
                s.push_str(&format!(
                    r#"<image x="{x}" y="{y}" width="{width}" height="{height}" href="{url}"/>"#
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
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let array = js_sys::Uint8Array::from(data);
    let parts = js_sys::Array::new();
    parts.push(&array.buffer());
    let blob_props = web_sys::BlobPropertyBag::new();
    blob_props.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_buffer_source_sequence_and_options(&parts, &blob_props)
    else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
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
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Ok(img_el) = document.create_element("img") else {
        return;
    };
    let Ok(img) = img_el.dyn_into::<HtmlImageElement>() else {
        return;
    };
    let Ok(canvas_el) = document.create_element("canvas") else {
        return;
    };
    let Ok(canvas) = canvas_el.dyn_into::<HtmlCanvasElement>() else {
        return;
    };
    canvas.set_width(1920);
    canvas.set_height(1080);
    let svg_b64 = base64::engine::general_purpose::STANDARD.encode(svg_str.as_bytes());
    let data_url = format!("data:image/svg+xml;base64,{svg_b64}");
    let filename = filename.to_string();
    let canvas_for_load = canvas.clone();
    let img_for_load = img.clone();
    let onload = Closure::once_into_js(move || {
        let Ok(Some(ctx_raw)) = canvas_for_load.get_context("2d") else {
            return;
        };
        let Ok(ctx) = ctx_raw.dyn_into::<CanvasRenderingContext2d>() else {
            return;
        };
        let _ = ctx.draw_image_with_html_image_element(&img_for_load, 0.0, 0.0);
        let Ok(url) = canvas_for_load.to_data_url_with_type("image/png") else {
            return;
        };
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        let Ok(anchor_el) = document.create_element("a") else {
            return;
        };
        let Ok(anchor) = anchor_el.dyn_into::<web_sys::HtmlAnchorElement>() else {
            return;
        };
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
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = s.trim();
    if trimmed.is_empty() {
        "whiteboard".to_string()
    } else {
        trimmed.to_string()
    }
}
fn board_background_svg(background: &str) -> String {
    let svg = build_svg_string(&[], background);
    let base = svg.strip_suffix("</svg>").unwrap_or(&svg);
    format!(
        r##"{base}<defs><pattern id="board-grid" width="20" height="20" patternUnits="userSpaceOnUse"><path d="M 20 0 L 0 0 0 20" fill="none" stroke="#e0e0e0" stroke-width="0.5"/></pattern></defs><rect width="100%" height="100%" fill="url(#board-grid)"/></svg>"##
    )
}

fn board_elements(content: &Value) -> Vec<Value> {
    let mut elements = content["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|layer| layer["elements"].as_array().into_iter().flatten())
        .map(canonical_to_ui)
        .collect::<Vec<_>>();
    elements.extend(
        content["elements"]
            .as_array()
            .into_iter()
            .flatten()
            .map(canonical_to_ui),
    );
    elements
}
fn board_element_editable(content: &Value, index: usize) -> bool {
    let mut offset = 0;
    for layer in content["layers"].as_array().into_iter().flatten() {
        let count = layer["elements"].as_array().map_or(0, Vec::len);
        if index < offset + count {
            return layer["visible"] != false && layer["locked"] != true;
        }
        offset += count;
    }
    true
}
fn board_element_visible(content: &Value, index: usize) -> bool {
    let mut offset = 0;
    for layer in content["layers"].as_array().into_iter().flatten() {
        let count = layer["elements"].as_array().map_or(0, Vec::len);
        if index < offset + count {
            return layer["visible"] != false;
        }
        offset += count;
    }
    true
}
fn board_object_source(element: &Value) -> String {
    use base64::Engine;
    format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(build_svg_string(
            std::slice::from_ref(element),
            "transparent"
        ))
    )
}
fn board_visible_elements(content: &Value) -> Vec<Value> {
    let mut elements = content["layers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|layer| layer["visible"] != false)
        .flat_map(|layer| layer["elements"].as_array().into_iter().flatten())
        .map(canonical_to_ui)
        .collect::<Vec<_>>();
    elements.extend(
        content["elements"]
            .as_array()
            .into_iter()
            .flatten()
            .map(canonical_to_ui),
    );
    elements
}
fn board_hit(content: &Value, x: f64, y: f64) -> Option<usize> {
    board_elements(content)
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, element)| {
            (board_element_editable(content, index)
                && element_bbox(element).is_some_and(|bounds| bbox_contains(bounds, x, y)))
            .then_some(index)
        })
}
fn board_background(content: &Value) -> String {
    content["background_color"]
        .as_str()
        .unwrap_or("#ffffff")
        .into()
}
fn save_elements(session: &EditorSession, elements: &[Value]) {
    session.edit(merge_whiteboard(
        &session.content.get(),
        elements,
        &board_background(&session.content.get()),
        &session.name.get(),
    ));
}
fn draft_element(draft: &DraftStroke, text: &str) -> Value {
    let (x, y) = (
        draft.start.0.min(draft.end.0),
        draft.start.1.min(draft.end.1),
    );
    let (w, h) = (
        (draft.end.0 - draft.start.0).abs().max(1.),
        (draft.end.1 - draft.start.1).abs().max(1.),
    );
    let mut element = match draft.tool.as_str() {
        "pen" => json!({"type":"pen","points":draft.points}),
        "rect" => json!({"type":"rect","x":x,"y":y,"width":w,"height":h}),
        "circle" => json!({"type":"circle","cx":x+w/2.,"cy":y+h/2.,"rx":w/2.,"ry":h/2.}),
        "line" => {
            json!({"type":"line","x1":draft.start.0,"y1":draft.start.1,"x2":draft.end.0,"y2":draft.end.1})
        }
        "text" => {
            json!({"type":"text","x":draft.start.0,"y":draft.start.1,"content":text,"size":28.,"color":draft.color})
        }
        _ => Value::Null,
    };
    if element.is_object() {
        element["id"] = json!(uuid::Uuid::new_v4());
        element["stroke"] = json!(draft.color);
        element["stroke_width"] = json!(draft.stroke_width);
    }
    element
}

#[allow(non_snake_case)]
pub fn Whiteboard(_scope: Scope, auth: Cage<AuthState>, id: String) -> impl Widget {
    widget(move |scope| {
        let session = EditorSession::new(scope, auth, id.clone(), "whiteboard");
        let content = session.content;
        let tool = scope.cage(Some("pen".to_string()));
        let color = scope.cage("#2563eb".to_string());
        let width = scope.cage("3".to_string());
        let label = scope.cage("Text".to_string());
        let selected = scope.cage(None::<usize>);
        let draft = scope.cage(None::<DraftStroke>);
        let drag_origin = scope.cage(None::<(f64, f64)>);
        let dragging = scope.cage(None::<Vec<Value>>);
        let zoom = scope.cage(100u32);
        let comments = scope.cage(false);
        let versions = scope.cage(false);
        let properties = scope.cage(false);
        let background = content.map(board_background);
        let bg_edit = session.clone();
        let preview = content.map(|value| {
            use base64::Engine;
            format!(
                "data:image/svg+xml;base64,{}",
                base64::engine::general_purpose::STANDARD
                    .encode(board_background_svg(&board_background(value)))
            )
        });
        let render_objects = scope.bond(move || {
            let value = content.get();
            let elements = if let Some(moved) = dragging.get() {
                moved
                    .into_iter()
                    .enumerate()
                    .filter(|(index, _)| board_element_visible(&value, *index))
                    .map(|(_, element)| element)
                    .collect()
            } else {
                board_visible_elements(&value)
            };
            let mut items = elements
                .into_iter()
                .enumerate()
                .map(|(index, element)| {
                    (
                        element["id"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("object-{index}")),
                        element,
                    )
                })
                .collect::<Vec<_>>();
            if let Some(stroke) = draft.get() {
                let element = draft_element(&stroke, &label.get());
                if !element.is_null() {
                    items.push(("drawing-draft".into(), element));
                }
            }
            items
        });
        let gesture = session.clone();
        let canvas = view()
            .class(scope.bond(move || {
                format!(
                    "whiteboard-canvas tool-{}",
                    tool.get().unwrap_or_else(|| "pen".into())
                )
            }))
            .name("Whiteboard drawing canvas")
            .style(scope.bond(move || {
                Style::new()
                    .position(Position::Relative)
                    .overflow_x(Overflow::Hidden)
                    .overflow_y(Overflow::Hidden)
                    .width(Length::Percent(zoom.get() as f64))
                    .height(Length::Percent(zoom.get() as f64))
            }))
            .semantics(Some(Box::new(glory::render::Semantics {
                drag_capture: Some(glory::render::DragCapture::Press),
                ..Default::default()
            })))
            .on(glory::render::EventKind::Drag, move |event| {
                let glory::render::Event::Drag { phase, x, y, .. } = event else {
                    return;
                };
                let Some(point) = svg_point(x, y) else { return };
                let mode = tool.get().unwrap_or_else(|| "pen".into());
                use glory::render::DragPhase;
                match phase {
                    DragPhase::Start => {
                        let elements = board_elements(&gesture.content.get());
                        if mode == "select" || mode == "eraser" {
                            let index = board_hit(&gesture.content.get(), point.0, point.1);
                            selected.revise(|value| *value = index);
                            if mode == "eraser" {
                                if let Some(index) = index {
                                    let mut elements = elements;
                                    elements.remove(index);
                                    save_elements(&gesture, &elements);
                                }
                                return;
                            }
                            drag_origin.revise(|value| *value = Some(point));
                            dragging.revise(|value| *value = Some(elements));
                        } else {
                            draft.revise(|value| {
                                *value = Some(DraftStroke {
                                    tool: mode,
                                    color: color.get(),
                                    stroke_width: width
                                        .get()
                                        .parse::<f64>()
                                        .unwrap_or(3.)
                                        .clamp(0.5, 100.),
                                    points: vec![point],
                                    start: point,
                                    end: point,
                                })
                            });
                        }
                    }
                    DragPhase::Move => {
                        if mode == "select" {
                            if let (Some(index), Some(origin)) = (selected.get(), drag_origin.get())
                            {
                                let mut elements = board_elements(&gesture.content.get());
                                if let Some(element) = elements.get_mut(index) {
                                    move_element(element, point.0 - origin.0, point.1 - origin.1);
                                }
                                dragging.revise(|value| *value = Some(elements));
                            }
                        } else {
                            draft.revise(|value| {
                                if let Some(draft) = value {
                                    draft.end = point;
                                    draft.points.push(point);
                                }
                            });
                        }
                    }
                    DragPhase::Drop => {
                        if mode == "select" {
                            if let Some(elements) = dragging.get() {
                                save_elements(&gesture, &elements);
                            }
                            dragging.revise(|value| *value = None);
                            drag_origin.revise(|value| *value = None);
                        } else if let Some(mut stroke) = draft.get() {
                            stroke.end = point;
                            stroke.points.push(point);
                            let mut elements = board_elements(&gesture.content.get());
                            let element = draft_element(&stroke, &label.get());
                            if !element.is_null() {
                                elements.push(element);
                                save_elements(&gesture, &elements);
                            }
                            draft.revise(|value| *value = None);
                        }
                    }
                    DragPhase::Cancel => {
                        draft.revise(|value| *value = None);
                        dragging.revise(|value| *value = None);
                        drag_origin.revise(|value| *value = None);
                    }
                }
            })
            .fill((
                image()
                    .source(preview)
                    .alt("Drawing preview")
                    .style(Style::new().width(Length::Percent(100.))),
                super::document_editor::diagram_objects(
                    render_objects,
                    board_object_source,
                    VIEW_W,
                    VIEW_H,
                ),
            ));
        let remove = session.clone();
        let duplicate = session.clone();
        let clear = session.clone();
        let svg = session.clone();
        let png = session.clone();
        let property_session = session.clone();
        let menu_commands: std::rc::Rc<dyn Fn(&str)> =
            std::rc::Rc::new(move |command| match command {
                "comments" => comments.revise(|value| *value = !*value),
                "versions" => versions.revise(|value| *value = !*value),
                "objects" => properties.revise(|value| *value = !*value),
                "zoom-in" => zoom.revise(|value| *value = (*value + 25).min(300)),
                "zoom-out" => zoom.revise(|value| *value = value.saturating_sub(25).max(25)),
                command if command.starts_with("tool-") => {
                    tool.revise(|value| *value = Some(command[5..].to_string()))
                }
                _ => {}
            });
        col()
            .class("whiteboard-app editor-shell")
            .fill((
                editor_header(&session, "Whiteboard", &["pdf", "html"], menu_commands),
                row()
                    .class("whiteboard-toolbar editor-toolbar board-tools-strip")
                    .fill((
                        row().class("drawing-toolbar").fill((
                            row().class("toolbar-group whiteboard-tools").fill(
                                [
                                    ("pen", "Pen", "✎"),
                                    ("select", "Select / move", "↖"),
                                    ("rect", "Rectangle", "□"),
                                    ("circle", "Ellipse", "○"),
                                    ("line", "Line", "╱"),
                                    ("text", "Text", "T"),
                                    ("eraser", "Eraser", "⌫"),
                                ]
                                .into_iter()
                                .map(|(key, label, icon)| {
                                    sapid::button()
                                        .theme(crate::components::ui::theme())
                                        .name(label)
                                        .label(icon)
                                        .class(scope.bond(move || {
                                            if tool.get().as_deref() == Some(key) {
                                                "tool-btn active".into()
                                            } else {
                                                "tool-btn".into()
                                            }
                                        }))
                                        .on_press(move || {
                                            tool.revise(|value| *value = Some(key.into()))
                                        })
                                })
                                .collect::<Vec<_>>(),
                            ),
                            sapid::native_select("whiteboard-tool")
                                .class("whiteboard-tool-select")
                                .theme(crate::components::ui::theme())
                                .name("Drawing tool")
                                .value(tool)
                                .options(
                                    [
                                        ("pen", "Pen"),
                                        ("select", "Select / move"),
                                        ("rect", "Rectangle"),
                                        ("circle", "Ellipse"),
                                        ("line", "Line"),
                                        ("text", "Text"),
                                        ("eraser", "Eraser"),
                                    ]
                                    .map(|(key, label)| sapid::select_option(key, label)),
                                )
                                .on_value_change(move |value| {
                                    tool.revise(|current| *current = value)
                                }),
                            row().class("toolbar-group").fill((
                                text().class("toolbar-label").content("Color"),
                                view()
                                    .class("whiteboard-color-swatch")
                                    .style(color.map(|value| {
                                        let mut style = Style::new();
                                        style.background = sapid::parse_hex_color(value).ok();
                                        style
                                    }))
                                    .fill(
                                        sapid::color_input("board-stroke")
                                            .show_alpha(false)
                                            .class("whiteboard-color-input")
                                            .theme(crate::components::ui::theme())
                                            .name("Stroke color")
                                            .value(color)
                                            .on_value_change(move |value| {
                                                color.revise(|current| *current = value)
                                            }),
                                    ),
                            )),
                            row().class("toolbar-group").fill((
                                text().class("toolbar-label").content("Size"),
                                sapid::text_field()
                                    .theme(crate::components::ui::theme())
                                    .name("Stroke width")
                                    .value(width)
                                    .on_value_change(move |value| {
                                        width.revise(|current| *current = value)
                                    }),
                            )),
                            when(
                                tool.map(|value| value.as_deref() == Some("text")),
                                move || {
                                    sapid::text_field()
                                        .theme(crate::components::ui::theme())
                                        .name("Text to draw")
                                        .value(label)
                                        .on_value_change(move |value| {
                                            label.revise(|current| *current = value)
                                        })
                                },
                            ),
                            row().class("toolbar-group").fill((
                                text().class("toolbar-label").content("Background"),
                                view()
                                    .class("whiteboard-color-swatch")
                                    .style(scope.bond(move || {
                                        let mut style = Style::new();
                                        style.background =
                                            sapid::parse_hex_color(&background.get()).ok();
                                        style
                                    }))
                                    .fill(
                                        sapid::color_input("board-background")
                                            .show_alpha(false)
                                            .class("whiteboard-color-input")
                                            .theme(crate::components::ui::theme())
                                            .name("Background color")
                                            .value(background)
                                            .on_commit(move |value| {
                                                let next = merge_whiteboard(
                                                    &bg_edit.content.get(),
                                                    &board_elements(&bg_edit.content.get()),
                                                    &value,
                                                    &bg_edit.name.get(),
                                                );
                                                bg_edit.edit(next);
                                            }),
                                    ),
                            )),
                        )),
                        row().class("whiteboard-edit-toolbar").fill((
                            sapid::button()
                                .theme(crate::components::ui::theme())
                                .label("Delete selected")
                                .on_press(move || {
                                    if let Some(index) = selected.get() {
                                        if !board_element_editable(&remove.content.get(), index) {
                                            return;
                                        }
                                        let mut elements = board_elements(&remove.content.get());
                                        if index < elements.len() {
                                            elements.remove(index);
                                            save_elements(&remove, &elements);
                                            selected.revise(|value| *value = None);
                                        }
                                    }
                                }),
                            sapid::button()
                                .theme(crate::components::ui::theme())
                                .label("Duplicate selected")
                                .on_press(move || {
                                    if let Some(index) = selected.get() {
                                        if !board_element_editable(&duplicate.content.get(), index)
                                        {
                                            return;
                                        }
                                        let mut elements = board_elements(&duplicate.content.get());
                                        if let Some(mut value) = elements.get(index).cloned() {
                                            value["id"] = json!(uuid::Uuid::new_v4());
                                            move_element(&mut value, 20., 20.);
                                            elements.push(value);
                                            save_elements(&duplicate, &elements);
                                        }
                                    }
                                }),
                            sapid::button()
                                .theme(crate::components::ui::theme())
                                .label("Clear board")
                                .on_press(move || {
                                    let content = clear.content.get();
                                    let retained = board_elements(&content)
                                        .into_iter()
                                        .enumerate()
                                        .filter(|(index, _)| {
                                            !board_element_editable(&content, *index)
                                        })
                                        .map(|(_, element)| element)
                                        .collect::<Vec<_>>();
                                    save_elements(&clear, &retained);
                                }),
                            sapid::button()
                                .theme(crate::components::ui::theme())
                                .label("SVG")
                                .on_press(move || {
                                    let content = svg.content.get();
                                    trigger_download(
                                        &format!("{}.svg", sanitize_filename(&svg.name.get())),
                                        "image/svg+xml",
                                        build_svg_string(
                                            &board_elements(&content),
                                            &board_background(&content),
                                        )
                                        .as_bytes(),
                                    );
                                }),
                            sapid::button()
                                .theme(crate::components::ui::theme())
                                .label("PNG")
                                .on_press(move || {
                                    let content = png.content.get();
                                    svg_to_png_and_download(
                                        &build_svg_string(
                                            &board_elements(&content),
                                            &board_background(&content),
                                        ),
                                        &format!("{}.png", sanitize_filename(&png.name.get())),
                                    );
                                }),
                            sapid::button()
                                .theme(crate::components::ui::theme())
                                .label("−")
                                .on_press(move || {
                                    zoom.revise(|value| *value = value.saturating_sub(25).max(25))
                                }),
                            text().content(zoom.map(|value| format!("{value}%"))),
                            sapid::button()
                                .theme(crate::components::ui::theme())
                                .label("+")
                                .on_press(move || {
                                    zoom.revise(|value| *value = (*value + 25).min(300))
                                }),
                        )),
                    )),
                row().class("editor-main whiteboard-main").fill((
                    view()
                        .class("whiteboard-canvas-container whiteboard-viewport")
                        .style(
                            Style::new()
                                .overflow_x(Overflow::Auto)
                                .overflow_y(Overflow::Auto),
                        )
                        .fill(canvas),
                    when(properties, move || {
                        board_properties(property_session.clone(), selected)
                    }),
                    crate::components::comments_pane::CommentsPane(
                        scope.clone(),
                        auth,
                        id.clone(),
                        comments,
                    ),
                )),
                text().class("status-bar whiteboard-status").content(
                    content.map(|value| format!("{} objects", board_elements(value).len())),
                ),
                crate::components::version_history::VersionHistory(
                    scope.clone(),
                    auth,
                    id.clone(),
                    versions,
                ),
                crate::components::ai_assist::AiAssist(scope.clone(), auth, id.clone()),
            ))
            .show_in(scope)
    })
}
fn board_properties(session: EditorSession, selected: Cage<Option<usize>>) -> impl Widget {
    widget(move |scope| {
        let content = session.content;
        let items = content.map(|value| {
            board_elements(value)
                .into_iter()
                .enumerate()
                .map(|(index, element)| {
                    (
                        index,
                        element["type"].as_str().unwrap_or("object").to_owned(),
                    )
                })
                .collect::<Vec<_>>()
        });
        let dx = scope.cage("10".to_string());
        let dy = scope.cage("10".to_string());
        let move_session = session.clone();
        col()
            .class("whiteboard-properties")
            .fill((
                sapid::title(2)
                    .theme(crate::components::ui::theme())
                    .content("Objects & layers"),
                each(
                    items,
                    |item| item.0,
                    move |item, _| {
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .label(item.map(|item| format!("{}. {}", item.0 + 1, item.1)))
                            .on_press(move || selected.revise(|value| *value = Some(item.get().0)))
                    },
                ),
                row().fill((
                    sapid::text_field()
                        .theme(crate::components::ui::theme())
                        .name("Move X")
                        .value(dx)
                        .on_value_change(move |value| dx.revise(|current| *current = value)),
                    sapid::text_field()
                        .theme(crate::components::ui::theme())
                        .name("Move Y")
                        .value(dy)
                        .on_value_change(move |value| dy.revise(|current| *current = value)),
                    sapid::button()
                        .theme(crate::components::ui::theme())
                        .label("Move selected")
                        .on_press(move || {
                            if let Some(index) = selected.get() {
                                if !board_element_editable(&move_session.content.get(), index) {
                                    return;
                                }
                                let mut elements = board_elements(&move_session.content.get());
                                if let Some(element) = elements.get_mut(index) {
                                    move_element(
                                        element,
                                        dx.get().parse().unwrap_or(0.),
                                        dy.get().parse().unwrap_or(0.),
                                    );
                                    save_elements(&move_session, &elements);
                                }
                            }
                        }),
                )),
                each(
                    content.map(|value| {
                        value["layers"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .enumerate()
                            .map(|(index, layer)| (index, layer.clone()))
                            .collect::<Vec<_>>()
                    }),
                    |item| item.0,
                    {
                        let session = session.clone();
                        move |item, _| {
                            let rename = session.clone();
                            let visible = session.clone();
                            let locked = session.clone();
                            row().fill((
                                sapid::text_field()
                                    .theme(crate::components::ui::theme())
                                    .name("Layer name")
                                    .value(item.map(|item| {
                                        item.1["name"].as_str().unwrap_or_default().to_owned()
                                    }))
                                    .on_commit(move |name| {
                                        let mut value = rename.content.get();
                                        value["layers"][item.get().0]["name"] = json!(name);
                                        rename.edit(value);
                                    }),
                                sapid::checkbox()
                                    .theme(crate::components::ui::theme())
                                    .label("Visible")
                                    .checked(
                                        item.map(|item| {
                                            item.1["visible"].as_bool().unwrap_or(true)
                                        }),
                                    )
                                    .on_checked_change(move |checked| {
                                        let mut value = visible.content.get();
                                        value["layers"][item.get().0]["visible"] = json!(checked);
                                        visible.edit(value);
                                    }),
                                sapid::checkbox()
                                    .theme(crate::components::ui::theme())
                                    .label("Locked")
                                    .checked(
                                        item.map(|item| {
                                            item.1["locked"].as_bool().unwrap_or(false)
                                        }),
                                    )
                                    .on_checked_change(move |checked| {
                                        let mut value = locked.content.get();
                                        value["layers"][item.get().0]["locked"] = json!(checked);
                                        locked.edit(value);
                                    }),
                            ))
                        }
                    },
                ),
            ))
            .show_in(scope)
    })
}

#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn ui_shapes_are_saved_as_native_whiteboard_elements() {
        let ui = serde_json::json!({"type":"rect","x":10.0,"y":20.0,"width":100.0,"height":40.0,"stroke":"#123456","stroke_width":2.0});
        let content = merge_whiteboard(&serde_json::json!({}), &[ui], "#ffffff", "Board");
        let native = content.pointer("/layers/0/elements/0").unwrap();
        assert_eq!(
            native.get("type").and_then(|v| v.as_str()),
            Some("Rectangle")
        );
        assert_eq!(
            canonical_to_ui(native).get("type").and_then(|v| v.as_str()),
            Some("rect")
        );
        serde_json::from_value::<contrix_doc_engine::Whiteboard>(content).unwrap();
    }

    #[test]
    fn collaboration_replacement_is_scoped_to_whiteboards() {
        let payload = serde_json::json!({"user_id":"peer","op_type":{"type":"Replace","path":["whiteboard"],"new_value":{"layers":[]}}});
        assert_eq!(
            whiteboard_replace_from_operation(&payload).map(|v| v.0),
            Some("peer".into())
        );
    }

    #[test]
    fn saving_preserves_native_layers_and_unedited_shape_properties() {
        let ui = serde_json::json!({"type":"rect","id":uuid::Uuid::new_v4(),"x":10.0,"y":20.0,"width":100.0,"height":40.0,"stroke":"#123456","stroke_width":2.0});
        let mut base = merge_whiteboard(&serde_json::json!({}), &[ui], "#ffffff", "Board");
        base["layers"][0]["name"] = serde_json::json!("Diagram");
        base["layers"][0]["elements"][0]["rotation"] = serde_json::json!(35.0);
        base["layers"][0]["elements"][0]["fill_color"] = serde_json::json!("#abcdef");
        let mut ui = canonical_to_ui(&base["layers"][0]["elements"][0]);
        ui["x"] = serde_json::json!(25.0);
        let saved = merge_whiteboard(&base, &[ui], "#ffffff", "Board");
        assert_eq!(saved["layers"][0]["name"], "Diagram");
        assert_eq!(saved["layers"][0]["elements"][0]["rotation"], 35.0);
        assert_eq!(saved["layers"][0]["elements"][0]["fill_color"], "#abcdef");
        assert_eq!(saved["layers"][0]["elements"][0]["x"], 25.0);
    }
}
