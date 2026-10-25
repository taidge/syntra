use crate::{
    components::{navbar::Navbar, ui::*},
    state::{AuthState, FileResponse, UserProfile},
};
use glory::prelude::*;
#[allow(non_snake_case)]
pub fn Dashboard(scope: Scope, auth: Cage<AuthState>) -> impl Widget {
    let feedback = scope.cage(String::new());
    let busy = scope.cage(false);
    let recent = scope.cage(None);
    let profile = scope.cage(None);
    load::<Vec<FileResponse>>(&scope, "/recent".into(), auth, recent);
    load::<UserProfile>(&scope, "/auth/me".into(), auth, profile);
    let docs = scope.bond(move || {
        recent
            .get()
            .and_then(|value| value.ok())
            .unwrap_or_default()
    });
    col().fill((
        Navbar(scope.clone(), auth),
        col().class("dashboard").fill((
            heading(crate::i18n::t("nav.dashboard")),
            row().class("quick-actions").fill(
                ["writer", "calc", "slides", "whiteboard"]
                    .into_iter()
                    .map(|kind| create_document(&scope, auth, kind, None, feedback, busy))
                    .collect::<Vec<_>>(),
            ),
            status(feedback),
            heading(crate::i18n::t("dashboard.recent")),
            result_text(&scope, recent, |docs| {
                if docs.is_empty() {
                    crate::i18n::t("dashboard.recent_empty")
                } else {
                    String::new()
                }
            }),
            col().class("doc-grid").fill(each(
                docs,
                |doc| doc.id,
                |item, _| {
                    let doc = item.get();
                    col().class("doc-card").fill((
                        anchor(
                            doc.name,
                            doc.document_id
                                .map(|id| format!("/documents/{id}"))
                                .unwrap_or_else(|| "/files".into()),
                        ),
                        label(doc.updated_at),
                    ))
                },
            )),
            heading(crate::i18n::t("profile.storage")),
            result_text(&scope, profile, |profile| {
                format!(
                    "{} MB / {} MB",
                    profile.storage_used / 1_048_576,
                    profile.storage_quota / 1_048_576
                )
            }),
            heading(crate::i18n::t("dashboard.activity")),
            col().fill(each(
                docs,
                |doc| doc.id,
                |item, _| {
                    let doc = item.get();
                    label(format!("{} · {}", doc.name, doc.updated_at))
                },
            )),
        )),
    ))
}
