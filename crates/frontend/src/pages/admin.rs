use dioxus::prelude::*;

use crate::components::navbar::Navbar;
use crate::state::{AuthState, api_get};

#[derive(Clone, PartialEq, serde::Deserialize)]
struct AdminStats {
    total_users: u64,
    total_files: u64,
    total_documents: u64,
    total_storage_used: i64,
}

#[derive(Clone, PartialEq, serde::Deserialize)]
struct UserSummary {
    id: String,
    username: String,
    email: String,
    role: String,
    storage_used: i64,
    last_login_at: Option<String>,
}

#[derive(Clone, PartialEq, serde::Deserialize)]
struct ActivityEntry {
    id: String,
    user_id: String,
    action: String,
    resource_type: String,
    resource_id: String,
    created_at: String,
}

#[component]
pub fn Admin() -> Element {
    let _ = use_context::<Signal<String>>().read();
    let mut active_tab = use_signal(|| "overview".to_string());

    let title = crate::i18n::t("admin.title");
    let tab_overview = crate::i18n::t("admin.tab.overview");
    let tab_users = crate::i18n::t("admin.tab.users");
    let tab_activity = crate::i18n::t("admin.tab.activity");
    let tab_settings = crate::i18n::t("admin.tab.settings");

    rsx! {
        Navbar {}
        div { class: "admin-panel",
            h1 { "{title}" }

            div { class: "admin-tabs",
                button {
                    class: if *active_tab.read() == "overview" { "tab-btn active" } else { "tab-btn" },
                    onclick: move |_| active_tab.set("overview".to_string()),
                    "{tab_overview}"
                }
                button {
                    class: if *active_tab.read() == "users" { "tab-btn active" } else { "tab-btn" },
                    onclick: move |_| active_tab.set("users".to_string()),
                    "{tab_users}"
                }
                button {
                    class: if *active_tab.read() == "activity" { "tab-btn active" } else { "tab-btn" },
                    onclick: move |_| active_tab.set("activity".to_string()),
                    "{tab_activity}"
                }
                button {
                    class: if *active_tab.read() == "settings" { "tab-btn active" } else { "tab-btn" },
                    onclick: move |_| active_tab.set("settings".to_string()),
                    "{tab_settings}"
                }
            }

            match active_tab.read().as_str() {
                "overview" => rsx! { AdminOverview {} },
                "users" => rsx! { AdminUsers {} },
                "activity" => rsx! { AdminActivity {} },
                "settings" => rsx! { AdminSettings {} },
                _ => rsx! { AdminOverview {} },
            }
        }
    }
}

#[component]
fn AdminOverview() -> Element {
    let _ = use_context::<Signal<String>>().read();
    let auth = use_context::<Signal<AuthState>>();

    let stats = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        async move { api_get::<AdminStats>("/admin/stats", &token).await }
    });

    let (users_val, files_val, docs_val, storage_val) = match &*stats.read() {
        Some(Ok(s)) => (
            s.total_users.to_string(),
            s.total_files.to_string(),
            s.total_documents.to_string(),
            format!("{} MB", s.total_storage_used / 1_048_576),
        ),
        Some(Err(_)) => (
            "—".to_string(),
            "—".to_string(),
            "—".to_string(),
            "—".to_string(),
        ),
        None => (
            "…".to_string(),
            "…".to_string(),
            "…".to_string(),
            "…".to_string(),
        ),
    };

    rsx! {
        div { class: "admin-grid",
            div { class: "stat-card",
                div { class: "stat-icon users-icon", "U" }
                div { class: "stat-info",
                    h3 { {crate::i18n::t("admin.total_users")} }
                    p { class: "stat-value", "{users_val}" }
                }
            }
            div { class: "stat-card",
                div { class: "stat-icon files-icon", "F" }
                div { class: "stat-info",
                    h3 { {crate::i18n::t("admin.total_files")} }
                    p { class: "stat-value", "{files_val}" }
                }
            }
            div { class: "stat-card",
                div { class: "stat-icon docs-icon", "D" }
                div { class: "stat-info",
                    h3 { {crate::i18n::t("admin.total_documents")} }
                    p { class: "stat-value", "{docs_val}" }
                }
            }
            div { class: "stat-card",
                div { class: "stat-icon storage-icon", "S" }
                div { class: "stat-info",
                    h3 { {crate::i18n::t("admin.storage_used")} }
                    p { class: "stat-value", "{storage_val}" }
                }
            }
        }

        div { class: "admin-charts",
            div { class: "chart-card",
                h3 { {crate::i18n::t("admin.docs_chart")} }
                div { class: "chart-placeholder",
                    p { {crate::i18n::t("admin.chart_placeholder")} }
                }
            }
            div { class: "chart-card",
                h3 { {crate::i18n::t("admin.users_chart")} }
                div { class: "chart-placeholder",
                    p { {crate::i18n::t("admin.chart_placeholder")} }
                }
            }
        }
    }
}

