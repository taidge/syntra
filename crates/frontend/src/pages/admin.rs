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
    let mut active_tab = use_signal(|| "overview".to_string());

    rsx! {
        Navbar {}
        div { class: "admin-panel",
            h1 { "Admin Panel" }

            div { class: "admin-tabs",
                button {
                    class: if *active_tab.read() == "overview" { "tab-btn active" } else { "tab-btn" },
                    onclick: move |_| active_tab.set("overview".to_string()),
                    "Overview"
                }
                button {
                    class: if *active_tab.read() == "users" { "tab-btn active" } else { "tab-btn" },
                    onclick: move |_| active_tab.set("users".to_string()),
                    "Users"
                }
                button {
                    class: if *active_tab.read() == "activity" { "tab-btn active" } else { "tab-btn" },
                    onclick: move |_| active_tab.set("activity".to_string()),
                    "Activity Log"
                }
                button {
                    class: if *active_tab.read() == "settings" { "tab-btn active" } else { "tab-btn" },
                    onclick: move |_| active_tab.set("settings".to_string()),
                    "Settings"
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
                    h3 { "Total Users" }
                    p { class: "stat-value", "{users_val}" }
                }
            }
            div { class: "stat-card",
                div { class: "stat-icon files-icon", "F" }
                div { class: "stat-info",
                    h3 { "Total Files" }
                    p { class: "stat-value", "{files_val}" }
                }
            }
            div { class: "stat-card",
                div { class: "stat-icon docs-icon", "D" }
                div { class: "stat-info",
                    h3 { "Total Documents" }
                    p { class: "stat-value", "{docs_val}" }
                }
            }
            div { class: "stat-card",
                div { class: "stat-icon storage-icon", "S" }
                div { class: "stat-info",
                    h3 { "Storage Used" }
                    p { class: "stat-value", "{storage_val}" }
                }
            }
        }

        div { class: "admin-charts",
            div { class: "chart-card",
                h3 { "Documents Created (Last 7 Days)" }
                div { class: "chart-placeholder",
                    p { "Chart will be rendered here" }
                }
            }
            div { class: "chart-card",
                h3 { "Active Users (Last 7 Days)" }
                div { class: "chart-placeholder",
                    p { "Chart will be rendered here" }
                }
            }
        }
    }
}

#[component]
fn AdminUsers() -> Element {
    let auth = use_context::<Signal<AuthState>>();

    let users = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        async move { api_get::<Vec<UserSummary>>("/admin/users", &token).await }
    });

    rsx! {
        div { class: "admin-section",
            div { class: "section-header",
                h2 { "Users" }
                button { class: "btn btn-primary btn-sm", "Add User" }
            }

            div { class: "admin-filter-bar",
                input {
                    r#type: "search",
                    class: "form-input",
                    placeholder: "Search users...",
                }
                select { class: "form-select",
                    option { "All Roles" }
                    option { "Admin" }
                    option { "Editor" }
                    option { "Viewer" }
                }
            }

            table { class: "admin-table",
                thead {
                    tr {
                        th { "Username" }
                        th { "Email" }
                        th { "Role" }
                        th { "Storage Used" }
                        th { "Last Login" }
                        th { "Actions" }
                    }
                }
                tbody {
                    match &*users.read() {
                        None => rsx! {
                            tr {
                                td { colspan: "6", "Loading users…" }
                            }
                        },
                        Some(Err(e)) => rsx! {
                            tr {
                                td { colspan: "6", "Failed to load users: {e}" }
                            }
                        },
                        Some(Ok(list)) if list.is_empty() => rsx! {
                            tr {
                                td { colspan: "6", "No users yet." }
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
                                    let last_login = user.last_login_at.clone().unwrap_or_else(|| "Never".to_string());
                                    let key = user.id.clone();
                                    rsx! {
                                        tr { key: "{key}",
                                            td { "{user.username}" }
                                            td { "{user.email}" }
                                            td { span { class: "{badge_class}", "{user.role}" } }
                                            td { "{storage_text}" }
                                            td { "{last_login}" }
                                            td { class: "table-actions",
                                                button { class: "btn-icon", "Edit" }
                                                button { class: "btn-icon danger", "Disable" }
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
                button { class: "btn btn-outline btn-sm", disabled: true, "Previous" }
                span { class: "page-info", "Page 1 of 1" }
                button { class: "btn btn-outline btn-sm", disabled: true, "Next" }
            }
        }
    }
}

#[component]
fn AdminActivity() -> Element {
    let auth = use_context::<Signal<AuthState>>();

    let activity = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        async move { api_get::<Vec<ActivityEntry>>("/admin/activity", &token).await }
    });

    rsx! {
        div { class: "admin-section",
            h2 { "Activity Log" }

            div { class: "admin-filter-bar",
                input {
                    r#type: "search",
                    class: "form-input",
                    placeholder: "Search activity...",
                }
                select { class: "form-select",
                    option { "All Actions" }
                    option { "Create" }
                    option { "Update" }
                    option { "Delete" }
                    option { "Share" }
                    option { "Login" }
                }
                input { r#type: "date", class: "form-input" }
            }

            div { class: "activity-log",
                match &*activity.read() {
                    None => rsx! {
                        div { class: "log-entry",
                            span { class: "log-time", "Loading activity…" }
                        }
                    },
                    Some(Err(e)) => rsx! {
                        div { class: "log-entry",
                            span { class: "log-time", "Failed to load activity: {e}" }
                        }
                    },
                    Some(Ok(list)) if list.is_empty() => rsx! {
                        div { class: "log-entry",
                            span { class: "log-time", "No activity yet." }
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
    rsx! {
        div { class: "admin-section",
            h2 { "System Settings" }

            div { class: "settings-form",
                div { class: "form-group",
                    label { "Default Storage Quota (MB)" }
                    input {
                        r#type: "number",
                        class: "form-input",
                        value: "1024",
                    }
                }
                div { class: "form-group",
                    label { "Max Upload Size (MB)" }
                    input {
                        r#type: "number",
                        class: "form-input",
                        value: "50",
                    }
                }
                div { class: "form-group",
                    label { "Allow Public Registration" }
                    select { class: "form-select",
                        option { value: "true", "Yes" }
                        option { value: "false", "No" }
                    }
                }
                div { class: "form-group",
                    label { "Default User Role" }
                    select { class: "form-select",
                        option { "editor" }
                        option { "viewer" }
                    }
                }
                div { class: "form-group",
                    label { "SMTP Server (for notifications)" }
                    input {
                        r#type: "text",
                        class: "form-input",
                        placeholder: "smtp.example.com",
                    }
                }
                button { class: "btn btn-primary", "Save Settings" }
            }
        }
    }
}
