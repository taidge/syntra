use super::document_editor::{EditorSession, editor_header};
use crate::state::AuthState;
use glory::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
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
    let canonical = slides
        .iter()
        .map(|slide| {
            let old = previous
                .and_then(|items| {
                    items.iter().find(|item| {
                        item.get("id").and_then(Value::as_str) == Some(slide.native_id.as_str())
                    })
                })
                .or_else(|| slide.source.is_object().then_some(&slide.source));
            let mut result = old.cloned().unwrap_or_else(|| json!({}));
            let mut replaced = 0usize;
            let mut elements = Vec::new();
            for element in old
                .and_then(|old| old["elements"].as_array())
                .into_iter()
                .flatten()
            {
                if element["type"] == "TextBox" && replaced < 2 {
                    let text = if replaced == 0 {
                        &slide.title
                    } else {
                        &slide.body
                    };
                    let mut next = element.clone();
                    if text_from_element(element).as_deref() != Some(text.as_str()) {
                        let runs = element["content"]
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or(&[]);
                        next["content"] = json!(super::writer::replace_writer_runs(runs, text));
                    }
                    elements.push(next);
                    replaced += 1;
                } else {
                    elements.push(element.clone());
                }
            }
            for (index, text) in [&slide.title, &slide.body]
                .into_iter()
                .enumerate()
                .skip(replaced)
            {
                elements.push(text_box(json!(uuid::Uuid::new_v4()), text, index == 0));
            }
            let original = old.map(|old| hydrate_presentation(&json!({"slides":[old]})).remove(0));
            result["id"] = json!(slide.native_id);
            result["elements"] = json!(elements);
            if original
                .as_ref()
                .is_none_or(|old| old.bg_color != slide.bg_color)
                || result.get("background").is_none()
            {
                result["background"] = json!({"Solid":slide.bg_color});
            }
            if original
                .as_ref()
                .is_none_or(|old| old.transition != slide.transition)
                || result.get("transition").is_none()
            {
                result["transition"] = canonical_transition(&slide.transition);
            }
            if original
                .as_ref()
                .is_none_or(|old| old.layout != slide.layout)
                || result.get("layout").is_none()
            {
                result["layout"] = json!(canonical_layout(&slide.layout));
            }
            if result.get("animations").is_none() {
                result["animations"] = json!([]);
            }
            result["notes"] = json!(slide.notes);
            if result.get("title").is_some() {
                result["title"] = json!(slide.title);
            }
            if result.get("body").is_some() {
                result["body"] = json!(slide.body);
            }
            result
        })
        .collect::<Vec<_>>();
    object.insert("slides".into(), serde_json::Value::Array(canonical));
    result
}

#[cfg(test)]
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

fn edit_slide(session: &EditorSession, index: usize, edit: impl FnOnce(&mut SlideData)) {
    let base = session.content.get();
    let mut slides = hydrate_presentation(&base);
    if let Some(slide) = slides.get_mut(index) {
        edit(slide);
    }
    session.edit(merge_presentation(&base, &slides, &session.name.get()));
}
fn slide_svg(content: &Value, index: usize) -> String {
    use base64::Engine;
    let escape = |value: &str| {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let slides = hydrate_presentation(content);
    let Some(slide) = slides.get(index) else {
        return String::new();
    };
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1920\" height=\"1080\" viewBox=\"0 0 1920 1080\"><rect width=\"1920\" height=\"1080\" fill=\"{}\"/>",
        escape(&slide.bg_color)
    );
    let elements = content
        .pointer(&format!("/slides/{index}/elements"))
        .and_then(Value::as_array);
    for element in elements.into_iter().flatten() {
        let num = |name: &str, default: f64| {
            element[name]
                .as_f64()
                .filter(|n| n.is_finite())
                .unwrap_or(default)
        };
        let (x, y, width, height) = (
            num("x", 120.),
            num("y", 100.),
            num("width", 500.),
            num("height", 300.),
        );
        match element["type"].as_str().unwrap_or_default() {
            "Shape" => {
                let fill = escape(element["fill_color"].as_str().unwrap_or("#DBEAFE"));
                svg.push_str(&format!("<rect x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" fill=\"{fill}\" stroke=\"#2563eb\"/>"));
            }
            "TextBox" => {
                let size = element
                    .pointer("/content/0/font_size")
                    .and_then(Value::as_f64)
                    .unwrap_or(28.0);
                let text = text_from_element(element).unwrap_or_default();
                for (line, text) in text.lines().enumerate() {
                    let y = y + size + line as f64 * size * 1.3;
                    svg.push_str(&format!("<text x=\"{x}\" y=\"{y}\" font-family=\"Arial,sans-serif\" font-size=\"{size}\" fill=\"#111827\">{}</text>",escape(text)));
                }
            }
            "Image" => {
                let url = escape(element["url"].as_str().unwrap_or_default());
                svg.push_str(&format!("<image href=\"{url}\" x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\"/>"));
            }
            "Chart" => {
                let values = element
                    .pointer("/data/datasets/0/values")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let max = values
                    .iter()
                    .filter_map(Value::as_f64)
                    .map(f64::abs)
                    .fold(1., f64::max);
                for (column, value) in values.iter().enumerate() {
                    let bar = value.as_f64().unwrap_or_default().abs() / max * height;
                    let x = x + column as f64 * width / values.len().max(1) as f64;
                    let y = y + height - bar;
                    let w = width / values.len().max(1) as f64 * 0.8;
                    svg.push_str(&format!("<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{bar}\" fill=\"#2563eb\"/>"));
                }
            }
            _ => {}
        }
    }
    if elements.is_none_or(Vec::is_empty) {
        svg.push_str(&format!("<text x=\"120\" y=\"160\" font-size=\"48\">{}</text><text x=\"120\" y=\"320\" font-size=\"28\">{}</text>",escape(&slide.title),escape(&slide.body)));
    }
    svg.push_str("</svg>");
    format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    )
}

