use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::navbar::Navbar;
use crate::state::{AuthState, api_get, api_put};

#[derive(Clone, Deserialize, PartialEq)]
struct ProfileData {
    id: String,
    username: String,
    email: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
    role: String,
    storage_used: i64,
    storage_quota: i64,
}

#[derive(Serialize)]
struct UpdateProfileRequest {
    display_name: Option<String>,
}

#[derive(Serialize)]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

#[component]
pub fn Profile() -> Element {
    let auth = use_context::<Signal<AuthState>>();
    let token = auth.read().token.clone().unwrap_or_default();
    let token_for_resource = token.clone();

    let me = use_resource(move || {
        let token = token_for_resource.clone();
        async move { api_get::<ProfileData>("/auth/me", &token).await }
    });

    let mut display_name = use_signal(String::new);
    let mut profile_status = use_signal(String::new);
    let mut current_pw = use_signal(String::new);
    let mut new_pw = use_signal(String::new);
    let mut new_pw_confirm = use_signal(String::new);
    let mut pw_status = use_signal(String::new);
    let mut initialized = use_signal(|| false);

    {
        let me = me.clone();
        use_effect(move || {
            if !*initialized.read() {
                if let Some(Ok(p)) = me.read().as_ref() {
                    display_name.set(p.display_name.clone().unwrap_or_default());
                    initialized.set(true);
                }
            }
        });
    }

    rsx! {
        Navbar {}
        div { class: "profile-container",
            h1 { "Profile" }
            match me.read().as_ref() {
                None => rsx! { div { class: "editor-loading", "Loading..." } },
                Some(Err(e)) => rsx! { div { class: "editor-error", "Failed to load: {e}" } },
                Some(Ok(p)) => {
                    let storage_mb = (p.storage_used as f64) / 1_048_576.0;
                    let quota_gb = (p.storage_quota as f64) / 1_073_741_824.0;
                    let pct = if p.storage_quota > 0 {
                        ((p.storage_used as f64 / p.storage_quota as f64) * 100.0).min(100.0)
                    } else { 0.0 };
                    rsx! {
                        section { class: "profile-section",
                            h2 { "Account" }
                            div { class: "profile-row",
                                label { "Username" }
                                span { "{p.username}" }
                            }
                            div { class: "profile-row",
                                label { "Email" }
                                span { "{p.email}" }
                            }
                            div { class: "profile-row",
                                label { "Role" }
                                span { class: "badge", "{p.role}" }
                            }
                            div { class: "profile-row",
                                label { "Storage" }
                                div { class: "storage-bar-wrap",
                                    div { class: "storage-bar",
                                        div { class: "storage-bar-fill", style: "width: {pct}%;" }
                                    }
                                    span { class: "storage-text",
                                        "{storage_mb:.1} MB of {quota_gb:.2} GB used"
                                    }
                                }
                            }
                        }
                        section { class: "profile-section",
                            h2 { "Display name" }
                            input {
                                class: "form-input",
                                value: "{display_name}",
                                oninput: move |e| display_name.set(e.value()),
                            }
                            button {
                                class: "btn btn-primary",
                                onclick: {
                                    let token = token.clone();
                                    move |_| {
                                        let token = token.clone();
                                        let dn = display_name.read().clone();
                                        let mut status = profile_status;
                                        wasm_bindgen_futures::spawn_local(async move {
                                            status.set("Saving...".into());
                                            let body = UpdateProfileRequest {
                                                display_name: if dn.is_empty() { None } else { Some(dn) },
                                            };
                                            match api_put::<serde_json::Value, _>("/auth/me", &body, &token).await {
                                                Ok(_) => status.set("Saved".into()),
                                                Err(e) => status.set(format!("Save failed: {e}")),
                                            }
                                        });
                                    }
                                },
                                "Save"
                            }
                            if !profile_status.read().is_empty() {
                                span { class: "profile-status", "{profile_status}" }
                            }
                        }
                        section { class: "profile-section",
                            h2 { "Change password" }
                            div { class: "form-group",
                                label { "Current password" }
                                input {
                                    class: "form-input",
                                    r#type: "password",
                                    value: "{current_pw}",
                                    oninput: move |e| current_pw.set(e.value()),
                                }
                            }
                            div { class: "form-group",
                                label { "New password" }
                                input {
                                    class: "form-input",
                                    r#type: "password",
                                    value: "{new_pw}",
                                    oninput: move |e| new_pw.set(e.value()),
                                }
                            }
                            div { class: "form-group",
                                label { "Confirm new password" }
                                input {
                                    class: "form-input",
                                    r#type: "password",
                                    value: "{new_pw_confirm}",
                                    oninput: move |e| new_pw_confirm.set(e.value()),
                                }
                            }
                            button {
                                class: "btn btn-primary",
                                onclick: {
                                    let token = token.clone();
                                    move |_| {
                                        let token = token.clone();
                                        let cp = current_pw.read().clone();
                                        let np = new_pw.read().clone();
                                        let cf = new_pw_confirm.read().clone();
                                        let mut status = pw_status;
                                        if np.len() < 8 {
                                            status.set("New password must be at least 8 characters".into());
                                            return;
                                        }
                                        if np != cf {
                                            status.set("Passwords do not match".into());
                                            return;
                                        }
                                        wasm_bindgen_futures::spawn_local(async move {
                                            status.set("Saving...".into());
                                            let body = ChangePasswordRequest {
                                                current_password: cp,
                                                new_password: np,
                                            };
                                            match api_put::<serde_json::Value, _>("/auth/password", &body, &token).await {
                                                Ok(_) => {
                                                    status.set("Password changed".into());
                                                    current_pw.set(String::new());
                                                    new_pw.set(String::new());
                                                    new_pw_confirm.set(String::new());
                                                }
                                                Err(e) => status.set(format!("Failed: {e}")),
                                            }
                                        });
                                    }
                                },
                                "Change password"
                            }
                            if !pw_status.read().is_empty() {
                                span { class: "profile-status", "{pw_status}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