#[component]
fn AdminUsers() -> Element {
    let _ = use_context::<Signal<String>>().read();
    let auth = use_context::<Signal<AuthState>>();

    let users = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        async move { api_get::<Vec<UserSummary>>("/admin/users", &token).await }
    });

    rsx! {
        div { class: "admin-section",
            div { class: "section-header",
                h2 { {crate::i18n::t("admin.users_heading")} }
                button { class: "btn btn-primary btn-sm", {crate::i18n::t("admin.add_user")} }
            }

            div { class: "admin-filter-bar",
                input {
                    r#type: "search",
                    class: "form-input",
                    placeholder: crate::i18n::t("admin.search_users"),
                }
                select { class: "form-select",
                    option { {crate::i18n::t("admin.all_roles")} }
                    option { {crate::i18n::t("admin.role.admin")} }
                    option { {crate::i18n::t("admin.role.editor")} }
                    option { {crate::i18n::t("admin.role.viewer")} }
                }
            }

            table { class: "admin-table",
                thead {
                    tr {
                        th { {crate::i18n::t("admin.col.username")} }
                        th { {crate::i18n::t("admin.col.email")} }
                        th { {crate::i18n::t("admin.col.role")} }
                        th { {crate::i18n::t("admin.col.storage")} }
                        th { {crate::i18n::t("admin.col.last_login")} }
                        th { {crate::i18n::t("admin.col.actions")} }
                    }
                }
                tbody {
                    match &*users.read() {
                        None => rsx! {
                            tr {
                                td { colspan: "6", {crate::i18n::t("admin.loading_users")} }
                            }
                        },
                        Some(Err(e)) => rsx! {
                            tr {
                                td { colspan: "6", {crate::i18n::t("admin.load_users_failed").replace("{err}", &e.to_string())} }
                            }
                        },
                        Some(Ok(list)) if list.is_empty() => rsx! {
                            tr {
                                td { colspan: "6", {crate::i18n::t("admin.no_users")} }
                            }
                        },
                        Some(Ok(list)) => rsx! {
                            for user in list.iter() {
                                {
                                    let badge_class = match user.role.as_str() {
                                        "admin" => "badge badge-admin",
                                        "editor" => "badge badge-editor",
                                        "viewer" => "badge badge-viewer",
                                        _ => "badge",
                                    };
                                    let storage_text = format!("{} MB", user.storage_used / 1_048_576);
                                    let last_login = user.last_login_at.clone().unwrap_or_else(|| crate::i18n::t("admin.never"));
                                    let key = user.id.clone();
                                    rsx! {
                                        tr { key: "{key}",
                                            td { "{user.username}" }
                                            td { "{user.email}" }
                                            td { span { class: "{badge_class}", "{user.role}" } }
                                            td { "{storage_text}" }
                                            td { "{last_login}" }
                                            td { class: "table-actions",
                                                button { class: "btn-icon", {crate::i18n::t("admin.edit")} }
                                                button { class: "btn-icon danger", {crate::i18n::t("admin.disable")} }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }
                }
            }

            div { class: "pagination",
                button { class: "btn btn-outline btn-sm", disabled: true, {crate::i18n::t("common.previous")} }
                span { class: "page-info", {crate::i18n::t("admin.page_info")} }
                button { class: "btn btn-outline btn-sm", disabled: true, {crate::i18n::t("common.next")} }
            }
        }
    }
}

#[component]
fn AdminActivity() -> Element {
    let _ = use_context::<Signal<String>>().read();
    let auth = use_context::<Signal<AuthState>>();

    let activity = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        async move { api_get::<Vec<ActivityEntry>>("/admin/activity", &token).await }
    });

    rsx! {
        div { class: "admin-section",
            h2 { {crate::i18n::t("admin.activity_log")} }

            div { class: "admin-filter-bar",
                input {
                    r#type: "search",
                    class: "form-input",
                    placeholder: crate::i18n::t("admin.search_activity"),
                }
                select { class: "form-select",
                    option { {crate::i18n::t("admin.all_actions")} }
                    option { {crate::i18n::t("admin.action.create")} }
                    option { {crate::i18n::t("admin.action.update")} }
                    option { {crate::i18n::t("admin.action.delete")} }
                    option { {crate::i18n::t("admin.action.share")} }
                    option { {crate::i18n::t("admin.action.login")} }
                }
                input { r#type: "date", class: "form-input" }
            }

            div { class: "activity-log",
                match &*activity.read() {
                    None => rsx! {
                        div { class: "log-entry",
                            span { class: "log-time", {crate::i18n::t("admin.loading_activity")} }
                        }
                    },
                    Some(Err(e)) => rsx! {
                        div { class: "log-entry",
                            span { class: "log-time", {crate::i18n::t("admin.load_activity_failed").replace("{err}", &e.to_string())} }
                        }
                    },
                    Some(Ok(list)) if list.is_empty() => rsx! {
                        div { class: "log-entry",
                            span { class: "log-time", {crate::i18n::t("admin.no_activity")} }
                        }
                    },
                    Some(Ok(list)) => rsx! {
                        for entry in list.iter() {
                            {
                                let key = entry.id.clone();
                                let resource_text = format!("{} {}", entry.resource_type, entry.resource_id);
                                rsx! {
                                    div { key: "{key}", class: "log-entry",
                                        span { class: "log-time", "{entry.created_at}" }
                                        span { class: "log-user", "{entry.user_id}" }
                                        span { class: "log-action", "{entry.action}" }
                                        span { class: "log-resource", "{resource_text}" }
                                    }
                                }
                            }
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn AdminSettings() -> Element {
    let _ = use_context::<Signal<String>>().read();
    rsx! {
        div { class: "admin-section",
            h2 { {crate::i18n::t("admin.system_settings")} }

            div { class: "settings-form",
                div { class: "form-group",
                    label { {crate::i18n::t("admin.default_quota")} }
                    input {
                        r#type: "number",
                        class: "form-input",
                        value: "1024",
                    }
                }
                div { class: "form-group",
                    label { {crate::i18n::t("admin.max_upload")} }
                    input {
                        r#type: "number",
                        class: "form-input",
                        value: "50",
                    }
                }
                div { class: "form-group",
                    label { {crate::i18n::t("admin.allow_registration")} }
                    select { class: "form-select",
                        option { value: "true", {crate::i18n::t("admin.yes")} }
                        option { value: "false", {crate::i18n::t("admin.no")} }
                    }
                }
                div { class: "form-group",
                    label { {crate::i18n::t("admin.default_role")} }
                    select { class: "form-select",
                        option { {crate::i18n::t("admin.role.editor")} }
                        option { {crate::i18n::t("admin.role.viewer")} }
                    }
                }
                div { class: "form-group",
                    label { {crate::i18n::t("admin.smtp")} }
                    input {
                        r#type: "text",
                        class: "form-input",
                        placeholder: crate::i18n::t("admin.smtp_placeholder"),
                    }
                }
                button { class: "btn btn-primary", {crate::i18n::t("admin.save_settings")} }
            }
        }
    }
}
