//! Small application helpers built from Glory semantics and Sapid controls.
pub use super::icons::{Icon, icon, icon_drawing, icon_sized};
use crate::state::AuthState;
use glory::prelude::*;

/// Explicit application preferences take precedence over the operating system.
pub fn current_theme() -> &'static str {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .map(|window| {
                let explicit = |value: String| match value.as_str() {
                    "dark" => Some("dark"),
                    "light" => Some("light"),
                    _ => None,
                };
                window
                    .document()
                    .and_then(|document| document.document_element())
                    .and_then(|root| root.get_attribute("data-theme"))
                    .and_then(explicit)
                    .or_else(|| {
                        window
                            .local_storage()
                            .ok()
                            .flatten()
                            .and_then(|storage| storage.get_item("theme").ok().flatten())
                            .and_then(explicit)
                    })
                    .unwrap_or_else(|| {
                        if window
                            .match_media("(prefers-color-scheme: dark)")
                            .ok()
                            .flatten()
                            .is_some_and(|query| query.matches())
                        {
                            "dark"
                        } else {
                            "light"
                        }
                    })
            })
            .unwrap_or("light")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        "light"
    }
}

/// Sapid uses mount-time colors. semantic-layout.css maps its live control colors
/// to the app's root theme variables, including already-mounted popup surfaces.
pub fn theme() -> sapid::Theme {
    let scheme = if current_theme() == "dark" {
        sapid::ColorScheme::Dark
    } else {
        sapid::ColorScheme::Light
    };
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
pub fn format_size(bytes: i64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.)
    } else {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.)
    }
}
pub fn storage_percent(profile: &crate::state::UserProfile) -> f64 {
    if profile.storage_quota > 0 {
        (profile.storage_used as f64 / profile.storage_quota as f64 * 100.).clamp(0., 100.)
    } else {
        0.
    }
}
pub fn label(value: impl Into<String>) -> glory::ui::Text {
    text().content(value.into())
}
pub fn field(name: impl Into<String>, value: Cage<String>, purpose: Purpose) -> impl Widget {
    field_placeholder(name, value, purpose, String::new())
}
pub fn field_placeholder(
    name: impl Into<String>,
    value: Cage<String>,
    purpose: Purpose,
    placeholder: impl Into<String>,
) -> impl Widget {
    let name = name.into();
    view().class("form-group").fill((
        label(name.clone()).class("form-label"),
        sapid::text_field()
            .theme(theme())
            .unstyled()
            .class("form-input")
            .name(name)
            .placeholder(placeholder.into())
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
pub fn icon_button(
    name: impl Into<String>,
    kind: Icon,
    callback: impl FnMut() + 'static,
) -> sapid::Button {
    let name = name.into();
    action("", callback)
        .name(name)
        .class("icon-button")
        .fill(icon(kind))
}
pub fn anchor(name: impl Into<String>, target: impl Into<String>) -> glory::ui::Link {
    link().label(name.into()).target(target.into())
}
pub fn status(value: Cage<String>) -> glory::ui::Text {
    text()
        .class("form-status")
        .content(value)
        .style(value.map(|value| {
            Style::new().display(if value.is_empty() {
                Display::None
            } else {
                Display::Block
            })
        }))
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
