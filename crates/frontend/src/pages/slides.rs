use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_timers::callback::{Interval, Timeout};
use serde::Deserialize;

use crate::Route;
use crate::collab_client::{CollabHandle, CollabMessage, ConnectionState, connect_with_reconnect};
use crate::components::comments_pane::CommentsPane;
use crate::components::context_menu::{ContextMenu, MenuEntry};
use crate::components::menubar::Menubar;
use crate::components::navbar::Navbar;
use crate::components::toast::{Toast, ToastKind, push_toast};
use crate::components::version_history::VersionHistory;
use crate::state::{AuthState, api_get, api_put, export_document};

#[derive(Clone, Deserialize)]
struct DocumentResponse {
    name: String,
    content: serde_json::Value,
}

#[derive(Deserialize, Default)]
struct SlideDto {
    #[serde(default)]
    id: serde_json::Value,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    notes: String,
    #[serde(default = "default_transition")]
    transition: serde_json::Value,
    #[serde(default = "default_bg")]
    bg_color: String,
    #[serde(default = "default_layout")]
    layout: String,
    #[serde(default)]
    elements: Vec<serde_json::Value>,
    #[serde(default)]
    background: serde_json::Value,
}

fn default_transition() -> serde_json::Value {
    serde_json::json!("none")
}
fn default_bg() -> String {
    "#ffffff".into()
}
fn default_layout() -> String {
    "title-body".into()
}

#[derive(Deserialize)]
struct PresentationLite {
    #[serde(default)]
    slides: Vec<SlideDto>,
}

fn text_from_element(element: &serde_json::Value) -> Option<String> {
    (element.get("type")?.as_str()? == "TextBox").then(|| {
        element
            .get("content")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|run| run.get("text").and_then(|text| text.as_str()))
            .collect::<String>()
    })
}

fn hydrate_presentation(content: &serde_json::Value) -> Vec<SlideData> {
    let parsed = serde_json::from_value::<PresentationLite>(content.clone()).ok();
    let Some(presentation) = parsed else {
        return vec![SlideData::empty(0)];
    };
    let slides = presentation
        .slides
        .into_iter()
        .enumerate()
        .map(|(i, slide)| {
            let native_id = slide
                .id
                .as_str()
                .filter(|id| id.parse::<uuid::Uuid>().is_ok())
                .map(str::to_string)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let source = content
                .get("slides")
                .and_then(|value| value.as_array())
                .and_then(|items| items.get(i))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let texts: Vec<String> = slide
                .elements
                .iter()
                .filter_map(text_from_element)
                .collect();
            let background = slide
                .background
                .get("Solid")
                .and_then(|value| value.as_str())
                .unwrap_or(&slide.bg_color)
                .to_string();
            let transition = slide
                .transition
                .as_str()
                .map(str::to_string)
                .or_else(|| {
                    slide
                        .transition
                        .get("transition_type")
                        .and_then(|value| value.as_str())
                        .map(|value| value.to_ascii_lowercase())
                })
                .unwrap_or_else(|| "none".into());
            let layout = match slide.layout.as_str() {
                "Blank" => "blank",
                "TitleOnly" => "title-only",
                "TwoColumn" => "two-column",
                "SectionHeader" => "section-header",
                "TitleSlide" => "title-slide",
                "TitleAndContent" => "title-body",
                other => other,
            }
            .to_string();
            SlideData {
                id: i,
                native_id,
                source,
                title: if slide.title.is_empty() {
                    texts.first().cloned().unwrap_or_default()
                } else {
                    slide.title
                },
                body: if slide.body.is_empty() {
                    texts.get(1).cloned().unwrap_or_default()
                } else {
                    slide.body
                },
                notes: slide.notes,
                transition,
                bg_color: background,
                layout,
            }
        })
        .collect::<Vec<_>>();
    if slides.is_empty() {
        vec![SlideData::empty(0)]
    } else {
        slides
    }
}

fn canonical_layout(layout: &str) -> &'static str {
    match layout {
        "blank" => "Blank",
        "title-only" => "TitleOnly",
        "two-column" => "TwoColumn",
        "section-header" => "SectionHeader",
        "title-slide" => "TitleSlide",
        _ => "TitleAndContent",
    }
}

fn canonical_transition(transition: &str) -> serde_json::Value {
    if transition == "none" {
        serde_json::Value::Null
    } else {
        let name = match transition {
            "fade" => "Fade",
            "slide" => "Slide",
            "zoom" => "Zoom",
            "flip" => "Flip",
            "push" => "Push",
            "dissolve" => "Dissolve",
            _ => "None",
        };
        serde_json::json!({"transition_type": name, "duration_ms": 500})
    }
}

fn text_box(id: serde_json::Value, text: &str, title: bool) -> serde_json::Value {
    serde_json::json!({
        "type": "TextBox", "id": id,
        "x": 120.0, "y": if title { 100.0 } else { 300.0 },
        "width": 1680.0, "height": if title { 160.0 } else { 600.0 }, "rotation": 0.0,
        "content": [{"text": text, "bold": title, "italic": false, "underline": false,
            "strikethrough": false, "font_family": null,
            "font_size": if title { 44.0 } else { 24.0 }, "color": null, "link": null}],
        "background_color": null, "border": null, "padding": 8.0
    })
}

