use crate::{
    components::ui::*,
    state::{AuthState, UserProfile},
};
use glory::prelude::*;
#[allow(non_snake_case)]
pub fn Sidebar(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    let profile = scope.cage(None);
    let feedback = scope.cage(String::new());
    let busy = scope.cage(false);
    load::<UserProfile>(&scope, "/auth/me".into(), auth, profile);
    let actions = [
        ("writer", "sidebar.document"),
        ("calc", "sidebar.spreadsheet"),
        ("slides", "sidebar.presentation"),
        ("whiteboard", "sidebar.whiteboard"),
    ]
    .into_iter()
    .map(|(kind, key)| {
        create_document(&scope, auth, kind, None, feedback, busy)
            .unstyled()
            .class("sidebar-action")
            .name(crate::i18n::t(key))
            .label("")
            .fill((label("+").class("action-icon"), label(crate::i18n::t(key))))
    })
    .collect::<Vec<_>>();
    view().class("sidebar").fill((
        view().class("sidebar-section").fill((
            text()
                .kind(TextKind::Heading(4))
                .class("sidebar-heading")
                .content(crate::i18n::t("sidebar.create_new")),
            view().class("sidebar-actions").fill(actions),
            status(feedback),
        )),
        view().class("sidebar-storage").fill((
            view().class("storage-label").fill((
                label(crate::i18n::t("sidebar.storage")),
                result_text(&scope, profile, |profile| {
                    format!(
                        "{} / {}",
                        format_size(profile.storage_used),
                        format_size(profile.storage_quota)
                    )
                }),
            )),
            view()
                .class("storage-bar")
                .fill(view().class("storage-fill").style(profile.map(|value| {
                    Style::new().width(Length::Percent(
                        value
                            .as_ref()
                            .and_then(|result| result.as_ref().ok())
                            .map(storage_percent)
                            .unwrap_or(0.),
                    ))
                }))),
        )),
    ))
}
