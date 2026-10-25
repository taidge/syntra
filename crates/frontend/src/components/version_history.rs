use crate::{
    components::ui::*,
    state::{self, AuthState},
};
use glory::prelude::*;
use serde::Deserialize;
#[derive(Deserialize, Clone, PartialEq)]
pub struct VersionItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub version_number: i32,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub label: Option<String>,
}
#[allow(non_snake_case)]
pub fn VersionHistory(
    scope: Scope,
    auth: Cage<AuthState>,
    doc_id: String,
    open: Cage<bool>,
) -> impl Widget {
    let items = scope.cage(Vec::<VersionItem>::new());
    let feedback = scope.cage(String::new());
    let token_value = token(auth);
    let fetch_id = doc_id.clone();
    let _ = scope.spawn(async move {
        match state::api_get::<Vec<VersionItem>>(
            &format!("/documents/{fetch_id}/versions"),
            &token_value,
        )
        .await
        {
            Ok(value) => set(items, value),
            Err(error) => set(feedback, error),
        }
    });
    let weak = scope.downgrade();
    when(open, move || {
        let weak = weak.clone();
        let id = doc_id.clone();
        col().class("version-history").fill((
            heading(crate::i18n::t("versions.title")),
            action("Close", move || set(open, false)),
            status(feedback),
            col().fill(each(
                items,
                |item| item.id.clone(),
                move |item, _| {
                    let weak = weak.clone();
                    let id = id.clone();
                    let version = item.get();
                    let version_id = version.id;
                    row().fill((
                        label(format!(
                            "v{} · {} · {}",
                            version.version_number,
                            version.created_at,
                            version.label.unwrap_or_default()
                        )),
                        action(crate::i18n::t("versions.restore"), move || {
                            let Some(scope) = weak.upgrade() else {
                                return;
                            };
                            let token = token(auth);
                            let path = format!("/documents/{id}/restore/{version_id}");
                            let _ = scope.spawn(async move {
                                match state::api_post::<serde_json::Value, _>(
                                    &path,
                                    &serde_json::json!({}),
                                    &token,
                                )
                                .await
                                {
                                    Ok(_) => {
                                        if let Some(window) = web_sys::window() {
                                            let _ = window.location().reload();
                                        }
                                    }
                                    Err(error) => set(feedback, error),
                                }
                            });
                        }),
                    ))
                },
            )),
        ))
    })
}
