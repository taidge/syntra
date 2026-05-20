use dioxus::prelude::*;

use crate::Route;
use crate::state::{self, AuthResponseData, AuthState, RegisterRequest, UserInfo};

/// Registration page with username, email, password, and confirmation.
#[component]
pub fn Register() -> Element {
    let mut auth = use_context::<Signal<AuthState>>();
    let nav = use_navigator();
    let mut username = use_signal(|| String::new());
    let mut email = use_signal(|| String::new());
    let mut password = use_signal(|| String::new());
    let mut confirm_password = use_signal(|| String::new());
    let mut error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);

    let handle_submit = move |evt: Event<FormData>| {
        evt.prevent_default();

        let uname = username.read().clone();
        let em = email.read().clone();
        let pw = password.read().clone();
        let cpw = confirm_password.read().clone();

        // Client-side validation
        if uname.is_empty() || em.is_empty() || pw.is_empty() || cpw.is_empty() {
            error.set(Some("Please fill in all fields.".into()));
            return;
        }
        if pw.len() < 8 {
            error.set(Some("Password must be at least 8 characters.".into()));
            return;
        }
        if pw != cpw {
            error.set(Some("Passwords do not match.".into()));
            return;
        }

        loading.set(true);
        error.set(None);

        let nav = nav.clone();
        spawn(async move {
            let body = RegisterRequest {
                username: uname,
                email: em,
                password: pw,
            };
            match state::api_post::<AuthResponseData, _>("/auth/register", &body, "").await {
                Ok(resp) => {
                    let new_state = AuthState {
                        token: Some(resp.token),
                        user: Some(UserInfo {
                            id: resp.user.id,
                            username: resp.user.username,
                            email: resp.user.email,
                            display_name: resp.user.display_name,
                            role: resp.user.role,
                        }),
                    };
                    state::persist_auth_state(&new_state);
                    auth.set(new_state);
                    nav.push(Route::Dashboard {});
                }
                Err(msg) => {
                    error.set(Some(msg));
                }
            }
            loading.set(false);
        });
    };

    rsx! {
        div { class: "auth-page",
            div { class: "auth-card",
                div { class: "auth-header",
                    h2 { "Create Account" }
                    p { class: "auth-subtitle", "Join Contrix Office" }
                }

                form { onsubmit: handle_submit,
                    div { class: "form-group",
                        label { r#for: "username", "Username" }
                        input {
                            id: "username",
                            r#type: "text",
                            class: "form-input",
                            placeholder: "Choose a username",
                            value: "{username}",
                            oninput: move |e| username.set(e.value()),
                        }
                    }

                    div { class: "form-group",
                        label { r#for: "reg-email", "Email" }
                        input {
                            id: "reg-email",
                            r#type: "email",
                            class: "form-input",
                            placeholder: "you@example.com",
                            value: "{email}",
                            oninput: move |e| email.set(e.value()),
                        }
                    }

                    div { class: "form-group",
                        label { r#for: "reg-password", "Password" }
                        input {
                            id: "reg-password",
                            r#type: "password",
                            class: "form-input",
                            placeholder: "At least 8 characters",
                            value: "{password}",
                            oninput: move |e| password.set(e.value()),
                        }
                    }

                    div { class: "form-group",
                        label { r#for: "confirm-password", "Confirm Password" }
                        input {
                            id: "confirm-password",
                            r#type: "password",
                            class: "form-input",
                            placeholder: "Repeat your password",
                            value: "{confirm_password}",
                            oninput: move |e| confirm_password.set(e.value()),
                        }
                    }

                    if let Some(err) = error() {
                        div { class: "form-error", "{err}" }
                    }

                    button {
                        r#type: "submit",
                        class: "btn btn-primary btn-block",
                        disabled: *loading.read(),
                        if *loading.read() { "Creating account..." } else { "Sign Up" }
                    }
                }

                div { class: "auth-footer",
                    p {
                        "Already have an account? "
                        Link { to: Route::Login {}, "Sign In" }
                    }
                }
            }
        }
    }
}
