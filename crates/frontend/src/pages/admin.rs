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
            })
            .collect::<Vec<_>>()
    });
    let overview = when(scope.bond(move || tab.get() == "overview"), move || {
        widget(move |mounted_scope| {
            let totals = result_text(mounted_scope, stats, |stats| {
                format!(
                    "Users: {}\nFiles: {}\nDocuments: {}\nStorage: {} MB",
                    stats.total_users,
                    stats.total_files,
                    stats.total_documents,
                    stats.total_storage_used / 1_048_576,
                )
            });
            col().class("admin-grid").fill(totals).build(mounted_scope)
        })
    });

    let user_panel = when(scope.bond(move || tab.get() == "users"), move || {
        let create_weak = create_weak.clone();
        let users_weak = users_weak.clone();
        let rows = each(
            shown,
            |user| user.id.clone(),
            move |item, _| {
                let user = item.get();
                let role_weak = users_weak.clone();
                let disable_weak = users_weak.clone();
                let role_id = user.id.clone();
                let disable_id = user.id.clone();
                let details = label(format!(
                    "{} · {} · {} · {} bytes · {}",
                    user.username,
                    user.email,
                    user.role,
                    user.storage_used,
                    user.last_login_at.unwrap_or_default(),
                ));
                let update_role = action("Set role", move || {
                    let Some(window) = web_sys::window() else {
                        return;
                    };
                    let Ok(Some(role)) = window.prompt_with_message("Role: admin, editor, viewer")
                    else {
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
                });
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
                row().fill((details, update_role, disable))
            },
        );
        let create = action("Create user", move || {
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
                    }
                    Err(error) => set(feedback, error),
                }
            });
        });
        col().fill((
            field(crate::i18n::t("common.search"), search, Purpose::Search),
            col().fill(rows),
            heading("Create user"),
            field(crate::i18n::t("auth.username"), username, Purpose::Text),
            field(crate::i18n::t("auth.email"), email, Purpose::Email),
            field(crate::i18n::t("auth.password"), password, Purpose::Password),
            field("Role", selected_role, Purpose::Text),
            create,
        ))
    });

    let activity_panel = when(scope.bond(move || tab.get() == "activity"), move || {
        col().fill(each(
            activity,
            |entry| entry.id.clone(),
            |entry, _| {
                let entry = entry.get();
                label(format!(
                    "{} · {} · {} · {}/{}",
                    entry.created_at,
                    entry.user_id,
                    entry.action,
                    entry.resource_type,
                    entry.resource_id,
                ))
            },
        ))
    });

    let settings_panel = when(scope.bond(move || tab.get() == "settings"), move || {
        let weak = settings_weak.clone();
        let save = action(crate::i18n::t("common.save"), move || {
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
        col().fill((
            field("Default quota (MB)", quota, Purpose::Number),
            field("Maximum upload (MB)", upload, Purpose::Number),
            field(
                "Allow registration (true/false)",
                registration,
                Purpose::Text,
            ),
            field("Default role", role, Purpose::Text),
            save,
        ))
    });

    let tabs = ["overview", "users", "activity", "settings"]
        .into_iter()
        .map(|value| {
            action(crate::i18n::t(&format!("admin.tab.{value}")), move || {
                set(tab, value.into());
            })
        })
        .collect::<Vec<_>>();
    let panel = col().class("admin-panel").fill((
        heading(crate::i18n::t("admin.title")),
        row().class("admin-tabs").fill(tabs),
        status(feedback),
        overview,
        user_panel,
        activity_panel,
        settings_panel,
    ));
    Box::new(col().fill((Navbar(scope, auth), panel))) as Box<dyn Widget>
}
