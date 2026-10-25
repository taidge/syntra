use crate::{
    components::{navbar::Navbar, ui::*},
    state::{AuthState, FileResponse, UserProfile},
};
use glory::prelude::*;
fn document_kind(document: &FileResponse) -> &'static str {
    match document.mime_type.as_deref() {
        Some(value) if value.contains("spreadsheet") || value.contains("calc") => "calc",
        Some(value) if value.contains("presentation") || value.contains("slides") => "slides",
        Some(value) if value.contains("whiteboard") => "whiteboard",
        _ => "writer",
    }
}
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
    let quick = ["writer", "calc", "slides", "whiteboard"]
        .into_iter()
        .map(|kind| {
            let icon = match kind {
                "writer" => "W",
                "calc" => "C",
                "slides" => "S",
                _ => "B",
            };
            create_document(&scope, auth, kind, None, feedback, busy)
                .unstyled()
                .class(format!("quick-action-btn {kind}"))
                .name(crate::i18n::t(&format!("dashboard.new_{kind}")))
                .label("")
                .fill((
                    label(icon).class("qa-icon"),
                    label(crate::i18n::t(&format!("dashboard.new_{kind}"))),
                ))
        })
        .collect::<Vec<_>>();
    let header = view().class("dashboard-header").fill((
        heading(crate::i18n::t("nav.dashboard")),
        view().class("quick-actions").fill(quick).fill(
            link()
                .target("/files")
                .class("quick-action-btn")
                .name(crate::i18n::t("dashboard.from_template"))
                .fill((
                    label("T").class("qa-icon"),
                    label(crate::i18n::t("dashboard.from_template")),
                )),
        ),
    ));
    let cards = each(
        docs,
        |doc| doc.id,
        |item, _| {
            widget(move |scope| {
                let doc = item.get();
                let kind = document_kind(&doc);
                let icon = match kind {
                    "calc" => "C",
                    "slides" => "S",
                    "whiteboard" => "B",
                    _ => "W",
                };
                link()
                    .class("doc-card")
                    .target(
                        doc.document_id
                            .map(|id| format!("/documents/{id}"))
                            .unwrap_or_else(|| "/files".into()),
                    )
                    .name(item.map(|doc| doc.name.clone()))
                    .fill((
                        view()
                            .class(format!("doc-card-preview {kind}-preview"))
                            .fill(label(icon)),
                        view().class("doc-card-info").fill((
                            text()
                                .kind(TextKind::Heading(4))
                                .content(item.map(|doc| doc.name.clone())),
                            text()
                                .kind(TextKind::Paragraph)
                                .class("doc-meta")
                                .content(item.map(|doc| {
                                    crate::i18n::t("dashboard.modified")
                                        .replace("{when}", &doc.updated_at)
                                })),
                        )),
                    ))
                    .build(scope)
            })
        },
    );
    let recents = view().class("recent-documents").fill((
        text()
            .kind(TextKind::Heading(2))
            .content(crate::i18n::t("dashboard.recent")),
        result_text(&scope, recent, |docs| {
            if docs.is_empty() {
                crate::i18n::t("dashboard.recent_empty")
            } else {
                String::new()
            }
        })
        .class("doc-meta"),
        view().class("doc-grid").fill(cards),
    ));
    let storage = view().class("storage-info").fill((
        text()
            .kind(TextKind::Heading(3))
            .content(crate::i18n::t("profile.storage")),
        view()
            .class("storage-bar-large")
            .fill(view().class("storage-fill").style(profile.map(|value| {
                Style::new().width(Length::Percent(
                    value
                        .as_ref()
                        .and_then(|result| result.as_ref().ok())
                        .map(storage_percent)
                        .unwrap_or(0.),
                ))
            }))),
        result_text(&scope, profile, |profile| {
            crate::i18n::t("dashboard.storage_usage")
                .replace("{used}", &format_size(profile.storage_used))
                .replace("{quota}", &format_size(profile.storage_quota))
        })
        .class("storage-text"),
    ));
    let activity = view().class("activity-feed").fill((
        text()
            .kind(TextKind::Heading(3))
            .content(crate::i18n::t("dashboard.activity")),
        when(scope.bond(move || docs.get().is_empty()), || {
            label(crate::i18n::t("dashboard.activity_empty")).class("doc-meta")
        }),
        view().class("activity-list").fill(each(
            scope.bond(move || docs.get().into_iter().take(5).collect::<Vec<_>>()),
            |doc| doc.id,
            |item, _| {
                view().class("activity-item").fill((
                    view().class("activity-dot"),
                    text().content(item.map(|doc| {
                        crate::i18n::t("dashboard.activity_edited").replace("{name}", &doc.name)
                    })),
                    text().class("activity-time").content(item.map(|doc| {
                        crate::i18n::t("dashboard.modified").replace("{when}", &doc.updated_at)
                    })),
                ))
            },
        )),
    ));
    view().class("app-page dashboard-page").fill((
        Navbar(scope, auth),
        view()
            .class("dashboard")
            .fill((header, status(feedback), recents, storage, activity)),
    ))
}
