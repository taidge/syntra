use dioxus::prelude::*;
use uuid::Uuid;

use crate::Route;
use crate::state::{self, AuthState, FileResponse};

/// Renders the grid/list of files and folders backed by the live API.
#[component]
pub fn FileList(
    current_parent_id: ReadSignal<Option<Uuid>>,
    refresh_key: ReadSignal<u64>,
    /// Case-insensitive name filter applied to the displayed files.
    #[props(default)]
    filter: ReadSignal<String>,
    on_open_folder: EventHandler<FileResponse>,
    on_refresh: EventHandler<()>,
) -> Element {
    let auth = use_context::<Signal<AuthState>>();
    let token = auth.read().token.clone().unwrap_or_default();
    let mut view_mode = use_signal(|| "grid".to_string());

    let files = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        let path = current_parent_id()
            .map(|id| format!("/files?parent_id={id}"))
            .unwrap_or_else(|| "/files".to_string());
        // Reading the key makes the resource re-fetch whenever it is bumped.
        let _refresh_key = refresh_key();
        async move {
            state::api_get::<Vec<FileResponse>>(&path, &token)
                .await
                .unwrap_or_default()
        }
    });

    rsx! {
        div { class: "file-view-toggle",
            button {
                class: if *view_mode.read() == "grid" { "toggle-btn active" } else { "toggle-btn" },
                onclick: move |_| view_mode.set("grid".to_string()),
                {crate::i18n::t("file_list.view_grid")}
            }
            button {
                class: if *view_mode.read() == "list" { "toggle-btn active" } else { "toggle-btn" },
                onclick: move |_| view_mode.set("list".to_string()),
                {crate::i18n::t("file_list.view_list")}
            }
        }

        match &*files.read() {
            Some(rows) if rows.is_empty() => rsx! {
                div { class: "empty-state",
                    h3 { {crate::i18n::t("file_list.empty_title")} }
                    p { {crate::i18n::t("file_list.empty_hint")} }
                }
            },
            Some(rows) => {
                let needle = filter().trim().to_lowercase();
                let rows: Vec<FileResponse> = if needle.is_empty() {
                    rows.clone()
                } else {
                    rows.iter()
                        .filter(|f| f.name.to_lowercase().contains(&needle))
                        .cloned()
                        .collect()
                };
                if *view_mode.read() == "grid" {
                    rsx! {
                        div { class: "file-grid",
                            for file in rows {
                                {
                                    let file_copy = file.clone();
                                    let file_name = file.name.clone();
                                    let icon = file_icon(&file);
                                    let cls = icon_class(&file);
                                    rsx! {
                                        div { class: "file-card-shell",
                                            if file.is_folder {
                                                button {
                                                    class: "file-card file-card-button",
                                                    onclick: move |_| on_open_folder.call(file_copy.clone()),
                                                    div { class: "{cls}", "{icon}" }
                                                    div { class: "file-name", "{file_name}" }
                                                }
                                            } else if let Some(route) = file_route(&file) {
                                                Link { to: route, class: "file-card",
                                                    div { class: "{cls}", "{icon}" }
                                                    div { class: "file-name", "{file_name}" }
                                                }
                                            } else {
                                                div { class: "file-card file-card-static",
                                                    div { class: "{cls}", "{icon}" }
                                                    div { class: "file-name", "{file_name}" }
                                                }
                                            }
                                            {render_grid_actions(file, token.clone(), on_refresh.clone())}
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    rsx! {
                        table { class: "file-table",
                            thead {
                                tr {
                                    th { {crate::i18n::t("file_list.col_name")} }
                                    th { {crate::i18n::t("file_list.col_size")} }
                                    th { {crate::i18n::t("file_list.col_modified")} }
                                    th { {crate::i18n::t("file_list.col_actions")} }
                                }
                            }
                            tbody {
                                for file in rows {
                                    {render_list_row(file, token.clone(), on_open_folder.clone(), on_refresh.clone())}
                                }
                            }
                        }
                    }
                }
            }
            None => rsx! {
                div { class: "empty-state",
                    h3 { {crate::i18n::t("file_list.loading")} }
                }
            },
        }
    }
}

/// Returns the icon text for a given file/folder entry.
fn file_icon(entry: &FileResponse) -> &'static str {
    if entry.is_folder {
        return "F";
    }
    match infer_document_type(entry).as_deref() {
        Some("writer") => "W",
        Some("calc") => "C",
        Some("slides") => "S",
        Some("whiteboard") => "B",
        _ => "?",
    }
}

/// Returns the CSS class for the icon badge.
fn icon_class(entry: &FileResponse) -> &'static str {
    if entry.is_folder {
        return "file-icon folder";
    }
    match infer_document_type(entry).as_deref() {
        Some("writer") => "file-icon writer",
        Some("calc") => "file-icon calc",
        Some("slides") => "file-icon slides",
        Some("whiteboard") => "file-icon whiteboard",
        _ => "file-icon",
    }
}

fn infer_document_type(entry: &FileResponse) -> Option<String> {
    let mime = entry.mime_type.as_deref()?;
    if mime.contains("contrix.writer") {
        Some("writer".to_string())
    } else if mime.contains("contrix.calc") {
        Some("calc".to_string())
    } else if mime.contains("contrix.slides") {
        Some("slides".to_string())
    } else if mime.contains("contrix.whiteboard") {
        Some("whiteboard".to_string())
    } else {
        None
    }
}

/// Route for opening a file entry, when it maps to an internal Contrix document.
fn file_route(entry: &FileResponse) -> Option<Route> {
    if entry.is_folder || infer_document_type(entry).is_none() {
        return None;
    }
    let document_id = entry.document_id?;
    Some(Route::DocumentEditor {
        id: document_id.to_string(),
    })
}

fn format_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn render_grid_actions(file: FileResponse, token: String, on_refresh: EventHandler<()>) -> Element {
    rsx! {
        div { class: "file-card-actions",
            {render_rename_button(file.clone(), token.clone(), on_refresh.clone())}
            {render_delete_button(file, token, on_refresh)}
        }
    }
}

fn render_list_row(
    file: FileResponse,
    token: String,
    on_open_folder: EventHandler<FileResponse>,
    on_refresh: EventHandler<()>,
) -> Element {
    let folder_target = file.clone();
    let icon = file_icon(&file);
    let cls = icon_class(&file);
    let name = file.name.clone();
    let size = if file.is_folder {
        "--".to_string()
    } else {
        format_size(file.size)
    };
    let modified = file.updated_at.clone();

    rsx! {
        tr { class: "file-row",
            td {
                if file.is_folder {
                    button {
                        class: "file-name-cell file-link-button",
                        onclick: move |_| on_open_folder.call(folder_target.clone()),
                        span { class: "{cls} small", "{icon}" }
                        span { "{name}" }
                    }
                } else if let Some(route) = file_route(&file) {
                    Link { to: route, class: "file-name-cell",
                        span { class: "{cls} small", "{icon}" }
                        span { "{name}" }
                    }
                } else {
                    div { class: "file-name-cell",
                        span { class: "{cls} small", "{icon}" }
                        span { "{name}" }
                    }
                }
            }
            td { "{size}" }
            td { "{modified}" }
            td { class: "file-actions",
                {render_rename_button(file.clone(), token.clone(), on_refresh.clone())}
                {render_delete_button(file, token, on_refresh)}
            }
        }
    }
}

fn render_rename_button(
    file: FileResponse,
    token: String,
    on_refresh: EventHandler<()>,
) -> Element {
    rsx! {
        button {
            class: "btn-icon",
            title: crate::i18n::t("file_list.rename"),
            onclick: move |_| {
                let Some(window) = web_sys::window() else {
                    return;
                };
                let Ok(Some(new_name)) = window.prompt_with_message_and_default(
                    &crate::i18n::t("file_list.rename_prompt"),
                    &file.name,
                ) else {
                    return;
                };
                let trimmed = new_name.trim().to_string();
                if trimmed.is_empty() || trimmed == file.name {
                    return;
                }

                let file_id = file.id;
                let token = token.clone();
                spawn(async move {
                    let body = serde_json::json!({ "name": trimmed });
                    if state::api_put::<serde_json::Value, _>(
                        &format!("/files/{file_id}"),
                        &body,
                        &token,
                    )
                    .await
                    .is_ok()
                    {
                        on_refresh.call(());
                    }
                });
            },
            {crate::i18n::t("file_list.rename")}
        }
    }
}

fn render_delete_button(
    file: FileResponse,
    token: String,
    on_refresh: EventHandler<()>,
) -> Element {
    let label = if file.is_folder {
        crate::i18n::t("file_list.trash_folder")
    } else {
        crate::i18n::t("file_list.trash_file")
    };

    rsx! {
        button {
            class: "btn-icon danger",
            title: "{label}",
            onclick: move |_| {
                let Some(window) = web_sys::window() else {
                    return;
                };
                let Ok(confirmed) = window.confirm_with_message(
                    &crate::i18n::t("file_list.confirm_trash").replace("{name}", &file.name),
                ) else {
                    return;
                };
                if !confirmed {
                    return;
                }

                let file_id = file.id;
                let token = token.clone();
                spawn(async move {
                    if state::api_delete(&format!("/files/{file_id}"), &token)
                        .await
                        .is_ok()
                    {
                        on_refresh.call(());
                    }
                });
            },
            {crate::i18n::t("file_list.delete")}
        }
    }
}
