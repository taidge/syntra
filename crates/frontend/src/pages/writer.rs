use super::document_editor::{EditorSession, editor_header};
use super::writer_surface::{WriterController, writer_surface};
use crate::state::AuthState;
use glory::prelude::*;
use serde_json::{Value, json};
use std::rc::Rc;
#[cfg(test)]
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

#[cfg(test)]
use contrix_doc_engine::writer::extract_run_text;
pub(super) use contrix_doc_engine::writer::{
    plain_writer_run, replace_writer_runs, writer_content_to_plain, writer_content_with_text,
};

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

#[allow(non_snake_case)]
pub fn Writer(_scope: Scope, auth: Cage<AuthState>, id: String) -> impl Widget {
    widget(move |scope| {
        let session = EditorSession::new(scope, auth, id.clone(), "writer");
        let comments = scope.cage(false);
        let versions = scope.cage(false);
        let settings = scope.cage(false);
        let find_open = scope.cage(false);
        let insert_open = scope.cage(false);
        let sidebar_open = scope.cage(true);
        let search = scope.cage(String::new());
        let replacement = scope.cage(String::new());
        let link_url = scope.cage(String::new());
        let tracking = scope.cage(false);
        let content = session.content;
        let controller = scope.cage(None::<WriterController>);
        let change = session.clone();
        let editor = writer_surface(
            scope,
            content,
            session.loaded,
            controller,
            Rc::new(move |mut next| {
                let base = change.content.get();
                let text = writer_content_to_plain(&next);
                if tracking.get() && writer_content_to_plain(&base) != text {
                    let user = auth
                        .get()
                        .user
                        .map(|user| user.username)
                        .unwrap_or_default();
                    let mut revisions = base
                        .pointer("/professional/revisions")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    revisions.push(json!({"id":uuid::Uuid::new_v4(),"author":user,"timestamp":chrono::Utc::now(),
                        "block_index":0,"kind":"Insert","text":text}));
                    next = update_professional_field(&next, "revisions", json!(revisions));
                }
                change.edit(next);
            }),
        );
        let replace = session.clone();
        let add_link = session.clone();
        let settings_session = session.clone();
        let settings_widget = when(settings, move || writer_settings(settings_session.clone()));
        let count = content.map(|value| {
            let text = writer_content_to_plain(value);
            format!(
                "{} words · {} characters · ~{} pages",
                text.split_whitespace().count(),
                text.chars().count(),
                (text.chars().count() / 3000).max(1)
            )
        });
        let menu_session = session.clone();
        let menu_commands: Rc<dyn Fn(&str)> = Rc::new(move |command| match command {
            "find" => find_open.revise(|value| *value = true),
            "toggle-sidebar" => sidebar_open.revise(|value| *value = !*value),
            "comments" => comments.revise(|value| *value = !*value),
            "versions" => versions.revise(|value| *value = !*value),
            "settings" => settings.revise(|value| *value = !*value),
            "tracking" => tracking.revise(|value| *value = !*value),
            "insert-controls" => insert_open.revise(|value| *value = !*value),
            command if command.starts_with("insert-") => insert_writer_kind(&menu_session, command),
            command if command.starts_with("format-") => {
                if let Some(controller) = controller.get() {
                    let format = match command {
                        "format-bold" => sapid::RichFormat::Bold(true),
                        "format-italic" => sapid::RichFormat::Italic(true),
                        "format-code" => sapid::RichFormat::Code(true),
                        "format-heading" => {
                            sapid::RichFormat::Block(sapid::RichBlockKind::Heading(2))
                        }
                        _ => sapid::RichFormat::Block(sapid::RichBlockKind::Paragraph),
                    };
                    let _ = controller.format(format);
                }
            }
            _ => {}
        });
        col()
            .class("writer-app editor-shell")
            .fill((
                editor_header(
                    &session,
                    "Writer",
                    &["docx", "pdf", "html", "markdown", "odt", "txt"],
                    menu_commands,
                ),
                writer_formatting(controller, session.loaded, tracking, settings),
                when(find_open, move || {
                    let replace = replace.clone();
                    let add_link = add_link.clone();
                    row().class("editor-toolbar writer-find-toolbar").fill((
                        sapid::text_field()
                            .theme(crate::components::ui::theme())
                            .name("Find")
                            .value(search)
                            .on_value_change(move |value| {
                                search.revise(|current| *current = value)
                            }),
                        sapid::text_field()
                            .theme(crate::components::ui::theme())
                            .name("Replace with")
                            .value(replacement)
                            .on_value_change(move |value| {
                                replacement.revise(|current| *current = value)
                            }),
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .label("Replace all")
                            .on_press(move || {
                                let query = search.get();
                                if !query.is_empty() {
                                    let base = replace.content.get();
                                    let text = writer_content_to_plain(&base)
                                        .replace(&query, &replacement.get());
                                    replace.edit(writer_content_with_text(&base, &text));
                                }
                            }),
                        sapid::text_field()
                            .theme(crate::components::ui::theme())
                            .name("Link URL")
                            .value(link_url)
                            .on_value_change(move |value| {
                                link_url.revise(|current| *current = value)
                            }),
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .label("Link selection")
                            .on_press(move || {
                                if let Some(controller) = controller.get()
                                    && let Err(error) = controller
                                        .format(sapid::RichFormat::Link(Some(link_url.get())))
                                {
                                    add_link.status.revise(|value| *value = error.to_string());
                                }
                            }),
                    ))
                }),
                when(insert_open, {
                    let session = session.clone();
                    move || writer_insert_controls(session.clone())
                }),
                row().class("editor-main writer-main").fill((
                    when(sidebar_open, move || {
                        widget(move |scope| {
                            crate::components::sidebar::Sidebar(scope.clone(), auth).build(scope)
                        })
                    }),
                    col()
                        .class("editor-canvas writer-canvas")
                        .fill(col().class("page writer-page").fill(editor)),
                    settings_widget,
                    crate::components::comments_pane::CommentsPane(
                        scope.clone(),
                        auth,
                        id.clone(),
                        comments,
                    ),
                )),
                text().class("status-bar editor-statusbar").content(count),
                crate::components::version_history::VersionHistory(
                    scope.clone(),
                    auth,
                    id.clone(),
                    versions,
                    session.version,
                ),
                crate::components::ai_assist::AiAssist(scope.clone(), auth, id.clone()),
            ))
            .show_in(scope)
    })
}

