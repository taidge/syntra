use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use gloo_timers::callback::Timeout;
use serde::Deserialize;

use crate::Route;
use crate::components::comments_pane::CommentsPane;
use crate::components::context_menu::{ContextMenu, MenuEntry};
use crate::components::menubar::Menubar;
use crate::components::navbar::Navbar;
use crate::components::toast::{Toast, ToastKind, push_toast};
use crate::components::toolbar::Toolbar;
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
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    notes: String,
    #[serde(default = "default_transition")]
    transition: String,
    #[serde(default = "default_bg")]
    bg_color: String,
    #[serde(default = "default_layout")]
    layout: String,
}

fn default_transition() -> String { "none".into() }
fn default_bg() -> String { "#ffffff".into() }
fn default_layout() -> String { "title-body".into() }

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
    layout: String,
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
                        layout: s.layout,
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
                "layout": s.layout,
            }))
            .collect();
        serde_json::json!({ "slides": payload })
    };

    let perform_save = {
        let id = id.clone();
        move |body: serde_json::Value, mut status: Signal<String>, token: String| {
            let id = id.clone();
            status.set(crate::i18n::t("common.saving"));
            wasm_bindgen_futures::spawn_local(async move {
                let req = serde_json::json!({ "content": body });
                match api_put::<serde_json::Value, _>(&format!("/documents/{id}"), &req, &token).await {
                    Ok(_) => status.set(crate::i18n::t("common.saved")),
                    Err(e) => status.set(crate::i18n::t("editor.save_failed").replace("{err}", &e.to_string())),
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
            saved_status.set(crate::i18n::t("common.unsaved"));
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
    let cur_layout = slides.read().get(cur_idx).map(|s| s.layout.clone()).unwrap_or_else(|| "title-body".into());

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
                        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                            if let Some(el) = doc.get_element_by_id("present-stage") {
                                let _ = el.request_fullscreen();
                            }
                        }
                    },
                    {crate::i18n::t("editor.slides.present")}
                }
            }

            {
                let mut flush_for_menu = flush_save.clone();
                let mut schedule_for_menu = schedule_save.clone();
                let id_for_menu = id.clone();
                let mut bus_for_menu = toast_bus;
                let nav_for_menu = nav;
                rsx! {
                    Menubar {
                        doc_type: "slides".to_string(),
                        on_command: move |cmd: String| {
                            let mut bus = bus_for_menu;
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
                                "insert-slide" => {
                                    let next_id = slides.read().iter().map(|s| s.id).max().unwrap_or(0) + 1;
                                    slides.write().push(SlideData::empty(next_id));
                                    let new_idx = slides.read().len() - 1;
                                    current_slide.set(new_idx);
                                    schedule_for_menu();
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
                                "export-pdf" | "export-pptx" | "export-html" => {
                                    let format = match cmd.as_str() {
                                        "export-pdf" => "pdf",
                                        "export-pptx" => "pptx",
                                        _ => "html",
                                    };
                                    let id = id_for_menu.clone();
                                    let token = auth.read().token.clone().unwrap_or_default();
                                    let mut bus2 = bus;
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

            Toolbar { doc_type: "slides".to_string() }

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
                        style: "background-color: {cur_bg};",

                        div {
                            class: "slide-title",
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
                        select { class: "property-select",
                            option { {crate::i18n::t("editor.slides.layout.title_slide")} }
                            option { {crate::i18n::t("editor.slides.layout.title_content")} }
                            option { {crate::i18n::t("editor.slides.layout.two_columns")} }
                            option { {crate::i18n::t("editor.slides.layout.blank")} }
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
                div { class: "present-slide-title", "{cur_title}" }
                div { class: "present-slide-body", "{cur_body}" }
                div { class: "present-counter", "{cur_idx + 1} / {slide_count}" }
            }

            CommentsPane { doc_id: id.clone(), open: comments_open }
            VersionHistory { doc_id: id.clone(), open: versions_open }
            ContextMenu {
                state: ctx_menu,
                on_command: {
                    let mut bus = toast_bus;
                    let mut schedule_save_ctx = schedule_save.clone();
                    move |cmd: String| {
                        match cmd.as_str() {
                            "duplicate-slide" => {
                                let cur = *current_slide.read();
                                let snapshot = slides.read().get(cur).cloned();
                                if let Some(mut s) = snapshot {
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
