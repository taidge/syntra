//! Browser-owned Writer surface. The canonical document remains the save model;
//! DOM editing never goes through the deliberately smaller Sapid RichDocument.
use glory::prelude::*;
use serde_json::{Value, json};
use std::rc::Rc;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

#[cfg(target_arch = "wasm32")]
use syntra_office_ui::writer::{create_surface, destroy_surface, format_surface, update_surface};

#[derive(Clone)]
pub(super) struct WriterController {
    #[cfg(target_arch = "wasm32")]
    handle: JsValue,
}

impl WriterController {
    pub(super) fn format(&self, format: sapid::RichFormat) -> std::result::Result<(), String> {
        format.validate().map_err(|error| error.to_string())?;
        let command = match format {
            sapid::RichFormat::Bold(value) => json!({"kind":"bold","value":value}),
            sapid::RichFormat::Italic(value) => json!({"kind":"italic","value":value}),
            sapid::RichFormat::Code(value) => json!({"kind":"code","value":value}),
            sapid::RichFormat::Link(value) => json!({"kind":"link","value":value}),
            sapid::RichFormat::Block(kind) => json!({"kind":"block","value":match kind {
                sapid::RichBlockKind::Heading(level) => format!("h{level}"),
                sapid::RichBlockKind::Quote => "blockquote".into(),
                sapid::RichBlockKind::Paragraph => "p".into(),
            }}),
        };
        #[cfg(target_arch = "wasm32")]
        {
            format_surface(&self.handle, &command.to_string())
                .map_err(|error| error.as_string().unwrap_or_else(|| format!("{error:?}")))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = command;
            Err("Writer editing requires a browser".into())
        }
    }
}

pub(super) fn writer_surface(
    _scope: &Scope,
    content: Cage<Value>,
    loaded: Cage<bool>,
    controller: Cage<Option<WriterController>>,
    on_change: Rc<dyn Fn(Value)>,
) -> impl Widget {
    widget(move |scope| {
        let identity = format!("writer-surface-{}", uuid::Uuid::new_v4().simple());
        #[cfg(target_arch = "wasm32")]
        {
            let changed = on_change.clone();
            let callback = Closure::<dyn FnMut(String)>::new(move |payload: String| {
                let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                    return;
                };
                let next = if let Some(text) = value["__writer_plain_text"]
                    .as_str()
                    .filter(|_| value.as_object().is_some_and(|object| object.len() == 1))
                {
                    // Whole-surface replacement (including accessibility fill)
                    // retains native blocks and untouched run metadata.
                    syntra_doc_engine::writer::writer_content_with_text(&content.get(), text)
                } else {
                    value
                };
                changed(next);
            });
            let handle = create_surface(&format!(".{identity}"), callback.as_ref().unchecked_ref());
            controller.revise(|value| {
                *value = Some(WriterController {
                    handle: handle.clone(),
                })
            });
            let updating = handle.clone();
            scope.effect(move || {
                update_surface(&updating, &content.get().to_string(), loaded.get())
            });
            let _ = scope.on_cleanup(move || {
                destroy_surface(&handle);
                let _ = controller.try_revise(|value| *value = None);
                drop(callback);
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (content, loaded, controller, &on_change);
        view()
            .class(format!(
                "writer-body writer-content writer-surface {identity}"
            ))
            .name("Document body")
            .show_in(scope)
    })
}
