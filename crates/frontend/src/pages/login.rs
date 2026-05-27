use dioxus::prelude::*;

use crate::Route;
use crate::state::{self, AuthResponseData, AuthState, LoginRequest, UserInfo};

/// Login page with email/password form.
#[component]
pub fn Login() -> Element {
    let mut auth = use_context::<Signal<AuthState>>();
    let nav = use_navigator();
    let mut email = use_signal(|| String::new());
    let mut password = use_signal(|| String::new());
    let mut error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| false);

    let handle_submit = move |evt: Event<FormData>| {
        evt.prevent_default();

        let email_val = email.read().clone();
        let password_val = password.read().clone();

        // Basic client-side validation
        if email_val.is_empty() || password_val.is_empty() {
            error.set(Some("Please fill in all fields.".into()));
            return;
        }

        loading.set(true);
        error.set(None);

        let nav = nav.clone();
        spawn(async move {
            let body = LoginRequest {
                email: email_val,
                password: password_val,
            };
            match state::api_post::<AuthResponseData, _>("/auth/login", &body, "").await {
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

    let _locale = use_context::<Signal<String>>();
    let title = crate::i18n::t("auth.login_title");
    let email_label = crate::i18n::t("auth.email");
    let password_label = crate::i18n::t("auth.password");
    let sign_in_label = crate::i18n::t("auth.sign_in");
    let no_account_label = crate::i18n::t("auth.no_account");
    let register_link_label = crate::i18n::t("auth.register_link");

    rsx! {
        div { class: "auth-page",
            div { class: "auth-card",
                div { class: "auth-header",
                    h2 { "{title}" }
                    p { class: "auth-subtitle", "Welcome back to Contrix Office" }
                }

                form { onsubmit: handle_submit,
                    div { class: "form-group",
                        label { r#for: "email", "{email_label}" }
                        input {
                            id: "email",
                            r#type: "email",
                            class: "form-input",
                            placeholder: "you@example.com",
                            value: "{email}",
                            oninput: move |e| email.set(e.value()),
                        }
                    }

                    div { class: "form-group",
                        label { r#for: "password", "{password_label}" }
                        input {
                            id: "password",
                            r#type: "password",
                            class: "form-input",
                            placeholder: "Enter your password",
                            value: "{password}",
                            oninput: move |e| password.set(e.value()),
                        }
                    }

                    if let Some(err) = error() {
                        div { class: "form-error", "{err}" }
                    }

                    button {
                        r#type: "submit",
                        class: "btn btn-primary btn-block",
                        disabled: *loading.read(),
                        if *loading.read() { "Signing in..." } else { "{sign_in_label}" }
                    }
                }

                div { class: "auth-footer",
                    p {
                        "{no_account_label} "
                        Link { to: Route::Register {}, "{register_link_label}" }
                    }
                }
            }
        }
    }
}
