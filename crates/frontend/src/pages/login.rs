use crate::{
    components::ui::*,
    state::{self, AuthResponseData, AuthState, LoginRequest, UserInfo},
};
use glory::prelude::*;

fn submit(
    weak: &glory::ui::WeakScope,
    auth: Cage<AuthState>,
    email: Cage<String>,
    password: Cage<String>,
    code: Cage<String>,
    error: Cage<String>,
    loading: Cage<bool>,
) {
    if loading.get() {
        return;
    }
    if email.get().trim().is_empty() || password.get().is_empty() {
        set(error, crate::i18n::t("auth.fill_all_fields"));
        return;
    }
    let Some(scope) = weak.upgrade() else {
        return;
    };
    set(loading, true);
    set(error, String::new());
    let body = LoginRequest {
        email: email.get().trim().into(),
        password: password.get(),
        totp_code: (!code.get().trim().is_empty()).then(|| code.get().trim().to_string()),
    };
    let _ = scope.spawn(async move {
        match state::api_post::<AuthResponseData, _>("/auth/login", &body, "").await {
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
pub fn Login(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    let email = scope.cage(String::new());
    let password = scope.cage(String::new());
    let code = scope.cage(String::new());
    let error = scope.cage(String::new());
    let loading = scope.cage(false);
    let weak = scope.downgrade();
    view().class("auth-page").fill(
        view().class("auth-card").fill((
            view().class("auth-header").fill((
                heading(crate::i18n::t("auth.login_title")),
                label(crate::i18n::t("auth.login_subtitle")).class("auth-subtitle"),
            )),
            sapid::form("login")
                .theme(theme())
                .unstyled()
                .enabled(scope.bond(move || !loading.get()))
                .on_submit(move |_| submit(&weak, auth, email, password, code, error, loading))
                .fill((
                    field_placeholder(
                        crate::i18n::t("auth.email"),
                        email,
                        Purpose::Email,
                        crate::i18n::t("auth.email_placeholder"),
                    ),
                    field_placeholder(
                        crate::i18n::t("auth.password"),
                        password,
                        Purpose::Password,
                        crate::i18n::t("auth.password_placeholder"),
                    ),
                    field_placeholder(
                        crate::i18n::t("auth.totp_code"),
                        code,
                        Purpose::Text,
                        crate::i18n::t("auth.totp_placeholder"),
                    ),
                    label(crate::i18n::t("auth.totp_hint")).class("form-hint"),
                    status(error).class("form-error"),
                    sapid::submit_button()
                        .theme(theme())
                        .unstyled()
                        .class("btn btn-primary btn-block")
                        .label(scope.bond(move || {
                            crate::i18n::t(if loading.get() {
                                "auth.signing_in"
                            } else {
                                "auth.sign_in"
                            })
                        })),
                )),
            view().class("auth-footer").fill((
                label(crate::i18n::t("auth.no_account")),
                anchor(crate::i18n::t("auth.register_link"), "/register"),
            )),
        )),
    )
}
