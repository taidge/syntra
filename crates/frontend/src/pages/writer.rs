use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_timers::callback::Timeout;
use serde::Deserialize;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{MessageEvent, WebSocket};

use crate::collab_client::{CollabMessage, RemoteCursor, connect_collab};
use crate::components::navbar::Navbar;
use crate::components::sidebar::Sidebar;
use crate::state::{AuthState, api_get, api_put};

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
    if let Some(obj) = v.as_object() {
        if let Some(o) = obj.get("offset").and_then(|x| x.as_u64()) {
            return Some(o as u32);
        }
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
        if let Some(el) = doc.get_element_by_id(WRITER_BODY_ID) {
            if let Some(html_el) = el.dyn_ref::<web_sys::HtmlElement>() {
                let _ = html_el.focus();
            }
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
    let mut out = String::new();
    for block in &doc.body {
        let runs = block
            .get("runs")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        if !runs.is_empty() {
            out.push_str(&extract_run_text(&runs));
            out.push('\n');
        }
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Rich-text document editor (word processor).
#[component]
pub fn Writer(id: String) -> Element {
    let mut doc_title = use_signal(|| "Untitled Document".to_string());
    let mut word_count = use_signal(|| 0u32);
    let mut char_count = use_signal(|| 0u32);
    let mut page_count = use_signal(|| 1u32);
    let mut saved_status = use_signal(|| "All changes saved".to_string());
    let mut show_sidebar = use_signal(|| true);
    let mut doc_body_text = use_signal(String::new);
    let mut initial_html = use_signal(String::new);
    let mut loaded = use_signal(|| false);

    let auth = use_context::<Signal<AuthState>>();
    let presence = use_signal(Vec::<RemoteCursor>::new);
    let mut ws_handle = use_signal(|| None::<WebSocket>);

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
            let text = writer_content_to_plain(&doc.content);
            let wc = text.split_whitespace().count() as u32;
            let cc = text.len() as u32;
            let html = text
                .split('\n')
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
                .collect::<String>();
            initial_html.set(html);
            doc_body_text.set(text);
            word_count.set(wc);
            char_count.set(cc);
            let _ = page_count;
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
        match connect_collab(&doc_id, &token) {
            Ok(ws) => {
                let mut presence_for_msg = presence;
                let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(
                    move |evt: MessageEvent| {
                        if let Some(text) = evt.data().as_string() {
                            if let Ok(msg) = serde_json::from_str::<CollabMessage>(&text) {
                                match msg {
                                    CollabMessage::UserJoined { user_id, username, color } => {
                                        let mut list = presence_for_msg.write();
                                        if !list.iter().any(|c| c.user_id == user_id) {
                                            list.push(RemoteCursor {
                                                user_id,
                                                username,
                                                color,
                                                is_active: true,
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
                                            let Some(uid) = cursor_user_id(&parsed.user_id) else { return; };
                                            if uid == self_id {
                                                return;
                                            }
                                            let off = parsed
                                                .position
                                                .as_ref()
                                                .and_then(cursor_offset);
                                            let mut list = presence_for_msg.write();
                                            if let Some(existing) = list.iter_mut().find(|c| c.user_id == uid) {
                                                if off.is_some() { existing.position = off; }
                                                if let Some(u) = parsed.username { if existing.username.is_empty() { existing.username = u; } }
                                                if let Some(c) = parsed.color { if existing.color.is_empty() { existing.color = c; } }
                                            } else {
                                                list.push(RemoteCursor {
                                                    user_id: uid,
                                                    username: parsed.username.unwrap_or_default(),
                                                    color: parsed.color.unwrap_or_else(|| "#888".into()),
                                                    is_active: true,
                                                    position: off,
                                                });
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    },
                );
                ws.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
                onmessage.forget();
                ws_handle.set(Some(ws));
            }
            Err(_) => {
                ws_handle.set(None);
            }
        }
    });

    let ws_for_selection = ws_handle;
    let user_id_for_selection = local_user_id.clone();
    use_effect(move || {
        let Some(window) = web_sys::window() else { return; };
        let Some(document) = window.document() else { return; };
        let last_sent: Rc<RefCell<f64>> = Rc::new(RefCell::new(0.0));
        let perf = window.performance();
        let self_id = user_id_for_selection.clone();
        let ws_signal = ws_for_selection;
        let cb = Closure::<dyn FnMut()>::new(move || {
            let Some(ws) = ws_signal.read().clone() else { return; };
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
            let Some(off) = caret_offset_in_body() else { return; };
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
        if let Some(ws) = ws_handle.write().take() {
            let _ = ws.close();
        }
    });

    let save_timer: Rc<RefCell<Option<Timeout>>> = use_hook(|| Rc::new(RefCell::new(None)));

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
        move || {
            if !*loaded.read() {
                return;
            }
            saved_status.set("Unsaved changes".to_string());
            let body = serde_json::json!({
                "body": [{"runs": [{"text": doc_body_text.read().clone()}]}]
            });
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
            let body = serde_json::json!({
                "body": [{"runs": [{"text": doc_body_text.read().clone()}]}]
            });
            let token = auth.read().token.clone().unwrap_or_default();
            perform_save(body, saved_status, token);
        }
    };

    rsx! {
        Navbar {}
        div {
            class: "editor-container",
            tabindex: "0",
            onkeydown: {
                let mut flush_save = flush_save.clone();
                let mut schedule_save = schedule_save.clone();
                move |evt: KeyboardEvent| {
                    if !evt.modifiers().contains(Modifiers::CONTROL) {
                        return;
                    }
                    if let Key::Character(ref ch) = evt.key() {
                        match ch.as_str() {
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
                            _ => {}
                        }
                    }
                }
            },
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
                    placeholder: "Untitled Document",
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
                    if *show_sidebar.read() { "Hide Sidebar" } else { "Show Sidebar" }
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
                rsx! {
                    div { class: "toolbar",
                        div { class: "toolbar-group",
                            select {
                                class: "toolbar-select font-family",
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
                                class: "toolbar-btn", title: "Bold (Ctrl+B)",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_b.clone(); r("bold", None); },
                                "B"
                            }
                            button {
                                class: "toolbar-btn italic", title: "Italic (Ctrl+I)",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_i.clone(); r("italic", None); },
                                "I"
                            }
                            button {
                                class: "toolbar-btn underline", title: "Underline (Ctrl+U)",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_u.clone(); r("underline", None); },
                                "U"
                            }
                            button {
                                class: "toolbar-btn strikethrough", title: "Strikethrough",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_s.clone(); r("strikeThrough", None); },
                                "S"
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button {
                                class: "toolbar-btn", title: "Bulleted List",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_ul.clone(); r("insertUnorderedList", None); },
                                "* List"
                            }
                            button {
                                class: "toolbar-btn", title: "Numbered List",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_ol.clone(); r("insertOrderedList", None); },
                                "1. List"
                            }
                            button {
                                class: "toolbar-btn", title: "Indent",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_in.clone(); r("indent", None); },
                                ">>"
                            }
                            button {
                                class: "toolbar-btn", title: "Outdent",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_out.clone(); r("outdent", None); },
                                "<<"
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            button {
                                class: "toolbar-btn", title: "Align Left",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_jl.clone(); r("justifyLeft", None); },
                                "L"
                            }
                            button {
                                class: "toolbar-btn", title: "Align Center",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_jc.clone(); r("justifyCenter", None); },
                                "C"
                            }
                            button {
                                class: "toolbar-btn", title: "Align Right",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_jr.clone(); r("justifyRight", None); },
                                "R"
                            }
                            button {
                                class: "toolbar-btn", title: "Justify",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                onclick: move |_| { let mut r = run_jf.clone(); r("justifyFull", None); },
                                "J"
                            }
                        }
                        div { class: "toolbar-separator" }
                        div { class: "toolbar-group",
                            input {
                                r#type: "color", class: "toolbar-color", title: "Text Color", value: "#000000",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                oninput: move |e: FormEvent| { let mut r = run_fore.clone(); r("foreColor", Some(e.value())); },
                            }
                            input {
                                r#type: "color", class: "toolbar-color", title: "Highlight Color", value: "#ffff00",
                                onmousedown: move |evt: MouseEvent| { evt.prevent_default(); },
                                oninput: move |e: FormEvent| {
                                    let val = e.value();
                                    let mut r = run_hi.clone();
                                    r("hiliteColor", Some(val.clone()));
                                    r("backColor", Some(val));
                                },
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

                div { class: "editor-canvas writer-canvas",
                    match doc_resource.read().as_ref() {
                        None => rsx! { div { class: "editor-loading", "Loading document..." } },
                        Some(Err(e)) => rsx! { div { class: "editor-error", "Failed to load: {e}" } },
                        Some(Ok(_)) => rsx! {
                            div { class: "page",
                                div {
                                    id: WRITER_BODY_ID,
                                    contenteditable: "true",
                                    class: "writer-content",
                                    dangerous_inner_html: "{initial_html}",
                                    oninput: {
                                        let mut schedule_save = schedule_save.clone();
                                        move |_evt: FormEvent| {
                                            let text = editor_text();
                                            let wc = text.split_whitespace().count() as u32;
                                            let cc = text.len() as u32;
                                            word_count.set(wc);
                                            char_count.set(cc);
                                            doc_body_text.set(text);
                                            schedule_save();
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
            div { class: "status-bar",
                span { "Page {page_count} of {page_count}" }
                span { "Words: {word_count}" }
                span { "Characters: {char_count}" }
                span { "Document ID: {id}" }
            }
        }
    }
}