fn merge_presentation(
    base: &serde_json::Value,
    slides: &[SlideData],
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
    object.entry("master").or_insert_with(|| {
        serde_json::json!({
            "background": {"Solid":"#FFFFFF"}, "default_font":"Arial", "default_font_size":18.0,
            "title_style":null, "body_style":null
        })
    });
    object.entry("theme").or_insert_with(|| {
        serde_json::json!({
            "name":"Contrix Light", "primary_color":"#2563EB", "secondary_color":"#0F172A",
            "accent_colors":["#2563EB","#7C3AED","#059669","#D97706","#DC2626","#0891B2"],
            "heading_font":"Aptos Display", "body_font":"Aptos"
        })
    });
    object.entry("metadata").or_insert_with(|| {
        serde_json::json!({
            "author":null, "created_at":chrono::Utc::now(), "modified_at":chrono::Utc::now(),
            "slide_width":1920.0, "slide_height":1080.0
        })
    });
    if let Some(metadata) = object
        .get_mut("metadata")
        .and_then(|value| value.as_object_mut())
    {
        metadata.insert("modified_at".into(), serde_json::json!(chrono::Utc::now()));
    }
    let previous = base.get("slides").and_then(|value| value.as_array());
    let canonical = slides.iter().map(|slide| {
        let old = previous.and_then(|items| items.iter().find(|item| item.get("id").and_then(|value| value.as_str()) == Some(slide.native_id.as_str())))
            .or_else(|| slide.source.is_object().then_some(&slide.source));
        let old_elements = old.and_then(|value| value.get("elements")).and_then(|value| value.as_array());
        let mut replaced_text = 0usize;
        let mut non_text = Vec::new();
        for element in old_elements.into_iter().flatten() {
            if element.get("type").and_then(|value| value.as_str()) == Some("TextBox")
                && replaced_text < 2
            {
                replaced_text += 1;
            } else {
                non_text.push(element.clone());
            }
        }
        let old_text_ids = old_elements.into_iter().flatten()
            .filter(|element| element.get("type").and_then(|value| value.as_str()) == Some("TextBox"))
            .filter_map(|element| element.get("id").cloned()).collect::<Vec<_>>();
        let title_id = old_text_ids.first().cloned().unwrap_or_else(|| serde_json::json!(uuid::Uuid::new_v4()));
        let body_id = old_text_ids.get(1).cloned().unwrap_or_else(|| serde_json::json!(uuid::Uuid::new_v4()));
        let mut elements = vec![text_box(title_id, &slide.title, true), text_box(body_id, &slide.body, false)];
        elements.append(&mut non_text);
        serde_json::json!({
            "id": slide.native_id,
            "elements": elements, "background": {"Solid": slide.bg_color},
            "transition": canonical_transition(&slide.transition),
            "animations": old.and_then(|v| v.get("animations")).cloned().unwrap_or_else(|| serde_json::json!([])),
            "notes": slide.notes, "layout": canonical_layout(&slide.layout)
        })
    }).collect::<Vec<_>>();
    object.insert("slides".into(), serde_json::Value::Array(canonical));
    result
}

fn presentation_replace_from_operation(
    payload: &serde_json::Value,
) -> Option<(String, serde_json::Value)> {
    let user = payload.get("user_id")?.as_str()?.to_string();
    let operation = payload.get("op_type")?;
    if operation.get("type")?.as_str()? != "Replace" {
        return None;
    }
    let path = operation.get("path")?.as_array()?;
    if path.len() != 1 || path[0].as_str()? != "presentation" {
        return None;
    }
    Some((user, operation.get("new_value")?.clone()))
}

fn append_slide_element(
    content: &mut serde_json::Value,
    slide: usize,
    element: serde_json::Value,
) -> bool {
    let Some(elements) = content
        .get_mut("slides")
        .and_then(|value| value.as_array_mut())
        .and_then(|slides| slides.get_mut(slide))
        .and_then(|slide| slide.get_mut("elements"))
        .and_then(|elements| elements.as_array_mut())
    else {
        return false;
    };
    elements.push(element);
    true
}

fn add_default_animation(content: &mut serde_json::Value, slide: usize) -> bool {
    let Some(slide) = content
        .get_mut("slides")
        .and_then(|value| value.as_array_mut())
        .and_then(|slides| slides.get_mut(slide))
    else {
        return false;
    };
    let Some(target) = slide
        .get("elements")
        .and_then(|value| value.as_array())
        .and_then(|elements| elements.first())
        .and_then(|element| element.get("id"))
        .cloned()
    else {
        return false;
    };
    let Some(object) = slide.as_object_mut() else {
        return false;
    };
    let animations = object
        .entry("animations")
        .or_insert_with(|| serde_json::json!([]));
    let Some(animations) = animations.as_array_mut() else {
        return false;
    };
    animations.push(serde_json::json!({
        "target_id":target, "effect":"Fade", "trigger":"OnClick",
        "duration_ms":500, "delay_ms":0, "order":animations.len() as u32 + 1
    }));
    true
}

fn set_presentation_theme(content: &mut serde_json::Value, name: &str) {
    let theme = match name {
        "Midnight" => {
            serde_json::json!({"name":"Midnight","primary_color":"#38BDF8","secondary_color":"#020617","accent_colors":["#38BDF8","#A78BFA","#34D399","#FBBF24","#FB7185","#22D3EE"],"heading_font":"Aptos Display","body_font":"Aptos"})
        }
        "Editorial" => {
            serde_json::json!({"name":"Editorial","primary_color":"#B91C1C","secondary_color":"#292524","accent_colors":["#B91C1C","#C2410C","#A16207","#4D7C0F","#0F766E","#1D4ED8"],"heading_font":"Georgia","body_font":"Aptos"})
        }
        _ => {
            serde_json::json!({"name":"Contrix Light","primary_color":"#2563EB","secondary_color":"#0F172A","accent_colors":["#2563EB","#7C3AED","#059669","#D97706","#DC2626","#0891B2"],"heading_font":"Aptos Display","body_font":"Aptos"})
        }
    };
    if let Some(object) = content.as_object_mut() {
        object.insert("theme".into(), theme);
    }
}

