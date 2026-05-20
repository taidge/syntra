use dioxus::prelude::*;

use crate::components::navbar::Navbar;
use crate::components::toolbar::Toolbar;

/// Presentation editor with slide panel, canvas, and properties panel.
#[component]
pub fn Slides(id: String) -> Element {
    let mut current_slide = use_signal(|| 0usize);
    let mut slide_count = use_signal(|| 1usize);
    let mut doc_title = use_signal(|| "Untitled Presentation".to_string());
    let mut saved_status = use_signal(|| "All changes saved".to_string());
    let mut bg_color = use_signal(|| "#ffffff".to_string());
    let mut transition = use_signal(|| "None".to_string());

    rsx! {
        Navbar {}
        div { class: "editor-container",
            // Title bar
            div { class: "doc-title-bar",
                input {
                    class: "doc-title-input",
                    value: "{doc_title}",
                    oninput: move |e| doc_title.set(e.value()),
                    placeholder: "Untitled Presentation",
                }
                span { class: "save-status", "{saved_status}" }
            }

            // Toolbar
            Toolbar { doc_type: "slides".to_string() }

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
                        onclick: move |_| {
                            let count = *slide_count.read();
                            slide_count.set(count + 1);
                            current_slide.set(count);
                            saved_status.set("Unsaved changes".to_string());
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
                                    oninput: move |_| saved_status.set("Unsaved changes".to_string()),
                                    "Click to add title"
                                }
                                p {
                                    contenteditable: "true",
                                    class: "slide-subtitle-text",
                                    oninput: move |_| saved_status.set("Unsaved changes".to_string()),
                                    "Click to add subtitle"
                                }
                            }
                        } else {
                            // Content slide
                            div { class: "slide-placeholder content-slide",
                                h3 {
                                    contenteditable: "true",
                                    class: "slide-heading-text",
                                    oninput: move |_| saved_status.set("Unsaved changes".to_string()),
                                    "Click to add heading"
                                }
                                div {
                                    contenteditable: "true",
                                    class: "slide-body-text",
                                    oninput: move |_| saved_status.set("Unsaved changes".to_string()),
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
                            oninput: move |_| saved_status.set("Unsaved changes".to_string()),
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
