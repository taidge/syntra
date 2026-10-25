use crate::{
    components::ui::*,
    state::{AuthState, UserProfile},
};
use glory::prelude::*;
#[allow(non_snake_case)]
pub fn Sidebar(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    let profile = scope.cage(None);
    load::<UserProfile>(&scope, "/auth/me".into(), auth, profile);
    col().class("sidebar").fill((
        anchor(crate::i18n::t("nav.dashboard"), "/dashboard"),
        anchor(crate::i18n::t("file_mgr.my_files"), "/files"),
        anchor(crate::i18n::t("profile.title"), "/profile"),
        result_text(&scope, profile, |profile| {
            format!(
                "{} / {} MB",
                profile.storage_used / 1_048_576,
                profile.storage_quota / 1_048_576
            )
        }),
    ))
}
