use crate::{
    components::ui::*,
    state::{self, AuthResponseData, AuthState, RegisterRequest, UserInfo},
};
use glory::prelude::*;

#[derive(Clone, Copy)]
struct Registration {
    auth: Cage<AuthState>,
    username: Cage<String>,
    email: Cage<String>,
    password: Cage<String>,
    confirm: Cage<String>,
    error: Cage<String>,
    loading: Cage<bool>,
}

fn submit(weak: &glory::ui::WeakScope, form: Registration) {
    let Registration {
        auth,
        username,
        email,
        password,
        confirm,
        error,
        loading,
    } = form;
    if loading.get() {
        return;
    }
    let validation = if username.get().trim().is_empty()
        || email.get().trim().is_empty()
        || password.get().is_empty()
    {
        Some("auth.fill_all_fields")
    } else if password.get().len() < 8 {
        Some("profile.password_too_short")
    } else if password.get() != confirm.get() {
        Some("profile.password_mismatch")
    } else {
        None
    };
    if let Some(key) = validation {
        set(error, crate::i18n::t(key));
        return;
    }
    let Some(scope) = weak.upgrade() else {
        return;
    };
    set(loading, true);
    set(error, String::new());
    let body = RegisterRequest {
        username: username.get().trim().into(),
        email: email.get().trim().into(),
        password: password.get(),
    };
    let _ = scope.spawn(async move {
        match state::api_post::<AuthResponseData, _>("/auth/register", &body, "").await {
            Ok(response) => {
                let next = AuthState {
                    token: Some(response.token),
                    user: Some(UserInfo {
                        id: response.user.id,
                        username: response.user.username,
                        email: response.user.email,
                        display_name: response.user.display_name,
                        role: response.user.role,
                    }),
                };
                state::persist_auth_state(&next);
                set(auth, next);
                go("/dashboard");
            }
            Err(message) => set(error, message),
        }
        set(loading, false);
    });
}
#[allow(non_snake_case)]
pub fn Register(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    let username = scope.cage(String::new());
    let email = scope.cage(String::new());
    let password = scope.cage(String::new());
    let confirm = scope.cage(String::new());
    let error = scope.cage(String::new());
    let loading = scope.cage(false);
    let weak = scope.downgrade();
    let form = Registration {
        auth,
        username,
        email,
        password,
        confirm,
        error,
        loading,
    };
    view().class("auth-page").fill(
        view().class("auth-card").fill((
            view().class("auth-header").fill((
                heading(crate::i18n::t("auth.register_title")),
                label(crate::i18n::t("auth.register_subtitle")).class("auth-subtitle"),
            )),
            sapid::form("register")
                .theme(theme())
                .unstyled()
                .enabled(scope.bond(move || !loading.get()))
                .on_submit(move |_| submit(&weak, form))
                .fill((
                    field(crate::i18n::t("auth.username"), username, Purpose::Text),
                    field(crate::i18n::t("auth.email"), email, Purpose::Email),
                    field(crate::i18n::t("auth.password"), password, Purpose::Password),
                    field(
                        crate::i18n::t("auth.confirm_password"),
                        confirm,
                        Purpose::Password,
                    ),
                    status(error).class("form-error"),
                    sapid::submit_button()
                        .theme(theme())
                        .unstyled()
                        .class("btn btn-primary btn-block")
                        .label(crate::i18n::t("auth.register_link")),
                )),
            view().class("auth-footer").fill((
                label(crate::i18n::t("auth.have_account")),
                anchor(crate::i18n::t("auth.sign_in"), "/login"),
            )),
        )),
    )
}
