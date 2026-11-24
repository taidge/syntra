use crate::{
    components::{navbar::Navbar, ui::*},
    state::{self, AuthState},
};
use glory::prelude::*;
#[derive(Clone, serde::Deserialize)]
struct Stats {
    total_users: u64,
    total_files: u64,
    total_documents: u64,
    total_storage_used: i64,
}
#[derive(Clone, serde::Deserialize)]
struct User {
    id: String,
    username: String,
    email: String,
    role: String,
    storage_used: i64,
    last_login_at: Option<String>,
}
#[derive(Clone, serde::Deserialize)]
struct Activity {
    id: String,
    user_id: String,
    action: String,
    resource_type: String,
    resource_id: String,
    created_at: String,
}
#[derive(Clone, serde::Deserialize)]
struct Setting {
    key: String,
    value: String,
}
fn users_load(
    scope: &Scope,
    auth: Cage<AuthState>,
    users: Cage<Vec<User>>,
    feedback: Cage<String>,
) {
    let token = token(auth);
    let _ = scope.spawn(async move {
        match state::api_get::<Vec<User>>("/admin/users", &token).await {
            Ok(value) => set(users, value),
            Err(error) => set(feedback, error),
        }
    });
}
fn choice(name: &str, value: Cage<String>, options: Vec<(&str, String)>) -> sapid::NativeSelect {
    sapid::native_select(name.replace(' ', "-").to_ascii_lowercase())
        .theme(theme())
        .unstyled()
        .class("form-select")
        .label(name)
        .value(value.map(|value| Some(value.clone())))
        .options(
            options
                .into_iter()
                .map(|(key, label)| sapid::select_option(key, label)),
        )
        .on_value_change(move |selected| set(value, selected.unwrap_or_default()))
}
#[allow(non_snake_case)]
pub fn Admin(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    let allowed = auth.get().user.is_some_and(|user| user.role == "admin");
    if !allowed {
        return Box::new(col().fill((
            Navbar(scope, auth),
            heading("Administrator access required"),
        ))) as Box<dyn Widget>;
    }
    let tab = scope.cage("overview".to_string());
    let stats = scope.cage(None);
    let users = scope.cage(Vec::<User>::new());
    let activity = scope.cage(Vec::<Activity>::new());
    let feedback = scope.cage(String::new());
    let search = scope.cage(String::new());
    let role_filter = scope.cage("all".to_string());
    let activity_search = scope.cage(String::new());
    let action_filter = scope.cage("all".to_string());
    load::<Stats>(&scope, "/admin/stats".into(), auth, stats);
    users_load(&scope, auth, users, feedback);
    let activity_token = token(auth);
    let _ = scope.spawn(async move {
        match state::api_get::<Vec<Activity>>("/admin/activity", &activity_token).await {
            Ok(value) => set(activity, value),
            Err(error) => set(feedback, error),
        }
    });
    let settings = scope.cage(Vec::<Setting>::new());
    let settings_token = token(auth);
    let _ = scope.spawn(async move {
        match state::api_get::<Vec<Setting>>("/settings", &settings_token).await {
            Ok(value) => set(settings, value),
            Err(error) => set(feedback, error),
        }
    });
    let quota = scope.cage("1024".to_string());
    let upload = scope.cage("50".to_string());
    let registration = scope.cage("true".to_string());
    let role = scope.cage("editor".to_string());
    let seeded = scope.cage(false);
    scope.effect(move || {
        if !seeded.get() && !settings.get().is_empty() {
            for Setting { key, value } in settings.get() {
                match key.as_str() {
                    "default_quota_mb" => set(quota, value),
                    "max_upload_mb" => set(upload, value),
                    "allow_registration" => set(registration, value),
                    "default_role" => set(role, value),
                    _ => {}
                }
            }
            set(seeded, true);
        }
    });
    let username = scope.cage(String::new());
    let email = scope.cage(String::new());
    let password = scope.cage(String::new());
    let selected_role = scope.cage("editor".to_string());
    let create_open = scope.cage(false);
    let create_weak = scope.downgrade();
    let users_weak = scope.downgrade();
    let settings_weak = scope.downgrade();
    let shown = scope.bond(move || {
        let search = search.get().to_lowercase();
        users
            .get()
            .into_iter()
            .filter(|user| {
                format!("{} {}", user.username, user.email)
                    .to_lowercase()
                    .contains(&search)
                    && (role_filter.get() == "all" || user.role == role_filter.get())
            })
            .collect::<Vec<_>>()
    });
    let overview = when(scope.bond(move || tab.get() == "overview"), move || {
        widget(move |mounted_scope| {
            let cards = [
                ("U", "users", "admin.total_users"),
                ("F", "files", "admin.total_files"),
                ("D", "docs", "admin.total_documents"),
                ("S", "storage", "admin.storage_used"),
            ]
            .into_iter()
            .map(|(icon, key, title)| {
                view().class("stat-card").fill((
                    view()
                        .class(format!("stat-icon {key}-icon"))
                        .fill(label(icon)),
                    view().class("stat-info").fill((
                        text()
                            .kind(TextKind::Heading(3))
                            .content(crate::i18n::t(title)),
                        result_text(mounted_scope, stats, move |stats| match key {
                            "users" => stats.total_users.to_string(),
                            "files" => stats.total_files.to_string(),
                            "docs" => stats.total_documents.to_string(),
                            _ => format!("{} MB", stats.total_storage_used / 1_048_576),
                        })
                        .class("stat-value"),
                    )),
                ))
            })
            .collect::<Vec<_>>();
            let bars = [
                ("admin.total_users", "users"),
                ("admin.total_files", "files"),
                ("admin.total_documents", "docs"),
            ]
            .into_iter()
            .map(|(title, key)| {
                view().class("chart-bar-row").fill((
                    label(crate::i18n::t(title)).class("chart-bar-label"),
                    view()
                        .class("chart-bar-track")
                        .fill(
                            view()
                                .class("chart-bar-fill")
                                .style(stats.map(move |value| {
                                    let (value, max) = value
                                        .as_ref()
                                        .and_then(|value| value.as_ref().ok())
                                        .map(|stats| {
                                            let n = match key {
                                                "users" => stats.total_users,
                                                "files" => stats.total_files,
                                                _ => stats.total_documents,
                                            };
                                            (
                                                n,
                                                stats
                                                    .total_users
                                                    .max(stats.total_files)
                                                    .max(stats.total_documents)
                                                    .max(1),
                                            )
                                        })
                                        .unwrap_or((0, 1));
                                    Style::new()
                                        .width(Length::Percent(value as f64 / max as f64 * 100.))
                                })),
                        ),
                    result_text(mounted_scope, stats, move |stats| match key {
                        "users" => stats.total_users.to_string(),
                        "files" => stats.total_files.to_string(),
                        _ => stats.total_documents.to_string(),
                    })
                    .class("chart-bar-value"),
                ))
            })
            .collect::<Vec<_>>();
            view()
                .fill((
                    view().class("admin-grid").fill(cards),
                    view().class("admin-charts").fill(
                        view().class("chart-card").fill((
                            text()
                                .kind(TextKind::Heading(3))
                                .content(crate::i18n::t("admin.totals_chart")),
                            view().class("chart-bars").fill(bars),
                        )),
                    ),
                ))
                .build(mounted_scope)
        })
    });
    let user_panel = when(scope.bond(move || tab.get() == "users"), move || {
        let create_weak = create_weak.clone();
        let users_weak = users_weak.clone();
        let rows = each(shown)
            .key(|user| user.id.clone())
            .children(move |item_row| {
                let item = item_row.value;
                let user = item.get();
                let role_weak = users_weak.clone();
                let disable_weak = users_weak.clone();
                let role_id = user.id.clone();
                let disable_id = user.id.clone();
                let update_role = move |selected: Option<String>| {
                    let Some(role) = selected else {
                        return;
                    };
                    if !["admin", "editor", "viewer"].contains(&role.as_str()) {
                        set(feedback, "Choose admin, editor, or viewer".into());
                        return;
                    }
                    let Some(scope) = role_weak.upgrade() else {
                        return;
                    };
                    let token = token(auth);
                    let path = format!("/admin/users/{role_id}");
                    let weak = scope.downgrade();
                    let _ = scope.spawn(async move {
                        let body = serde_json::json!({ "role": role });
                        match state::api_put::<serde_json::Value, _>(&path, &body, &token).await {
                            Ok(_) => {
                                if let Some(scope) = weak.upgrade() {
                                    users_load(&scope, auth, users, feedback);
                                }
                            }
                            Err(error) => set(feedback, error),
                        }
                    });
                };
                let disable = action("Disable", move || {
                    let confirmed = web_sys::window().is_some_and(|window| {
                        window
                            .confirm_with_message("Disable this account?")
                            .unwrap_or(false)
                    });
                    if !confirmed {
                        return;
                    }
                    let Some(scope) = disable_weak.upgrade() else {
                        return;
                    };
                    let token = token(auth);
                    let path = format!("/admin/users/{disable_id}/disable");
                    let weak = scope.downgrade();
                    let _ = scope.spawn(async move {
                        match state::api_put::<serde_json::Value, _>(
                            &path,
                            &serde_json::json!({}),
                            &token,
                        )
                        .await
                        {
                            Ok(_) => {
                                if let Some(scope) = weak.upgrade() {
                                    users_load(&scope, auth, users, feedback);
                                }
                            }
                            Err(error) => set(feedback, error),
                        }
                    });
                });
                view().class("admin-user-row").fill((
                    text().content(item.map(|user| user.username.clone())),
                    text().content(item.map(|user| user.email.clone())),
                    sapid::native_select(format!("user-role-{}", user.id))
                        .theme(theme())
                        .unstyled()
                        .class("form-select")
                        .label(crate::i18n::t("admin.col.role"))
                        .value(item.map(|user| Some(user.role.clone())))
                        .options(["admin", "editor", "viewer"].map(|role| {
                            sapid::select_option(
                                role,
                                crate::i18n::t(&format!("admin.role.{role}")),
                            )
                        }))
                        .on_value_change(update_role),
                    text()
                        .content(item.map(|user| format!("{} MB", user.storage_used / 1_048_576))),
                    text().content(item.map(|user| user.last_login_at.clone().unwrap_or_default())),
                    row()
                        .class("table-actions")
                        .fill(disable.unstyled().class("btn btn-outline btn-sm danger")),
                ))
            });
        let create_submit = std::rc::Rc::new(std::cell::RefCell::new(move || {
            if password.get().len() < 8 {
                set(feedback, crate::i18n::t("profile.password_too_short"));
                return;
            }
            let role = selected_role.get();
            if !["admin", "editor", "viewer"].contains(&role.as_str()) {
                set(feedback, "Choose admin, editor, or viewer".into());
                return;
            }
            let Some(scope) = create_weak.upgrade() else {
                return;
            };
            let body = serde_json::json!({
                "username": username.get(),
                "email": email.get(),
                "password": password.get(),
            });
            let weak = scope.downgrade();
            let token = token(auth);
            let _ = scope.spawn(async move {
                match state::api_post::<serde_json::Value, _>("/auth/register", &body, "").await {
                    Ok(value) => {
                        if let Some(id) = value
                            .get("user")
                            .and_then(|user| user.get("id"))
                            .and_then(|id| id.as_str())
                        {
                            let body = serde_json::json!({ "role": role });
                            if let Err(error) = state::api_put::<serde_json::Value, _>(
                                &format!("/admin/users/{id}"),
                                &body,
                                &token,
                            )
                            .await
                            {
                                set(feedback, error);
                                return;
                            }
                        }
                        set(password, String::new());
                        set(username, String::new());
                        set(email, String::new());
                        if let Some(scope) = weak.upgrade() {
                            users_load(&scope, auth, users, feedback);
                        }
                        set(feedback, "User created".into());
                        set(create_open, false);
                    }
                    Err(error) => set(feedback, error),
                }
            });
        }));
        view().class("admin-section").fill((
            view().class("section-header").fill((
                text()
                    .kind(TextKind::Heading(2))
                    .content(crate::i18n::t("admin.users_heading")),
                action(crate::i18n::t("admin.add_user"), move || {
                    set(create_open, true)
                })
                .unstyled()
                .class("btn btn-primary btn-sm"),
            )),
            view().class("admin-filter-bar").fill((
                sapid::text_field()
                    .theme(theme())
                    .unstyled()
                    .class("form-input")
                    .name(crate::i18n::t("admin.search_users"))
                    .placeholder(crate::i18n::t("admin.search_users"))
                    .value(search)
                    .on_value_change(move |value| set(search, value)),
                choice(
                    "Role filter",
                    role_filter,
                    vec![
                        ("all", crate::i18n::t("admin.all_roles")),
                        ("admin", crate::i18n::t("admin.role.admin")),
                        ("editor", crate::i18n::t("admin.role.editor")),
                        ("viewer", crate::i18n::t("admin.role.viewer")),
                    ],
                ),
            )),
            view().class("admin-user-table").fill((
                view().class("admin-user-row admin-user-head").fill(
                    [
                        "Username",
                        "Email",
                        "Role",
                        "Storage Used",
                        "Last login",
                        "Actions",
                    ]
                    .into_iter()
                    .map(label)
                    .collect::<Vec<_>>(),
                ),
                rows,
            )),
            sapid::dialog("admin-create-user")
                .theme(theme())
                .class("syntra-modal")
                .title(crate::i18n::t("admin.add_user"))
                .close_label(crate::i18n::t("common.close"))
                .close_content(|| text().kind(TextKind::Inline).content("×"))
                .show_trigger(false)
                .open(create_open)
                .on_open_change(move |value| set(create_open, value))
                .content({
                    let create_submit = create_submit.clone();
                    move || {
                        view().class("modal-form modal-body").fill((
                            field(crate::i18n::t("auth.username"), username, Purpose::Text),
                            field(crate::i18n::t("auth.email"), email, Purpose::Email),
                            field(crate::i18n::t("auth.password"), password, Purpose::Password),
                            field("Role", selected_role, Purpose::Text),
                            view().class("modal-actions").fill((
                                action(crate::i18n::t("common.cancel"), move || {
                                    set(create_open, false)
                                })
                                .unstyled()
                                .class("btn btn-outline"),
                                action("Create user", {
                                    let create_submit = create_submit.clone();
                                    move || (create_submit.borrow_mut())()
                                })
                                .unstyled()
                                .class("btn btn-primary"),
                            )),
                        ))
                    }
                }),
        ))
    });

    let shown_activity = scope.bond(move || {
        let needle = activity_search.get().to_lowercase();
        activity
            .get()
            .into_iter()
            .filter(|entry| {
                (action_filter.get() == "all"
                    || entry.action.to_lowercase().contains(&action_filter.get()))
                    && format!(
                        "{} {} {} {}",
                        entry.user_id, entry.action, entry.resource_type, entry.resource_id
                    )
                    .to_lowercase()
                    .contains(&needle)
            })
            .collect::<Vec<_>>()
    });
    let activity_panel = when(scope.bond(move || tab.get() == "activity"), move || {
        view().class("admin-section admin-activity").fill((
            text()
                .kind(TextKind::Heading(2))
                .content(crate::i18n::t("admin.activity_log")),
            view().class("admin-filter-bar").fill((
                sapid::text_field()
                    .theme(theme())
                    .unstyled()
                    .class("form-input")
                    .name(crate::i18n::t("admin.search_activity"))
                    .placeholder(crate::i18n::t("admin.search_activity"))
                    .value(activity_search)
                    .on_value_change(move |value| set(activity_search, value)),
                choice(
                    "Action filter",
                    action_filter,
                    std::iter::once(("all", crate::i18n::t("admin.all_actions")))
                        .chain(
                            ["create", "update", "delete", "share", "login"]
                                .into_iter()
                                .map(|action| {
                                    (action, crate::i18n::t(&format!("admin.action.{action}")))
                                }),
                        )
                        .collect(),
                ),
            )),
            view().class("activity-log").fill(
                each(shown_activity)
                    .key(|entry| entry.id.clone())
                    .children(move |item_row| {
                        let entry = item_row.value;
                        let entry = entry.get();
                        view().class("log-entry").fill((
                            label(entry.created_at).class("log-time"),
                            label(entry.user_id).class("log-user"),
                            label(entry.action).class("log-action"),
                            label(format!("{}/{}", entry.resource_type, entry.resource_id))
                                .class("log-resource"),
                        ))
                    }),
            ),
        ))
    });

    let settings_panel = when(scope.bond(move || tab.get() == "settings"), move || {
        let weak = settings_weak.clone();
        let save = action(crate::i18n::t("admin.save_settings"), move || {
            let Some(scope) = weak.upgrade() else {
                return;
            };
            let token = token(auth);
            let pairs = [
                ("default_quota_mb", quota.get()),
                ("max_upload_mb", upload.get()),
                ("allow_registration", registration.get()),
                ("default_role", role.get()),
            ];
            let _ = scope.spawn(async move {
                for (key, value) in pairs {
                    let body = serde_json::json!({ "value": value });
                    if let Err(error) = state::api_put::<serde_json::Value, _>(
                        &format!("/settings/{key}"),
                        &body,
                        &token,
                    )
                    .await
                    {
                        set(feedback, error);
                        return;
                    }
                }
                set(feedback, crate::i18n::t("admin.settings_saved"));
            });
        });
        view().class("admin-section admin-settings").fill((
            text()
                .kind(TextKind::Heading(2))
                .content(crate::i18n::t("admin.system_settings")),
            view().class("settings-form").fill((
                field(
                    crate::i18n::t("admin.default_quota"),
                    quota,
                    Purpose::Number,
                ),
                field(crate::i18n::t("admin.max_upload"), upload, Purpose::Number),
                view().class("form-group").fill((
                    label(crate::i18n::t("admin.allow_registration")).class("form-label"),
                    choice(
                        "Allow registration",
                        registration,
                        vec![
                            ("true", crate::i18n::t("admin.yes")),
                            ("false", crate::i18n::t("admin.no")),
                        ],
                    ),
                )),
                view().class("form-group").fill((
                    label(crate::i18n::t("admin.default_role")).class("form-label"),
                    choice(
                        "Default role",
                        role,
                        vec![
                            ("editor", crate::i18n::t("admin.role.editor")),
                            ("viewer", crate::i18n::t("admin.role.viewer")),
                        ],
                    ),
                )),
                save.unstyled().class("btn btn-primary"),
            )),
        ))
    });

    let tabs = ["overview", "users", "activity", "settings"]
        .into_iter()
        .map(|value| {
            action(crate::i18n::t(&format!("admin.tab.{value}")), move || {
                set(tab, value.into());
            })
            .unstyled()
            .class(tab.map(move |tab| {
                if tab == value {
                    "tab-btn active".to_string()
                } else {
                    "tab-btn".to_string()
                }
            }))
        })
        .collect::<Vec<_>>();
    let panel = view().class("admin-panel").fill((
        heading(crate::i18n::t("admin.title")),
        view().class("admin-tabs").fill(tabs),
        status(feedback),
        overview,
        user_panel,
        activity_panel,
        settings_panel,
    ));
    Box::new(
        view()
            .class("app-page admin-page")
            .fill((Navbar(scope, auth), panel)),
    ) as Box<dyn Widget>
}