fn writer_formatting(
    controller: Cage<Option<WriterController>>,
    enabled: Cage<bool>,
    tracking: Cage<bool>,
    settings: Cage<bool>,
) -> impl Widget {
    widget(move |scope| {
        let buttons = [("Bold", "B"), ("Italic", "I")]
            .into_iter()
            .enumerate()
            .map(|(index, (name, label))| {
                let active = scope.cage(false);
                sapid::button()
                    .theme(crate::components::ui::theme())
                    .class(scope.bond(move || {
                        if active.get() {
                            "toolbar-btn active".into()
                        } else {
                            "toolbar-btn".into()
                        }
                    }))
                    .name(name)
                    .label(label)
                    .enabled(enabled)
                    .on_press(move || {
                        let value = !active.get();
                        active.revise(|active| *active = value);
                        if let Some(controller) = controller.get() {
                            let format = match index {
                                0 => sapid::RichFormat::Bold(value),
                                1 => sapid::RichFormat::Italic(value),
                                _ => sapid::RichFormat::Code(value),
                            };
                            let _ = controller.format(format);
                        }
                    })
            })
            .collect::<Vec<_>>();
        row()
            .class("toolbar writer-format-toolbar")
            .fill((
                sapid::native_select("writer-font-family")
                    .theme(crate::components::ui::theme())
                    .class("toolbar-select font-family")
                    .name("Font family")
                    .enabled(false)
                    .default_value(Some("Arial".to_string()))
                    .options([sapid::select_option("Arial", "Arial")]),
                sapid::native_select("writer-font-size")
                    .theme(crate::components::ui::theme())
                    .class("toolbar-select font-size")
                    .name("Font size")
                    .enabled(false)
                    .default_value(Some("12".to_string()))
                    .options([sapid::select_option("12", "12pt")]),
                view().class("toolbar-separator"),
                row().class("toolbar-group").fill((
                    group().fill(buttons),
                    sapid::button()
                        .theme(crate::components::ui::theme())
                        .class("toolbar-btn underline")
                        .name("Underline")
                        .label("U")
                        .enabled(false),
                    sapid::button()
                        .theme(crate::components::ui::theme())
                        .class("toolbar-btn strikethrough")
                        .name("Strikethrough")
                        .label("S")
                        .enabled(false),
                )),
                view().class("toolbar-separator"),
                row().class("toolbar-group").fill(
                    [
                        ("Bulleted list", "* List"),
                        ("Numbered list", "1. List"),
                        ("Indent", ">>"),
                        ("Outdent", "<<"),
                    ]
                    .into_iter()
                    .map(|(name, label)| {
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .class("toolbar-btn")
                            .name(name)
                            .label(label)
                            .enabled(false)
                    })
                    .collect::<Vec<_>>(),
                ),
                view().class("toolbar-separator"),
                row().class("toolbar-group").fill(
                    [
                        ("Align left", "L"),
                        ("Align center", "C"),
                        ("Align right", "R"),
                        ("Justify", "J"),
                    ]
                    .into_iter()
                    .map(|(name, label)| {
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .class("toolbar-btn")
                            .name(name)
                            .label(label)
                            .enabled(false)
                    })
                    .collect::<Vec<_>>(),
                ),
            ))
            .fill((
                view().class("toolbar-separator"),
                sapid::button()
                    .theme(crate::components::ui::theme())
                    .class("toolbar-color writer-text-color")
                    .name("Text color")
                    .label("■")
                    .enabled(false),
                sapid::button()
                    .theme(crate::components::ui::theme())
                    .class("toolbar-color writer-highlight-color")
                    .name("Highlight color")
                    .label("■")
                    .enabled(false),
                view().class("toolbar-separator"),
                sapid::button()
                    .theme(crate::components::ui::theme())
                    .class("toolbar-btn")
                    .name("Track changes")
                    .label("TC")
                    .on_press(move || tracking.revise(|value| *value = !*value)),
                sapid::button()
                    .theme(crate::components::ui::theme())
                    .class("toolbar-btn")
                    .label("Changes")
                    .on_press(move || settings.revise(|value| *value = !*value)),
            ))
            .show_in(scope)
    })
}

