//! Owner-scoped document transport shared by the Glory editors.
use crate::collab_client::{CollabHandle, RemoteCursor};
#[cfg(target_arch = "wasm32")]
use crate::collab_client::{CollabMessage, ConnectionState, connect_with_reconnect};
use crate::state::{AuthState, api_get, api_put, export_document};
use gloo_timers::callback::Timeout;
use glory::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Clone, Deserialize)]
struct Document {
    name: String,
    content: Value,
}

#[derive(Clone)]
pub(super) struct EditorSession {
    pub id: Rc<String>,
    pub auth: Cage<AuthState>,
    pub name: Cage<String>,
    pub content: Cage<Value>,
    pub status: Cage<String>,
    pub loaded: Cage<bool>,
    pub peers: Cage<Vec<RemoteCursor>>,
    pub connection: Cage<String>,
    remote: Cage<Option<Value>>,
    dirty: Rc<Cell<bool>>,
    scope: glory::ui::WeakScope,
    kind: &'static str,
    timer: Rc<RefCell<Option<Timeout>>>,
    socket: Rc<RefCell<Option<CollabHandle>>>,
    undo: Rc<RefCell<Vec<Value>>>,
    redo: Rc<RefCell<Vec<Value>>>,
    pending: Rc<RefCell<Option<Value>>>,
    exports: Rc<RefCell<std::collections::VecDeque<&'static str>>>,
    saving: Rc<Cell<bool>>,
}
impl EditorSession {
    pub fn new(scope: &Scope, auth: Cage<AuthState>, id: String, kind: &'static str) -> Self {
        let session = Self {
            id: Rc::new(id),
            auth,
            name: scope.cage(String::new()),
            content: scope.cage(Value::Null),
            status: scope.cage(crate::i18n::t("editor.loading_document")),
            loaded: scope.cage(false),
            peers: scope.cage(Vec::new()),
            connection: scope.cage(String::new()),
            remote: scope.cage(None),
            dirty: Default::default(),
            scope: scope.downgrade(),
            kind,
            timer: Default::default(),
            socket: Default::default(),
            undo: Default::default(),
            redo: Default::default(),
            pending: Default::default(),
            exports: Default::default(),
            saving: Default::default(),
        };
        let timer = session.timer.clone();
        let socket = session.socket.clone();
        let _ = scope.on_cleanup(move || {
            timer.borrow_mut().take();
            if let Some(socket) = socket.borrow_mut().take() {
                socket.close();
            }
        });
        session.reload();
        session.connect();
        session
    }
    pub fn token(&self) -> String {
        self.auth.get().token.unwrap_or_default()
    }
    pub fn reload(&self) {
        let Some(scope) = self.scope.upgrade() else {
            return;
        };
        let session = self.clone();
        let _ = scope.spawn(async move {
            let response =
                api_get::<Document>(&format!("/documents/{}", session.id), &session.token()).await;
            if session.scope.upgrade().is_none() {
                return;
            }
            match response {
                Ok(document) => {
                    session.name.revise(|name| *name = document.name);
                    session.content.revise(|value| *value = document.content);
                    session.loaded.revise(|value| *value = true);
                    session.dirty.set(false);
                    session
                        .status
                        .revise(|value| *value = crate::i18n::t("common.saved"));
                    session.undo.borrow_mut().clear();
                    session.redo.borrow_mut().clear();
                }
                Err(error) => session.status.revise(|value| *value = error),
            }
        });
    }
    fn connect(&self) {
        #[cfg(target_arch = "wasm32")]
        {
            let incoming = self.clone();
            let state = self.clone();
            let handle = connect_with_reconnect(
                (*self.id).clone(),
                self.token(),
                move |message| {
                    if incoming.scope.upgrade().is_none() {
                        return;
                    }
                    match message {
                        CollabMessage::UserJoined {
                            user_id,
                            username,
                            color,
                        } => incoming.peers.revise(|peers| {
                            if !peers.iter().any(|peer| peer.user_id == user_id) {
                                peers.push(RemoteCursor {
                                    user_id,
                                    username,
                                    color,
                                    position: None,
                                });
                            }
                        }),
                        CollabMessage::UserLeft { user_id } => incoming
                            .peers
                            .revise(|peers| peers.retain(|peer| peer.user_id != user_id)),
                        CollabMessage::CursorUpdate(payload) => {
                            let user = payload["user_id"].as_str().unwrap_or_default();
                            incoming.peers.revise(|peers| {
                                if let Some(peer) =
                                    peers.iter_mut().find(|peer| peer.user_id == user)
                                {
                                    peer.position = payload
                                        .pointer("/position/offset")
                                        .and_then(Value::as_u64)
                                        .map(|n| n as u32);
                                }
                            });
                        }
                        CollabMessage::Operation(payload) => {
                            let user = incoming
                                .auth
                                .get()
                                .user
                                .map(|user| user.id.to_string())
                                .unwrap_or_default();
                            if payload["user_id"].as_str() == Some(user.as_str()) {
                                return;
                            }
                            let operation = &payload["op_type"];
                            let path = operation["path"].as_array();
                            if operation["type"] != "Replace"
                                || path.is_none_or(|path| path.len() != 1)
                            {
                                return;
                            }
                            let path = path.unwrap()[0].as_str().unwrap_or_default();
                            let expected = if incoming.kind == "writer" {
                                "body_text"
                            } else {
                                incoming.kind
                            };
                            if path != expected {
                                return;
                            }
                            let next = if incoming.kind == "writer" {
                                super::writer::writer_content_with_text(
                                    &incoming.content.get(),
                                    operation["new_value"].as_str().unwrap_or_default(),
                                )
                            } else {
                                operation["new_value"].clone()
                            };
                            if incoming.dirty.get() {
                                incoming.remote.revise(|value| *value = Some(next));
                            } else {
                                incoming.content.revise(|value| *value = next);
                                incoming
                                    .status
                                    .revise(|value| *value = crate::i18n::t("common.saved"));
                            }
                        }
                        CollabMessage::Error { message } => {
                            incoming.status.revise(|value| *value = message)
                        }
                        _ => {}
                    }
                },
                move |connection| {
                    if state.scope.upgrade().is_none() {
                        return;
                    }
                    state.connection.revise(|value| {
                        *value = match connection {
                            ConnectionState::Connecting => "Connecting".into(),
                            ConnectionState::Connected => "Connected".into(),
                            ConnectionState::Reconnecting(attempt) => {
                                format!("Reconnecting ({attempt})")
                            }
                            ConnectionState::Failed => "Disconnected".into(),
                        }
                    });
                },
            );
            *self.socket.borrow_mut() = Some(handle);
        }
    }
    pub fn edit(&self, next: Value) {
        if !self.loaded.get() || self.content.get() == next {
            return;
        }
        let previous = self.content.get();
        let mut undo = self.undo.borrow_mut();
        undo.push(previous.clone());
        if undo.len() > 32 {
            undo.remove(0);
        }
        drop(undo);
        self.redo.borrow_mut().clear();
        self.replace(next, previous);
    }
    fn replace(&self, next: Value, previous: Value) {
        self.dirty.set(true);
        self.content.revise(|value| *value = next.clone());
        if let Some(socket) = self.socket.borrow().as_ref() {
            let user = self
                .auth
                .get()
                .user
                .map(|user| user.id.to_string())
                .unwrap_or_default();
            let operation = if self.kind == "writer" {
                json!({"type":"Replace", "path":["body_text"], "old_value":super::writer::writer_content_to_plain(&previous), "new_value":super::writer::writer_content_to_plain(&next)})
            } else {
                json!({"type":"Replace", "path":[self.kind], "old_value":previous, "new_value":next})
            };
            let _ = socket.send_operation(&user, operation);
        }
        self.schedule_save();
    }
    pub fn undo(&self, redo: bool) {
        let value = if redo {
            self.redo.borrow_mut().pop()
        } else {
            self.undo.borrow_mut().pop()
        };
        if let Some(value) = value {
            let previous = self.content.get();
            if redo {
                self.undo.borrow_mut().push(previous.clone());
            } else {
                self.redo.borrow_mut().push(previous.clone());
            }
            self.replace(value, previous);
        }
    }
    pub fn schedule_save(&self) {
        self.status
            .revise(|value| *value = crate::i18n::t("common.unsaved"));
        #[cfg(target_arch = "wasm32")]
        {
            let session = self.clone();
            *self.timer.borrow_mut() = Some(Timeout::new(1000, move || session.save()));
        }
    }
    // Serialize PUTs and coalesce queued snapshots so older responses never win.
    pub fn save(&self) {
        if !self.loaded.get() {
            return;
        }
        *self.pending.borrow_mut() = Some(self.content.get());
        if self.saving.replace(true) {
            return;
        }
        let Some(scope) = self.scope.upgrade() else {
            self.saving.set(false);
            return;
        };
        let session = self.clone();
        let _ = scope.spawn(async move {
            loop {
                let body = session.pending.borrow_mut().take();
                let Some(body) = body else {
                    let format = session.exports.borrow_mut().pop_front();
                    if let Some(format) = format {
                        let result = export_document(&session.id, format, &session.token()).await;
                        if session.scope.upgrade().is_none() {
                            break;
                        }
                        if let Err(error) = result {
                            session.status.revise(|value| *value = error);
                        }
                        continue;
                    }
                    break;
                };
                session
                    .status
                    .revise(|value| *value = crate::i18n::t("common.saving"));
                let result = api_put::<Value, _>(
                    &format!("/documents/{}", session.id),
                    &json!({"content":body}),
                    &session.token(),
                )
                .await;
                if session.scope.upgrade().is_none() {
                    break;
                }
                match result {
                    Ok(_) => {
                        if session.content.get() == body {
                            session.dirty.set(false);
                        }
                        session.status.revise(|value| {
                            *value = if session.content.get() == body {
                                crate::i18n::t("common.saved")
                            } else {
                                crate::i18n::t("common.unsaved")
                            }
                        });
                    }
                    Err(error) => {
                        session.exports.borrow_mut().clear();
                        session.status.revise(|value| {
                            *value = crate::i18n::t("editor.save_failed").replace("{err}", &error)
                        });
                    }
                }
            }
            session.saving.set(false);
        });
    }
    pub fn export(&self, format: &'static str) {
        self.exports.borrow_mut().push_back(format);
        self.save();
    }
}
pub(super) fn editor_header(
    session: &EditorSession,
    kind: &'static str,
    formats: &'static [&'static str],
) -> impl Widget {
    let save = session.clone();
    let undo = session.clone();
    let redo = session.clone();
    let accept_remote = session.clone();
    let keep_local = session.clone();
    let retry = session.clone();
    let exports = formats
        .iter()
        .map(|format| {
            let export = session.clone();
            let format = *format;
            sapid::button()
                .theme(crate::components::ui::theme())
                .label(format.to_ascii_uppercase())
                .on_press(move || export.export(format))
        })
        .collect::<Vec<_>>();
    col().class("editor-header").fill((
        row().class("editor-topbar").fill((
            link().target("/files").label("← Files"),
            text().content(kind.to_string()),
            text().class("editor-title").content(session.name),
            text().content(session.status),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label(crate::i18n::t("common.save"))
                .enabled(session.loaded)
                .on_press(move || save.save()),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Undo")
                .enabled(session.loaded)
                .on_press(move || undo.undo(false)),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Redo")
                .enabled(session.loaded)
                .on_press(move || redo.undo(true)),
            row().fill(exports),
        )),
        row().class("editor-presence").fill((
            text().content(session.connection),
            text().content(session.peers.map(|peers| {
                peers
                    .iter()
                    .map(|peer| peer.username.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })),
        )),
        row().class("editor-conflict").fill((
            text().content(session.remote.map(|remote| {
                if remote.is_some() {
                    "A collaborator changed this document while you have unsaved edits.".to_string()
                } else {
                    String::new()
                }
            })),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Use collaborator's version")
                .enabled(session.remote.map(Option::is_some))
                .on_press(move || {
                    if let Some(next) = accept_remote.remote.get() {
                        accept_remote.remote.revise(|value| *value = None);
                        accept_remote.edit(next);
                        accept_remote.save();
                    }
                }),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Keep my version")
                .enabled(session.remote.map(Option::is_some))
                .on_press(move || {
                    if let Some(previous) = keep_local.remote.get() {
                        keep_local.remote.revise(|value| *value = None);
                        keep_local.replace(keep_local.content.get(), previous);
                        keep_local.save();
                    }
                }),
            sapid::button()
                .theme(crate::components::ui::theme())
                .label("Retry loading")
                .enabled(session.loaded.map(|loaded| !*loaded))
                .on_press(move || retry.reload()),
        )),
    ))
}

/// Separate semantic images preserve object paint order and let the browser
/// load remote image resources normally. External resources inside an SVG
/// displayed as an image are intentionally blocked by browsers.
pub(super) fn diagram_objects(
    items: impl Into<Lotus<Vec<(String, Value)>>>,
    vector_source: fn(&Value) -> String,
    width: f64,
    height: f64,
) -> impl Widget {
    each(
        items,
        |item| format!("{}:{}", item.0, item.1["type"].as_str().unwrap_or_default()),
        move |item, _| {
            let external = item.get().1["type"] == "Image";
            image()
                .class(if external {
                    "diagram-object diagram-image"
                } else {
                    "diagram-object diagram-vector"
                })
                .source(item.map(move |item| {
                    if external {
                        item.1["url"].as_str().unwrap_or_default().to_string()
                    } else {
                        vector_source(&item.1)
                    }
                }))
                .alt(item.map(|item| {
                    item.1["alt_text"]
                        .as_str()
                        .or_else(|| item.1["type"].as_str())
                        .unwrap_or("Document object")
                        .to_string()
                }))
                .style(item.map(move |item| {
                    let number = |key: &str, fallback: f64| {
                        item.1[key]
                            .as_f64()
                            .filter(|value| value.is_finite())
                            .unwrap_or(fallback)
                    };
                    let (x, y, w, h) = if external {
                        (
                            number("x", 0.),
                            number("y", 0.),
                            number("width", 100.).max(0.01),
                            number("height", 100.).max(0.01),
                        )
                    } else {
                        (0., 0., width, height)
                    };
                    Style::new()
                        .position(Position::Absolute)
                        .left(Length::Percent(x / width * 100.))
                        .top(Length::Percent(y / height * 100.))
                        .width(Length::Percent(w / width * 100.))
                        .height(Length::Percent(h / height * 100.))
                }))
        },
    )
}

#[allow(non_snake_case)]
pub fn DocumentEditor(_scope: Scope, auth: Cage<AuthState>, id: String) -> impl Widget {
    widget(move |scope| {
        let status = scope.cage(crate::i18n::t("editor.loading_document"));
        let document_id = id.clone();
        let weak = scope.downgrade();
        let _ = scope.spawn(async move {
            match api_get::<Value>(
                &format!("/documents/{document_id}"),
                &auth.get().token.unwrap_or_default(),
            )
            .await
            {
                Ok(document) if weak.upgrade().is_some() => {
                    let kind = document["document_type"]
                        .as_str()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    if ["writer", "calc", "slides", "whiteboard"].contains(&kind.as_str()) {
                        #[cfg(target_arch = "wasm32")]
                        if let Some(window) = web_sys::window() {
                            let _ = window.location().replace(&format!("/{kind}/{document_id}"));
                        }
                    } else {
                        status
                            .revise(|value| *value = format!("Unsupported document type: {kind}"));
                    }
                }
                Err(error) if weak.upgrade().is_some() => status.revise(|value| *value = error),
                _ => {}
            }
        });
        col()
            .class("loading-page")
            .fill((
                text().content(status),
                text().content(format!("Document: {id}")),
                row().fill(
                    ["writer", "calc", "slides", "whiteboard"]
                        .into_iter()
                        .map(|kind| link().label(kind).target(format!("/{kind}/{id}")))
                        .collect::<Vec<_>>(),
                ),
                link().target("/files").label("Files"),
            ))
            .show_in(scope)
    })
}
