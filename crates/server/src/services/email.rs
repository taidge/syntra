use horme::prelude::*;
use std::result::Result;

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};

use syntra_core::models::SmtpSettings;
use syntra_core::schema::smtp_settings::dsl as smtp;

pub struct EmailService;

impl EmailService {
    /// Send an email using SMTP settings from the database, returning an error
    /// describing any failure (no SMTP configured, bad address, transport/send
    /// error). Use this when the caller needs to know the outcome (e.g. an
    /// admin "test SMTP" action).
    pub async fn send(
        pool: &crate::state::DbPool,
        to_email: &str,
        subject: &str,
        body_html: &str,
    ) -> Result<(), String> {
        let cfg = {
            let mut conn = pool
                .get()
                .map_err(|e| format!("failed to get db connection: {e}"))?;
            smtp::smtp_settings
                .first::<SmtpSettings>(&mut conn)
                .await
                .optional()
                .map_err(|e| format!("failed to query smtp_settings: {e}"))?
                .ok_or_else(|| "no SMTP settings configured".to_string())?
        };

        let from_addr = format!("{} <{}>", cfg.from_name, cfg.from_email);
        let from = from_addr
            .parse()
            .or_else(|_| cfg.from_email.parse())
            .map_err(|_| format!("invalid from address: {}", cfg.from_email))?;
        let to = to_email
            .parse()
            .map_err(|_| format!("invalid recipient address: {to_email}"))?;

        let email = Message::builder()
            .from(from)
            .to(to)
            .subject(subject)
            .header(ContentType::TEXT_HTML)
            .body(body_html.to_string())
            .map_err(|e| format!("failed to build email: {e}"))?;

        // SMTP transport setup, DNS/TLS and send are synchronous in lettre.
        // Only owned settings/message enter the blocking worker, never a DB lease.
        tokio::task::spawn_blocking(move || {
            let creds = match (cfg.username.as_deref(), cfg.password.as_deref()) {
                (Some(user), Some(pass)) if !user.is_empty() => {
                    Some(Credentials::new(user.to_string(), pass.to_string()))
                }
                _ => None,
            };

            let transport = if cfg.use_tls {
                let mut b = SmtpTransport::relay(&cfg.host)
                    .map_err(|e| format!("failed to create SMTP transport to {}: {e}", cfg.host))?
                    .port(cfg.port as u16);
                if let Some(creds) = creds {
                    b = b.credentials(creds);
                }
                b.build()
            } else {
                let mut b = SmtpTransport::builder_dangerous(&cfg.host).port(cfg.port as u16);
                if let Some(creds) = creds {
                    b = b.credentials(creds);
                }
                b.build()
            };

            transport
                .send(&email)
                .map(|_| ())
                .map_err(|e| format!("failed to send email via {}:{} — {e}", cfg.host, cfg.port))
        })
        .await
        .map_err(|error| format!("SMTP worker failed: {error}"))?
    }

    /// Best-effort notification email: when SMTP is unconfigured or sending
    /// fails, the error is logged but never propagated to callers.
    pub async fn send_notification(
        pool: &crate::state::DbPool,
        to_email: &str,
        subject: &str,
        body_html: &str,
    ) {
        match Self::send(pool, to_email, subject, body_html).await {
            Ok(()) => {
                tracing::info!("EmailService: email sent to={to_email} subject=\"{subject}\"")
            }
            Err(e) => tracing::warn!(
                "EmailService: skipped/failed email to={to_email} subject=\"{subject}\": {e}"
            ),
        }
    }
}
