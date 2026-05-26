use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_timers::callback::Timeout;
use serde::Deserialize;

use crate::components::navbar::Navbar;
use crate::components::toolbar::Toolbar;
use crate::state::{AuthState, api_get, api_put};

#[derive(Clone, Deserialize)]
struct DocumentResponse {
    name: String,
    content: serde_json::Value,
}

#[derive(Deserialize, Default)]
struct SlideDto {
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    notes: String,
    #[serde(default = "default_transition")]
    transition: String,
    #[serde(default = "default_bg")]
    bg_color: String,
}

fn default_transition() -> String { "none".into() }
fn default_bg() -> String { "#ffffff".into() }

#[derive(Deserialize)]
struct PresentationLite {
    #[serde(default)]
    slides: Vec<SlideDto>,
}

#[derive(Clone, PartialEq)]
struct SlideData {
    id: usize,
    title: String,
    body: String,
    notes: String,
    transition: String,
    bg_color: String,
}

impl SlideData {
    fn empty(id: usize) -> Self {
        Self {
            id,
            title: String::new(),
            body: String::new(),
            notes: String::new(),
            transition: "none".into(),
            bg_color: "#ffffff".into(),
        }
    }
}

#[component]
pub fn Slides(id: String) -> Element {
    let mut slides = use_signal(|| vec![SlideData::empty(0)]);
    let mut current_slide = use_signal(|| 0usize);
    let mut doc_title = use_signal(|| "Untitled Presentation".to_string());
    let mut saved_status = use_signal(|| "All changes saved".to_string());
    let mut loaded = use_signal(|| false);
    let mut drag_index = use_signal(|| None::<usize>);
    let mut present_mode = use_signal(|| false);

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
            let parsed = serde_json::from_value::<PresentationLite>(doc.content.clone()).ok();
            let new_slides: Vec<SlideData> = match parsed {
                Some(p) if !p.slides.is_empty() => p.slides.into_iter().enumerate().map(|(i, s)| {
                    SlideData {
                        id: i,
                        title: s.title,
                        body: s.body,
                        notes: s.notes,
                        transition: s.transition,
                        bg_color: s.bg_color,
                    }
                }).collect(),
                _ => vec![SlideData::empty(0)],
            };
            slides.set(new_slides);
            current_slide.set(0);
            loaded.set(true);
        }
    });

    let save_timer: Rc<RefCell<Option<Timeout>>> = use_hook(|| Rc::new(RefCell::new(None)));

    let serialize_slides = move || -> serde_json::Value {
        let payload: Vec<serde_json::Value> = slides
            .read()
            .iter()
            .map(|s| serde_json::json!({
                "id": s.id,
                "title": s.title,
                "body": s.body,
                "notes": s.notes,
                "transition": s.transition,
                "bg_color": s.bg_color,
            }))
            .collect();
        serde_json::json!({ "slides": payload })
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
        let serialize_slides = serialize_slides.clone();
        move || {
            if !*loaded.read() {
                return;
            }
            saved_status.set("Unsaved changes".to_string());
            let body = serialize_slides();
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
        let serialize_slides = serialize_slides.clone();
        move || {
            if !*loaded.read() {
                return;
            }
            save_timer.borrow_mut().take();
            let body = serialize_slides();
            let token = auth.read().token.clone().unwrap_or_default();
            perform_save(body, saved_status, token);
        }
    };

    let mut exit_present = move || {
        present_mode.set(false);
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            let _ = doc.exit_fullscreen();
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
    let cur_title = slides.read().get(cur_idx).map(|s| s.title.clone()).unwrap_or_default();
    let cur_body = slides.read().get(cur_idx).map(|s| s.body.clone()).unwrap_or_default();
    let cur_notes = slides.read().get(cur_idx).map(|s| s.notes.clone()).unwrap_or_default();
    let cur_bg = slides.read().get(cur_idx).map(|s| s.bg_color.clone()).unwrap_or_else(|| "#ffffff".into());
    let cur_transition = slides.read().get(cur_idx).map(|s| s.transition.clone()).unwrap_or_else(|| "none".into());

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
                    placeholder: "Untitled Presentation",
                }
                span { class: "save-status", "{saved_status}" }
                button {
                    class: "present-btn",
                    onclick: move |_| {
                        present_mode.set(true);
                        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                            if let Some(el) = doc.get_element_by_id("present-stage") {
                                let _ = el.request_fullscreen();
                            }
                        }
                    },
                    "Present"
                }
            }

            Toolbar { doc_type: "slides".to_string() }

            match doc_resource.read().as_ref() {
                None => rsx! { div { class: "editor-loading", "Loading document..." } },
                Some(Err(e)) => rsx! { div { class: "editor-error", "Failed to load: {e}" } },
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
                                        if let Some(from) = *drag_index.read() {
                                            if from != i {
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
                        "+ Add Slide"
                    }
                }

                div { class: "slide-canvas",
                    div {
                        class: "slide-content",
                        style: "background-color: {cur_bg};",

                        if cur_idx == 0 {
                            div { class: "slide-placeholder title-slide",
                                h2 {
                                    contenteditable: "true",
                                    class: "slide-title-text",
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
                                p {
                                    contenteditable: "true",
                                    class: "slide-subtitle-text",
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
                            }
                        } else {
                            div { class: "slide-placeholder content-slide",
                                h3 {
                                    contenteditable: "true",
                                    class: "slide-heading-text",
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
                                    contenteditable: "true",
                                    class: "slide-body-text",
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
                            }
                        }
                    }
                }

                div { class: "properties-panel",
                    h3 { "Properties" }

                    div { class: "property-group",
                        label { "Background" }
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
                        label { "Transition" }
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
                            option { value: "none", "None" }
                            option { value: "fade", "Fade" }
                            option { value: "slide", "Slide" }
                            option { value: "zoom", "Zoom" }
                            option { value: "flip", "Flip" }
                        }
                    }

                    div { class: "property-group",
                        label { "Layout" }
                        select { class: "property-select",
                            option { "Title Slide" }
                            option { "Title and Content" }
                            option { "Two Columns" }
                            option { "Blank" }
                        }
                    }

                    div { class: "property-group",
                        label { "Speaker Notes" }
                        textarea {
                            class: "speaker-notes",
                            placeholder: "Add notes for this slide...",
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

            div { class: "status-bar",
                span { "Slide {cur_idx + 1} of {slide_count}" }
                span { "Transition: {cur_transition}" }
                span { "ID: {id}" }
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
                div { class: "present-slide-title", "{cur_title}" }
                div { class: "present-slide-body", "{cur_body}" }
                div { class: "present-counter", "{cur_idx + 1} / {slide_count}" }
            }
        }
    }
}
