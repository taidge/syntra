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

#[derive(Deserialize)]
struct PresentationLite {
    #[serde(default)]
    slides: Vec<serde_json::Value>,
}

/// Presentation editor with slide panel, canvas, and properties panel.
#[component]
pub fn Slides(id: String) -> Element {
    let mut current_slide = use_signal(|| 0usize);
    let mut slide_count = use_signal(|| 1usize);
    let mut doc_title = use_signal(|| "Untitled Presentation".to_string());
    let mut saved_status = use_signal(|| "All changes saved".to_string());
    let mut bg_color = use_signal(|| "#ffffff".to_string());
    let mut transition = use_signal(|| "None".to_string());
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
            if let Ok(p) = serde_json::from_value::<PresentationLite>(doc.content.clone()) {
                let count = p.slides.len().max(1);
                slide_count.set(count);
                // TODO: deep-hydrate slide elements (text, layout, etc.)
            }
            current_slide.set(0);
            loaded.set(true);
        }
    });

    let save_timer: Rc<RefCell<Option<Timeout>>> = use_hook(|| Rc::new(RefCell::new(None)));

    let serialize_slides = move || -> serde_json::Value {
        let count = *slide_count.read();
        let slides: Vec<serde_json::Value> = (0..count)
            .map(|i| serde_json::json!({
                "id": i,
                "elements": [],
                "notes": ""
            }))
            .collect();
        serde_json::json!({ "slides": slides })
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
                    placeholder: "Untitled Presentation",
                }
                span { class: "save-status", "{saved_status}" }
            }

            // Toolbar
            Toolbar { doc_type: "slides".to_string() }

            // Loading / error overlay
            match doc_resource.read().as_ref() {
                None => rsx! { div { class: "editor-loading", "Loading document..." } },
                Some(Err(e)) => rsx! { div { class: "editor-error", "Failed to load: {e}" } },
                Some(Ok(_)) => rsx! {},
            }

            // Main editor
            div { class: "slides-editor",
                // Left panel: slide thumbnails
                div { class: "slide-panel",
                    for i in 0..*slide_count.read() {
                        div {
                            class: if *current_slide.read() == i { "slide-thumbnail active" } else { "slide-thumbnail" },
                            onclick: move |_| current_slide.set(i),
                            div { class: "slide-thumb-number", "{i + 1}" }
                            div { class: "slide-thumb-preview",
                                // Miniature preview of the slide
                                if i == 0 {
                                    div { class: "thumb-content",
                                        div { class: "thumb-title-bar" }
                                        div { class: "thumb-subtitle-bar" }
                                    }
                                } else {
                                    div { class: "thumb-content",
                                        div { class: "thumb-body-bar" }
                                        div { class: "thumb-body-bar short" }
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
                                let count = *slide_count.read();
                                slide_count.set(count + 1);
                                current_slide.set(count);
                                schedule_save();
                            }
                        },
                        "+ Add Slide"
                    }
                }

                // Main canvas
                div { class: "slide-canvas",
                    div {
                        class: "slide-content",
                        style: "background-color: {bg_color};",

                        if *current_slide.read() == 0 {
                            // Title slide
                            div { class: "slide-placeholder title-slide",
                                h2 {
                                    contenteditable: "true",
                                    class: "slide-title-text",
                                    oninput: {
                                        let mut schedule_save = schedule_save.clone();
                                        move |_| schedule_save()
                                    },
                                    "Click to add title"
                                }
                                p {
                                    contenteditable: "true",
                                    class: "slide-subtitle-text",
                                    oninput: {
                                        let mut schedule_save = schedule_save.clone();
                                        move |_| schedule_save()
                                    },
                                    "Click to add subtitle"
                                }
                            }
                        } else {
                            // Content slide
                            div { class: "slide-placeholder content-slide",
                                h3 {
                                    contenteditable: "true",
                                    class: "slide-heading-text",
                                    oninput: {
                                        let mut schedule_save = schedule_save.clone();
                                        move |_| schedule_save()
                                    },
                                    "Click to add heading"
                                }
                                div {
                                    contenteditable: "true",
                                    class: "slide-body-text",
                                    oninput: {
                                        let mut schedule_save = schedule_save.clone();
                                        move |_| schedule_save()
                                    },
                                    "Click to add content"
                                }
                            }
                        }
                    }
                }

                // Right panel: properties
                div { class: "properties-panel",
                    h3 { "Properties" }

                    div { class: "property-group",
                        label { "Background" }
                        input {
                            r#type: "color",
                            class: "property-input",
                            value: "{bg_color}",
                            oninput: move |e| bg_color.set(e.value()),
                        }
                    }

                    div { class: "property-group",
                        label { "Transition" }
                        select {
                            class: "property-select",
                            value: "{transition}",
                            oninput: move |e| transition.set(e.value()),
                            option { value: "None", "None" }
                            option { value: "Fade", "Fade" }
                            option { value: "Slide", "Slide" }
                            option { value: "Zoom", "Zoom" }
                            option { value: "Flip", "Flip" }
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
                            oninput: {
                                let mut schedule_save = schedule_save.clone();
                                move |_| schedule_save()
                            },
                        }
                    }
                }
            }

            // Status bar
            div { class: "status-bar",
                span { "Slide {*current_slide.read() + 1} of {slide_count}" }
                span { "Transition: {transition}" }
                span { "ID: {id}" }
            }
        }
    }
}
