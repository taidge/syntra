use dioxus::prelude::*;

use crate::components::modal::Modal;
use crate::components::navbar::Navbar;
use crate::components::{Toast, ToastKind, push_toast};
use crate::state::{AuthState, api_get, api_post, api_put};

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

    // Build simple proportional bars from the live totals.
    let bars: Vec<(String, u64, f64)> = match &*stats.read() {
        Some(Ok(s)) => {
            let entries = [
                (crate::i18n::t("admin.total_users"), s.total_users),
                (crate::i18n::t("admin.total_files"), s.total_files),
                (crate::i18n::t("admin.total_documents"), s.total_documents),
            ];
            let max = entries.iter().map(|(_, v)| *v).max().unwrap_or(1).max(1);
            entries
                .into_iter()
                .map(|(label, v)| (label, v, (v as f64 / max as f64) * 100.0))
                .collect()
        }
        _ => Vec::new(),
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
                h3 { {crate::i18n::t("admin.totals_chart")} }
                if bars.is_empty() {
                    div { class: "chart-placeholder",
                        p { {crate::i18n::t("admin.chart_placeholder")} }
                    }
                } else {
                    div { class: "chart-bars",
                        for (label, value, pct) in bars {
                            div { class: "chart-bar-row",
                                span { class: "chart-bar-label", "{label}" }
                                div { class: "chart-bar-track",
                                    div {
                                        class: "chart-bar-fill",
                                        style: "width: {pct}%;",
                                    }
                                }
                                span { class: "chart-bar-value", "{value}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AdminUsers() -> Element {
    let _ = use_context::<Signal<String>>().read();
    let auth = use_context::<Signal<AuthState>>();
    let toast_bus = use_context::<Signal<Vec<Toast>>>();
    let refresh = use_signal(|| 0u32);
    let mut search = use_signal(String::new);
    let mut role_filter = use_signal(String::new);
    let mut show_add = use_signal(|| false);
    let mut new_username = use_signal(String::new);
    let mut new_email = use_signal(String::new);
    let mut new_password = use_signal(String::new);
    let mut add_error = use_signal(|| None::<String>);

    let users = use_resource(move || {
        let _ = refresh.read();
        let token = auth.read().token.clone().unwrap_or_default();
        async move { api_get::<Vec<UserSummary>>("/admin/users", &token).await }
    });

    // Update a user's role via the admin API.
    let update_role = move |(user_id, role): (String, String)| {
        let token = auth.read().token.clone().unwrap_or_default();
        let bus = toast_bus;
        let mut refresh = refresh;
        spawn(async move {
            let body = serde_json::json!({ "role": role });
            match api_put::<serde_json::Value, _>(&format!("/admin/users/{user_id}"), &body, &token)
                .await
            {
                Ok(_) => {
                    push_toast(
                        bus,
                        ToastKind::Success,
                        crate::i18n::t("admin.user_updated"),
                    );
                    let cur = *refresh.read();
                    refresh.set(cur + 1);
                }
                Err(e) => push_toast(bus, ToastKind::Error, e),
            }
        });
    };

    // Disable a user account.
    let disable_user = move |user_id: String| {
        let token = auth.read().token.clone().unwrap_or_default();
        let bus = toast_bus;
        let mut refresh = refresh;
        spawn(async move {
            let body = serde_json::json!({});
            match api_put::<serde_json::Value, _>(
                &format!("/admin/users/{user_id}/disable"),
                &body,
                &token,
            )
            .await
            {
                Ok(_) => {
                    push_toast(
                        bus,
                        ToastKind::Success,
                        crate::i18n::t("admin.user_disabled"),
                    );
                    let cur = *refresh.read();
                    refresh.set(cur + 1);
                }
                Err(e) => push_toast(bus, ToastKind::Error, e),
            }
        });
    };

    let needle = search().trim().to_lowercase();
    let role_sel = role_filter();

    rsx! {
        div { class: "admin-section",
            div { class: "section-header",
                h2 { {crate::i18n::t("admin.users_heading")} }
                button {
                    class: "btn btn-primary btn-sm",
                    onclick: move |_| show_add.set(true),
                    {crate::i18n::t("admin.add_user")}
                }
            }

            div { class: "admin-filter-bar",
                input {
                    r#type: "search",
                    class: "form-input",
                    placeholder: crate::i18n::t("admin.search_users"),
                    value: "{search}",
                    oninput: move |e| search.set(e.value()),
                }
                select {
                    class: "form-select",
                    value: "{role_sel}",
                    onchange: move |e| role_filter.set(e.value()),
                    option { value: "", {crate::i18n::t("admin.all_roles")} }
                    option { value: "admin", {crate::i18n::t("admin.role.admin")} }
                    option { value: "editor", {crate::i18n::t("admin.role.editor")} }
                    option { value: "viewer", {crate::i18n::t("admin.role.viewer")} }
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
                            for user in list.iter().filter(|u| {
                                (needle.is_empty()
                                    || u.username.to_lowercase().contains(&needle)
                                    || u.email.to_lowercase().contains(&needle))
                                    && (role_sel.is_empty() || u.role == role_sel)
                            }) {
                                {
                                    let storage_text = format!("{} MB", user.storage_used / 1_048_576);
                                    let last_login = user.last_login_at.clone().unwrap_or_else(|| crate::i18n::t("admin.never"));
                                    let key = user.id.clone();
                                    let uid_for_role = user.id.clone();
                                    let uid_for_disable = user.id.clone();
                                    let role_now = user.role.clone();
                                    let update_role = update_role.clone();
                                    let disable_user = disable_user.clone();
                                    rsx! {
                                        tr { key: "{key}",
                                            td { "{user.username}" }
                                            td { "{user.email}" }
                                            td {
                                                select {
                                                    class: "form-select form-select-sm",
                                                    value: "{role_now}",
                                                    onchange: move |e| update_role((uid_for_role.clone(), e.value())),
                                                    option { value: "admin", {crate::i18n::t("admin.role.admin")} }
                                                    option { value: "editor", {crate::i18n::t("admin.role.editor")} }
                                                    option { value: "viewer", {crate::i18n::t("admin.role.viewer")} }
                                                }
                                            }
                                            td { "{storage_text}" }
                                            td { "{last_login}" }
                                            td { class: "table-actions",
                                                button {
                                                    class: "btn-icon danger",
                                                    onclick: move |_| disable_user(uid_for_disable.clone()),
                                                    {crate::i18n::t("admin.disable")}
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }
                }
            }
        }

        Modal { title: crate::i18n::t("admin.add_user"), show: show_add,
            div { class: "modal-form",
                div { class: "form-group",
                    label { {crate::i18n::t("admin.col.username")} }
                    input {
                        r#type: "text",
                        class: "form-input",
                        value: "{new_username}",
                        oninput: move |e| new_username.set(e.value()),
                    }
                }
                div { class: "form-group",
                    label { {crate::i18n::t("admin.col.email")} }
                    input {
                        r#type: "email",
                        class: "form-input",
                        value: "{new_email}",
                        oninput: move |e| new_email.set(e.value()),
                    }
                }
                div { class: "form-group",
                    label { {crate::i18n::t("auth.password")} }
                    input {
                        r#type: "password",
                        class: "form-input",
                        value: "{new_password}",
                        oninput: move |e| new_password.set(e.value()),
                    }
                }
                if let Some(err) = add_error() {
                    div { class: "form-error", "{err}" }
                }
                div { class: "modal-actions",
                    button {
                        class: "btn btn-outline",
                        onclick: move |_| show_add.set(false),
                        {crate::i18n::t("common.cancel")}
                    }
                    button {
                        class: "btn btn-primary",
                        onclick: move |_| {
                            let username = new_username.read().clone();
                            let email = new_email.read().clone();
                            let password = new_password.read().clone();
                            if username.is_empty() || email.is_empty() || password.is_empty() {
                                add_error.set(Some(crate::i18n::t("admin.add_user_required")));
                                return;
                            }
                            let bus = toast_bus;
                            let mut refresh = refresh;
                            let mut show_add = show_add;
                            let mut add_error = add_error;
                            spawn(async move {
                                let body = serde_json::json!({
                                    "username": username,
                                    "email": email,
                                    "password": password,
                                });
                                match api_post::<serde_json::Value, _>("/auth/register", &body, "").await {
                                    Ok(_) => {
                                        push_toast(bus, ToastKind::Success, crate::i18n::t("admin.user_created"));
                                        show_add.set(false);
                                        add_error.set(None);
                                        let cur = *refresh.read();
                                        refresh.set(cur + 1);
                                    }
                                    Err(e) => add_error.set(Some(e)),
                                }
                            });
                        },
                        {crate::i18n::t("admin.add_user")}
                    }
                }
            }
        }
    }
}

#[component]
fn AdminActivity() -> Element {
    let _ = use_context::<Signal<String>>().read();
    let auth = use_context::<Signal<AuthState>>();

    let mut search = use_signal(String::new);
    let mut action_filter = use_signal(String::new);

    let activity = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        async move { api_get::<Vec<ActivityEntry>>("/admin/activity", &token).await }
    });

    let needle = search().trim().to_lowercase();
    let action_sel = action_filter();

    rsx! {
        div { class: "admin-section",
            h2 { {crate::i18n::t("admin.activity_log")} }

            div { class: "admin-filter-bar",
                input {
                    r#type: "search",
                    class: "form-input",
                    placeholder: crate::i18n::t("admin.search_activity"),
                    value: "{search}",
                    oninput: move |e| search.set(e.value()),
                }
                select {
                    class: "form-select",
                    value: "{action_sel}",
                    onchange: move |e| action_filter.set(e.value()),
                    option { value: "", {crate::i18n::t("admin.all_actions")} }
                    option { value: "create", {crate::i18n::t("admin.action.create")} }
                    option { value: "update", {crate::i18n::t("admin.action.update")} }
                    option { value: "delete", {crate::i18n::t("admin.action.delete")} }
                    option { value: "share", {crate::i18n::t("admin.action.share")} }
                    option { value: "login", {crate::i18n::t("admin.action.login")} }
                }
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
                        for entry in list.iter().filter(|e| {
                            (action_sel.is_empty() || e.action.to_lowercase().contains(&action_sel))
                                && (needle.is_empty()
                                    || e.action.to_lowercase().contains(&needle)
                                    || e.resource_type.to_lowercase().contains(&needle)
                                    || e.resource_id.to_lowercase().contains(&needle)
                                    || e.user_id.to_lowercase().contains(&needle))
                        }) {
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
    let auth = use_context::<Signal<AuthState>>();
    let toast_bus = use_context::<Signal<Vec<Toast>>>();

    // Load any persisted settings to seed the form.
    let settings = use_resource(move || {
        let token = auth.read().token.clone().unwrap_or_default();
        async move {
            api_get::<Vec<(String, String)>>("/settings", &token)
                .await
                .unwrap_or_default()
        }
    });
    let lookup = move |key: &str, default: &str| -> String {
        settings
            .read()
            .as_ref()
            .and_then(|list| list.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
            .unwrap_or_else(|| default.to_string())
    };

    let mut default_quota = use_signal(|| String::new());
    let mut max_upload = use_signal(|| String::new());
    let mut allow_registration = use_signal(|| String::new());
    let mut default_role = use_signal(|| String::new());

    // Seed inputs once settings have loaded.
    use_effect(move || {
        if settings.read().is_some() {
            if default_quota.read().is_empty() {
                default_quota.set(lookup("default_quota_mb", "1024"));
            }
            if max_upload.read().is_empty() {
                max_upload.set(lookup("max_upload_mb", "50"));
            }
            if allow_registration.read().is_empty() {
                allow_registration.set(lookup("allow_registration", "true"));
            }
            if default_role.read().is_empty() {
                default_role.set(lookup("default_role", "editor"));
            }
        }
    });

    let save = move |_| {
        let token = auth.read().token.clone().unwrap_or_default();
        let bus = toast_bus;
        let pairs = vec![
            ("default_quota_mb", default_quota.read().clone()),
            ("max_upload_mb", max_upload.read().clone()),
            ("allow_registration", allow_registration.read().clone()),
            ("default_role", default_role.read().clone()),
        ];
        spawn(async move {
            let mut ok = true;
            for (key, value) in pairs {
                let body = serde_json::json!({ "value": value });
                if api_put::<serde_json::Value, _>(&format!("/settings/{key}"), &body, &token)
                    .await
                    .is_err()
                {
                    ok = false;
                }
            }
            if ok {
                push_toast(
                    bus,
                    ToastKind::Success,
                    crate::i18n::t("admin.settings_saved"),
                );
            } else {
                push_toast(
                    bus,
                    ToastKind::Error,
                    crate::i18n::t("admin.settings_save_failed"),
                );
            }
        });
    };

    rsx! {
        div { class: "admin-section",
            h2 { {crate::i18n::t("admin.system_settings")} }

            div { class: "settings-form",
                div { class: "form-group",
                    label { {crate::i18n::t("admin.default_quota")} }
                    input {
                        r#type: "number",
                        class: "form-input",
                        value: "{default_quota}",
                        oninput: move |e| default_quota.set(e.value()),
                    }
                }
                div { class: "form-group",
                    label { {crate::i18n::t("admin.max_upload")} }
                    input {
                        r#type: "number",
                        class: "form-input",
                        value: "{max_upload}",
                        oninput: move |e| max_upload.set(e.value()),
                    }
                }
                div { class: "form-group",
                    label { {crate::i18n::t("admin.allow_registration")} }
                    select {
                        class: "form-select",
                        value: "{allow_registration}",
                        onchange: move |e| allow_registration.set(e.value()),
                        option { value: "true", {crate::i18n::t("admin.yes")} }
                        option { value: "false", {crate::i18n::t("admin.no")} }
                    }
                }
                div { class: "form-group",
                    label { {crate::i18n::t("admin.default_role")} }
                    select {
                        class: "form-select",
                        value: "{default_role}",
                        onchange: move |e| default_role.set(e.value()),
                        option { value: "editor", {crate::i18n::t("admin.role.editor")} }
                        option { value: "viewer", {crate::i18n::t("admin.role.viewer")} }
                    }
                }
                button {
                    class: "btn btn-primary",
                    onclick: save,
                    {crate::i18n::t("admin.save_settings")}
                }
            }
        }
    }
}
