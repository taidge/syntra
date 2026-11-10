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
    version: i32,
}

#[derive(Clone)]
pub(super) struct EditorSession {
    pub id: Rc<String>,
    pub auth: Cage<AuthState>,
    pub name: Cage<String>,
    pub content: Cage<Value>,
    pub status: Cage<String>,
    pub loaded: Cage<bool>,
    pub version: Cage<i32>,
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
    submitted: Rc<RefCell<Option<Value>>>,
    remote_version: Rc<Cell<i32>>,
}
impl EditorSession {
    fn prepare(scope: &Scope, auth: Cage<AuthState>, id: String, kind: &'static str) -> Self {
        Self {
            id: Rc::new(id),
            auth,
            name: scope.cage(String::new()),
            content: scope.cage(Value::Null),
            status: scope.cage(crate::i18n::t("editor.loading_document")),
            loaded: scope.cage(false),
            version: scope.cage(0),
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
            submitted: Default::default(),
            remote_version: Default::default(),
        }
    }
    pub fn new(scope: &Scope, auth: Cage<AuthState>, id: String, kind: &'static str) -> Self {
        let session = Self::prepare(scope, auth, id, kind);
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
                    if document.version < session.version.get() {
                        return;
                    }
                    if session.dirty.get() {
                        session.receive_snapshot(document.content, document.version);
                        return;
                    }
                    session.version.revise(|value| *value = document.version);
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
    fn receive_snapshot(&self, content: Value, version: i32) {
        if version <= self.version.get() {
            return;
        }
        if self.submitted.borrow().as_ref() == Some(&content) {
            self.version.revise(|value| *value = version);
        } else if self.dirty.get() && self.content.get() != content {
            self.remote.revise(|value| *value = Some(content));
            self.remote_version.set(version);
            self.timer.borrow_mut().take();
            self.status.revise(|value| {
                *value = "Conflict: local changes preserved; choose a version before saving.".into()
            });
        } else {
            self.version.revise(|value| *value = version);
            self.content.revise(|value| *value = content);
            self.loaded.revise(|value| *value = true);
            self.dirty.set(false);
            self.status
                .revise(|value| *value = crate::i18n::t("common.saved"));
        }
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
                            let legacy_writer = incoming.kind == "writer" && path == "body_text";
                            if path != incoming.kind && !legacy_writer {
                                return;
                            }
                            let next = if legacy_writer {
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
                        CollabMessage::DocumentState { content, version } => {
                            incoming.receive_snapshot(
                                content,
                                i32::try_from(version).unwrap_or(i32::MAX),
                            );
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
    fn replace(&self, next: Value, _previous: Value) {
        self.dirty.set(true);
        self.content.revise(|value| *value = next.clone());
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
    fn snapshot_for_save(&self) -> Option<Value> {
        debug_assert!(matches!(
            self.kind,
            "writer" | "presentation" | "whiteboard"
        ));
        (self.loaded.get() && self.remote.get().is_none()).then(|| self.content.get())
    }
    pub fn save(&self) {
        let Some(content) = self.snapshot_for_save() else {
            return;
        };
        *self.pending.borrow_mut() = Some(content);
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
                if session.remote.get().is_some() {
                    break;
                }
                *session.submitted.borrow_mut() = Some(body.clone());
                let expected_version = session.version.get();
                session
                    .status
                    .revise(|value| *value = crate::i18n::t("common.saving"));
                let result = api_put::<Value, _>(
                    &format!("/documents/{}", session.id),
                    &json!({"content":body,"expected_version":expected_version}),
                    &session.token(),
                )
                .await;
                if session.scope.upgrade().is_none() {
                    break;
                }
                match result {
                    Ok(response) => {
                        if let Some(version) = response["version"].as_i64() {
                            session
                                .version
                                .revise(|value| *value = (*value).max(version as i32));
                        }
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
                        break;
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
    commands: Rc<dyn Fn(&str)>,
) -> impl Widget {
    let undo = session.clone();
    let redo = session.clone();
    let accept_remote = session.clone();
    let keep_local = session.clone();
    let retry = session.clone();
    let present = commands.clone();
    let session = session.clone();
    widget(move |scope| {
        let undo = undo.clone();
        let redo = redo.clone();
        let accept_remote = accept_remote.clone();
        let keep_local = keep_local.clone();
        let retry = retry.clone();
        let present = present.clone();
        let conflict = session.remote;
        let retry_visible = scope.bond({
            let session = session.clone();
            move || {
                !session.loaded.get()
                    && session.status.get() != crate::i18n::t("editor.loading_document")
            }
        });
        col()
            .class(format!(
                "editor-header editor-header-{}",
                kind.to_lowercase()
            ))
            .fill((
                crate::components::navbar::Navbar(scope.clone(), session.auth),
                row().class("doc-title-bar editor-topbar").fill((
                    link().target("/dashboard").class("editor-back").label("←"),
                    text()
                        .class("document-type-badge")
                        .content(kind.to_string()),
                    text()
                        .class("doc-title-input editor-title")
                        .content(session.name),
                    text().class("save-status").content(session.status),
                    row().class("editor-presence").fill((
                        text()
                            .class("connection-status")
                            .content(session.connection),
                        text().content(session.peers.map(|peers| {
                            peers
                                .iter()
                                .map(|peer| peer.username.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })),
                    )),
                    when(kind == "Slides", move || {
                        let present = present.clone();
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .class("btn-present editor-present")
                            .label("Present")
                            .on_press(move || present("present"))
                    }),
                )),
                row().class("editor-commandbar").fill((
                    editor_menus(&session, kind, formats, commands.clone()),
                    row().class("editor-history-actions").fill((
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .name("Undo")
                            .label("↶")
                            .enabled(session.loaded)
                            .on_press(move || undo.undo(false)),
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .name("Redo")
                            .label("↷")
                            .enabled(session.loaded)
                            .on_press(move || redo.undo(true)),
                    )),
                )),
                when(conflict.map(Option::is_some), move || {
                    let accept_remote = accept_remote.clone();
                    let keep_local = keep_local.clone();
                    row().class("editor-conflict").fill((
                        text().content(
                            "A collaborator changed this document while you have unsaved edits.",
                        ),
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .label("Use collaborator's version")
                            .on_press(move || {
                                if let Some(next) = accept_remote.remote.get() {
                                    accept_remote.remote.revise(|value| *value = None);
                                    let version = accept_remote.remote_version.get();
                                    accept_remote.version.revise(|value| *value = version);
                                    accept_remote.pending.borrow_mut().take();
                                    accept_remote.timer.borrow_mut().take();
                                    accept_remote.content.revise(|value| *value = next);
                                    accept_remote.dirty.set(false);
                                    accept_remote.undo.borrow_mut().clear();
                                    accept_remote.redo.borrow_mut().clear();
                                    accept_remote
                                        .status
                                        .revise(|value| *value = crate::i18n::t("common.saved"));
                                }
                            }),
                        sapid::button()
                            .theme(crate::components::ui::theme())
                            .label("Keep my version")
                            .on_press(move || {
                                if let Some(previous) = keep_local.remote.get() {
                                    keep_local.remote.revise(|value| *value = None);
                                    keep_local
                                        .version
                                        .revise(|value| *value = keep_local.remote_version.get());
                                    keep_local.replace(keep_local.content.get(), previous);
                                    keep_local.save();
                                }
                            }),
                    ))
                }),
                when(retry_visible, move || {
                    let retry = retry.clone();
                    sapid::button()
                        .theme(crate::components::ui::theme())
                        .class("editor-retry")
                        .label("Retry loading")
                        .on_press(move || retry.reload())
                }),
            ))
            .show_in(scope)
    })
}

fn editor_menus(
    session: &EditorSession,
    kind: &'static str,
    formats: &'static [&'static str],
    commands: Rc<dyn Fn(&str)>,
) -> impl Widget {
    let save = session.clone();
    let undo = session.clone();
    let redo = session.clone();
    let action = |key: &str, label: &str, command: &'static str| {
        let commands = commands.clone();
        sapid::menu_item(key)
            .label(label)
            .on_activate(move || commands(command))
    };
    let unavailable = |key: &str, label: &str| sapid::menu_item(key).label(label).enabled(false);
    let mut exports = sapid::menu_submenu("exports").label("Export");
    for format in formats {
        let session = session.clone();
        let format = *format;
        exports = exports.item(
            sapid::menu_item(format!("export-{format}"))
                .label(format.to_ascii_uppercase())
                .on_activate(move || session.export(format)),
        );
    }
    let file = sapid::menubar_menu("file")
        .label(crate::i18n::t("menubar.file"))
        .item(
            sapid::menu_item("new")
                .label("New document")
                .on_activate(|| crate::components::ui::go("/dashboard")),
        )
        .item(
            sapid::menu_item("open")
                .label("Open…")
                .on_activate(|| crate::components::ui::go("/files")),
        )
        .separator("open-save")
        .item(
            sapid::menu_item("save")
                .label("Save    Ctrl+S")
                .on_activate(move || save.save()),
        )
        .item(unavailable("save-as", "Save as…"))
        .submenu(exports)
        .item(sapid::menu_item("print").label("Print").on_activate(|| {
            #[cfg(target_arch = "wasm32")]
            if let Some(window) = web_sys::window() {
                let _ = window.print();
            }
        }))
        .item(
            sapid::menu_item("close")
                .label("Close")
                .on_activate(|| crate::components::ui::go("/dashboard")),
        );
    let edit = sapid::menubar_menu("edit")
        .label(crate::i18n::t("menubar.edit"))
        .item(
            sapid::menu_item("undo")
                .label("Undo")
                .on_activate(move || undo.undo(false)),
        )
        .item(
            sapid::menu_item("redo")
                .label("Redo")
                .on_activate(move || redo.undo(true)),
        )
        .separator("clipboard")
        .item(unavailable("cut", "Cut"))
        .item(unavailable("copy", "Copy"))
        .item(unavailable("paste", "Paste"))
        .item(action("find", "Find & replace…", "find").enabled(kind == "Writer"));
    let edit = if kind == "Slides" {
        edit.separator("slide-actions")
            .item(action(
                "duplicate-slide",
                "Duplicate slide",
                "duplicate-slide",
            ))
            .item(action("delete-slide", "Delete slide", "delete-slide"))
            .item(action("move-up", "Move slide up", "move-up"))
            .item(action("move-down", "Move slide down", "move-down"))
    } else {
        edit
    };
    let view_menu = sapid::menubar_menu("view")
        .label(crate::i18n::t("menubar.view"))
        .item(action("sidebar", "Toggle sidebar", "toggle-sidebar").enabled(kind == "Writer"))
        .item(action("zoom-in", "Zoom in", "zoom-in").enabled(kind == "Whiteboard"))
        .item(action("zoom-out", "Zoom out", "zoom-out").enabled(kind == "Whiteboard"))
        .item(action("present", "Present / exit", "present").enabled(kind == "Slides"))
        .item(
            sapid::menu_item("theme")
                .label("Toggle dark mode")
                .on_activate(|| {
                    #[cfg(target_arch = "wasm32")]
                    {
                        let dark = web_sys::window()
                            .and_then(|window| window.local_storage().ok().flatten())
                            .and_then(|storage| storage.get_item("theme").ok().flatten())
                            .as_deref()
                            == Some("dark");
                        crate::components::navbar::set_theme(if dark { "light" } else { "dark" });
                    }
                }),
        );
    let insert = match kind {
        "Writer" => sapid::menubar_menu("insert")
            .label(crate::i18n::t("menubar.insert"))
            .items([
                action("image", "Image…", "insert-image"),
                action("table", "Table", "insert-table"),
                action("list", "List", "insert-list"),
                action("code", "Code block", "insert-code"),
                action("link", "Link…", "find"),
                action("page-break", "Page break", "insert-page-break"),
                action("insert-controls", "More insert options…", "insert-controls"),
            ]),
        "Slides" => sapid::menubar_menu("insert")
            .label(crate::i18n::t("menubar.insert"))
            .items([
                action("new-slide", "New slide", "new-slide"),
                action("objects", "Shape / image / chart…", "objects"),
            ]),
        _ => sapid::menubar_menu("insert")
            .label(crate::i18n::t("menubar.insert"))
            .items([
                action("rectangle", "Rectangle", "tool-rect"),
                action("ellipse", "Ellipse", "tool-circle"),
                action("line", "Line", "tool-line"),
                action("text", "Text", "tool-text"),
            ]),
    };
    let format = if kind == "Writer" {
        sapid::menubar_menu("format")
            .label(crate::i18n::t("menubar.format"))
            .items([
                action("bold", "Bold", "format-bold"),
                action("italic", "Italic", "format-italic"),
                action("code", "Code", "format-code"),
                action("heading", "Heading", "format-heading"),
                action("paragraph", "Normal text", "format-paragraph"),
            ])
    } else {
        sapid::menubar_menu("format")
            .label(crate::i18n::t("menubar.format"))
            .item(action(
                "properties",
                "Properties…",
                if kind == "Slides" {
                    "properties"
                } else {
                    "objects"
                },
            ))
    };
    let tools = sapid::menubar_menu("tools")
        .label(crate::i18n::t("menubar.tools"))
        .item(action("comments", "Comments", "comments"))
        .item(action("versions", "Version history", "versions"))
        .item(action("tracking", "Toggle track changes", "tracking").enabled(kind == "Writer"))
        .item(
            action("settings", "Page & document settings…", "settings").enabled(kind == "Writer"),
        );
    let help = sapid::menubar_menu("help")
        .label(crate::i18n::t("menubar.help"))
        .item(
            sapid::menu_item("project")
                .label("Contrix Office on GitHub")
                .on_activate(|| crate::components::ui::go("https://github.com/contrix-org/office")),
        );
    sapid::menubar(format!("{}-document-menu", kind.to_lowercase()))
        .name("Document menu")
        .theme(crate::components::ui::theme())
        .class("menubar office-menubar")
        .menus([file, edit, view_menu, insert, format, tools, help])
}

/// Separate semantic images preserve object paint order and let the browser
/// load remote image resources normally. External resources inside an SVG
/// displayed as an image are intentionally blocked by browsers.
pub(super) fn diagram_objects(
    items: impl Into<Vary<Vec<(String, Value)>>>,
    vector_source: fn(&Value) -> String,
    width: f64,
    height: f64,
) -> impl Widget {
    let items: Vary<Vec<(String, Value)>> = items.into();
    each(items)
        .key(|item| format!("{}:{}", item.0, item.1["type"].as_str().unwrap_or_default()))
        .children(move |item_row| {
            let item = item_row.value;
            let external = item.get().1["type"] == "Image"
                && item.get().1["crop"].is_null()
                && item.get().1["rotation"].as_f64().unwrap_or(0.0) == 0.0;
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
                    let width = number("_canvas_width", width).max(1.0);
                    let height = number("_canvas_height", height).max(1.0);
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
        })
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

#[cfg(test)]
mod snapshot_regressions {
    use super::*;
    #[test]
    fn all_actual_editor_protocol_kinds_can_queue_a_loaded_snapshot() {
        let app = App::memory();
        app.mount(widget(|scope| {
            for kind in ["writer","presentation","whiteboard"] {
                let session=EditorSession::prepare(scope,scope.cage(AuthState::default()),"document".into(),kind);
                session.loaded.revise(|value|*value=true);
                let document=json!({"slides":[{"elements":[{"type":"TextBox","content":[{"text":"Quarterly review"}]}],"notes":"Discuss the margin"}]});
                session.content.revise(|value|*value=document.clone());
                assert_eq!(session.snapshot_for_save(),Some(document),"{kind} must reach the same save queue in debug and release builds");
            }
            view().build(scope)
        })).unwrap();
        app.clear().unwrap();
    }
    #[test]
    fn reconnect_restores_full_rich_state_and_preserves_unsaved_conflicts() {
        let app = App::memory();
        app.mount(widget(|scope| {
            let session = EditorSession::prepare(
                scope,
                scope.cage(AuthState::default()),
                "document".into(),
                "writer",
            );
            let initial =
                json!({"body":[{"type":"Paragraph","runs":[{"text":"same","bold":false}]}]});
            session.receive_snapshot(initial.clone(), 1);
            let mut formatted = initial.clone();
            formatted["body"][0]["runs"][0]["bold"] = json!(true);
            session.receive_snapshot(formatted.clone(), 2);
            assert_eq!(session.content.get(), formatted);
            assert_eq!(session.version.get(), 2);
            session.dirty.set(true);
            session.content.revise(|value| value["local"] = json!(true));
            let local = session.content.get();
            let remote = json!({"body":[],"professional":{"header":"other author"}});
            session.receive_snapshot(remote.clone(), 3);
            assert_eq!(session.content.get(), local);
            assert_eq!(session.version.get(), 2);
            assert_eq!(session.remote.get(), Some(remote));
            assert!(session.dirty.get());
            session.receive_snapshot(initial, 1);
            assert_eq!(
                session.content.get(),
                local,
                "stale GET/snapshot cannot win"
            );
            view().build(scope)
        }))
        .unwrap();
        app.clear().unwrap();
    }
    #[test]
    fn own_persistent_echo_updates_version_without_overwriting_newer_typing() {
        let app = App::memory();
        app.mount(widget(|scope| {
            let session = EditorSession::prepare(
                scope,
                scope.cage(AuthState::default()),
                "document".into(),
                "writer",
            );
            session.version.revise(|value| *value = 1);
            session.dirty.set(true);
            *session.submitted.borrow_mut() = Some(json!({"body":"submitted"}));
            session
                .content
                .revise(|value| *value = json!({"body":"typed while saving"}));
            session.receive_snapshot(json!({"body":"submitted"}), 2);
            assert_eq!(session.version.get(), 2);
            assert!(session.remote.get().is_none());
            assert!(session.dirty.get());
            assert_eq!(session.content.get()["body"], "typed while saving");
            view().build(scope)
        }))
        .unwrap();
        app.clear().unwrap();
    }
}
