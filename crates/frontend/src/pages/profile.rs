use crate::{
    components::{navbar::Navbar, totp_security::TotpSecurity, ui::*},
    state::{self, AuthState, UserProfile},
};
use glory::prelude::*;

#[allow(non_snake_case)]
pub fn Profile(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    let profile = scope.cage(None);
    let display = scope.cage(String::new());
    let current = scope.cage(String::new());
    let password = scope.cage(String::new());
    let confirm = scope.cage(String::new());
    let feedback = scope.cage(String::new());
    let initial_token = token(auth);
    let _ = scope.spawn(async move {
        let response = state::api_get::<UserProfile>("/auth/me", &initial_token).await;
        if let Ok(profile) = &response {
            set(display, profile.display_name.clone().unwrap_or_default());
        }
        set(profile, Some(response));
    });

    let profile_weak = scope.downgrade();
    let save_profile = action(crate::i18n::t("common.save"), move || {
        let Some(scope) = profile_weak.upgrade() else {
            return;
        };
        let token = token(auth);
        let name = display.get();
        let body = serde_json::json!({
            "display_name": if name.is_empty() { None } else { Some(name) },
        });
        let _ = scope.spawn(async move {
            match state::api_put::<UserProfile, _>("/auth/me", &body, &token).await {
                Ok(saved) => {
                    let _ = auth.try_revise(|auth| {
                        if let Some(user) = auth.user.as_mut() {
                            user.display_name = saved.display_name.clone();
                        }
                    });
                    set(profile, Some(Ok(saved)));
                    state::persist_auth_state(&auth.get());
                    set(feedback, crate::i18n::t("profile.saved"));
                }
                Err(error) => set(feedback, error),
            }
        });
    });

    let password_weak = scope.downgrade();
    let change_password = action(crate::i18n::t("profile.change_password"), move || {
        if password.get().len() < 8 {
            set(feedback, crate::i18n::t("profile.password_too_short"));
            return;
        }
        if password.get() != confirm.get() {
            set(feedback, crate::i18n::t("profile.password_mismatch"));
            return;
        }
        let Some(scope) = password_weak.upgrade() else {
            return;
        };
        let token = token(auth);
        let body = serde_json::json!({
            "current_password": current.get(),
            "new_password": password.get(),
        });
        let _ = scope.spawn(async move {
            match state::api_put::<serde_json::Value, _>("/auth/password", &body, &token).await {
                Ok(_) => {
                    set(current, String::new());
                    set(password, String::new());
                    set(confirm, String::new());
                    state::clear_auth_state();
                    set(auth, AuthState::default());
                    go("/login");
                }
                Err(error) => set(feedback, error),
            }
        });
    });

    let account_value = move |key: &'static str| {
        profile.map(move |result| {
            result
                .as_ref()
                .and_then(|value| value.as_ref().ok())
                .map(|profile| match key {
                    "username" => profile.username.clone(),
                    "email" => profile.email.clone(),
                    "role" => profile.role.clone(),
                    _ => String::new(),
                })
                .unwrap_or_else(|| crate::i18n::t("common.loading"))
        })
    };
    let account = view().class("profile-section").fill((
        text()
            .kind(TextKind::Heading(2))
            .content(crate::i18n::t("profile.account")),
        view().class("profile-row").fill((
            label(crate::i18n::t("auth.username")).class("profile-label"),
            text().content(account_value("username")),
        )),
        view().class("profile-row").fill((
            label(crate::i18n::t("auth.email")).class("profile-label"),
            text().content(account_value("email")),
        )),
        view().class("profile-row").fill((
            label(crate::i18n::t("profile.role")).class("profile-label"),
            text().class("badge").content(account_value("role")),
        )),
        view().class("profile-row").fill((
            label(crate::i18n::t("profile.storage")).class("profile-label"),
            view().class("storage-bar-wrap").fill((
                view()
                    .class("storage-bar")
                    .fill(view().class("storage-bar-fill").style(profile.map(|value| {
                        Style::new().width(Length::Percent(
                            value
                                .as_ref()
                                .and_then(|value| value.as_ref().ok())
                                .map(storage_percent)
                                .unwrap_or(0.),
                        ))
                    }))),
                result_text(&scope, profile, |profile| {
                    crate::i18n::t("profile.storage_usage")
                        .replace(
                            "{used}",
                            &format!("{:.1}", profile.storage_used as f64 / 1_048_576.),
                        )
                        .replace(
                            "{quota}",
                            &format!("{:.2}", profile.storage_quota as f64 / 1_073_741_824.),
                        )
                })
                .class("storage-text"),
            )),
        )),
    ));
    let content = view().class("profile-container").fill((
        heading(crate::i18n::t("profile.title")),
        account,
        view().class("profile-section").fill((
            text()
                .kind(TextKind::Heading(2))
                .content(crate::i18n::t("profile.display_name")),
            sapid::text_field()
                .theme(theme())
                .unstyled()
                .class("form-input")
                .name(crate::i18n::t("profile.display_name"))
                .value(display)
                .on_value_change(move |value| set(display, value)),
            save_profile.unstyled().class("btn btn-primary"),
        )),
        view().class("profile-section").fill((
            text()
                .kind(TextKind::Heading(2))
                .content(crate::i18n::t("profile.change_password")),
            field(
                crate::i18n::t("profile.current_password"),
                current,
                Purpose::Password,
            ),
            field(
                crate::i18n::t("profile.new_password"),
                password,
                Purpose::Password,
            ),
            field(
                crate::i18n::t("profile.confirm_new_password"),
                confirm,
                Purpose::Password,
            ),
            change_password.unstyled().class("btn btn-primary"),
            status(feedback),
        )),
        TotpSecurity(scope.clone(), token(auth)),
    ));
    view()
        .class("app-page profile-page")
        .fill((Navbar(scope, auth), content))
}
