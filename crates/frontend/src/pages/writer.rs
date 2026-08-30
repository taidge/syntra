use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_timers::callback::Timeout;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::WebSocket;

use crate::Route;
use crate::collab_client::{
    CollabHandle, CollabMessage, ConnectionState, RemoteCursor, connect_with_reconnect,
};
use crate::components::comments_pane::CommentsPane;
use crate::components::context_menu::{ContextMenu, MenuEntry};
use crate::components::find_dialog::FindDialog;
use crate::components::menubar::Menubar;
use crate::components::navbar::Navbar;
use crate::components::sidebar::Sidebar;
use crate::components::toast::{Toast, ToastKind, push_toast};
use crate::components::version_history::VersionHistory;
use crate::state::{AuthState, api_get, api_put, export_document};

fn window_find(query: &str, backward: bool) -> bool {
    if query.is_empty() {
        return false;
    }
    let window = match web_sys::window() {
        Some(w) => w,
        None => return false,
    };
    let func = match js_sys::Reflect::get(&window, &"find".into()) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let func: js_sys::Function = match func.dyn_into() {
        Ok(f) => f,
        Err(_) => return false,
    };
    let args = js_sys::Array::new();
    args.push(&query.into());
    args.push(&wasm_bindgen::JsValue::FALSE);
    args.push(&wasm_bindgen::JsValue::from_bool(backward));
    args.push(&wasm_bindgen::JsValue::TRUE);
    let this: wasm_bindgen::JsValue = window.into();
    match func.apply(&this, &args) {
        Ok(v) => v.as_bool().unwrap_or(false),
        Err(_) => false,
    }
}

#[derive(Deserialize)]
struct CursorPosLite {
    user_id: serde_json::Value,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    position: Option<serde_json::Value>,
}

fn cursor_user_id(v: &serde_json::Value) -> Option<String> {
    v.as_str().map(|s| s.to_string())
}

fn cursor_offset(v: &serde_json::Value) -> Option<u32> {
    if let Some(n) = v.as_u64() {
        return Some(n as u32);
    }
    if let Some(obj) = v.as_object()
        && let Some(o) = obj.get("offset").and_then(|x| x.as_u64())
    {
        return Some(o as u32);
    }
    None
}

fn caret_offset_in_body() -> Option<u32> {
    let window = web_sys::window()?;
    let document = window.document()?;
    let sel = document.get_selection().ok().flatten()?;
    if sel.range_count() == 0 {
        return None;
    }
    let body = document.get_element_by_id(WRITER_BODY_ID)?;
    let range = sel.get_range_at(0).ok()?;
    let pre = document.create_range().ok()?;
    pre.select_node_contents(&body).ok()?;
    let end_container = range.end_container().ok()?;
    let end_offset = range.end_offset().ok()?;
    pre.set_end(&end_container, end_offset).ok()?;
    let s = pre.to_string().as_string().unwrap_or_default();
    Some(s.chars().count() as u32)
}

fn char_offset_to_pixel(offset: u32) -> Option<(f64, f64)> {
    let window = web_sys::window()?;
    let document = window.document()?;
    let body_el = document.get_element_by_id(WRITER_BODY_ID)?;
    let body_rect = body_el.get_bounding_client_rect();
    let body_node: &web_sys::Node = body_el.as_ref();
    let walker = document
        .create_tree_walker_with_what_to_show(body_node, 0x4)
        .ok()?;
    let mut remaining = offset as usize;
    let mut last_text: Option<web_sys::Node> = None;
    let mut last_len: usize = 0;
    loop {
        let node = walker.next_node().ok().flatten();
        match node {
            None => break,
            Some(n) => {
                let text = n.text_content().unwrap_or_default();
                let len = text.chars().count();
                last_text = Some(n.clone());
                last_len = len;
                if remaining <= len {
                    let range = document.create_range().ok()?;
                    let byte_off = text
                        .char_indices()
                        .nth(remaining)
                        .map(|(i, _)| i)
                        .unwrap_or(text.len()) as u32;
                    range.set_start(&n, byte_off).ok()?;
                    range.set_end(&n, byte_off).ok()?;
                    let rect = range.get_bounding_client_rect();
                    return Some((rect.left() - body_rect.left(), rect.top() - body_rect.top()));
                }
                remaining -= len;
            }
        }
    }
    if let Some(n) = last_text {
        let text = n.text_content().unwrap_or_default();
        let range = document.create_range().ok()?;
        let byte_off = text
            .char_indices()
            .nth(last_len)
            .map(|(i, _)| i)
            .unwrap_or(text.len()) as u32;
        range.set_start(&n, byte_off).ok()?;
        range.set_end(&n, byte_off).ok()?;
        let rect = range.get_bounding_client_rect();
        return Some((rect.left() - body_rect.left(), rect.top() - body_rect.top()));
    }
    Some((0.0, 0.0))
}

const WRITER_BODY_ID: &str = "writer-body";

fn exec_cmd(name: &str, value: Option<&str>) {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if let Some(el) = doc.get_element_by_id(WRITER_BODY_ID)
            && let Some(html_el) = el.dyn_ref::<web_sys::HtmlElement>()
        {
            let _ = html_el.focus();
        }
        if let Ok(html_doc) = doc.dyn_into::<web_sys::HtmlDocument>() {
            let val = value.unwrap_or("");
            let _ = html_doc.exec_command_with_show_ui_and_value(name, false, val);
        }
    }
}

fn editor_text() -> String {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(WRITER_BODY_ID))
        .and_then(|el| el.dyn_ref::<web_sys::HtmlElement>().map(|h| h.inner_text()))
        .unwrap_or_default()
}

fn plain_text_to_html(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let escaped = line
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            if escaped.is_empty() {
                "<div><br></div>".to_string()
            } else {
                format!("<div>{escaped}</div>")
            }
        })
        .collect()
}

fn writer_replace_from_operation(payload: &serde_json::Value) -> Option<(String, String)> {
    let user_id = payload.get("user_id")?.as_str()?.to_string();
    let op_type = payload.get("op_type")?;
    if op_type.get("type")?.as_str()? != "Replace" {
        return None;
    }
    let path = op_type.get("path")?.as_array()?;
    if path.len() != 1 || path[0].as_str()? != "body_text" {
        return None;
    }
    let text = op_type.get("new_value")?.as_str()?.to_string();
    Some((user_id, text))
}

fn apply_writer_text_to_dom(text: &str) {
    if let Some(body) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id(WRITER_BODY_ID))
    {
        body.set_inner_html(&plain_text_to_html(text));
    }
}

#[derive(Clone, Deserialize)]
struct DocumentResponse {
    name: String,
    content: serde_json::Value,
}

#[derive(Deserialize)]
struct WriterDocLite {
    #[serde(default)]
    body: Vec<serde_json::Value>,
}

fn extract_run_text(runs: &[serde_json::Value]) -> String {
    runs.iter()
        .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
        .collect::<String>()
}

