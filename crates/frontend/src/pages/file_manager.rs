use crate::{
    components::{navbar::Navbar, sidebar::Sidebar, ui::*},
    state::{self, AuthState, CreateFolderRequest, FileResponse},
};
use glory::prelude::*;
use uuid::Uuid;
#[derive(Clone)]
struct Breadcrumb {
    id: Option<Uuid>,
    name: String,
}

fn refresh(
    scope: &Scope,
    auth: Cage<AuthState>,
    parent: Cage<Option<Uuid>>,
    files: Cage<Vec<FileResponse>>,
    feedback: Cage<String>,
) {
    let requested_parent = parent.get();
    let path = requested_parent
        .map(|id| format!("/files?parent_id={id}"))
        .unwrap_or_else(|| "/files".into());
    let token = token(auth);
    let _ = scope.spawn(async move {
        let result = state::api_get::<Vec<FileResponse>>(&path, &token).await;
        if parent.get() != requested_parent {
            return;
        }
        match result {
            Ok(value) => {
                set(files, value);
                set(feedback, String::new());
            }
            Err(error) => set(feedback, error),
        }
    });
}
struct UploadContext {
    weak: glory::ui::WeakScope,
    auth: Cage<AuthState>,
    parent: Cage<Option<Uuid>>,
    files: Cage<Vec<FileResponse>>,
    feedback: Cage<String>,
}

