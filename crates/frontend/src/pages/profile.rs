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

    let account = result_text(&scope, profile, |profile| {
        format!(
            "{} · {} · {}\n{} MB / {} MB",
            profile.username,
            profile.email,
            profile.role,
            profile.storage_used / 1_048_576,
            profile.storage_quota / 1_048_576,
        )
    });
    let content = col().class("profile-container").fill((
        heading(crate::i18n::t("profile.title")),
        account,
        field(
            crate::i18n::t("profile.display_name"),
            display,
            Purpose::Text,
        ),
        save_profile,
        heading(crate::i18n::t("profile.change_password")),
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
        change_password,
        status(feedback),
        TotpSecurity(scope.clone(), token(auth)),
    ));
    col().fill((Navbar(scope, auth), content))
}