#[allow(non_snake_case)]
pub fn Slides(_scope: Scope, auth: Cage<AuthState>, id: String) -> impl Widget {
    widget(move |scope| {
        let session = EditorSession::new(scope, auth, id.clone(), "presentation");
        let content = session.content;
        let current = scope.cage(0usize);
        let presenting = scope.cage(false);
        let comments = scope.cage(false);
        let versions = scope.cage(false);
        let objects = scope.cage(false);
        let titles = content.map(|value| {
            hydrate_presentation(value)
                .into_iter()
                .enumerate()
                .map(|(index, slide)| (index, slide.title))
                .collect::<Vec<_>>()
        });
        let title = scope.bond(move || {
            hydrate_presentation(&content.get())
                .get(current.get())
                .map(|slide| slide.title.clone())
                .unwrap_or_default()
        });
        let body = scope.bond(move || {
            hydrate_presentation(&content.get())
                .get(current.get())
                .map(|slide| slide.body.clone())
                .unwrap_or_default()
        });
        let notes = scope.bond(move || {
            hydrate_presentation(&content.get())
                .get(current.get())
                .map(|slide| slide.notes.clone())
                .unwrap_or_default()
        });
        let bg = scope.bond(move || {
            hydrate_presentation(&content.get())
                .get(current.get())
                .map(|slide| slide.bg_color.clone())
                .unwrap_or_else(|| "#ffffff".into())
        });
        let transition = scope.bond(move || {
            hydrate_presentation(&content.get())
                .get(current.get())
                .map(|slide| Some(slide.transition.clone()))
                .unwrap_or_default()
        });
        let layout = scope.bond(move || {
            hydrate_presentation(&content.get())
                .get(current.get())
                .map(|slide| Some(slide.layout.clone()))
                .unwrap_or_default()
        });
        let preview = scope.bond(move || slide_background_source(&content.get(), current.get()));
        let render_objects = scope.bond(move || {
            let value = content.get();
            let mut items = Vec::new();
            collect_slide_objects(
                value
                    .pointer(&format!("/slides/{}/elements", current.get()))
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                "slide",
                &mut items,
            );
            items
        });
        let edit_title = session.clone();
        let edit_body = session.clone();
        let edit_notes = session.clone();
        let edit_bg = session.clone();
        let edit_transition = session.clone();
        let edit_layout = session.clone();
        let add = session.clone();
        let duplicate = session.clone();
        let delete = session.clone();
        let move_up = session.clone();
        let move_down = session.clone();
        let previous = session.clone();
        let next = session.clone();
        let object_session = session.clone();
        let controls = row().class("editor-toolbar").fill((
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("New slide")
                .on_press(move || {
                    let base = add.content.get();
                    let mut slides = hydrate_presentation(&base);
                    slides.push(SlideData::empty(slides.len()));
                    current.revise(|value| *value = slides.len() - 1);
                    add.edit(merge_presentation(&base, &slides, &add.name.get()));
                }),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Duplicate")
                .on_press(move || {
                    let base = duplicate.content.get();
                    let mut slides = hydrate_presentation(&base);
                    if let Some(mut slide) = slides.get(current.get()).cloned() {
                        slide.native_id = uuid::Uuid::new_v4().to_string();
                        let pos = current.get() + 1;
                        slides.insert(pos, slide);
                        current.revise(|value| *value = pos);
                        duplicate.edit(merge_presentation(&base, &slides, &duplicate.name.get()));
                    }
                }),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Delete slide")
                .on_press(move || {
                    let base = delete.content.get();
                    let mut slides = hydrate_presentation(&base);
                    if slides.len() > 1 {
                        slides.remove(current.get().min(slides.len() - 1));
                        current.revise(|value| *value = (*value).min(slides.len() - 1));
                        delete.edit(merge_presentation(&base, &slides, &delete.name.get()));
                    }
                }),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Move up")
                .on_press(move || {
                    let base = move_up.content.get();
                    let mut slides = hydrate_presentation(&base);
                    let pos = current.get();
                    if pos > 0 && pos < slides.len() {
                        slides.swap(pos, pos - 1);
                        current.revise(|value| *value -= 1);
                        move_up.edit(merge_presentation(&base, &slides, &move_up.name.get()));
                    }
                }),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Move down")
                .on_press(move || {
                    let base = move_down.content.get();
                    let mut slides = hydrate_presentation(&base);
                    let pos = current.get();
                    if pos + 1 < slides.len() {
                        slides.swap(pos, pos + 1);
                        current.revise(|value| *value += 1);
                        move_down.edit(merge_presentation(&base, &slides, &move_down.name.get()));
                    }
                }),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Present / exit")
                .on_press(move || presenting.revise(|value| *value = !*value)),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Objects & theme")
                .on_press(move || objects.revise(|value| *value = !*value)),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Comments")
                .on_press(move || comments.revise(|value| *value = !*value)),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Versions")
                .on_press(move || versions.revise(|value| *value = !*value)),
        ));
        col()
            .class("slides-app editor-shell")
            .fill((
                editor_header(&session, "Slides", &["pptx", "pdf", "odp", "html"]),
                controls,
                row().class("slides-workspace").fill((
                    col().class("slide-list").fill(each(
                        titles,
                        |item| item.0,
                        move |item, _| {
                            sapid::button()
                                .theme(crate::components::ui::theme())
                                .label(item.map(|item| format!("{}. {}", item.0 + 1, item.1)))
                                .on_press(move || current.revise(|value| *value = item.get().0))
                        },
                    )),
                    col()
                        .class_fn(move || {
                            if presenting.get() {
                                "slide-stage presenting".into()
                            } else {
                                "slide-stage".into()
                            }
                        })
                        .fill((
                            view()
                                .class("slide-canvas")
                                .style(
                                    Style::new()
                                        .position(Position::Relative)
                                        .width(Length::Percent(100.))
                                        .overflow_x(Overflow::Hidden)
                                        .overflow_y(Overflow::Hidden),
                                )
                                .fill((
                                    image()
                                        .source(preview)
                                        .alt("Slide preview")
                                        .style(Style::new().width(Length::Percent(100.))),
                                    super::document_editor::diagram_objects(
                                        render_objects,
                                        slide_object_source,
                                        1920.,
                                        1080.,
                                    ),
                                )),
                            row().fill((
                                sapid::button()
                                    .theme(crate::components::ui::theme())
                                    .label("Previous")
                                    .on_press(move || {
                                        let count =
                                            hydrate_presentation(&previous.content.get()).len();
                                        current.revise(|value| {
                                            *value =
                                                value.saturating_sub(1).min(count.saturating_sub(1))
                                        });
                                    }),
                                text().content(scope.bond(move || {
                                    format!(
                                        "Slide {} / {}",
                                        current.get() + 1,
                                        hydrate_presentation(&content.get()).len()
                                    )
                                })),
                                sapid::button()
                                    .theme(crate::components::ui::theme())
                                    .label("Next")
                                    .on_press(move || {
                                        let count = hydrate_presentation(&next.content.get()).len();
                                        current.revise(|value| {
                                            *value = (*value + 1).min(count.saturating_sub(1))
                                        });
                                    }),
                            )),
                            sapid::text_field()
                                .theme(crate::components::ui::theme())
                                .name("Slide title")
                                .value(title)
                                .on_value_change(move |value| {
                                    edit_slide(&edit_title, current.get(), |slide| {
                                        slide.title = value
                                    })
                                }),
                            sapid::textarea()
                                .theme(crate::components::ui::theme())
                                .name("Slide body")
                                .rows(8)
                                .value(body)
                                .on_value_change(move |value| {
                                    edit_slide(&edit_body, current.get(), |slide| {
                                        slide.body = value
                                    })
                                }),
                            sapid::textarea()
                                .theme(crate::components::ui::theme())
                                .name("Speaker notes")
                                .rows(4)
                                .value(notes)
                                .on_value_change(move |value| {
                                    edit_slide(&edit_notes, current.get(), |slide| {
                                        slide.notes = value
                                    })
                                }),
                            row().fill((
                                sapid::text_field()
                                    .theme(crate::components::ui::theme())
                                    .name("Background color")
                                    .value(bg)
                                    .on_value_change(move |value| {
                                        edit_slide(&edit_bg, current.get(), |slide| {
                                            slide.bg_color = value
                                        })
                                    }),
                                sapid::native_select("slide-transition")
                                    .theme(crate::components::ui::theme())
                                    .name("Transition")
                                    .value(transition)
                                    .options(
                                        [
                                            "none", "fade", "slide", "zoom", "flip", "push",
                                            "dissolve",
                                        ]
                                        .map(|key| sapid::select_option(key, key)),
                                    )
                                    .on_value_change(move |value| {
                                        if let Some(value) = value {
                                            edit_slide(&edit_transition, current.get(), |slide| {
                                                slide.transition = value
                                            })
                                        }
                                    }),
                                sapid::native_select("slide-layout")
                                    .theme(crate::components::ui::theme())
                                    .name("Slide layout")
                                    .value(layout)
                                    .options(
                                        [
                                            "title-body",
                                            "title-slide",
                                            "title-only",
                                            "two-column",
                                            "section-header",
                                            "blank",
                                        ]
                                        .map(|key| sapid::select_option(key, key)),
                                    )
                                    .on_value_change(move |value| {
                                        if let Some(value) = value {
                                            edit_slide(&edit_layout, current.get(), |slide| {
                                                slide.layout = value
                                            })
                                        }
                                    }),
                            )),
                        )),
                )),
                when(objects, move || {
                    slide_objects(object_session.clone(), current)
                }),
                crate::components::comments_pane::CommentsPane(
                    scope.clone(),
                    auth,
                    id.clone(),
                    comments,
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

fn slide_objects(session: EditorSession, current: Cage<usize>) -> impl Widget {
    widget(move |scope| {
        let source = scope.cage(String::new());
        let url = scope.cage(String::new());
        let status = scope.cage(String::new());
        let shapes = session.clone();
        let image = session.clone();
        let chart = session.clone();
        let animation = session.clone();
        let apply = session.clone();
        let theme = session.clone();
        col().class("slide-object-panel").fill((sapid::title(2).theme(crate::components::ui::theme()).content("Objects & theme"),
            row().fill((sapid::button().theme(crate::components::ui::theme()).label("Insert rectangle").on_press(move||{let mut value=merge_presentation(&shapes.content.get(),&hydrate_presentation(&shapes.content.get()),&shapes.name.get());append_slide_element(&mut value,current.get(),json!({"type":"Shape","id":uuid::Uuid::new_v4(),"shape_type":"Rectangle","x":300.,"y":300.,"width":400.,"height":240.,"rotation":0.,"fill_color":"#DBEAFE","stroke_color":"#2563eb","stroke_width":2.}));shapes.edit(value);}),
                sapid::text_field().theme(crate::components::ui::theme()).name("Image URL").value(url).on_value_change(move|value|url.revise(|current|*current=value)),
                sapid::button().theme(crate::components::ui::theme()).label("Insert image").on_press(move||{let mut value=merge_presentation(&image.content.get(),&hydrate_presentation(&image.content.get()),&image.name.get());append_slide_element(&mut value,current.get(),json!({"type":"Image","id":uuid::Uuid::new_v4(),"url":url.get(),"alt_text":"Slide image","x":300.,"y":300.,"width":640.,"height":360.,"rotation":0.}));image.edit(value);}),
                sapid::button().theme(crate::components::ui::theme()).label("Insert chart").on_press(move||{let mut value=merge_presentation(&chart.content.get(),&hydrate_presentation(&chart.content.get()),&chart.name.get());append_slide_element(&mut value,current.get(),json!({"type":"Chart","id":uuid::Uuid::new_v4(),"chart_type":"Bar","x":800.,"y":400.,"width":800.,"height":400.,"data":{"labels":["A","B","C"],"datasets":[{"name":"Series 1","values":[12.,24.,18.],"color":"#2563eb"}]}}));chart.edit(value);}),
                sapid::button().theme(crate::components::ui::theme()).label("Animate first object").on_press(move||{let mut value=animation.content.get();add_default_animation(&mut value,current.get());animation.edit(value);}),
                sapid::native_select("presentation-theme").theme(crate::components::ui::theme()).name("Theme").options(["Contrix Light","Midnight","Editorial"].map(|key|sapid::select_option(key,key))).on_value_change(move|value|if let Some(value)=value{let mut next=theme.content.get();set_presentation_theme(&mut next,&value);theme.edit(next);})
            )),
            text().content("Canonical presentation JSON retains positions, images, charts, animations and theme metadata."),
            sapid::textarea().theme(crate::components::ui::theme()).name("Presentation JSON").rows(10).value(source).on_value_change(move|value|source.revise(|current|*current=value)),
            row().fill((sapid::button().theme(crate::components::ui::theme()).label("Load JSON").on_press({let session=session.clone();move||source.revise(|value|*value=serde_json::to_string_pretty(&session.content.get()).unwrap_or_default())}),
                sapid::button().theme(crate::components::ui::theme()).label("Apply JSON").on_press(move||match serde_json::from_str::<Value>(&source.get()){Ok(value)if value["slides"].is_array()=>{current.revise(|index|*index=0);apply.edit(value);status.revise(|value|value.clear());},Ok(_)=>status.revise(|value|*value="Expected an object containing slides".into()),Err(error)=>status.revise(|value|*value=error.to_string())}))),text().content(status),
        )).show_in(scope)
    })
}

#[cfg(test)]
mod glory_roundtrip_tests {
    use super::*;
    #[test]
    fn editing_text_preserves_geometry_formatting_and_slide_metadata() {
        let slides = vec![SlideData::empty(0)];
        let mut base = merge_presentation(&json!({}), &slides, "Deck");
        base["slides"][0]["elements"][0]["content"][0]["text"] = json!("Original");
        base["slides"][0]["elements"][0]["content"][0]["font_family"] = json!("Georgia");
        base["slides"][0]["elements"][0]["content"][0]["underline"] = json!(true);
        base["slides"][0]["elements"][0]["x"] = json!(245.);
        base["slides"][0]["elements"][0]["rotation"] = json!(15.);
        base["slides"][0]["custom_metadata"] = json!({"preserve":"yes"});
        base["slides"][0]["transition"] = json!({"transition_type":"Fade","duration_ms":3500});
        let mut hydrated = hydrate_presentation(&base);
        hydrated[0].title = "Original edited".into();
        let saved = merge_presentation(&base, &hydrated, "Deck");
        assert_eq!(saved["slides"][0]["elements"][0]["x"], 245.);
        assert_eq!(saved["slides"][0]["elements"][0]["rotation"], 15.);
        assert_eq!(
            saved["slides"][0]["elements"][0]["content"][0]["font_family"],
            "Georgia"
        );
        assert_eq!(
            saved["slides"][0]["elements"][0]["content"][0]["underline"],
            true
        );
        assert_eq!(saved["slides"][0]["transition"]["duration_ms"], 3500);
        assert_eq!(
            saved["slides"][0]["custom_metadata"],
            base["slides"][0]["custom_metadata"]
        );
        serde_json::from_value::<contrix_doc_engine::Presentation>(saved).unwrap();
    }
}

fn slide_background_source(content: &Value, index: usize) -> String {
    use base64::Engine;
    let slides = hydrate_presentation(content);
    let color = slides
        .get(index)
        .map(|slide| slide.bg_color.as_str())
        .unwrap_or("#ffffff");
    let color = color
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;");
    format!("data:image/svg+xml;base64,{}",base64::engine::general_purpose::STANDARD.encode(format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1920\" height=\"1080\"><rect width=\"1920\" height=\"1080\" fill=\"{color}\"/></svg>")))
}
fn slide_object_source(element: &Value) -> String {
    slide_svg(
        &json!({"slides":[{"id":"6f6dd7e4-c5eb-4f12-9485-9ac0f3e25318","elements":[element],"background":{"Solid":"transparent"}}]}),
        0,
    )
}
fn collect_slide_objects(elements: &[Value], prefix: &str, result: &mut Vec<(String, Value)>) {
    for (index, element) in elements.iter().enumerate() {
        let key = format!(
            "{prefix}-{}",
            element["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| index.to_string())
        );
        if element["type"] == "Group" {
            collect_slide_objects(
                element["elements"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                &key,
                result,
            );
        } else {
            result.push((key, element.clone()));
        }
    }
}
