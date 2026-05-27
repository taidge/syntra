use dioxus::html::HasFileData;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::Route;
use crate::components::file_list::FileList;
use crate::components::modal::Modal;
use crate::components::navbar::Navbar;
use crate::components::sidebar::Sidebar;
use crate::state::{self, AuthState, CreateFolderRequest, FileResponse};

#[derive(Clone, PartialEq)]
struct BreadcrumbEntry {
    id: Option<Uuid>,
    name: String,
}

/// Full-page file manager with sidebar, toolbar, and file grid/list.
#[component]
pub fn FileManager() -> Element {
    let _ = use_context::<Signal<String>>().read();
    let auth = use_context::<Signal<AuthState>>();
    let nav = use_navigator();
    let mut show_new_folder_modal = use_signal(|| false);
    let mut new_folder_name = use_signal(|| String::new());
    let mut folder_error = use_signal(|| None::<String>);
    let mut upload_error = use_signal(|| None::<String>);
    let mut current_parent_id = use_signal(|| None::<Uuid>);
    let mut refresh_key = use_signal(|| 0u64);
    let mut breadcrumb = use_signal(|| {
        vec![BreadcrumbEntry {
            id: None,
            name: crate::i18n::t("file_mgr.my_files"),
        }]
    });
    let mut is_dragging = use_signal(|| false);

    rsx! {
        Navbar {}
        div { class: "file-manager",
            Sidebar {}

            div { class: "file-content",
                ondragover: move |evt: Event<DragData>| {
                    evt.prevent_default();
                    is_dragging.set(true);
                },
                ondragleave: move |_| {
                    is_dragging.set(false);
                },
                ondrop: move |evt: Event<DragData>| {
                    evt.prevent_default();
                    is_dragging.set(false);
                    let files = evt.files();
                    if files.is_empty() {
                        return;
                    }

                    let token = auth.read().token.clone().unwrap_or_default();
                    let parent_id = *current_parent_id.read();
                    spawn(async move {
                        let mut first_error = None::<String>;
                        let mut imported: Option<FileResponse> = None;

                        for file_data in &files {
                            let name = file_data.name();
                            let Some(file) = file_data.inner().downcast_ref::<web_sys::File>() else {
                                first_error.get_or_insert_with(|| format!("Unsupported dropped item: {name}."));
                                continue;
                            };

                            match state::upload_file(file.clone(), parent_id, &token).await {
                                Ok(resp) => {
                                    if imported.is_none() && resp.document_id.is_some() {
                                        imported = Some(resp);
                                    }
                                }
                                Err(msg) => {
                                    first_error.get_or_insert(msg);
                                }
                            }
                        }

                        if let Some(msg) = first_error {
                            upload_error.set(Some(msg));
                        } else {
                            upload_error.set(None);
                            let next = *refresh_key.read() + 1;
                            refresh_key.set(next);
                        }

                        if let Some(resp) = imported {
                            if let Some(doc_id) = resp.document_id {
                                let ext = resp.name
                                    .rsplit('.')
                                    .next()
                                    .map(|s| s.to_ascii_lowercase())
                                    .unwrap_or_default();
                                let route = match ext.as_str() {
                                    "docx" => Some(Route::Writer { id: doc_id.to_string() }),
                                    "xlsx" => Some(Route::Calc { id: doc_id.to_string() }),
                                    "pptx" => Some(Route::Slides { id: doc_id.to_string() }),
                                    _ => None,
                                };
                                if let Some(route) = route {
                                    nav.push(route);
                                }
                            }
                        }
                    });
                },

                if *is_dragging.read() {
                    div { class: "drop-zone-overlay",
                        p { {crate::i18n::t("file_mgr.drop_files")} }
                    }
                }

                // Breadcrumb
                div { class: "breadcrumb",
                    for (i, crumb) in breadcrumb.read().iter().enumerate() {
                        if i > 0 {
                            span { class: "breadcrumb-sep", " / " }
                        }
                        button { class: "breadcrumb-item",
                            onclick: move |_| {
                                let mut bc = breadcrumb.read().clone();
                                bc.truncate(i + 1);
                                let target = bc.last().and_then(|entry| entry.id);
                                current_parent_id.set(target);
                                breadcrumb.set(bc);
                            },
                            "{crumb.name}"
                        }
                    }
                }

                // Toolbar
                div { class: "file-toolbar",
                    label {
                        r#for: "file-upload-input",
                        class: "btn btn-primary btn-sm",
                        {crate::i18n::t("file_mgr.upload")}
                    }
                    input {
                        id: "file-upload-input",
                        r#type: "file",
                        multiple: true,
                        style: "display: none;",
                        onchange: move |evt: Event<FormData>| {
                            let files = evt.files();
                            if files.is_empty() {
                                return;
                            }

                            let token = auth.read().token.clone().unwrap_or_default();
                            let parent_id = *current_parent_id.read();
                            spawn(async move {
                                let mut first_error = None::<String>;
                                let mut imported: Option<FileResponse> = None;

                                for file_data in &files {
                                    let name = file_data.name();
                                    let Some(file) = file_data.inner().downcast_ref::<web_sys::File>() else {
                                        first_error.get_or_insert_with(|| format!("Unsupported selected item: {name}."));
                                        continue;
                                    };

                                    match state::upload_file(file.clone(), parent_id, &token).await {
                                        Ok(resp) => {
                                            if imported.is_none() && resp.document_id.is_some() {
                                                imported = Some(resp);
                                            }
                                        }
                                        Err(msg) => {
                                            first_error.get_or_insert(msg);
                                        }
                                    }
                                }

                                if let Some(msg) = first_error {
                                    upload_error.set(Some(msg));
                                } else {
                                    upload_error.set(None);
                                    let next = *refresh_key.read() + 1;
                                    refresh_key.set(next);
                                }

                                if let Some(resp) = imported {
                                    if let Some(doc_id) = resp.document_id {
                                        let ext = resp.name
                                            .rsplit('.')
                                            .next()
                                            .map(|s| s.to_ascii_lowercase())
                                            .unwrap_or_default();
                                        let route = match ext.as_str() {
                                            "docx" => Some(Route::Writer { id: doc_id.to_string() }),
                                            "xlsx" => Some(Route::Calc { id: doc_id.to_string() }),
                                            "pptx" => Some(Route::Slides { id: doc_id.to_string() }),
                                            _ => None,
                                        };
                                        if let Some(route) = route {
                                            nav.push(route);
                                        }
                                    }
                                }
                            });
                        },
                    }
                    button {
                        class: "btn btn-outline btn-sm",
                        onclick: move |_| show_new_folder_modal.set(true),
                        {crate::i18n::t("file_mgr.new_folder")}
                    }
                    div { class: "toolbar-spacer" }
                    input {
                        r#type: "search",
                        class: "file-search",
                        placeholder: crate::i18n::t("file_mgr.search_files"),
                    }
                }

                if let Some(err) = upload_error() {
                    div { class: "form-error", "{err}" }
                }

                // File list
                FileList {
                    current_parent_id: *current_parent_id.read(),
                    refresh_key: *refresh_key.read(),
                    on_open_folder: move |folder: FileResponse| {
                        current_parent_id.set(Some(folder.id));
                        let mut next = breadcrumb.read().clone();
                        next.push(BreadcrumbEntry {
                            id: Some(folder.id),
                            name: folder.name,
                        });
                        breadcrumb.set(next);
                    },
                    on_refresh: move |_| {
                        let next = *refresh_key.read() + 1;
                        refresh_key.set(next);
                    },
                }
            }
        }

        // New folder modal
        Modal { title: crate::i18n::t("file_mgr.create_folder"), show: show_new_folder_modal,
            div { class: "modal-form",
                div { class: "form-group",
                    label { {crate::i18n::t("file_mgr.folder_name")} }
                    input {
                        r#type: "text",
                        class: "form-input",
                        placeholder: crate::i18n::t("file_mgr.folder_name_placeholder"),
                        value: "{new_folder_name}",
                        oninput: move |e| new_folder_name.set(e.value()),
                    }
                }
                div { class: "modal-actions",
                    button {
                        class: "btn btn-outline",
                        onclick: move |_| {
                            show_new_folder_modal.set(false);
                            new_folder_name.set(String::new());
                        },
                        {crate::i18n::t("common.cancel")}
                    }
                    if let Some(err) = folder_error() {
                        div { class: "form-error", "{err}" }
                    }
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            let name = new_folder_name.read().clone();
                            if name.is_empty() {
                                folder_error.set(Some(crate::i18n::t("file_mgr.folder_name_required")));
                                return;
                            }
                            let token = auth.read().token.clone().unwrap_or_default();
                            spawn(async move {
                                let parent_id = *current_parent_id.read();
                                let body = CreateFolderRequest {
                                    name,
                                    parent_id,
                                };
                                match state::api_post::<FileResponse, _>("/folders", &body, &token).await {
                                    Ok(_resp) => {
                                        show_new_folder_modal.set(false);
                                        new_folder_name.set(String::new());
                                        folder_error.set(None);
                                        let next = *refresh_key.read() + 1;
                                        refresh_key.set(next);
                                    }
                                    Err(msg) => {
                                        folder_error.set(Some(msg));
                                    }
                                }
                            });
                        },
                        {crate::i18n::t("common.create")}
                    }
                }
            }
        }
    }
}
