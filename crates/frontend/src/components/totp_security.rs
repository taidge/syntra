use base64::Engine;
use glory::prelude::*;
use qrcode::{QrCode, render::svg};
use serde::{Deserialize, Serialize};

use crate::state::{api_get, api_post, download_text_file};

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct TotpStatus {
    enabled: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct TotpSetupData {
    secret: String,
    qr_url: String,
    backup_codes: Vec<String>,
}

#[derive(Serialize)]
struct TotpSetupRequest {
    current_password: String,
}

#[derive(Serialize)]
struct TotpCodeRequest {
    code: String,
}

fn qr_data_uri(value: &str) -> std::result::Result<String, String> {
    let svg = QrCode::new(value.as_bytes())
        .map_err(|error| format!("Could not generate QR code: {error}"))?
        .render::<svg::Color>()
        .min_dimensions(192, 192)
        .dark_color(svg::Color("#111827"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    ))
}

fn backup_codes_file(codes: &[String]) -> String {
    let mut contents = String::from("Contrix Office two-factor authentication backup codes\n\n");
    contents.push_str(&codes.join("\n"));
    contents.push_str("\n\nEach code can be used only once. Store these codes securely.\n");
    contents
}

fn normalize_totp_code(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_digit())
        .take(6)
        .collect()
}

fn normalize_second_factor_code(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .take(32)
        .collect()
}

#[allow(non_snake_case)]
pub fn TotpSecurity(scope: Scope, auth_token: String) -> impl Widget {
    use crate::components::ui::*;
    let enabled = scope.cage(false);
    let setup = scope.cage(None::<TotpSetupData>);
    let password = scope.cage(String::new());
    let code = scope.cage(String::new());
    let feedback = scope.cage(String::new());
    let busy = scope.cage(false);
    let initial_token = auth_token.clone();
    let _ = scope.spawn(async move {
        match api_get::<TotpStatus>("/totp/status", &initial_token).await {
            Ok(value) => set(enabled, value.enabled),
            Err(error) => set(feedback, error),
        }
    });
    let setup_weak = scope.downgrade();
    let setup_token = auth_token.clone();
    let verify_weak = scope.downgrade();
    let verify_token = auth_token.clone();
    let disable_weak = scope.downgrade();
    let qr = scope.bond(move || {
        setup
            .get()
            .and_then(|data| qr_data_uri(&data.qr_url).ok())
            .unwrap_or_default()
    });
    col().class("totp-security").fill((
        heading(crate::i18n::t("profile.two_factor")),
        text().content(scope.bond(move || {
            if enabled.get() {
                crate::i18n::t("profile.totp_enabled")
            } else {
                crate::i18n::t("profile.totp_disabled")
            }
        })),
        field(
            crate::i18n::t("profile.current_password"),
            password,
            Purpose::Password,
        ),
        sapid::button()
            .theme(theme())
            .label(crate::i18n::t("profile.totp_begin_setup"))
            .enabled(scope.bond(move || !enabled.get() && setup.get().is_none()))
            .loading(busy)
            .on_press(move || {
                let Some(scope) = setup_weak.upgrade() else {
                    return;
                };
                let auth_token = setup_token.clone();
                let body = TotpSetupRequest {
                    current_password: password.get(),
                };
                set(busy, true);
                let _ = scope.spawn(async move {
                    match api_post::<TotpSetupData, _>("/totp/setup", &body, &auth_token).await {
                        Ok(data) => {
                            set(setup, Some(data));
                            set(password, String::new());
                            set(feedback, crate::i18n::t("profile.totp_save_codes"));
                        }
                        Err(error) => set(feedback, error),
                    }
                    set(busy, false);
                });
            }),
        when(scope.bond(move || setup.get().is_some()), move || {
            col().fill((
                image().source(qr).alt("Authenticator QR code"),
                text().content_fn(move || {
                    setup
                        .get()
                        .map(|data| {
                            format!("Secret: {}\n{}", data.secret, data.backup_codes.join("\n"))
                        })
                        .unwrap_or_default()
                }),
                action(crate::i18n::t("profile.totp_backup_codes"), move || {
                    if let Some(data) = setup.get() {
                        let _ = download_text_file(
                            "contrix-backup-codes.txt",
                            &backup_codes_file(&data.backup_codes),
                        );
                    }
                }),
            ))
        }),
        sapid::text_field()
            .theme(theme())
            .name(crate::i18n::t("auth.totp_code"))
            .value(code)
            .on_value_change(move |value| set(code, normalize_second_factor_code(&value))),
        sapid::button()
            .theme(theme())
            .label(crate::i18n::t("profile.totp_verify_enable"))
            .enabled(
                scope.bond(move || {
                    setup.get().is_some() && normalize_totp_code(&code.get()).len() == 6
                }),
            )
            .loading(busy)
            .on_press(move || {
                if normalize_totp_code(&code.get()).len() != 6 {
                    set(feedback, "Enter a six-digit code".into());
                    return;
                }
                let Some(scope) = verify_weak.upgrade() else {
                    return;
                };
                let auth_token = verify_token.clone();
                let body = TotpCodeRequest {
                    code: normalize_totp_code(&code.get()),
                };
                set(busy, true);
                let _ = scope.spawn(async move {
                    match api_post::<serde_json::Value, _>("/totp/verify", &body, &auth_token).await
                    {
                        Ok(_) => {
                            set(enabled, true);
                            set(setup, None);
                            set(code, String::new());
                            set(feedback, crate::i18n::t("profile.totp_enabled_success"));
                        }
                        Err(error) => set(feedback, error),
                    }
                    set(busy, false);
                });
            }),
        sapid::button()
            .theme(theme())
            .label(crate::i18n::t("profile.totp_confirm_disable"))
            .enabled(scope.bond(move || enabled.get() && !code.get().is_empty()))
            .loading(busy)
            .on_press(move || {
                let Some(scope) = disable_weak.upgrade() else {
                    return;
                };
                let auth_token = auth_token.clone();
                let body = TotpCodeRequest { code: code.get() };
                set(busy, true);
                let _ = scope.spawn(async move {
                    match api_post::<serde_json::Value, _>("/totp/disable", &body, &auth_token)
                        .await
                    {
                        Ok(_) => {
                            set(enabled, false);
                            set(code, String::new());
                            set(feedback, crate::i18n::t("profile.totp_disabled"));
                        }
                        Err(error) => set(feedback, error),
                    }
                    set(busy, false);
                });
            }),
        status(feedback),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_is_generated_locally_as_an_svg_data_uri() {
        let uri = qr_data_uri("otpauth://totp/Contrix:test@example.com?secret=ABC123").unwrap();
        assert!(uri.starts_with("data:image/svg+xml;base64,"));
        assert!(!uri.contains("ABC123"));
    }

    #[test]
    fn backup_code_download_has_warning_and_each_code() {
        let file = backup_codes_file(&["abcd1234".into(), "efgh5678".into()]);
        assert!(file.contains("abcd1234\nefgh5678"));
        assert!(file.contains("only once"));
    }

    #[test]
    fn codes_are_limited_to_server_accepted_characters_and_length() {
        assert_eq!(normalize_totp_code("12 34-5a67"), "123456");
        assert_eq!(normalize_second_factor_code("ab-cd-GH-1234"), "abcd1234");
        assert_eq!(normalize_second_factor_code(&"A".repeat(40)).len(), 32);
    }
}