fn read_upload(
    controller: sapid::FileController,
    file: glory::render::FileInfo,
    offset: u64,
    data: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
    context: UploadContext,
) {
    let UploadContext {
        weak,
        auth,
        parent,
        files,
        feedback,
    } = context;
    let request = glory::render::FileRead {
        file: file.token,
        offset,
        length: (file.size.saturating_sub(offset).min(65_536)) as u32,
    };
    let next_controller = controller.clone();
    let next_weak = weak.clone();
    let result = controller.read(request, move |result| {
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) => {
                set(feedback, error.to_string());
                return;
            }
        };
        let end = offset + bytes.len() as u64;
        if end < file.size && bytes.is_empty() {
            set(
                feedback,
                "File read stopped before its declared size".into(),
            );
            return;
        }
        data.borrow_mut().extend(bytes);
        if end < file.size {
            read_upload(
                next_controller,
                file,
                end,
                data,
                UploadContext {
                    weak: next_weak,
                    auth,
                    parent,
                    files,
                    feedback,
                },
            );
            return;
        }
        let Some(scope) = next_weak.upgrade() else {
            return;
        };
        let parts = js_sys::Array::new();
        let bytes = js_sys::Uint8Array::from(data.borrow().as_slice());
        parts.push(&bytes);
        let properties = web_sys::FilePropertyBag::new();
        properties.set_type(&file.media_type);
        let file = match web_sys::File::new_with_u8_array_sequence_and_options(
            &parts,
            &file.name,
            &properties,
        ) {
            Ok(file) => file,
            Err(error) => {
                set(feedback, format!("{error:?}"));
                return;
            }
        };
        let token = token(auth);
        let parent_id = parent.get();
        let weak = scope.downgrade();
        let _ = scope.spawn(async move {
            match state::upload_file(file, parent_id, &token).await {
                Ok(_) => {
                    if let Some(scope) = weak.upgrade() {
                        refresh(&scope, auth, parent, files, feedback);
                    }
                }
                Err(error) => set(feedback, error),
            }
        });
    });
    if let Err(error) = result {
        set(feedback, error.to_string());
    }
}
#[allow(non_snake_case)]
pub fn FileManager(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    let parent = scope.cage(None::<Uuid>);
    let files = scope.cage(Vec::<FileResponse>::new());
    let feedback = scope.cage(crate::i18n::t("common.loading"));
    let query = scope.cage(String::new());
    let grid_view = scope.cage(true);
    let folder_name = scope.cage(String::new());
    let folder_open = scope.cage(false);
    let controller = scope.cage(None::<sapid::FileController>);
    let crumbs = scope.cage(vec![Breadcrumb {
        id: None,
        name: crate::i18n::t("file_mgr.my_files"),
    }]);
    refresh(&scope, auth, parent, files, feedback);
    let shown = scope.bond(move || {
        let query = query.get().to_lowercase();
        files
            .get()
            .into_iter()
            .filter(|file| file.name.to_lowercase().contains(&query))
            .collect::<Vec<_>>()
    });
    let folder_weak = scope.downgrade();
    let upload_weak = scope.downgrade();
    let breadcrumb_weak = scope.downgrade();
    let upload = sapid::file_button()
        .class("file-upload-control")
        .theme(theme())
        .label(crate::i18n::t("file_mgr.upload"))
        .configuration(glory::render::FileConfig {
            multiple: true,
            drop: true,
            max_files: 16,
            max_file_bytes: 50 * 1024 * 1024,
            max_total_bytes: 128 * 1024 * 1024,
            ..Default::default()
        })
        .controller_ref(controller)
        .on_files(move |selected| {
            if let Some(controller) = controller.get() {
                for file in selected {
                    read_upload(
                        controller.clone(),
                        file,
                        0,
                        Default::default(),
                        UploadContext {
                            weak: upload_weak.clone(),
                            auth,
                            parent,
                            files,
                            feedback,
                        },
                    );
                }
            }
        });
    let folder_dialog = crate::components::modal::Modal(
        "create-folder".into(),
        crate::i18n::t("file_mgr.create_folder"),
        folder_open,
        move || {
            let folder_weak = folder_weak.clone();
            col().class("modal-form").fill((
                field(
                    crate::i18n::t("file_mgr.folder_name"),
                    folder_name,
                    Purpose::Text,
                ),
                row().class("modal-actions").fill((
                    action(crate::i18n::t("common.cancel"), move || {
                        set(folder_open, false)
                    })
                    .variant(sapid::ButtonVariant::Outline),
                    action(crate::i18n::t("common.create"), move || {
                        if folder_name.get().trim().is_empty() {
                            return;
                        }
                        let Some(scope) = folder_weak.upgrade() else {
                            return;
                        };
                        let token = token(auth);
                        let body = CreateFolderRequest {
                            name: folder_name.get(),
                            parent_id: parent.get(),
                        };
                        let weak = scope.downgrade();
                        let _ = scope.spawn(async move {
                            match state::api_post::<FileResponse, _>("/folders", &body, &token)
                                .await
                            {
                                Ok(_) => {
                                    set(folder_name, String::new());
                                    set(folder_open, false);
                                    if let Some(scope) = weak.upgrade() {
                                        refresh(&scope, auth, parent, files, feedback);
                                    }
                                }
                                Err(error) => set(feedback, error),
                            }
                        });
                    }),
                )),
                status(feedback),
            ))
        },
    );
    col().class("files-page").fill((
        Navbar(scope.clone(), auth),
        row().class("file-manager").fill((
            Sidebar(scope.clone(), auth),
            col().class("file-content").fill((
                row().class("breadcrumb").fill(each(
                    crumbs,
                    |crumb| crumb.id,
                    move |crumb, index| {
                        if crumb.get().id.is_none() {
                            return Box::new(
                                link()
                                    .target("/files")
                                    .class("breadcrumb-item file-root-crumb")
                                    .fill(
                                        text()
                                            .kind(TextKind::Heading(1))
                                            .class("file-page-title")
                                            .content(crate::i18n::t("file_mgr.my_files")),
                                    ),
                            ) as Box<dyn Widget>;
                        }
                        let weak = breadcrumb_weak.clone();
                        Box::new(
                            action(crumb.get().name, move || {
                                set(parent, crumb.get().id);
                                let _ = crumbs.try_revise(|items| items.truncate(index.get() + 1));
                                if let Some(scope) = weak.upgrade() {
                                    refresh(&scope, auth, parent, files, feedback);
                                }
                            })
                            .variant(sapid::ButtonVariant::Subtle)
                            .class("breadcrumb-item"),
                        ) as Box<dyn Widget>
                    },
                )),
                row().class("file-toolbar").fill((
                    upload,
                    action(crate::i18n::t("file_mgr.new_folder"), move || {
                        set(folder_open, true)
                    })
                    .variant(sapid::ButtonVariant::Outline)
                    .class("btn btn-outline btn-sm"),
                    view().class("toolbar-spacer"),
                    sapid::text_field()
                        .theme(theme())
                        .class("file-search")
                        .name(crate::i18n::t("common.search"))
                        .purpose(Purpose::Search)
                        .placeholder(crate::i18n::t("file_mgr.search_files"))
                        .value(query)
                        .on_value_change(move |value| set(query, value)),
                )),
                status(feedback),
                row().class("file-view-toggle").fill((
                    action(crate::i18n::t("file_list.view_grid"), move || {
                        set(grid_view, true)
                    })
                    .variant(sapid::ButtonVariant::Subtle)
                    .class(grid_view.map(|grid| {
                        if *grid {
                            "toggle-btn active".to_string()
                        } else {
                            "toggle-btn".to_string()
                        }
                    })),
                    action(crate::i18n::t("file_list.view_list"), move || {
                        set(grid_view, false)
                    })
                    .variant(sapid::ButtonVariant::Subtle)
                    .class(grid_view.map(|grid| {
                        if !*grid {
                            "toggle-btn active".to_string()
                        } else {
                            "toggle-btn".to_string()
                        }
                    })),
                )),
                when(scope.bond(move || shown.get().is_empty()), || {
                    col().class("empty-state").fill((
                        text()
                            .kind(TextKind::Heading(3))
                            .content(crate::i18n::t("file_list.empty_title")),
                        text().content(crate::i18n::t("file_list.empty_hint")),
                    ))
                }),
                when(grid_view.map(|grid| !*grid), || {
                    row().class("file-list-heading").fill((
                        text().content("Name"),
                        text().content("Size"),
                        text().content("Modified"),
                        text().content("Actions"),
                    ))
                }),
                view()
                    .class_fn(move || {
                        if grid_view.get() {
                            "file-grid".into()
                        } else {
                            "file-list file-table-list".into()
                        }
                    })
                    .style(scope.bond(move || {
                        if grid_view.get() {
                            Style::new().display(Display::Grid)
                        } else {
                            Style::new()
                                .display(Display::Flex)
                                .flex_direction(FlexDirection::Column)
                        }
                    }))
                    .fill(each(
                        shown,
                        |file| file.id,
                        move |item, _| {
                            widget(move |row_scope| {
                                let open_weak = row_scope.downgrade();
                                let rename_weak = row_scope.downgrade();
                                let delete_weak = row_scope.downgrade();
                                let filename = row_scope.bond(move || item.get().name);
                                let metadata = row_scope.bond(move || {
                                    let file = item.get();
                                    format_size(file.size)
                                });
                                let card_style = move || {
                                    let direction = if grid_view.get() {
                                        FlexDirection::Column
                                    } else {
                                        FlexDirection::Row
                                    };
                                    Style::new()
                                        .display(Display::Flex)
                                        .flex_direction(direction)
                                };
                                view()
                                    .class(grid_view.map(|grid| {
                                        if *grid {
                                            "file-card".to_string()
                                        } else {
                                            "file-card file-row".to_string()
                                        }
                                    }))
                                    .style_fn(card_style)
                                    .fill((
                                        col().class("file-card-body").fill((
                                            text()
                                                .class(row_scope.bond(move || {
                                                    file_icon_class(&item.get()).to_owned()
                                                }))
                                                .content(row_scope.bond(move || {
                                                    file_icon(&item.get()).to_owned()
                                                })),
                                            sapid::button()
                                                .theme(theme())
                                                .variant(sapid::ButtonVariant::Subtle)
                                                .class("file-name")
                                                .label(filename)
                                                .on_press(move || {
                                                    let open_file = item.get();
                                                    if open_file.is_folder {
                                                        set(parent, Some(open_file.id));
                                                        let _ = crumbs.try_revise(|items| {
                                                            items.push(Breadcrumb {
                                                                id: Some(open_file.id),
                                                                name: open_file.name.clone(),
                                                            })
                                                        });
                                                        if let Some(scope) = open_weak.upgrade() {
                                                            refresh(
                                                                &scope, auth, parent, files,
                                                                feedback,
                                                            );
                                                        }
                                                    } else if let Some(id) = open_file.document_id {
                                                        go(&format!("/documents/{id}"));
                                                    } else {
                                                        set(
                                                            feedback,
                                                            "This file has no editable document"
                                                                .into(),
                                                        );
                                                    }
                                                }),
                                        )),
                                        text().class("file-size").content(metadata),
                                        text().class("file-modified").content(row_scope.bond(
                                            move || {
                                                item.get()
                                                    .updated_at
                                                    .get(..10)
                                                    .unwrap_or(&item.get().updated_at)
                                                    .to_owned()
                                            },
                                        )),
                                        row().class("file-actions").fill((
                                            action("Rename", move || {
                                                let rename_file = item.get();
                                                let Some(window) = web_sys::window() else {
                                                    return;
                                                };
                                                let Ok(Some(name)) = window
                                                    .prompt_with_message_and_default(
                                                        "File name",
                                                        &rename_file.name,
                                                    )
                                                else {
                                                    return;
                                                };
                                                if name.trim().is_empty() {
                                                    return;
                                                }
                                                let Some(scope) = rename_weak.upgrade() else {
                                                    return;
                                                };
                                                let token = token(auth);
                                                let path = format!("/files/{}", rename_file.id);
                                                let weak = scope.downgrade();
                                                let _ = scope.spawn(async move {
                                                    match state::api_put::<serde_json::Value, _>(
                                                        &path,
                                                        &serde_json::json!({"name":name.trim()}),
                                                        &token,
                                                    )
                                                    .await
                                                    {
                                                        Ok(_) => {
                                                            if let Some(scope) = weak.upgrade() {
                                                                refresh(
                                                                    &scope, auth, parent, files,
                                                                    feedback,
                                                                );
                                                            }
                                                        }
                                                        Err(error) => set(feedback, error),
                                                    }
                                                });
                                            })
                                            .variant(sapid::ButtonVariant::Subtle)
                                            .class("file-rename"),
                                            action("Delete", move || {
                                                let delete_file = item.get();
                                                if !web_sys::window().is_some_and(|window| {
                                                    window
                                                        .confirm_with_message(
                                                            "Move this file to trash?",
                                                        )
                                                        .unwrap_or(false)
                                                }) {
                                                    return;
                                                }
                                                let Some(scope) = delete_weak.upgrade() else {
                                                    return;
                                                };
                                                let token = token(auth);
                                                let path = format!("/files/{}", delete_file.id);
                                                let weak = scope.downgrade();
                                                let _ = scope.spawn(async move {
                                                    match state::api_delete(&path, &token).await {
                                                        Ok(_) => {
                                                            if let Some(scope) = weak.upgrade() {
                                                                refresh(
                                                                    &scope, auth, parent, files,
                                                                    feedback,
                                                                );
                                                            }
                                                        }
                                                        Err(error) => set(feedback, error),
                                                    }
                                                });
                                            })
                                            .variant(sapid::ButtonVariant::Subtle)
                                            .class("file-delete"),
                                        )),
                                    ))
                                    .build(row_scope)
                            })
                        },
                    )),
            )),
        )),
        folder_dialog,
    ))
}

fn format_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    }
}
fn file_kind(file: &FileResponse) -> &'static str {
    if file.is_folder {
        return "folder";
    }
    let mime = file.mime_type.as_deref().unwrap_or_default();
    if mime.contains("writer") || file.name.ends_with(".docx") {
        "writer"
    } else if mime.contains("calc") || file.name.ends_with(".xlsx") {
        "calc"
    } else if mime.contains("slides") || file.name.ends_with(".pptx") {
        "slides"
    } else if mime.contains("whiteboard") {
        "whiteboard"
    } else {
        "generic"
    }
}
fn file_icon(file: &FileResponse) -> &'static str {
    match file_kind(file) {
        "folder" => "F",
        "writer" => "W",
        "calc" => "C",
        "slides" => "S",
        "whiteboard" => "B",
        _ => "↧",
    }
}
fn file_icon_class(file: &FileResponse) -> &'static str {
    match file_kind(file) {
        "folder" => "file-icon folder",
        "writer" => "file-icon writer",
        "calc" => "file-icon calc",
        "slides" => "file-icon slides",
        "whiteboard" => "file-icon whiteboard",
        _ => "file-icon generic",
    }
}
