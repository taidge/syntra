use base64::Engine;
use dioxus::prelude::*;
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

fn qr_data_uri(value: &str) -> Result<String, String> {
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

#[component]
pub fn TotpSecurity(token: String) -> Element {
    let token_for_status = token.clone();
    let mut refresh = use_signal(|| 0_u32);
    let status = use_resource(move || {
        let token = token_for_status.clone();
        let _refresh = *refresh.read();
        async move { api_get::<TotpStatus>("/totp/status", &token).await }
    });

    let mut current_password = use_signal(String::new);
    let mut setup = use_signal(|| None::<TotpSetupData>);
    let mut verification_code = use_signal(String::new);
    let mut disable_code = use_signal(String::new);
    let mut confirm_disable = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut feedback = use_signal(String::new);
    let mut feedback_is_error = use_signal(|| false);

    let title = crate::i18n::t("profile.two_factor");
    let loading = crate::i18n::t("common.loading");

    rsx! {
        section { class: "profile-section totp-section",
            h2 { "{title}" }
            match status.read().as_ref() {
                None => rsx! { p { class: "totp-description", "{loading}" } },
                Some(Err(error)) => rsx! {
                    p { class: "profile-status status-error",
                        {crate::i18n::t("profile.totp_status_failed").replace("{err}", error)}
                    }
                },
                Some(Ok(totp_status)) if totp_status.enabled => rsx! {
                    div { class: "totp-state-row",
                        span { class: "badge badge-success", {crate::i18n::t("profile.totp_enabled")} }
                        p { class: "totp-description", {crate::i18n::t("profile.totp_enabled_hint")} }
                    }
                    if *confirm_disable.read() {
                        div { class: "totp-action-panel",
                            label { r#for: "totp-disable-code", {crate::i18n::t("profile.totp_disable_code")} }
                            input {
                                id: "totp-disable-code",
                                class: "form-input",
                                inputmode: "text",
                                autocomplete: "one-time-code",
                                maxlength: 32,
                                value: "{disable_code}",
                                oninput: move |event| disable_code.set(normalize_second_factor_code(&event.value())),
                            }
                            div { class: "totp-actions",
                                button {
                                    class: "btn btn-danger",
                                    disabled: *busy.read() || disable_code.read().is_empty(),
                                    onclick: {
                                        let token = token.clone();
                                        move |_| {
                                            let token = token.clone();
                                            let code = disable_code.read().clone();
                                            busy.set(true);
                                            feedback.set(String::new());
                                            wasm_bindgen_futures::spawn_local(async move {
                                                let body = TotpCodeRequest { code };
                                                match api_post::<serde_json::Value, _>("/totp/disable", &body, &token).await {
                                                    Ok(_) => {
                                                        disable_code.set(String::new());
                                                        confirm_disable.set(false);
                                                        feedback_is_error.set(false);
                                                        feedback.set(crate::i18n::t("profile.totp_disabled"));
                                                        refresh += 1;
                                                    }
                                                    Err(error) => {
                                                        feedback_is_error.set(true);
                                                        feedback.set(crate::i18n::t("profile.totp_action_failed").replace("{err}", &error));
                                                    }
                                                }
                                                busy.set(false);
                                            });
                                        }
                                    },
                                    {crate::i18n::t("profile.totp_confirm_disable")}
                                }
                                button {
                                    class: "btn btn-secondary",
                                    disabled: *busy.read(),
                                    onclick: move |_| {
                                        disable_code.set(String::new());
                                        confirm_disable.set(false);
                                        feedback.set(String::new());
                                    },
                                    {crate::i18n::t("common.cancel")}
                                }
                            }
                        }
                    } else {
                        button {
                            class: "btn btn-danger",
                            onclick: move |_| {
                                feedback.set(String::new());
                                confirm_disable.set(true);
                            },
                            {crate::i18n::t("profile.totp_disable")}
                        }
                    }
                },
                Some(Ok(_)) => {
                    if let Some(setup_data) = setup.read().as_ref() {
                        let qr_image = qr_data_uri(&setup_data.qr_url);
                        rsx! {
                            p { class: "totp-description", {crate::i18n::t("profile.totp_scan_hint")} }
                            div { class: "totp-setup-grid",
                                div { class: "totp-qr-wrap",
                                    match qr_image {
                                        Ok(source) => rsx! {
                                            img {
                                                class: "totp-qr",
                                                src: "{source}",
                                                width: "192",
                                                height: "192",
                                                alt: {crate::i18n::t("profile.totp_qr_alt")},
                                            }
                                        },
                                        Err(error) => rsx! { p { class: "status-error", "{error}" } },
                                    }
                                }
                                div { class: "totp-secret-wrap",
                                    strong { {crate::i18n::t("profile.totp_manual_secret")} }
                                    code { class: "totp-secret", "{setup_data.secret}" }
                                }
                            }
                            div { class: "totp-backup-panel",
                                strong { {crate::i18n::t("profile.totp_backup_codes")} }
                                p { class: "totp-description", {crate::i18n::t("profile.totp_backup_warning")} }
                                ul { class: "totp-backup-codes",
                                    for code in &setup_data.backup_codes {
                                        li { code { "{code}" } }
                                    }
                                }
                                button {
                                    class: "btn btn-secondary",
                                    onclick: {
                                        let codes = setup_data.backup_codes.clone();
                                        move |_| {
                                            match download_text_file("contrix-backup-codes.txt", &backup_codes_file(&codes)) {
                                                Ok(_) => {
                                                    feedback_is_error.set(false);
                                                    feedback.set(crate::i18n::t("profile.totp_codes_downloaded"));
                                                }
                                                Err(error) => {
                                                    feedback_is_error.set(true);
                                                    feedback.set(crate::i18n::t("profile.totp_action_failed").replace("{err}", &error));
                                                }
                                            }
                                        }
                                    },
                                    {crate::i18n::t("profile.totp_download_codes")}
                                }
                            }
                            div { class: "totp-action-panel",
                                label { r#for: "totp-verification-code", {crate::i18n::t("profile.totp_verification_code")} }
                                input {
                                    id: "totp-verification-code",
                                    class: "form-input",
                                    inputmode: "numeric",
                                    autocomplete: "one-time-code",
                                    maxlength: 6,
                                    value: "{verification_code}",
                                    oninput: move |event| verification_code.set(normalize_totp_code(&event.value())),
                                }
                                div { class: "totp-actions",
                                    button {
                                        class: "btn btn-primary",
                                        disabled: *busy.read() || verification_code.read().len() != 6,
                                        onclick: {
                                            let token = token.clone();
                                            move |_| {
                                                let token = token.clone();
                                                let code = verification_code.read().clone();
                                                busy.set(true);
                                                feedback.set(String::new());
                                                wasm_bindgen_futures::spawn_local(async move {
                                                    let body = TotpCodeRequest { code };
                                                    match api_post::<serde_json::Value, _>("/totp/verify", &body, &token).await {
                                                        Ok(_) => {
                                                            verification_code.set(String::new());
                                                            setup.set(None);
                                                            current_password.set(String::new());
                                                            feedback_is_error.set(false);
                                                            feedback.set(crate::i18n::t("profile.totp_enabled_success"));
                                                            refresh += 1;
                                                        }
                                                        Err(error) => {
                                                            feedback_is_error.set(true);
                                                            feedback.set(crate::i18n::t("profile.totp_action_failed").replace("{err}", &error));
                                                        }
                                                    }
                                                    busy.set(false);
                                                });
                                            }
                                        },
                                        {crate::i18n::t("profile.totp_verify_enable")}
                                    }
                                    button {
                                        class: "btn btn-secondary",
                                        disabled: *busy.read(),
                                        onclick: move |_| {
                                            verification_code.set(String::new());
                                            setup.set(None);
                                            current_password.set(String::new());
                                            feedback.set(String::new());
                                        },
                                        {crate::i18n::t("common.cancel")}
                                    }
                                }
                            }
                        }
                    } else {
                        rsx! {
                            p { class: "totp-description", {crate::i18n::t("profile.totp_disabled_hint")} }
                            div { class: "totp-action-panel",
                                label { r#for: "totp-current-password", {crate::i18n::t("profile.current_password")} }
                                input {
                                    id: "totp-current-password",
                                    class: "form-input",
                                    r#type: "password",
                                    autocomplete: "current-password",
                                    value: "{current_password}",
                                    oninput: move |event| current_password.set(event.value()),
                                }
                                button {
                                    class: "btn btn-primary",
                                    disabled: *busy.read() || current_password.read().is_empty(),
                                    onclick: {
                                        let token = token.clone();
                                        move |_| {
                                            let token = token.clone();
                                            let password = current_password.read().clone();
                                            busy.set(true);
                                            feedback.set(String::new());
                                            wasm_bindgen_futures::spawn_local(async move {
                                                let body = TotpSetupRequest { current_password: password };
                                                match api_post::<TotpSetupData, _>("/totp/setup", &body, &token).await {
                                                    Ok(data) => {
                                                        setup.set(Some(data));
                                                        current_password.set(String::new());
                                                        feedback_is_error.set(false);
                                                        feedback.set(crate::i18n::t("profile.totp_save_codes"));
                                                    }
                                                    Err(error) => {
                                                        feedback_is_error.set(true);
                                                        feedback.set(crate::i18n::t("profile.totp_action_failed").replace("{err}", &error));
                                                    }
                                                }
                                                busy.set(false);
                                            });
                                        }
                                    },
                                    {crate::i18n::t("profile.totp_begin_setup")}
                                }
                            }
                        }
                    }
                },
            }
            if !feedback.read().is_empty() {
                p {
                    class: if *feedback_is_error.read() { "profile-status status-error" } else { "profile-status status-success" },
                    role: "status",
                    "{feedback}"
                }
            }
        }
    }
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