fn writer_settings(session: EditorSession) -> impl Widget {
    widget(move |scope| {
        let content = session.content;
        let header = content.map(|value| {
            value
                .pointer("/professional/header")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        let footer = content.map(|value| {
            value
                .pointer("/professional/footer")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        let header_edit = session.clone();
        let footer_edit = session.clone();
        let portrait = session.clone();
        let landscape = session.clone();
        let revisions = scope.cage(String::new());
        let revision_session = session.clone();
        let clear = session.clone();
        col()
            .class("writer-settings")
            .fill((
                sapid::title(2)
                    .theme(crate::components::ui::theme())
                    .content("Page & document settings"),
                sapid::text_field()
                    .theme(crate::components::ui::theme())
                    .name("Header")
                    .value(header)
                    .on_value_change(move |value| {
                        header_edit.edit(update_professional_field(
                            &header_edit.content.get(),
                            "header",
                            json!(value),
                        ))
                    }),
                sapid::text_field()
                    .theme(crate::components::ui::theme())
                    .name("Footer")
                    .value(footer)
                    .on_value_change(move |value| {
                        footer_edit.edit(update_professional_field(
                            &footer_edit.content.get(),
                            "footer",
                            json!(value),
                        ))
                    }),
                row().fill((
                    sapid::button()
                        .theme(crate::components::ui::theme())
                        .label("Portrait")
                        .on_press(move || {
                            let mut value = portrait.content.get();
                            value["page_settings"]["orientation"] = json!("Portrait");
                            value["page_settings"]["width_mm"] = json!(210);
                            value["page_settings"]["height_mm"] = json!(297);
                            portrait.edit(value);
                        }),
                    sapid::button()
                        .theme(crate::components::ui::theme())
                        .label("Landscape")
                        .on_press(move || {
                            let mut value = landscape.content.get();
                            value["page_settings"]["orientation"] = json!("Landscape");
                            value["page_settings"]["width_mm"] = json!(297);
                            value["page_settings"]["height_mm"] = json!(210);
                            landscape.edit(value);
                        }),
                    sapid::button()
                        .theme(crate::components::ui::theme())
                        .label("Accept all tracked changes")
                        .on_press(move || {
                            clear.edit(update_professional_field(
                                &clear.content.get(),
                                "revisions",
                                json!([]),
                            ))
                        }),
                )),
                text().content(content.map(|value| {
                    format!(
                        "Pending tracked changes: {}",
                        value
                            .pointer("/professional/revisions")
                            .and_then(Value::as_array)
                            .map_or(0, Vec::len)
                    )
                })),
                sapid::textarea()
                    .theme(crate::components::ui::theme())
                    .name("Professional metadata JSON")
                    .rows(8)
                    .value(revisions)
                    .on_value_change(move |value| revisions.revise(|current| *current = value)),
                sapid::button()
                    .theme(crate::components::ui::theme())
                    .label("Apply metadata JSON")
                    .on_press(
                        move || match serde_json::from_str::<Value>(&revisions.get()) {
                            Ok(value) if value.is_object() => {
                                let mut next = revision_session.content.get();
                                next["professional"] = value;
                                revision_session.edit(next);
                            }
                            Ok(_) => revision_session
                                .status
                                .revise(|value| *value = "Metadata must be a JSON object".into()),
                            Err(error) => revision_session
                                .status
                                .revise(|value| *value = error.to_string()),
                        },
                    ),
            ))
            .show_in(scope)
    })
}

fn append_writer_block(session: &EditorSession, block: Value) {
    let mut content = session.content.get();
    if let Some(body) = content["body"].as_array_mut() {
        body.push(block);
        session.edit(content);
    }
}
fn insert_writer_kind(session: &EditorSession, command: &str) {
    match command {
        "insert-table" => {
            let paragraph = || json!({"type":"Paragraph","id":uuid::Uuid::new_v4(),"runs":[plain_writer_run("")],"alignment":"Left","indent_level":0,"spacing":{"before":0.,"after":8.,"line_height":1.15}});
            append_writer_block(
                session,
                json!({"type":"Table","id":uuid::Uuid::new_v4(),"column_widths":[240.,240.],"rows":(0..2).map(|_|json!({"cells":(0..2).map(|_|json!({"blocks":[paragraph()],"col_span":1,"row_span":1,"background_color":null})).collect::<Vec<_>>()})).collect::<Vec<_>>()}),
            );
        }
        "insert-list" => append_writer_block(
            session,
            json!({"type":"List","id":uuid::Uuid::new_v4(),"ordered":false,"items":[{"runs":[plain_writer_run("List item")],"children":[]}]}),
        ),
        "insert-code" => append_writer_block(
            session,
            json!({"type":"CodeBlock","id":uuid::Uuid::new_v4(),"language":null,"code":""}),
        ),
        "insert-image" => {
            #[cfg(target_arch = "wasm32")]
            if let Some(url) = web_sys::window()
                .and_then(|window| window.prompt_with_message("Image URL").ok().flatten())
                .filter(|url| !url.trim().is_empty())
            {
                append_writer_block(
                    session,
                    json!({"type":"Image","id":uuid::Uuid::new_v4(),"url":url,"alt_text":"Document image","width":null,"height":null,"alignment":"Left"}),
                );
            }
        }
        "insert-page-break" => append_writer_block(session, json!({"type":"PageBreak"})),
        _ => {}
    }
}
fn writer_insert_controls(session: EditorSession) -> impl Widget {
    let table = session.clone();
    let list = session.clone();
    let code = session.clone();
    let image = session.clone();
    let page = session.clone();
    row().class("editor-toolbar").fill((
        sapid::button().theme(crate::components::ui::theme()).label("Insert table").on_press(move||{
            let paragraph=||json!({"type":"Paragraph","id":uuid::Uuid::new_v4(),"runs":[plain_writer_run("")],"alignment":"Left","indent_level":0,"spacing":{"before":0.,"after":8.,"line_height":1.15}});
            append_writer_block(&table,json!({"type":"Table","id":uuid::Uuid::new_v4(),"column_widths":[240.,240.],"rows":(0..2).map(|_|json!({"cells":(0..2).map(|_|json!({"blocks":[paragraph()],"col_span":1,"row_span":1,"background_color":null})).collect::<Vec<_>>()})).collect::<Vec<_>>()}));
        }),
        sapid::button().theme(crate::components::ui::theme()).label("Insert list").on_press(move||append_writer_block(&list,json!({"type":"List","id":uuid::Uuid::new_v4(),"ordered":false,"items":[{"runs":[plain_writer_run("List item")],"children":[]}]}))),
        sapid::button().theme(crate::components::ui::theme()).label("Insert code block").on_press(move||append_writer_block(&code,json!({"type":"CodeBlock","id":uuid::Uuid::new_v4(),"language":null,"code":""}))),
        sapid::button().theme(crate::components::ui::theme()).label("Insert image").on_press(move||{
            #[cfg(target_arch="wasm32")]
            if let Some(url)=web_sys::window().and_then(|window|window.prompt_with_message("Image URL").ok().flatten()).filter(|url|!url.trim().is_empty()){
                append_writer_block(&image,json!({"type":"Image","id":uuid::Uuid::new_v4(),"url":url,"alt_text":"Document image","width":null,"height":null,"alignment":"Left"}));
            }
            #[cfg(not(target_arch="wasm32"))]
            let _=&image;
        }),
        sapid::button().theme(crate::components::ui::theme()).label("Insert page break").on_press(move||append_writer_block(&page,json!({"type":"PageBreak"}))),
    ))
}