fn render_slide_objects(content: &serde_json::Value, index: usize) -> Element {
    let elements = content
        .get("slides")
        .and_then(|value| value.as_array())
        .and_then(|slides| slides.get(index))
        .and_then(|slide| slide.get("elements"))
        .and_then(|elements| elements.as_array())
        .cloned()
        .unwrap_or_default();
    rsx! {
        for (index, element) in elements.iter().enumerate() {
            {
                let kind = element.get("type").and_then(|value| value.as_str()).unwrap_or("");
                let x = element.get("x").and_then(|value| value.as_f64()).unwrap_or(0.0) / 19.2;
                let y = element.get("y").and_then(|value| value.as_f64()).unwrap_or(0.0) / 10.8;
                let width = element.get("width").and_then(|value| value.as_f64()).unwrap_or(500.0) / 19.2;
                let height = element.get("height").and_then(|value| value.as_f64()).unwrap_or(300.0) / 10.8;
                let position = format!("position:absolute;left:{x}%;top:{y}%;width:{width}%;height:{height}%;");
                match kind {
                    "Image" => {
                        let url = element.get("url").and_then(|value| value.as_str()).unwrap_or("").to_string();
                        let alt = element.get("alt_text").and_then(|value| value.as_str()).unwrap_or("").to_string();
                        rsx! { img { style: "{position}object-fit:contain;", src: "{url}", alt: "{alt}" } }
                    }
                    "Video" => {
                        let url = element.get("url").and_then(|value| value.as_str()).unwrap_or("").to_string();
                        rsx! { video { style: "{position}", src: "{url}", controls: true } }
                    }
                    "Shape" => {
                        let fill = element.get("fill_color").and_then(|value| value.as_str()).unwrap_or("#DBEAFE");
                        rsx! { div { style: "{position}background:{fill};border:2px solid #2563EB;border-radius:12px;" } }
                    }
                    "TextBox" if index >= 2 => {
                        let text = text_from_element(element).unwrap_or_default();
                        rsx! { div { style: "{position}white-space:pre-wrap;", "{text}" } }
                    }
                    "Chart" => {
                        let labels = element.pointer("/data/labels").and_then(|value| value.as_array()).cloned().unwrap_or_default();
                        let values = element.pointer("/data/datasets/0/values").and_then(|value| value.as_array()).cloned().unwrap_or_default();
                        let maximum = values.iter().filter_map(|value| value.as_f64()).map(f64::abs).fold(1.0, f64::max);
                        rsx! { div { style: "{position}display:flex;gap:8px;align-items:flex-end;padding:16px;background:#fff;border:1px solid #cbd5e1;",
                            for (column, value) in values.iter().enumerate() {
                                {
                                    let value = value.as_f64().unwrap_or(0.0);
                                    let percent = value.abs() / maximum * 85.0;
                                    let label = labels.get(column).and_then(|value| value.as_str()).unwrap_or("");
                                    rsx! { div { style: "flex:1;display:flex;flex-direction:column;align-items:center;height:100%;justify-content:flex-end;",
                                        span { style: "font-size:11px;color:#111827;", "{value}" }
                                        div { style: "width:80%;height:{percent}%;background:#2563EB;min-height:2px;" }
                                        span { style: "font-size:11px;color:#111827;", "{label}" }
                                    } }
                                }
                            }
                        } }
                    }
                    _ => rsx! {},
                }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
struct SlideData {
    id: usize,
    native_id: String,
    source: serde_json::Value,
    title: String,
    body: String,
    notes: String,
    transition: String,
    bg_color: String,
    layout: String,
}

impl SlideData {
    fn empty(id: usize) -> Self {
        Self {
            id,
            native_id: uuid::Uuid::new_v4().to_string(),
            source: serde_json::Value::Null,
            title: String::new(),
            body: String::new(),
            notes: String::new(),
            transition: "none".into(),
            bg_color: "#ffffff".into(),
            layout: "title-body".into(),
        }
    }
}

#[component]
pub fn Slides(id: String) -> Element {
    let _ = use_context::<Signal<String>>().read();
    let mut slides = use_signal(|| vec![SlideData::empty(0)]);
    let mut current_slide = use_signal(|| 0usize);
    let mut doc_title = use_signal(|| crate::i18n::t("editor.slides.untitled"));
    let mut saved_status = use_signal(|| crate::i18n::t("common.saved"));
    let mut loaded = use_signal(|| false);
    let mut drag_index = use_signal(|| None::<usize>);
    let mut present_mode = use_signal(|| false);
    let mut ctx_menu = use_signal(|| None::<(f64, f64, Vec<MenuEntry>)>);
    let mut comments_open = use_signal(|| false);
    let mut versions_open = use_signal(|| false);
    let mut layouts_open = use_signal(|| false);
    let mut presenter_view = use_signal(|| false);
    let mut presentation_started = use_signal(|| None::<f64>);
    let mut elapsed_seconds = use_signal(|| 0u64);
    let _presentation_clock = use_hook(move || {
        Rc::new(Interval::new(1000, move || {
            if let Some(started) = *presentation_started.read() {
                elapsed_seconds.set(((js_sys::Date::now() - started) / 1000.0).max(0.0) as u64);
            }
        }))
    });
    let mut document_content = use_signal(|| serde_json::json!({}));
    let mut last_collab_content = use_signal(|| serde_json::Value::Null);
    let mut ws_handle = use_signal(|| None::<CollabHandle>);
    let mut conn_state = use_signal(|| ConnectionState::Connecting);

    let auth = use_context::<Signal<AuthState>>();
    let toast_bus = use_context::<Signal<Vec<Toast>>>();
    let nav = use_navigator();
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
            let new_slides = hydrate_presentation(&doc.content);
            slides.set(new_slides);
            document_content.set(doc.content.clone());
            last_collab_content.set(doc.content.clone());
            current_slide.set(0);
            loaded.set(true);
        }
    });

    let id_for_ws = id.clone();
    use_effect(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        let self_id = auth
            .read()
            .user
            .as_ref()
            .map(|user| user.id.to_string())
            .unwrap_or_default();
        let on_message = move |message: CollabMessage| {
            let content = match message {
                CollabMessage::Operation(payload) => {
                    let Some((author, content)) = presentation_replace_from_operation(&payload)
                    else {
                        return;
                    };
                    if author == self_id {
                        return;
                    }
                    if *saved_status.read() == crate::i18n::t("common.unsaved") {
                        push_toast(
                            toast_bus,
                            ToastKind::Info,
                            "Concurrent slide edit received; the latest remote version is shown.",
                        );
                    }
                    content
                }
                CollabMessage::DocumentState { content, .. } if content.is_object() => {
                    if *saved_status.read() == crate::i18n::t("common.unsaved") {
                        return;
                    }
                    content
                }
                CollabMessage::Error { message } => {
                    push_toast(toast_bus, ToastKind::Error, message);
                    return;
                }
                _ => return,
            };
            slides.set(hydrate_presentation(&content));
            document_content.set(content.clone());
            last_collab_content.set(content);
            let next_index = (*current_slide.read()).min(slides.read().len().saturating_sub(1));
            current_slide.set(next_index);
            saved_status.set(crate::i18n::t("common.saved"));
        };
        let on_state = move |state| conn_state.set(state);
        ws_handle.set(Some(connect_with_reconnect(
            id_for_ws.clone(),
            token,
            on_message,
            on_state,
        )));
    });

    use_drop(move || {
        if let Some(handle) = ws_handle.write().take() {
            handle.close();
        }
    });

    let save_timer: Rc<RefCell<Option<Timeout>>> = use_hook(|| Rc::new(RefCell::new(None)));

    let serialize_slides = move || -> serde_json::Value {
        merge_presentation(&document_content.read(), &slides.read(), &doc_title.read())
    };

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

    let mut broadcast_presentation = move |body: &serde_json::Value| {
        let previous = last_collab_content.read().clone();
        if previous == *body {
            return;
        }
        let user_id = auth
            .read()
            .user
            .as_ref()
            .map(|user| user.id.to_string())
            .unwrap_or_default();
        if user_id.is_empty() {
            return;
        }
        let operation = serde_json::json!({
            "type":"Replace", "path":["presentation"],
            "old_value":previous, "new_value":body
        });
        if let Some(handle) = ws_handle.read().as_ref()
            && handle.send_operation(&user_id, operation).is_ok()
        {
            last_collab_content.set(body.clone());
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
            let body = serialize_slides();
            document_content.set(body.clone());
            broadcast_presentation(&body);
            let token = auth.read().token.clone().unwrap_or_default();
            let status = saved_status;
            let perform_save = perform_save.clone();
            let new_timer = Timeout::new(1500, move || {
                perform_save(serialize_slides(), status, token);
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
            let body = serialize_slides();
            document_content.set(body.clone());
            broadcast_presentation(&body);
            let token = auth.read().token.clone().unwrap_or_default();
            perform_save(body, saved_status, token);
        }
    };

    let mut exit_present = move || {
        present_mode.set(false);
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            doc.exit_fullscreen();
        }
    };

    let mut go_next = move || {
        let len = slides.read().len();
        let cur = *current_slide.read();
        if cur + 1 < len {
            current_slide.set(cur + 1);
        }
    };

    let mut go_prev = move || {
        let cur = *current_slide.read();
        if cur > 0 {
            current_slide.set(cur - 1);
        }
    };

    let slide_count = slides.read().len();
    let cur_idx = *current_slide.read();
    let cur_title = slides
        .read()
        .get(cur_idx)
        .map(|s| s.title.clone())
        .unwrap_or_default();
    let cur_body = slides
        .read()
        .get(cur_idx)
        .map(|s| s.body.clone())
        .unwrap_or_default();
    let cur_notes = slides
        .read()
        .get(cur_idx)
        .map(|s| s.notes.clone())
        .unwrap_or_default();
    let cur_bg = slides
        .read()
        .get(cur_idx)
        .map(|s| s.bg_color.clone())
        .unwrap_or_else(|| "#ffffff".into());
    let cur_transition = slides
        .read()
        .get(cur_idx)
        .map(|s| s.transition.clone())
        .unwrap_or_else(|| "none".into());
    let cur_layout = slides
        .read()
        .get(cur_idx)
        .map(|s| s.layout.clone())
        .unwrap_or_else(|| "title-body".into());
    let next_title = slides
        .read()
        .get(cur_idx + 1)
        .map(|s| s.title.clone())
        .unwrap_or_default();
    let next_body = slides
        .read()
        .get(cur_idx + 1)
        .map(|s| s.body.clone())
        .unwrap_or_default();
    let current_theme = document_content
        .read()
        .pointer("/theme/name")
        .and_then(|value| value.as_str())
        .unwrap_or("Contrix Light")
        .to_string();
    let heading_font = document_content
        .read()
        .pointer("/theme/heading_font")
        .and_then(|value| value.as_str())
        .unwrap_or("Aptos Display")
        .to_string();
    let theme_color = document_content
        .read()
        .pointer("/theme/primary_color")
        .and_then(|value| value.as_str())
        .unwrap_or("#2563EB")
        .to_string();

    rsx! {
        Navbar {}
        div {
            class: "editor-container",
            tabindex: "0",
            onkeydown: {
                let mut flush_save = flush_save.clone();
                move |evt: KeyboardEvent| {
                    if evt.modifiers().contains(Modifiers::CONTROL)
                        && evt.key() == Key::Character("s".to_string()) {
                            evt.prevent_default();
                            flush_save();
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
                    placeholder: crate::i18n::t("editor.slides.untitled"),
                }
                span { class: "save-status", "{saved_status}" }
                button {
                    class: "btn-icon",
                    onclick: move |_| {
                        let cur = *layouts_open.read();
                        layouts_open.set(!cur);
                    },
                    {crate::i18n::t("editor.slides.layouts")}
                }
                button {
                    class: "present-btn",
                    onclick: move |_| {
                        present_mode.set(true);
                        presentation_started.set(Some(js_sys::Date::now()));
                        elapsed_seconds.set(0);
                        if let Some(doc) = web_sys::window().and_then(|w| w.document())
                            && let Some(el) = doc.get_element_by_id("present-stage") {
                                let _ = el.request_fullscreen();
                            }
                    },
                    {crate::i18n::t("editor.slides.present")}
                }
            }

            {
                let mut flush_for_menu = flush_save.clone();
                let mut schedule_for_menu = schedule_save.clone();
                let id_for_menu = id.clone();
                let bus_for_menu = toast_bus;
                let nav_for_menu = nav;
                rsx! {
                    Menubar {
                        document_type: "slides".to_string(),
                        on_command: move |cmd: String| {
                            let bus = bus_for_menu;
                            match cmd.as_str() {
                                "save" => flush_for_menu(),
                                "back" => { nav_for_menu.push(Route::Dashboard {}); }
                                "toggle-comments" => {
                                    let cur = *comments_open.read();
                                    comments_open.set(!cur);
                                }
                                "version-history" => {
                                    let cur = *versions_open.read();
                                    versions_open.set(!cur);
                                }
                                "presenter-view" => {
                                    let open = *presenter_view.read();
                                    presenter_view.set(!open);
                                    if !open {
                                        presentation_started.set(Some(js_sys::Date::now()));
                                        elapsed_seconds.set(0);
                                    }
                                }
                                "insert-slide" => {
                                    let next_id = slides.read().iter().map(|s| s.id).max().unwrap_or(0) + 1;
                                    slides.write().push(SlideData::empty(next_id));
                                    let new_idx = slides.read().len() - 1;
                                    current_slide.set(new_idx);
                                    schedule_for_menu();
                                }
                                "insert-textbox" => {
                                    if let Some(window) = web_sys::window()
                                        && let Ok(Some(text)) = window.prompt_with_message("Text box content")
                                        && !text.trim().is_empty()
                                    {
                                        let canonical = serialize_slides();
                                        document_content.set(canonical);
                                        let mut content = document_content.read().clone();
                                        append_slide_element(&mut content, *current_slide.read(), text_box(serde_json::json!(uuid::Uuid::new_v4()), &text, false));
                                        document_content.set(content);
                                        schedule_for_menu();
                                    }
                                }
                                "insert-image" => {
                                    if let Some(window) = web_sys::window()
                                        && let Ok(Some(url)) = window.prompt_with_message("Image URL or data URL")
                                        && !url.trim().is_empty()
                                    {
                                        let canonical = serialize_slides();
                                        document_content.set(canonical);
                                        let mut content = document_content.read().clone();
                                        append_slide_element(&mut content, *current_slide.read(), serde_json::json!({
                                            "type":"Image", "id":uuid::Uuid::new_v4(), "x":500.0, "y":250.0,
                                            "width":920.0, "height":520.0, "rotation":0.0, "url":url,
                                            "alt_text":"Inserted image", "crop":null
                                        }));
                                        document_content.set(content);
                                        schedule_for_menu();
                                    }
                                }
                                "insert-shape" => {
                                    let canonical = serialize_slides();
                                    document_content.set(canonical);
                                    let mut content = document_content.read().clone();
                                    append_slide_element(&mut content, *current_slide.read(), serde_json::json!({
                                        "type":"Shape", "id":uuid::Uuid::new_v4(), "x":700.0, "y":400.0,
                                        "width":520.0, "height":260.0, "rotation":0.0, "shape_type":"RoundedRectangle",
                                        "fill_color":"#DBEAFE", "border":{"color":"#2563EB","width":2.0}, "text":null
                                    }));
                                    document_content.set(content);
                                    schedule_for_menu();
                                }
                                "insert-animation" => {
                                    let canonical = serialize_slides();
                                    document_content.set(canonical);
                                    let mut content = document_content.read().clone();
                                    if add_default_animation(&mut content, *current_slide.read()) {
                                        document_content.set(content);
                                        schedule_for_menu();
                                        push_toast(bus, ToastKind::Success, "Fade animation added");
                                    }
                                }
                                "insert-chart" => {
                                    if let Some(window) = web_sys::window()
                                        && let Ok(Some(input)) = window.prompt_with_message_and_default("Chart data (label,value;label,value)", "Q1,10;Q2,15;Q3,12")
                                    {
                                        let values = input.split(';').filter_map(|entry| {
                                            let (label, value) = entry.split_once(',')?;
                                            Some((label.trim().to_string(), value.trim().parse::<f64>().ok()?))
                                        }).collect::<Vec<_>>();
                                        if !values.is_empty() {
                                            let canonical = serialize_slides();
                                            document_content.set(canonical);
                                            let mut content = document_content.read().clone();
                                            append_slide_element(&mut content, *current_slide.read(), serde_json::json!({
                                                "type":"Chart", "id":uuid::Uuid::new_v4(), "x":500.0,"y":300.0,"width":920.0,"height":520.0,
                                                "chart_type":"Bar", "data":{"labels":values.iter().map(|(label,_)| label.clone()).collect::<Vec<_>>(),
                                                    "datasets":[{"name":"Series 1","values":values.iter().map(|(_,value)| *value).collect::<Vec<_>>(),"color":"#2563EB"}]}
                                            }));
                                            document_content.set(content);
                                            schedule_for_menu();
                                        }
                                    }
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
                                "print" => {
                                    if let Some(w) = web_sys::window() { let _ = w.print(); }
                                }
                                "export-pdf" | "export-pptx" | "export-html" => {
                                    let format = match cmd.as_str() {
                                        "export-pdf" => "pdf",
                                        "export-pptx" => "pptx",
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

            {
                match *conn_state.read() {
                    ConnectionState::Reconnecting(attempt) => rsx! { div { class: "ws-banner ws-reconnecting", "Reconnecting collaboration ({attempt})…" } },
                    ConnectionState::Failed => rsx! { div { class: "ws-banner ws-failed", "Collaboration connection failed" } },
                    _ => rsx! {},
                }
            }

            match doc_resource.read().as_ref() {
                None => rsx! { div { class: "editor-loading", {crate::i18n::t("editor.loading_document")} } },
                Some(Err(e)) => rsx! { div { class: "editor-error", {crate::i18n::t("editor.failed_load").replace("{err}", &e.to_string())} } },
                Some(Ok(_)) => rsx! {},
            }

            div { class: "slides-editor",
                div { class: "slide-panel",
                    for i in 0..slide_count {
                        {
                            let mut schedule_save_drop = schedule_save.clone();
                            let mut schedule_save_del = schedule_save.clone();
                            rsx! {
                                div {
                                    key: "{slides.read()[i].id}-{i}",
                                    class: if cur_idx == i { "slide-thumbnail slide-thumb active" } else { "slide-thumbnail slide-thumb" },
                                    draggable: true,
                                    ondragstart: move |_| {
                                        drag_index.set(Some(i));
                                    },
                                    ondragover: move |e| {
                                        e.prevent_default();
                                    },
                                    ondrop: move |e| {
                                        e.prevent_default();
                                        if let Some(from) = *drag_index.read()
                                            && from != i {
                                                let mut list = slides.write();
                                                let item = list.remove(from);
                                                list.insert(i, item);
                                                drop(list);
                                                let cur = *current_slide.read();
                                                if cur == from {
                                                    current_slide.set(i);
                                                } else if from < cur && i >= cur {
                                                    current_slide.set(cur - 1);
                                                } else if from > cur && i <= cur {
                                                    current_slide.set(cur + 1);
                                                }
                                                schedule_save_drop();
                                            }
                                        drag_index.set(None);
                                    },
                                    onclick: move |_| current_slide.set(i),
                                    div { class: "slide-thumb-number", "{i + 1}" }
                                    div { class: "slide-thumb-preview",
                                        div { class: "thumb-content",
                                            if i == 0 {
                                                div { class: "thumb-title-bar" }
                                                div { class: "thumb-subtitle-bar" }
                                            } else {
                                                div { class: "thumb-body-bar" }
                                                div { class: "thumb-body-bar short" }
                                            }
                                        }
                                    }
                                    button {
                                        class: "slide-thumb-delete",
                                        onclick: move |e| {
                                            e.stop_propagation();
                                            let len = slides.read().len();
                                            if len <= 1 {
                                                return;
                                            }
                                            slides.write().remove(i);
                                            let cur = *current_slide.read();
                                            if cur >= slides.read().len() {
                                                current_slide.set(slides.read().len() - 1);
                                            } else if i < cur {
                                                current_slide.set(cur - 1);
                                            }
                                            schedule_save_del();
                                        },
                                        "×"
                                    }
                                }
                            }
                        }
                    }
                    button {
                        class: "add-slide",
                        onclick: {
                            let mut schedule_save = schedule_save.clone();
                            move |_| {
                                let next_id = slides.read().iter().map(|s| s.id).max().unwrap_or(0) + 1;
                                slides.write().push(SlideData::empty(next_id));
                                let new_idx = slides.read().len() - 1;
                                current_slide.set(new_idx);
                                schedule_save();
                            }
                        },
                        {crate::i18n::t("editor.slides.add_slide")}
                    }
                }

                div {
                    class: "slide-canvas",
                    "data-layout": "{cur_layout}",
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
                                MenuEntry::new(crate::i18n::t("ctx.duplicate_slide"), "duplicate-slide"),
                                MenuEntry::new(crate::i18n::t("ctx.delete_slide"), "delete-slide"),
                            ],
                        )));
                    },
                    div {
                        class: "slide-content",
                        style: "background-color: {cur_bg};position:relative;",

                        div {
                            class: "slide-title",
                            style: "font-family:{heading_font};color:{theme_color};",
                            contenteditable: "true",
                            oninput: {
                                let mut schedule_save = schedule_save.clone();
                                move |e: FormEvent| {
                                    if let Some(s) = slides.write().get_mut(cur_idx) {
                                        s.title = e.value();
                                    }
                                    schedule_save();
                                }
                            },
                            "{cur_title}"
                        }
                        div {
                            class: "slide-body",
                            contenteditable: "true",
                            oninput: {
                                let mut schedule_save = schedule_save.clone();
                                move |e: FormEvent| {
                                    if let Some(s) = slides.write().get_mut(cur_idx) {
                                        s.body = e.value();
                                    }
                                    schedule_save();
                                }
                            },
                            "{cur_body}"
                        }
                        {render_slide_objects(&document_content.read(), cur_idx)}
                    }
                }

                if *layouts_open.read() {
                    div { class: "layouts-rail",
                        for (kind, name_key) in [
                            ("title-only", "editor.slides.layout.title_only"),
                            ("title-body", "editor.slides.layout.title_body"),
                            ("two-column", "editor.slides.layout.two_column"),
                            ("blank", "editor.slides.layout.blank"),
                            ("section-header", "editor.slides.layout.section_header"),
                            ("image-title", "editor.slides.layout.image_title"),
                        ] {
                            {
                                let name = crate::i18n::t(name_key);
                                let mut schedule_save_l = schedule_save.clone();
                                let active = cur_layout == kind;
                                rsx! {
                                    button {
                                        class: if active { "layout-tile active" } else { "layout-tile" },
                                        onclick: move |_| {
                                            let cur = *current_slide.read();
                                            slides.with_mut(|sl| {
                                                if let Some(slide) = sl.get_mut(cur) {
                                                    slide.layout = kind.to_string();
                                                }
                                            });
                                            schedule_save_l();
                                        },
                                        div { class: "layout-tile-name", "{name}" }
                                        div { class: "layout-tile-preview" }
                                    }
                                }
                            }
                        }
                        button {
                            class: "btn btn-sm",
                            onclick: {
                                let mut schedule_save_all = schedule_save.clone();
                                move |_| {
                                    let cur_layout_now = slides.read().get(*current_slide.read())
                                        .map(|s| s.layout.clone())
                                        .unwrap_or_else(|| "title-body".into());
                                    slides.with_mut(|sl| {
                                        for slide in sl.iter_mut() {
                                            slide.layout = cur_layout_now.clone();
                                        }
                                    });
                                    schedule_save_all();
                                    layouts_open.set(false);
                                }
                            },
                            {crate::i18n::t("editor.slides.apply_to_all")}
                        }
                    }
                }

                div { class: "properties-panel",
                    h3 { {crate::i18n::t("editor.slides.properties")} }

                    div { class: "property-group",
                        label { "Presentation theme" }
                        select {
                            class: "property-select",
                            value: "{current_theme}",
                            onchange: {
                                let mut schedule_save = schedule_save.clone();
                                move |event: FormEvent| {
                                    let canonical = serialize_slides();
                                    document_content.set(canonical);
                                    let mut content = document_content.read().clone();
                                    set_presentation_theme(&mut content, &event.value());
                                    document_content.set(content);
                                    schedule_save();
                                }
                            },
                            option { value: "Contrix Light", "Contrix Light" }
                            option { value: "Midnight", "Midnight" }
                            option { value: "Editorial", "Editorial" }
                        }
                    }

                    div { class: "property-group",
                        label { {crate::i18n::t("editor.slides.background")} }
                        input {
                            r#type: "color",
                            class: "property-input",
                            value: "{cur_bg}",
                            oninput: {
                                let mut schedule_save = schedule_save.clone();
                                move |e: FormEvent| {
                                    if let Some(s) = slides.write().get_mut(cur_idx) {
                                        s.bg_color = e.value();
                                    }
                                    schedule_save();
                                }
                            },
                        }
                    }

                    div { class: "property-group",
                        label { {crate::i18n::t("editor.slides.transition")} }
                        select {
                            class: "property-select",
                            value: "{cur_transition}",
                            oninput: {
                                let mut schedule_save = schedule_save.clone();
                                move |e: FormEvent| {
                                    if let Some(s) = slides.write().get_mut(cur_idx) {
                                        s.transition = e.value();
                                    }
                                    schedule_save();
                                }
                            },
                            option { value: "none", {crate::i18n::t("editor.slides.transition.none")} }
                            option { value: "fade", {crate::i18n::t("editor.slides.transition.fade")} }
                            option { value: "slide", {crate::i18n::t("editor.slides.transition.slide")} }
                            option { value: "zoom", {crate::i18n::t("editor.slides.transition.zoom")} }
                            option { value: "flip", {crate::i18n::t("editor.slides.transition.flip")} }
                        }
                    }

                    div { class: "property-group",
                        label { {crate::i18n::t("editor.slides.layout")} }
                        select {
                            class: "property-select",
                            value: "{cur_layout}",
                            onchange: {
                                let mut schedule_save = schedule_save.clone();
                                move |e: FormEvent| {
                                    let kind = e.value();
                                    let cur = *current_slide.read();
                                    slides.with_mut(|sl| {
                                        if let Some(slide) = sl.get_mut(cur) {
                                            slide.layout = kind.clone();
                                        }
                                    });
                                    schedule_save();
                                }
                            },
                            option { value: "title-only", {crate::i18n::t("editor.slides.layout.title_only")} }
                            option { value: "title-body", {crate::i18n::t("editor.slides.layout.title_body")} }
                            option { value: "two-column", {crate::i18n::t("editor.slides.layout.two_column")} }
                            option { value: "blank", {crate::i18n::t("editor.slides.layout.blank")} }
                            option { value: "section-header", {crate::i18n::t("editor.slides.layout.section_header")} }
                            option { value: "image-title", {crate::i18n::t("editor.slides.layout.image_title")} }
                        }
                    }

                    div { class: "property-group",
                        label { {crate::i18n::t("editor.slides.speaker_notes")} }
                        textarea {
                            class: "speaker-notes",
                            placeholder: crate::i18n::t("editor.slides.notes_placeholder"),
                            value: "{cur_notes}",
                            oninput: {
                                let mut schedule_save = schedule_save.clone();
                                move |e: FormEvent| {
                                    if let Some(s) = slides.write().get_mut(cur_idx) {
                                        s.notes = e.value();
                                    }
                                    schedule_save();
                                }
                            },
                        }
                    }
                }
            }

            {
                let slide_str = crate::i18n::t("editor.slides.slide_of")
                    .replace("{cur}", &(cur_idx + 1).to_string())
                    .replace("{total}", &slide_count.to_string());
                let trans_str = crate::i18n::t("editor.slides.transition_label").replace("{kind}", &cur_transition);
                let id_str = crate::i18n::t("editor.slides.id_label").replace("{id}", &id);
                rsx! {
                    div { class: "status-bar",
                        span { "{slide_str}" }
                        span { "{trans_str}" }
                        span { "{id_str}" }
                    }
                }
            }

            div {
                id: "present-stage",
                class: if *present_mode.read() { "present-stage active" } else { "present-stage" },
                tabindex: "0",
                onkeydown: move |e: KeyboardEvent| {
                    match e.key() {
                        Key::ArrowRight => { e.prevent_default(); go_next(); }
                        Key::ArrowLeft => { e.prevent_default(); go_prev(); }
                        Key::Escape => { e.prevent_default(); exit_present(); }
                        Key::Character(ref ch) if ch == " " => { e.prevent_default(); go_next(); }
                        _ => {}
                    }
                },
                style: "background-color: {cur_bg};",
                div { class: "present-slide-title", style: "font-family:{heading_font};color:{theme_color};", "{cur_title}" }
                div { class: "present-slide-body", "{cur_body}" }
                {render_slide_objects(&document_content.read(), cur_idx)}
                div { class: "present-counter", "{cur_idx + 1} / {slide_count}" }
            }

            if *presenter_view.read() {
                div {
                    class: "modal-overlay",
                    onclick: move |_| presenter_view.set(false),
                    div {
                        class: "modal-dialog presenter-console",
                        onclick: move |event| event.stop_propagation(),
                        div { class: "modal-header",
                            h2 { "Presenter View" }
                            button { class: "modal-close", onclick: move |_| presenter_view.set(false), "×" }
                        }
                        div { class: "presenter-grid",
                            section {
                                h3 { "Current slide" }
                                div { class: "presenter-preview", style: "background-color: {cur_bg};",
                                    h2 { "{cur_title}" }
                                    p { "{cur_body}" }
                                }
                            }
                            section {
                                h3 { "Next slide" }
                                div { class: "presenter-preview next",
                                    h2 { "{next_title}" }
                                    p { "{next_body}" }
                                }
                            }
                        }
                        section { class: "presenter-notes",
                            h3 { "Speaker notes" }
                            p { "{cur_notes}" }
                        }
                        div { class: "presenter-controls",
                            button { class: "btn", onclick: move |_| go_prev(), "Previous" }
                            span { "{cur_idx + 1} / {slide_count}" }
                            span { "Elapsed: {elapsed_seconds}s" }
                            button { class: "btn", onclick: move |_| {
                                presentation_started.set(Some(js_sys::Date::now()));
                                elapsed_seconds.set(0);
                            }, "Reset timer" }
                            button { class: "btn btn-primary", onclick: move |_| go_next(), "Next" }
                        }
                    }
                }
            }

            CommentsPane { doc_id: id.clone(), open: comments_open }
            VersionHistory { doc_id: id.clone(), open: versions_open }
            ContextMenu {
                state: ctx_menu,
                on_command: {
                    let bus = toast_bus;
                    let mut schedule_save_ctx = schedule_save.clone();
                    move |cmd: String| {
                        match cmd.as_str() {
                            "duplicate-slide" => {
                                let cur = *current_slide.read();
                                let snapshot = slides.read().get(cur).cloned();
                                if let Some(mut s) = snapshot {
                                    s.source = document_content.read().get("slides").and_then(|value| value.as_array())
                                        .and_then(|items| items.iter().find(|item| item.get("id").and_then(|value| value.as_str()) == Some(s.native_id.as_str())))
                                        .cloned().unwrap_or_else(|| s.source.clone());
                                    s.native_id = uuid::Uuid::new_v4().to_string();
                                    let next_id = slides.read().iter().map(|sd| sd.id).max().unwrap_or(0) + 1;
                                    s.id = next_id;
                                    slides.write().insert(cur + 1, s);
                                    current_slide.set(cur + 1);
                                    schedule_save_ctx();
                                }
                            }
                            "delete-slide" => {
                                let len = slides.read().len();
                                if len <= 1 { return; }
                                let cur = *current_slide.read();
                                slides.write().remove(cur);
                                let new_len = slides.read().len();
                                if cur >= new_len {
                                    current_slide.set(new_len - 1);
                                }
                                schedule_save_ctx();
                            }
                            "cut" => push_toast(bus, ToastKind::Info, crate::i18n::t("editor.calc.use_ctrl_x")),
                            "copy" => push_toast(bus, ToastKind::Info, crate::i18n::t("editor.calc.use_ctrl_c")),
                            "paste" => push_toast(bus, ToastKind::Info, crate::i18n::t("editor.calc.use_ctrl_v")),
                            other => push_toast(bus, ToastKind::Info, crate::i18n::t("editor.coming_soon").replace("{what}", other)),
                        }
                    }
                },
            }
            crate::components::ai_assist::AiAssist { doc_id: id.clone() }
        }
    }
}

#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn simplified_slides_are_upgraded_to_canonical_elements() {
        let slides = vec![SlideData {
            id: 0,
            native_id: uuid::Uuid::new_v4().to_string(),
            source: serde_json::Value::Null,
            title: "Quarterly review".into(),
            body: "Revenue grew".into(),
            notes: "Mention margin".into(),
            transition: "fade".into(),
            bg_color: "#ffffff".into(),
            layout: "title-body".into(),
        }];
        let content = merge_presentation(&serde_json::json!({}), &slides, "Review");
        assert!(
            content
                .pointer("/slides/0/elements/0/content/0/text")
                .is_some_and(|v| v == "Quarterly review")
        );
        assert_eq!(
            content
                .pointer("/slides/0/transition/transition_type")
                .and_then(|v| v.as_str()),
            Some("Fade")
        );
        assert_eq!(hydrate_presentation(&content)[0].body, "Revenue grew");
        serde_json::from_value::<contrix_doc_engine::Presentation>(content).unwrap();
    }

    #[test]
    fn collaboration_replacement_is_scoped_to_presentations() {
        let payload = serde_json::json!({"user_id":"peer","op_type":{"type":"Replace","path":["presentation"],"new_value":{"slides":[]}}});
        assert_eq!(
            presentation_replace_from_operation(&payload).map(|v| v.0),
            Some("peer".into())
        );
    }

    #[test]
    fn reordering_keeps_each_slides_native_objects() {
        let mut first = SlideData::empty(0);
        first.title = "First".into();
        let mut second = SlideData::empty(1);
        second.title = "Second".into();
        let mut base = merge_presentation(
            &serde_json::json!({}),
            &[first.clone(), second.clone()],
            "Deck",
        );
        append_slide_element(
            &mut base,
            0,
            serde_json::json!({"type":"Image","id":uuid::Uuid::new_v4(),"url":"https://example.com/image.png"}),
        );
        let reordered = merge_presentation(&base, &[second, first], "Deck");
        assert!(reordered.pointer("/slides/0/elements/2").is_none());
        assert_eq!(
            reordered
                .pointer("/slides/1/elements/2/type")
                .and_then(|value| value.as_str()),
            Some("Image")
        );
    }
}