fn writer_content_to_plain(content: &serde_json::Value) -> String {
    let doc: WriterDocLite = match serde_json::from_value(content.clone()) {
        Ok(d) => d,
        Err(_) => return String::new(),
    };
    doc.body
        .iter()
        .filter_map(|block| {
            if block["type"] == "PageBreak" {
                Some("\u{000c}".to_string())
            } else {
                block
                    .get("runs")?
                    .as_array()
                    .map(|runs| extract_run_text(runs))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn plain_writer_run(text: &str) -> serde_json::Value {
    serde_json::json!({
        "text": text, "bold": false, "italic": false, "underline": false,
        "strikethrough": false, "font_family": null, "font_size": null,
        "color": null, "background_color": null, "link": null,
        "superscript": false, "subscript": false
    })
}

/// Splice the changed character range while retaining formatting on unaffected runs.
fn replace_writer_runs(runs: &[serde_json::Value], text: &str) -> Vec<serde_json::Value> {
    let old = extract_run_text(runs).chars().collect::<Vec<_>>();
    let new = text.chars().collect::<Vec<_>>();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let replacement: String = new[prefix..new.len() - suffix].iter().collect();
    let mut result = Vec::new();
    let mut offset = 0;
    let mut inserted = false;
    for run in runs {
        let chars = run["text"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .collect::<Vec<_>>();
        let end = offset + chars.len();
        let mut value: String = chars[..prefix.saturating_sub(offset).min(chars.len())]
            .iter()
            .collect();
        if !inserted && end >= prefix {
            value.push_str(&replacement);
            inserted = true;
        }
        value.extend(chars[(old.len() - suffix).saturating_sub(offset).min(chars.len())..].iter());
        if !value.is_empty() {
            let mut updated = run.clone();
            updated["text"] = serde_json::Value::String(value);
            result.push(updated);
        }
        offset = end;
    }
    if result.is_empty() {
        result.push(plain_writer_run(text));
    }
    result
}

fn writer_content_with_text(base: &serde_json::Value, text: &str) -> serde_json::Value {
    // A metadata-only save must never rewrite rich blocks or their stable IDs.
    if writer_content_to_plain(base) == text {
        return base.clone();
    }
    let mut content = base.clone();
    let Some(object) = content.as_object_mut() else {
        return serde_json::json!({ "body": [] });
    };
    let mut replacement = Vec::new();
    for line in text.split('\n') {
        for (index, part) in line.split('\u{000c}').enumerate() {
            if index > 0 {
                replacement.push(serde_json::json!({ "type": "PageBreak" }));
            }
            if part.is_empty() && line.contains('\u{000c}') {
                continue;
            }
            replacement.push(serde_json::json!({
                "type": "Paragraph",
                "id": uuid::Uuid::new_v4(),
                "runs": [plain_writer_run(part)],
                "alignment": "Left",
                "indent_level": 0,
                "spacing": { "before": 0.0, "after": 8.0, "line_height": 1.15 }
            }));
        }
    }
    let mut replacement = replacement.into_iter();
    let mut blocks = Vec::new();
    for original in base["body"].as_array().into_iter().flatten() {
        if original.get("runs").is_none() && original["type"] != "PageBreak" {
            // Tables, images, lists and other rich blocks are not represented by
            // the plain-text surface. Keep them rather than silently deleting them.
            blocks.push(original.clone());
            continue;
        }
        if let Some(mut next) = replacement.next() {
            if let (Some(runs), Some(next_runs)) =
                (original["runs"].as_array(), next["runs"].as_array())
            {
                let line = extract_run_text(next_runs);
                next = original.clone();
                if extract_run_text(runs) != line {
                    next["runs"] = serde_json::Value::Array(replace_writer_runs(runs, &line));
                }
            }
            blocks.push(next);
        }
    }
    blocks.extend(replacement);
    object.insert("body".into(), serde_json::Value::Array(blocks));
    content
}

fn update_professional_field(
    base: &serde_json::Value,
    key: &str,
    value: serde_json::Value,
) -> serde_json::Value {
    let mut content = base.clone();
    if !content.is_object() {
        content = serde_json::json!({});
    }
    let object = content.as_object_mut().expect("object assigned above");
    let professional = object
        .entry("professional")
        .or_insert_with(|| serde_json::json!({}));
    if !professional.is_object() {
        *professional = serde_json::json!({});
    }
    professional
        .as_object_mut()
        .expect("object assigned above")
        .insert(key.to_string(), value);
    content
}

fn attach_pending_revisions(base: serde_json::Value, changes: &[ChangeItem]) -> serde_json::Value {
    let revisions = changes
        .iter()
        .map(|change| {
            serde_json::json!({
                "id": change.dom_id.parse::<uuid::Uuid>().unwrap_or_else(|_| uuid::Uuid::new_v4()),
                "author": change.author,
                "timestamp": chrono::Utc::now(),
                "block_index": 0,
                "kind": if change.kind == "ins" { "Insert" } else { "Delete" },
                "text": change.text,
            })
        })
        .collect::<Vec<_>>();
    update_professional_field(&base, "revisions", serde_json::Value::Array(revisions))
}

#[derive(Clone, PartialEq)]
struct ChangeItem {
    kind: String,
    text: String,
    author: String,
    dom_id: String,
}

fn insert_tracked_text(text: &str, color: &str, author: &str) {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return,
    };
    let sel = match document.get_selection().ok().flatten() {
        Some(s) => s,
        None => return,
    };
    if sel.range_count() == 0 {
        return;
    }
    let range = match sel.get_range_at(0).ok() {
        Some(r) => r,
        None => return,
    };
    let _ = range.delete_contents();
    let span = match document.create_element("ins") {
        Ok(s) => s,
        Err(_) => return,
    };
    let _ = span.set_attribute("class", "tracked-ins");
    let _ = span.set_attribute("data-author", author);
    let _ = span.set_attribute(
        "style",
        &format!("background-color: {color}22; color: {color}; text-decoration: underline;"),
    );
    span.set_text_content(Some(text));
    if range.insert_node(&span).is_err() {
        return;
    }
    let span_node: &web_sys::Node = span.as_ref();
    let _ = range.set_start_after(span_node);
    let _ = range.set_end_after(span_node);
    let _ = sel.remove_all_ranges();
    let _ = sel.add_range(&range);
}

fn wrap_selection_as_deleted(author: &str) -> bool {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return false,
    };
    let document = match window.document() {
        Some(d) => d,
        None => return false,
    };
    let sel = match document.get_selection().ok().flatten() {
        Some(s) => s,
        None => return false,
    };
    if sel.range_count() == 0 {
        return false;
    }
    if sel.is_collapsed() {
        return false;
    }
    let range = match sel.get_range_at(0).ok() {
        Some(r) => r,
        None => return false,
    };
    let text = range.to_string().as_string().unwrap_or_default();
    if text.is_empty() {
        return false;
    }
    let _ = range.delete_contents();
    let del = match document.create_element("del") {
        Ok(d) => d,
        Err(_) => return false,
    };
    let _ = del.set_attribute("class", "tracked-del");
    let _ = del.set_attribute("data-author", author);
    let _ = del.set_attribute("style", "text-decoration: line-through; opacity: 0.7;");
    del.set_text_content(Some(&text));
    if range.insert_node(&del).is_err() {
        return false;
    }
    let del_node: &web_sys::Node = del.as_ref();
    let _ = range.set_start_after(del_node);
    let _ = range.set_end_after(del_node);
    let _ = sel.remove_all_ranges();
    let _ = sel.add_range(&range);
    true
}

fn list_pending_changes() -> Vec<ChangeItem> {
    let document = match web_sys::window().and_then(|w| w.document()) {
        Some(d) => d,
        None => return Vec::new(),
    };
    let body = match document.get_element_by_id(WRITER_BODY_ID) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let nodes = match body.query_selector_all(".tracked-ins, .tracked-del") {
        Ok(n) => n,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for i in 0..nodes.length() {
        let Some(node) = nodes.item(i) else { continue };
        let Some(el) = node.dyn_ref::<web_sys::Element>() else {
            continue;
        };
        let tag = el.tag_name().to_lowercase();
        let kind = if tag == "ins" { "ins" } else { "del" };
        let text = el.text_content().unwrap_or_default();
        let author = el.get_attribute("data-author").unwrap_or_default();
        let id = el.get_attribute("data-change-id").unwrap_or_else(|| {
            let new_id = uuid::Uuid::new_v4().to_string();
            let _ = el.set_attribute("data-change-id", &new_id);
            new_id
        });
        out.push(ChangeItem {
            kind: kind.to_string(),
            text,
            author,
            dom_id: id,
        });
    }
    out
}

fn accept_change(dom_id: &str) {
    let document = match web_sys::window().and_then(|w| w.document()) {
        Some(d) => d,
        None => return,
    };
    let body = match document.get_element_by_id(WRITER_BODY_ID) {
        Some(b) => b,
        None => return,
    };
    let selector = format!("[data-change-id=\"{dom_id}\"]");
    let Ok(Some(el)) = body.query_selector(&selector) else {
        return;
    };
    let tag = el.tag_name().to_lowercase();
    let parent = match el.parent_node() {
        Some(p) => p,
        None => return,
    };
    let el_node: &web_sys::Node = el.as_ref();
    if tag == "ins" {
        let text = el.text_content().unwrap_or_default();
        let text_node = document.create_text_node(&text);
        let text_node_ref: &web_sys::Node = text_node.as_ref();
        let _ = parent.replace_child(text_node_ref, el_node);
    } else {
        let _ = parent.remove_child(el_node);
    }
}

fn reject_change(dom_id: &str) {
    let document = match web_sys::window().and_then(|w| w.document()) {
        Some(d) => d,
        None => return,
    };
    let body = match document.get_element_by_id(WRITER_BODY_ID) {
        Some(b) => b,
        None => return,
    };
    let selector = format!("[data-change-id=\"{dom_id}\"]");
    let Ok(Some(el)) = body.query_selector(&selector) else {
        return;
    };
    let tag = el.tag_name().to_lowercase();
    let parent = match el.parent_node() {
        Some(p) => p,
        None => return,
    };
    let el_node: &web_sys::Node = el.as_ref();
    if tag == "ins" {
        let _ = parent.remove_child(el_node);
    } else {
        let text = el.text_content().unwrap_or_default();
        let text_node = document.create_text_node(&text);
        let text_node_ref: &web_sys::Node = text_node.as_ref();
        let _ = parent.replace_child(text_node_ref, el_node);
    }
}

fn accept_all_changes() {
    for ch in list_pending_changes() {
        accept_change(&ch.dom_id);
    }
}

fn reject_all_changes() {
    for ch in list_pending_changes() {
        reject_change(&ch.dom_id);
    }
}

/// Rough page estimate for the status bar: ~3000 characters per A4 page.
fn estimate_page_count(char_count: u32) -> u32 {
    (char_count / 3000) + 1
}

/// Rich-text document editor (word processor).
#[component]
pub fn Writer(id: String) -> Element {
    let _ = use_context::<Signal<String>>().read();
    let mut doc_title = use_signal(|| crate::i18n::t("editor.writer.untitled"));
    let mut word_count = use_signal(|| 0u32);
    let mut char_count = use_signal(|| 0u32);
    let mut page_count = use_signal(|| 1u32);
    let mut saved_status = use_signal(|| crate::i18n::t("common.saved"));
    let mut show_sidebar = use_signal(|| true);
    let mut doc_body_text = use_signal(String::new);
    let mut initial_html = use_signal(String::new);
    let mut document_content = use_signal(|| serde_json::json!({}));
    let mut loaded = use_signal(|| false);

    let mut find_open = use_signal(|| false);
    let mut find_query = use_signal(String::new);
    let mut find_info = use_signal(String::new);

    let mut ctx_menu = use_signal(|| None::<(f64, f64, Vec<MenuEntry>)>);
    let mut comments_open = use_signal(|| false);
    let mut versions_open = use_signal(|| false);
    let mut tracking_on = use_signal(|| false);
    let mut track_pane_open = use_signal(|| false);
    let mut changes_tick = use_signal(|| 0u32);

    let auth = use_context::<Signal<AuthState>>();
    let toast_bus = use_context::<Signal<Vec<Toast>>>();
    let nav = use_navigator();
    let presence = use_signal(Vec::<RemoteCursor>::new);
    let mut ws_handle = use_signal(|| None::<CollabHandle>);
    let mut conn_state = use_signal(|| ConnectionState::Connecting);

    let id_for_resource = id.clone();
    let doc_resource = use_resource(move || {
        let id = id_for_resource.clone();
        let token = auth.read().token.clone().unwrap_or_default();
        async move { api_get::<DocumentResponse>(&format!("/documents/{id}"), &token).await }
    });

    use_effect(move || {
        if *loaded.read() {
            return;
        }
        if let Some(Ok(doc)) = doc_resource.read().as_ref() {
            doc_title.set(doc.name.clone());
            let text = writer_content_to_plain(&doc.content);
            let wc = text.split_whitespace().count() as u32;
            let cc = text.chars().count() as u32;
            let html = plain_text_to_html(&text);
            document_content.set(doc.content.clone());
            initial_html.set(html);
            doc_body_text.set(text);
            word_count.set(wc);
            char_count.set(cc);
            page_count.set(estimate_page_count(cc));
            loaded.set(true);
        }
    });

    let doc_id_for_effect = id.clone();
    let local_user_id = auth
        .read()
        .user
        .as_ref()
        .map(|u| u.id.to_string())
        .unwrap_or_default();
    let local_user_id_for_effect = local_user_id.clone();
    use_effect(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        let doc_id = doc_id_for_effect.clone();
        let self_id = local_user_id_for_effect.clone();
        let mut presence_for_msg = presence;
        let on_message = move |msg: CollabMessage| match msg {
            CollabMessage::UserJoined {
                user_id,
                username,
                color,
            } => {
                let mut list = presence_for_msg.write();
                if !list.iter().any(|c| c.user_id == user_id) {
                    list.push(RemoteCursor {
                        user_id,
                        username,
                        color,
                        position: None,
                    });
                }
            }
            CollabMessage::UserLeft { user_id } => {
                let mut list = presence_for_msg.write();
                list.retain(|c| c.user_id != user_id);
            }
            CollabMessage::CursorUpdate(payload) => {
                if let Ok(parsed) = serde_json::from_value::<CursorPosLite>(payload) {
                    let Some(uid) = cursor_user_id(&parsed.user_id) else {
                        return;
                    };
                    if uid == self_id {
                        return;
                    }
                    let off = parsed.position.as_ref().and_then(cursor_offset);
                    let mut list = presence_for_msg.write();
                    if let Some(existing) = list.iter_mut().find(|c| c.user_id == uid) {
                        if off.is_some() {
                            existing.position = off;
                        }
                        if let Some(u) = parsed.username
                            && existing.username.is_empty()
                        {
                            existing.username = u;
                        }
                        if let Some(c) = parsed.color
                            && existing.color.is_empty()
                        {
                            existing.color = c;
                        }
                    } else {
                        list.push(RemoteCursor {
                            user_id: uid,
                            username: parsed.username.unwrap_or_default(),
                            color: parsed.color.unwrap_or_else(|| "#888".into()),
                            position: off,
                        });
                    }
                }
            }
            CollabMessage::Operation(payload) => {
                let Some((author_id, text)) = writer_replace_from_operation(&payload) else {
                    return;
                };
                if author_id == self_id {
                    return;
                }
                let wc = text.split_whitespace().count() as u32;
                let cc = text.chars().count() as u32;
                apply_writer_text_to_dom(&text);
                initial_html.set(plain_text_to_html(&text));
                doc_body_text.set(text);
                let next =
                    writer_content_with_text(&document_content.read(), &doc_body_text.read());
                document_content.set(next);
                word_count.set(wc);
                char_count.set(cc);
                page_count.set(estimate_page_count(cc));
                saved_status.set(crate::i18n::t("common.saved"));
            }
            _ => {}
        };
        let on_state = move |s: ConnectionState| {
            conn_state.set(s);
        };
        let handle = connect_with_reconnect(doc_id, token, on_message, on_state);
        ws_handle.set(Some(handle));
    });

    let ws_for_selection = ws_handle;
    let user_id_for_selection = local_user_id.clone();
    use_effect(move || {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let last_sent: Rc<RefCell<f64>> = Rc::new(RefCell::new(0.0));
        let perf = window.performance();
        let self_id = user_id_for_selection.clone();
        let ws_signal = ws_for_selection;
        let cb = Closure::<dyn FnMut()>::new(move || {
            let Some(handle) = ws_signal.read().clone() else {
                return;
            };
            let Some(ws) = handle.current_ws() else {
                return;
            };
            if ws.ready_state() != WebSocket::OPEN {
                return;
            }
            let now = perf.as_ref().map(|p| p.now()).unwrap_or(0.0);
            {
                let mut last = last_sent.borrow_mut();
                if now - *last < 200.0 {
                    return;
                }
                *last = now;
            }
            let Some(off) = caret_offset_in_body() else {
                return;
            };
            let payload = serde_json::json!({
                "type": "CursorUpdate",
                "user_id": self_id,
                "username": "",
                "color": "#888",
                "position": { "path": [], "offset": off },
                "selection": null,
                "last_active": chrono::Utc::now().to_rfc3339(),
            });
            if let Ok(s) = serde_json::to_string(&payload) {
                let _ = ws.send_with_str(&s);
            }
        });
        document.set_onselectionchange(Some(cb.as_ref().unchecked_ref()));
        cb.forget();
    });

    use_drop(move || {
        if let Some(handle) = ws_handle.write().take() {
            handle.close();
        }
    });

    let username_for_tracking = auth
        .read()
        .user
        .as_ref()
        .map(|u| u.username.clone())
        .unwrap_or_else(|| "you".to_string());
    use_effect(move || {
        if !*loaded.read() {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(document) = window.document() else {
            return;
        };
        let Some(body) = document.get_element_by_id(WRITER_BODY_ID) else {
            return;
        };
        if body.get_attribute("data-track-bound").is_some() {
            return;
        }
        let _ = body.set_attribute("data-track-bound", "1");
        let author = username_for_tracking.clone();
        let tracking_on_inner = tracking_on;
        let mut tick_inner = changes_tick;
        let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |evt: web_sys::Event| {
            if !*tracking_on_inner.read() {
                return;
            }
            let input_type = js_sys::Reflect::get(&evt, &"inputType".into())
                .ok()
                .and_then(|v| v.as_string())
                .unwrap_or_default();
            if input_type == "insertText" || input_type == "insertCompositionText" {
                let data = js_sys::Reflect::get(&evt, &"data".into())
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                if data.is_empty() {
                    return;
                }
                evt.prevent_default();
                insert_tracked_text(&data, "#2563eb", &author);
                let v = *tick_inner.read();
                tick_inner.set(v.wrapping_add(1));
            } else if input_type.starts_with("delete") && wrap_selection_as_deleted(&author) {
                evt.prevent_default();
                let v = *tick_inner.read();
                tick_inner.set(v.wrapping_add(1));
            }
        });
        let _ = body.add_event_listener_with_callback("beforeinput", cb.as_ref().unchecked_ref());
        cb.forget();
    });

    let save_timer: Rc<RefCell<Option<Timeout>>> = use_hook(|| Rc::new(RefCell::new(None)));

    let perform_save = {
        let id = id.clone();
        move |body: serde_json::Value, mut status: Signal<String>, token: String| {
            let id = id.clone();
            status.set(crate::i18n::t("common.saving"));
            wasm_bindgen_futures::spawn_local(async move {
                let req = serde_json::json!({ "content": body });
                match api_put::<serde_json::Value, _>(&format!("/documents/{id}"), &req, &token)
                    .await
                {
                    Ok(_) => status.set(crate::i18n::t("common.saved")),
                    Err(e) => status
                        .set(crate::i18n::t("editor.save_failed").replace("{err}", &e.to_string())),
                }
            });
        }
    };

    let schedule_save = {
        let save_timer = save_timer.clone();
        let perform_save = perform_save.clone();
        move || {
            if !*loaded.read() {
                return;
            }
            saved_status.set(crate::i18n::t("common.unsaved"));
            let body = writer_content_with_text(&document_content.read(), &doc_body_text.read());
            let body = attach_pending_revisions(body, &list_pending_changes());
            document_content.set(body.clone());
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
        move || {
            if !*loaded.read() {
                return;
            }
            save_timer.borrow_mut().take();
            let body = writer_content_with_text(&document_content.read(), &doc_body_text.read());
            let body = attach_pending_revisions(body, &list_pending_changes());
            document_content.set(body.clone());
            let token = auth.read().token.clone().unwrap_or_default();
            perform_save(body, saved_status, token);
        }
    };

    rsx! {
        Navbar {}
        {
            let state = *conn_state.read();
            match state {
                ConnectionState::Reconnecting(n) => {
                    let msg = crate::i18n::t("ws.reconnecting").replace("{n}", &n.to_string());
                    rsx! {
                        div { class: "ws-banner ws-reconnecting", "{msg}" }
                    }
                },
                ConnectionState::Failed => {
                    let msg = crate::i18n::t("ws.failed");
                    rsx! {
                        div { class: "ws-banner ws-failed", "{msg}" }
                    }
                },
                _ => rsx! {},
            }
        }
        div {
            class: "editor-container",
            tabindex: "0",
            onkeydown: {
                let mut flush_save = flush_save.clone();
                let mut schedule_save = schedule_save.clone();
                move |evt: KeyboardEvent| {
                    let modifiers = evt.modifiers();
                    if !modifiers.contains(Modifiers::CONTROL) {
                        return;
                    }
                    if let Key::Character(ref ch) = evt.key() {
                        match ch.as_str() {
                            "f" => {
                                evt.prevent_default();
                                find_open.set(true);
                            }
                            "s" => { evt.prevent_default(); flush_save(); }
                            "b" => {
                                evt.prevent_default();
                                exec_cmd("bold", None);
                                doc_body_text.set(editor_text());
                                schedule_save();
                            }
                            "i" => {
                                evt.prevent_default();
                                exec_cmd("italic", None);
                                doc_body_text.set(editor_text());
                                schedule_save();
                            }
                            "u" => {
                                evt.prevent_default();
                                exec_cmd("underline", None);
                                doc_body_text.set(editor_text());
                                schedule_save();
                            }
                            "z" if !modifiers.contains(Modifiers::SHIFT) => {
                                evt.prevent_default();
                                exec_cmd("undo", None);
                                doc_body_text.set(editor_text());
                                schedule_save();
                            }
                            "z" if modifiers.contains(Modifiers::SHIFT) => {
                                evt.prevent_default();
                                exec_cmd("redo", None);
                                doc_body_text.set(editor_text());
                                schedule_save();
                            }
                            "y" => {
                                evt.prevent_default();
                                exec_cmd("redo", None);
                                doc_body_text.set(editor_text());
                                schedule_save();
                            }
                            _ => {}
                        }
                    }
                }
            },
            // Menubar
            {
                let mut flush_for_menu = flush_save.clone();
                let mut schedule_for_menu = schedule_save.clone();
                let id_for_menu = id.clone();
                let bus_for_menu = toast_bus;
                let nav_for_menu = nav;
                let mut find_open_menu = find_open;
                let mut show_sidebar_menu = show_sidebar;
                rsx! {
                    Menubar {
                        document_type: "writer".to_string(),
                        on_command: move |cmd: String| {
                            let bus = bus_for_menu;
                            match cmd.as_str() {
                                "save" => flush_for_menu(),
                                "undo" => { exec_cmd("undo", None); schedule_for_menu(); }
                                "redo" => { exec_cmd("redo", None); schedule_for_menu(); }
                                "cut" => exec_cmd("cut", None),
                                "copy" => exec_cmd("copy", None),
                                "paste" => exec_cmd("paste", None),
                                "find" => { find_open_menu.set(true); }
                                "back" => { nav_for_menu.push(Route::Dashboard {}); }
                                "toggle-sidebar" => {
                                    let cur = *show_sidebar_menu.read();
                                    show_sidebar_menu.set(!cur);
                                }
                                "toggle-comments" => {
                                    let cur = *comments_open.read();
                                    comments_open.set(!cur);
                                }
                                "version-history" => {
                                    let cur = *versions_open.read();
                                    versions_open.set(!cur);
                                }
                                "track-changes" => {
                                    let cur = *track_pane_open.read();
                                    track_pane_open.set(!cur);
                                    let v = *changes_tick.read();
                                    changes_tick.set(v.wrapping_add(1));
                                }
                                "toggle-theme" => {
                                    if let Some(window) = web_sys::window()
                                        && let Some(el) = window.document().and_then(|d| d.document_element()) {
                                            let cur = el.get_attribute("data-theme").unwrap_or_default();
                                            let next = if cur == "dark" { "light" } else { "dark" };
                                            let _ = el.set_attribute("data-theme", next);
                                            if let Ok(Some(storage)) = window.local_storage() {
                                                let _ = storage.set_item("theme", next);
                                            }
                                        }
                                }
                                "format-bold" => { exec_cmd("bold", None); schedule_for_menu(); }
                                "format-italic" => { exec_cmd("italic", None); schedule_for_menu(); }
                                "format-underline" => { exec_cmd("underline", None); schedule_for_menu(); }
                                "insert-page-break" => {
                                    exec_cmd("insertText", Some("\u{000c}"));
                                    doc_body_text.set(editor_text());
                                    schedule_for_menu();
                                }
                                "insert-header" | "insert-footer" => {
                                    if let Some(window) = web_sys::window()
                                        && let Ok(Some(value)) = window.prompt_with_message(
                                            if cmd == "insert-header" { "Header text" } else { "Footer text" }
                                        )
                                    {
                                        let field = if cmd == "insert-header" { "header" } else { "footer" };
                                        let current = document_content.read().clone();
                                        document_content.set(update_professional_field(
                                            &current,
                                            field,
                                            serde_json::Value::String(value),
                                        ));
                                        if field == "footer" {
                                            let current = document_content.read().clone();
                                            document_content.set(update_professional_field(
                                                &current,
                                                "page_numbers",
                                                serde_json::Value::Bool(true),
                                            ));
                                        }
                                        schedule_for_menu();
                                    }
                                }
                                "insert-toc" => {
                                    let current = document_content.read().clone();
                                    document_content.set(update_professional_field(
                                        &current,
                                        "table_of_contents",
                                        serde_json::Value::Bool(true),
                                    ));
                                    schedule_for_menu();
                                }
                                "insert-section-break" => {
                                    let mut sections = document_content
                                        .read()
                                        .pointer("/professional/sections")
                                        .and_then(|value| value.as_array())
                                        .cloned()
                                        .unwrap_or_default();
                                    let page_settings = document_content
                                        .read()
                                        .get("page_settings")
                                        .cloned()
                                        .unwrap_or_else(|| serde_json::json!({
                                            "width_mm": 210.0, "height_mm": 297.0,
                                            "margin_top": 25.4, "margin_right": 25.4,
                                            "margin_bottom": 25.4, "margin_left": 25.4,
                                            "orientation": "Portrait"
                                        }));
                                    sections.push(serde_json::json!({
                                        "start_block": doc_body_text.read().lines().count(),
                                        "page_settings": page_settings,
                                        "header": null,
                                        "footer": null
                                    }));
                                    let current = document_content.read().clone();
                                    document_content.set(update_professional_field(
                                        &current,
                                        "sections",
                                        serde_json::Value::Array(sections),
                                    ));
                                    exec_cmd("insertText", Some("\u{000c}"));
                                    doc_body_text.set(editor_text());
                                    schedule_for_menu();
                                }
                                "insert-footnote" => {
                                    if let Some(window) = web_sys::window()
                                        && let Ok(Some(value)) = window.prompt_with_message("Footnote text")
                                        && !value.trim().is_empty()
                                    {
                                        let mut notes = document_content
                                            .read()
                                            .pointer("/professional/footnotes")
                                            .and_then(|value| value.as_array())
                                            .cloned()
                                            .unwrap_or_default();
                                        notes.push(serde_json::json!({
                                            "id": notes.len() + 1,
                                            "reference_block": doc_body_text.read().lines().count().saturating_sub(1),
                                            "text": value
                                        }));
                                        let current = document_content.read().clone();
                                        document_content.set(update_professional_field(
                                            &current,
                                            "footnotes",
                                            serde_json::Value::Array(notes),
                                        ));
                                        schedule_for_menu();
                                    }
                                }
                                "insert-citation" => {
                                    if let Some(window) = web_sys::window()
                                        && let Ok(Some(title)) = window.prompt_with_message("Citation title")
                                        && !title.trim().is_empty()
                                    {
                                        let mut bibliography = document_content
                                            .read()
                                            .pointer("/professional/bibliography")
                                            .and_then(|value| value.as_array())
                                            .cloned()
                                            .unwrap_or_default();
                                        bibliography.push(serde_json::json!({
                                            "id": uuid::Uuid::new_v4().to_string(),
                                            "author": "Unknown",
                                            "title": title,
                                            "year": null,
                                            "url": null
                                        }));
                                        let current = document_content.read().clone();
                                        document_content.set(update_professional_field(
                                            &current,
                                            "bibliography",
                                            serde_json::Value::Array(bibliography),
                                        ));
                                        schedule_for_menu();
                                    }
                                }
                                "insert-merge-field" => {
                                    if let Some(window) = web_sys::window()
                                        && let Ok(Some(name)) = window.prompt_with_message("Mail merge field name")
                                        && !name.trim().is_empty()
                                    {
                                        let mut fields = document_content
                                            .read()
                                            .pointer("/professional/mail_merge_fields")
                                            .and_then(|value| value.as_array())
                                            .cloned()
                                            .unwrap_or_default();
                                        fields.push(serde_json::json!({
                                            "name": name,
                                            "block_index": doc_body_text.read().lines().count().saturating_sub(1),
                                            "default_value": null
                                        }));
                                        let current = document_content.read().clone();
                                        document_content.set(update_professional_field(
                                            &current,
                                            "mail_merge_fields",
                                            serde_json::Value::Array(fields),
                                        ));
                                        schedule_for_menu();
                                    }
                                }
                                "print" => {
                                    if let Some(w) = web_sys::window() { let _ = w.print(); }
                                }
                                "export-pdf" | "export-docx" | "export-html" => {
                                    let format = match cmd.as_str() {
                                        "export-pdf" => "pdf",
                                        "export-docx" => "docx",
                                        _ => "html",
                                    };
                                    let id = id_for_menu.clone();
                                    let token = auth.read().token.clone().unwrap_or_default();
                                    let bus2 = bus;
                                    let fmt_owned = format.to_string();
                                    wasm_bindgen_futures::spawn_local(async move {
                                        match export_document(&id, &fmt_owned, &token).await {
                                            Ok(_) => push_toast(bus2, ToastKind::Success, crate::i18n::t("editor.export_succeeded").replace("{fmt}", &fmt_owned.to_uppercase())),
                                            Err(e) => push_toast(bus2, ToastKind::Error, crate::i18n::t("editor.export_failed").replace("{err}", &e.to_string())),
                                        }
                                    });
                                }
                                other => push_toast(bus, ToastKind::Info, crate::i18n::t("editor.coming_soon").replace("{what}", other)),
                            }
                        },
                    }
                }
            }

            // Document title bar
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
                    placeholder: crate::i18n::t("editor.writer.untitled"),
                }
                span { class: "save-status", "{saved_status}" }
                if !presence.read().is_empty() {
                    div {
                        class: "presence-list",
                        style: "display: inline-flex; gap: 4px; margin-left: 12px; align-items: center;",
                        for cursor in presence.read().iter() {
                            span {
                                key: "{cursor.user_id}",
                                class: "presence-pill",
                                title: "{cursor.username}",
                                style: "background: {cursor.color}; color: #fff; border-radius: 999px; padding: 2px 8px; font-size: 12px; font-weight: 600; line-height: 1;",
                                {cursor.username.chars().take(2).collect::<String>().to_uppercase()}
                            }
                        }
                    }
                }
                button {
                    class: "btn btn-outline btn-sm",
                    onclick: move |_| {
                        let current = *show_sidebar.read();
                        show_sidebar.set(!current);
                    },
                    {if *show_sidebar.read() { crate::i18n::t("editor.hide_sidebar") } else { crate::i18n::t("editor.show_sidebar") }}
                }
            }

            // Toolbar
            {
                let run = {
                    let mut schedule_save = schedule_save.clone();
                    move |name: &'static str, value: Option<String>| {
                        exec_cmd(name, value.as_deref());
                        doc_body_text.set(editor_text());
                        schedule_save();
                    }
                };
                let run_b = run.clone(); let run_i = run.clone(); let run_u = run.clone();
                let run_s = run.clone(); let run_jl = run.clone(); let run_jc = run.clone();
                let run_jr = run.clone(); let run_jf = run.clone(); let run_ul = run.clone();
                let run_ol = run.clone(); let run_in = run.clone(); let run_out = run.clone();
                let run_font = run.clone(); let run_size = run.clone();
                let run_fore = run.clone(); let run_hi = run.clone();
                let bold_title = crate::i18n::t("editor.bold");
                let italic_title = crate::i18n::t("editor.italic");
                let underline_title = crate::i18n::t("editor.underline");
                let strike_title = crate::i18n::t("editor.strikethrough");
                let bul_title = crate::i18n::t("editor.bulleted_list");
                let num_title = crate::i18n::t("editor.numbered_list");
                let indent_title = crate::i18n::t("editor.indent");
                let outdent_title = crate::i18n::t("editor.outdent");
                let alignl_title = crate::i18n::t("editor.align_left");
                let alignc_title = crate::i18n::t("editor.align_center");
                let alignr_title = crate::i18n::t("editor.align_right");
                let justify_title = crate::i18n::t("editor.justify");
                let tc_title = crate::i18n::t("editor.track_changes");
                let changes_title = crate::i18n::t("editor.changes");
                let font_title = crate::i18n::t("editor.writer.font_family");
                let size_title = crate::i18n::t("editor.writer.font_size");
                let textcol_title = crate::i18n::t("editor.text_color");
                let hlcol_title = crate::i18n::t("editor.highlight_color");
                rsx! {
                    div { class: "toolbar",
                        div { class: "toolbar-group",
                            select {
                                class: "toolbar-select font-family",
                                title: "{font_title}",
                                aria_label: "{font_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onchange: move |e: FormEvent| { let mut r = run_font.clone(); r("fontName", Some(e.value())); },
                                option { value: "Arial", "Arial" }
                                option { value: "Times New Roman", "Times New Roman" }
                                option { value: "Courier New", "Courier New" }
                                option { value: "Georgia", "Georgia" }
                                option { value: "Verdana", "Verdana" }
                                option { value: "Helvetica", "Helvetica" }
                            }
                            select {
                                class: "toolbar-select font-size",
                                title: "{size_title}",
                                aria_label: "{size_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onchange: move |e: FormEvent| { let mut r = run_size.clone(); r("fontSize", Some(e.value())); },
                                option { value: "2", "10pt" }
                                option { value: "3", selected: true, "12pt" }
                                option { value: "4", "14pt" }
                                option { value: "5", "18pt" }
                                option { value: "6", "24pt" }
                                option { value: "7", "36pt" }
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button {
                                class: "toolbar-btn", title: "{bold_title}",
                                aria_label: "{bold_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_b.clone(); r("bold", None); },
                                "B"
                            }
                            button {
                                class: "toolbar-btn italic", title: "{italic_title}",
                                aria_label: "{italic_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_i.clone(); r("italic", None); },
                                "I"
                            }
                            button {
                                class: "toolbar-btn underline", title: "{underline_title}",
                                aria_label: "{underline_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_u.clone(); r("underline", None); },
                                "U"
                            }
                            button {
                                class: "toolbar-btn strikethrough", title: "{strike_title}",
                                aria_label: "{strike_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_s.clone(); r("strikeThrough", None); },
                                "S"
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button {
                                class: "toolbar-btn", title: "{bul_title}",
                                aria_label: "{bul_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_ul.clone(); r("insertUnorderedList", None); },
                                "* List"
                            }
                            button {
                                class: "toolbar-btn", title: "{num_title}",
                                aria_label: "{num_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_ol.clone(); r("insertOrderedList", None); },
                                "1. List"
                            }
                            button {
                                class: "toolbar-btn", title: "{indent_title}",
                                aria_label: "{indent_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_in.clone(); r("indent", None); },
                                ">>"
                            }
                            button {
                                class: "toolbar-btn", title: "{outdent_title}",
                                aria_label: "{outdent_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_out.clone(); r("outdent", None); },
                                "<<"
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button {
                                class: "toolbar-btn", title: "{alignl_title}",
                                aria_label: "{alignl_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_jl.clone(); r("justifyLeft", None); },
                                "L"
                            }
                            button {
                                class: "toolbar-btn", title: "{alignc_title}",
                                aria_label: "{alignc_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_jc.clone(); r("justifyCenter", None); },
                                "C"
                            }
                            button {
                                class: "toolbar-btn", title: "{alignr_title}",
                                aria_label: "{alignr_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_jr.clone(); r("justifyRight", None); },
                                "R"
                            }
                            button {
                                class: "toolbar-btn", title: "{justify_title}",
                                aria_label: "{justify_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_jf.clone(); r("justifyFull", None); },
                                "J"
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            input {
                                r#type: "color", class: "toolbar-color", title: "{textcol_title}", value: "#000000",
                                aria_label: "{textcol_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                oninput: move |e: FormEvent| { let mut r = run_fore.clone(); r("foreColor", Some(e.value())); },
                            }
                            input {
                                r#type: "color", class: "toolbar-color", title: "{hlcol_title}", value: "#ffff00",
                                aria_label: "{hlcol_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                oninput: move |e: FormEvent| {
                                    let val = e.value();
                                    let mut r = run_hi.clone();
                                    r("hiliteColor", Some(val.clone()));
                                    r("backColor", Some(val));
                                },
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button {
                                class: if *tracking_on.read() { "toolbar-btn active" } else { "toolbar-btn" },
                                title: "{tc_title}",
                                aria_label: "{tc_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| {
                                    let cur = *tracking_on.read();
                                    tracking_on.set(!cur);
                                },
                                "TC"
                            }
                            button {
                                class: "toolbar-btn",
                                title: "{changes_title}",
                                aria_label: "{changes_title}",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| {
                                    let cur = *track_pane_open.read();
                                    track_pane_open.set(!cur);
                                    let v = *changes_tick.read();
                                    changes_tick.set(v.wrapping_add(1));
                                },
                                "{changes_title}"
                            }
                        }
                    }
                }
            }

            // Main editor area
            div { class: "editor-main",
                if *show_sidebar.read() {
                    Sidebar {}
                }

                div {
                    class: "editor-canvas writer-canvas",
                    oncontextmenu: move |evt: MouseEvent| {
                        evt.prevent_default();
                        let coords = evt.client_coordinates();
                        ctx_menu.set(Some((
                            coords.x,
                            coords.y,
                            vec![
                                MenuEntry::new(crate::i18n::t("ctx.cut"), "cut"),
                                MenuEntry::new(crate::i18n::t("ctx.copy"), "copy"),
                                MenuEntry::new(crate::i18n::t("ctx.paste"), "paste"),
                                MenuEntry::new(crate::i18n::t("ctx.delete"), "delete"),
                                MenuEntry::new(crate::i18n::t("ctx.insert_link"), "insert-link"),
                            ],
                        )));
                    },
                    match doc_resource.read().as_ref() {
                        None => rsx! { div { class: "editor-loading", {crate::i18n::t("editor.loading_document")} } },
                        Some(Err(e)) => rsx! { div { class: "editor-error", {crate::i18n::t("editor.failed_load").replace("{err}", &e.to_string())} } },
                        Some(Ok(_)) => rsx! {
                            div { class: "page",
                                div {
                                    id: WRITER_BODY_ID,
                                    contenteditable: "true",
                                    spellcheck: "true",
                                    class: "writer-content",
                                    dangerous_inner_html: "{initial_html}",
                                    oninput: {
                                        let mut schedule_save = schedule_save.clone();
                                        let self_id = local_user_id.clone();
                                        move |_evt: FormEvent| {
                                            let text = editor_text();
                                            let previous = doc_body_text.read().clone();
                                            let wc = text.split_whitespace().count() as u32;
                                            let cc = text.chars().count() as u32;
                                            word_count.set(wc);
                                            char_count.set(cc);
                                            page_count.set(estimate_page_count(cc));
                                            if previous != text {
                                                if let Some(handle) = ws_handle.read().as_ref() {
                                                    let _ = handle.send_operation(
                                                        &self_id,
                                                        serde_json::json!({
                                                            "type": "Replace",
                                                            "path": ["body_text"],
                                                            "old_value": previous,
                                                            "new_value": text.clone(),
                                                        }),
                                                    );
                                                }
                                                doc_body_text.set(text);
                                            }
                                            schedule_save();
                                            if *track_pane_open.read() {
                                                let v = *changes_tick.read();
                                                changes_tick.set(v.wrapping_add(1));
                                            }
                                        }
                                    },
                                }
                                div { class: "remote-cursors-layer",
                                    for cursor in presence.read().iter() {
                                        if let Some(pos) = cursor.position {
                                            if let Some((x, y)) = char_offset_to_pixel(pos) {
                                                {
                                                    let cx = x + 72.0;
                                                    let cy = y + 96.0;
                                                    rsx! {
                                                        div {
                                                            key: "{cursor.user_id}",
                                                            class: "remote-cursor",
                                                            style: "left: {cx}px; top: {cy}px; background: {cursor.color};",
                                                            title: "{cursor.username}",
                                                            div { class: "remote-cursor-flag",
                                                                style: "background: {cursor.color};",
                                                                "{cursor.username}"
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }
                }
            }

            // Status bar
            {
                let page_str = crate::i18n::t("editor.writer.pages")
                    .replace("{n}", &page_count.read().to_string());
                let words_str = crate::i18n::t("editor.writer.words").replace("{n}", &word_count.read().to_string());
                let chars_str = crate::i18n::t("editor.writer.characters").replace("{n}", &char_count.read().to_string());
                let id_str = crate::i18n::t("editor.writer.document_id").replace("{id}", &id);
                rsx! {
                    div { class: "status-bar",
                        span { "{page_str}" }
                        span { "{words_str}" }
                        span { "{chars_str}" }
                        span { "{id_str}" }
                    }
                }
            }

            FindDialog {
                open: find_open,
                match_info: find_info.read().clone(),
                on_search: move |q: String| {
                    find_query.set(q.clone());
                    if q.is_empty() {
                        find_info.set(String::new());
                    } else {
                        let ok = window_find(&q, false);
                        find_info.set(if ok { crate::i18n::t("editor.writer.match_label") } else { crate::i18n::t("editor.writer.no_matches") });
                    }
                },
                on_next: move |_| {
                    let q = find_query.read().clone();
                    if q.is_empty() { return; }
                    let ok = window_find(&q, false);
                    find_info.set(if ok { crate::i18n::t("editor.writer.match_label") } else { crate::i18n::t("editor.writer.no_matches") });
                },
                on_prev: move |_| {
                    let q = find_query.read().clone();
                    if q.is_empty() { return; }
                    let ok = window_find(&q, true);
                    find_info.set(if ok { crate::i18n::t("editor.writer.match_label") } else { crate::i18n::t("editor.writer.no_matches") });
                },
                on_replace: {
                    let mut schedule_save = schedule_save.clone();
                    let bus = toast_bus;
                    move |(find, replace): (String, String)| {
                        if find.is_empty() { return; }
                        if window_find(&find, false) {
                            exec_cmd("insertText", Some(&replace));
                            doc_body_text.set(editor_text());
                            schedule_save();
                            push_toast(bus, ToastKind::Success, crate::i18n::t("editor.writer.replaced"));
                        } else {
                            find_info.set(crate::i18n::t("editor.writer.no_matches"));
                        }
                    }
                },
                on_replace_all: {
                    let mut schedule_save = schedule_save.clone();
                    let bus = toast_bus;
                    move |(find, replace): (String, String)| {
                        if find.is_empty() { return; }
                        let mut count = 0u32;
                        while window_find(&find, false) {
                            exec_cmd("insertText", Some(&replace));
                            count += 1;
                            if count > 1000 { break; }
                        }
                        if count > 0 {
                            doc_body_text.set(editor_text());
                            schedule_save();
                        }
                        push_toast(bus, ToastKind::Success, crate::i18n::t("editor.writer.replaced_n").replace("{n}", &count.to_string()));
                    }
                },
            }

            CommentsPane { doc_id: id.clone(), open: comments_open }
            VersionHistory { doc_id: id.clone(), open: versions_open }
            crate::components::ai_assist::AiAssist { doc_id: id.clone() }
            ContextMenu {
                state: ctx_menu,
                on_command: {
                    let bus = toast_bus;
                    move |cmd: String| {
                        match cmd.as_str() {
                            "cut" => exec_cmd("cut", None),
                            "copy" => exec_cmd("copy", None),
                            "paste" => exec_cmd("paste", None),
                            "delete" => exec_cmd("delete", None),
                            "insert-link" => {
                                if let Some(window) = web_sys::window()
                                    && let Ok(Some(url)) = window.prompt_with_message(&crate::i18n::t("editor.writer.link_prompt"))
                                        && !url.is_empty() {
                                            exec_cmd("createLink", Some(&url));
                                        }
                            }
                            other => push_toast(bus, ToastKind::Info, crate::i18n::t("editor.coming_soon").replace("{what}", other)),
                        }
                    }
                },
            }

            {
                let _ = changes_tick.read();
                if *track_pane_open.read() {
                    let changes = list_pending_changes();
                    let track_title = crate::i18n::t("track.title");
                    let track_close = crate::i18n::t("track.close");
                    let track_accept_all = crate::i18n::t("track.accept_all");
                    let track_reject_all = crate::i18n::t("track.reject_all");
                    let track_empty = crate::i18n::t("track.empty");
                    let track_accept = crate::i18n::t("track.accept");
                    let track_reject = crate::i18n::t("track.reject");
                    rsx! {
                        aside { class: "track-changes-pane",
                            header { class: "track-changes-header",
                                strong { "{track_title}" }
                                button {
                                    class: "btn-icon",
                                    aria_label: "{track_close}",
                                    onclick: move |_| track_pane_open.set(false),
                                    "\u{2715}"
                                }
                            }
                            div { class: "track-changes-actions",
                                button {
                                    class: "btn btn-sm",
                                    onclick: move |_| {
                                        accept_all_changes();
                                        let v = *changes_tick.read();
                                        changes_tick.set(v.wrapping_add(1));
                                    },
                                    "{track_accept_all}"
                                }
                                button {
                                    class: "btn btn-sm",
                                    onclick: move |_| {
                                        reject_all_changes();
                                        let v = *changes_tick.read();
                                        changes_tick.set(v.wrapping_add(1));
                                    },
                                    "{track_reject_all}"
                                }
                            }
                            div { class: "track-changes-list",
                                if changes.is_empty() {
                                    p { class: "muted", "{track_empty}" }
                                }
                                for ch in changes.iter().cloned() {
                                    {
                                        let accept_id = ch.dom_id.clone();
                                        let reject_id = ch.dom_id.clone();
                                        let track_accept = track_accept.clone();
                                        let track_reject = track_reject.clone();
                                        rsx! {
                                            div { key: "{ch.dom_id}", class: "track-changes-item",
                                                span { class: "track-changes-kind", "{ch.kind}" }
                                                span { class: "track-changes-text", "{ch.text}" }
                                                span { class: "muted", "{ch.author}" }
                                                div { class: "track-changes-buttons",
                                                    button {
                                                        class: "btn-sm",
                                                        aria_label: "{track_accept}",
                                                        onclick: move |_| {
                                                            accept_change(&accept_id);
                                                            let v = *changes_tick.read();
                                                            changes_tick.set(v.wrapping_add(1));
                                                        },
                                                        "\u{2713}"
                                                    }
                                                    button {
                                                        class: "btn-sm",
                                                        aria_label: "{track_reject}",
                                                        onclick: move |_| {
                                                            reject_change(&reject_id);
                                                            let v = *changes_tick.read();
                                                            changes_tick.set(v.wrapping_add(1));
                                                        },
                                                        "\u{2715}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    rsx! {}
                }
            }
        }
    }
}

#[cfg(test)]
mod collaboration_tests {
    use super::*;
    use crate::collab_client::CollabMessage;

    #[test]
    fn text_save_preserves_rich_blocks_and_unaffected_run_formatting() {
        let mut doc = contrix_doc_engine::WriterDocument::new("Report".into());
        doc.add_paragraph("Hello 世界");
        doc.body
            .push(contrix_doc_engine::writer::Block::HorizontalRule);
        doc.body.push(contrix_doc_engine::writer::Block::PageBreak);
        doc.add_heading("Appendix", 2);
        let mut base = serde_json::to_value(doc).unwrap();
        base["body"][0]["runs"][0]["bold"] = serde_json::json!(true);
        base["professional"]["header"] = serde_json::json!("Private");
        assert_eq!(
            writer_content_with_text(&base, &writer_content_to_plain(&base)),
            base
        );
        let edited = writer_content_with_text(&base, "Hello 新世界\n\u{000c}\nAppendix");
        assert_eq!(edited["body"][0]["runs"][0]["bold"], true);
        assert_eq!(edited["body"][1], base["body"][1]);
        assert_eq!(edited["body"][3], base["body"][3]);
        assert_eq!(edited["professional"], base["professional"]);
        serde_json::from_value::<contrix_doc_engine::WriterDocument>(edited).unwrap();
    }

    #[test]
    fn newly_typed_text_is_valid_engine_content() {
        let base =
            serde_json::to_value(contrix_doc_engine::WriterDocument::new("New".into())).unwrap();
        let saved = writer_content_with_text(&base, "One\nTwo");
        let doc: contrix_doc_engine::WriterDocument = serde_json::from_value(saved).unwrap();
        assert_eq!(doc.body.len(), 2);
        let mut first = plain_writer_run("Hello ");
        first["bold"] = serde_json::json!(true);
        let mut second = plain_writer_run("世界");
        second["italic"] = serde_json::json!(true);
        let runs = replace_writer_runs(&[first, second], "Hello 新世界");
        assert_eq!(extract_run_text(&runs), "Hello 新世界");
        assert_eq!(runs.last().unwrap()["italic"], true);
    }

    #[test]
    fn server_operation_message_restores_writer_text() {
        let message: CollabMessage = serde_json::from_value(serde_json::json!({
            "type": "Operation",
            "id": {
                "counter": 7,
                "client_id": "4fd6e510-cf0c-4693-a623-f7a11bb62eef"
            },
            "timestamp": "2026-07-29T10:00:00Z",
            "user_id": "70b7564f-a186-4cec-bdb2-adcb94cb95cb",
            "base_version": 3,
            "op_type": {
                "type": "Replace",
                "path": ["body_text"],
                "old_value": "before",
                "new_value": "after"
            }
        }))
        .unwrap();

        let CollabMessage::Operation(payload) = message else {
            panic!("expected operation message");
        };
        assert_eq!(
            writer_replace_from_operation(&payload),
            Some((
                "70b7564f-a186-4cec-bdb2-adcb94cb95cb".to_string(),
                "after".to_string()
            ))
        );
    }
}
