//! Small application helpers built from Glory semantics and Sapid controls.
use crate::state::AuthState;
use glory::prelude::*;

/// Sapid resolves colors when a widget mounts, using the persisted app scheme.
pub fn theme() -> sapid::Theme {
    #[cfg(target_arch = "wasm32")]
    let scheme = if web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item("theme").ok().flatten())
        .as_deref()
        == Some("dark")
    {
        sapid::ColorScheme::Dark
    } else {
        sapid::ColorScheme::Light
    };
    #[cfg(not(target_arch = "wasm32"))]
    let scheme = sapid::ColorScheme::Light;
    sapid::Theme::new(&sapid::BLUE, scheme).expect("BLUE supports light and dark modes")
}

pub fn set<T: 'static>(value: Cage<T>, next: T) {
    let _ = value.try_revise(|current| *current = next);
}
pub fn token(auth: Cage<AuthState>) -> String {
    auth.get().token.unwrap_or_default()
}
pub fn go(path: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href(path);
    }
}
pub fn heading(value: impl Into<String>) -> glory::ui::Text {
    text().kind(TextKind::Heading(1)).content(value.into())
}
pub fn label(value: impl Into<String>) -> glory::ui::Text {
    text().content(value.into())
}
pub fn field(name: impl Into<String>, value: Cage<String>, purpose: Purpose) -> impl Widget {
    let name = name.into();
    col().class("form-group").fill((
        label(name.clone()),
        sapid::text_field()
            .theme(theme())
            .class("form-input")
            .name(name)
            .value(value)
            .purpose(purpose)
            .on_value_change(move |next| set(value, next)),
    ))
}
pub fn action(name: impl Into<String>, callback: impl FnMut() + 'static) -> sapid::Button {
    sapid::button()
        .theme(theme())
        .label(name.into())
        .on_press(callback)
}
pub fn anchor(name: impl Into<String>, target: impl Into<String>) -> glory::ui::Link {
    link().label(name.into()).target(target.into())
}
pub fn status(value: Cage<String>) -> glory::ui::Text {
    text().class("form-status").content(value)
}
pub fn load<T: serde::de::DeserializeOwned + Clone + 'static>(
    scope: &Scope,
    path: String,
    auth: Cage<AuthState>,
    output: Cage<Option<std::result::Result<T, String>>>,
) {
    let token = token(auth);
    let _ = scope.spawn(async move {
        set(output, Some(crate::state::api_get(&path, &token).await));
    });
}
pub fn result_text<T: Clone + 'static>(
    scope: &Scope,
    value: Cage<Option<std::result::Result<T, String>>>,
    format: impl Fn(T) -> String + 'static,
) -> glory::ui::Text {
    text().content(scope.bond(move || match value.get() {
        None => crate::i18n::t("common.loading"),
        Some(Err(error)) => error,
        Some(Ok(value)) => format(value),
    }))
}
pub fn create_document(
    scope: &Scope,
    auth: Cage<AuthState>,
    kind: &'static str,
    parent: Option<uuid::Uuid>,
    feedback: Cage<String>,
    busy: Cage<bool>,
) -> sapid::Button {
    let weak = scope.downgrade();
    sapid::button()
        .theme(theme())
        .label(crate::i18n::t(&format!("dashboard.new_{kind}")))
        .loading(busy)
        .on_press(move || {
            if busy.get() {
                return;
            }
            let Some(scope) = weak.upgrade() else {
                return;
            };
            let token = token(auth);
            let body = serde_json::json!({
                "name": format!("Untitled {kind}"),
                "document_type": kind,
                "parent_id": parent,
                "room_id": null,
            });
            set(busy, true);
            set(feedback, crate::i18n::t("common.loading"));
            let _ = scope.spawn(async move {
                match crate::state::api_post::<serde_json::Value, _>("/documents", &body, &token)
                    .await
                {
                    Ok(response) => {
                        let id = response
                            .get("id")
                            .and_then(|id| id.as_str())
                            .filter(|id| uuid::Uuid::parse_str(id).is_ok());
                        match id {
                            Some(id) => go(&format!("/{kind}/{id}")),
                            None => set(
                                feedback,
                                "The server returned a document without a valid identifier".into(),
                            ),
                        }
                    }
                    Err(error) => set(feedback, error),
                }
                set(busy, false);
            });
        })
}
