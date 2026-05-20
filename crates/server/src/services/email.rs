use diesel::SqliteConnection;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool};

use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};

use contrix_core::models::SmtpSettings;
use contrix_core::schema::smtp_settings::dsl as smtp;

pub struct EmailService;

impl EmailService {
    /// Send a notification email using SMTP settings from the database.
    ///
    /// This is a best-effort service: when SMTP is not configured or sending
    /// fails, the error is logged but never propagated to callers.
    pub fn send_notification(
        pool: &Pool<ConnectionManager<SqliteConnection>>,
        to_email: &str,
        subject: &str,
        body_html: &str,
    ) {
        let mut conn = match pool.get() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("EmailService: failed to get db connection: {e}");
                return;
            }
        };

        // Try to load SMTP configuration from the database.
        let settings: Option<SmtpSettings> = match smtp::smtp_settings
            .first::<SmtpSettings>(&mut conn)
            .optional()
        {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("EmailService: failed to query smtp_settings: {e}");
                None
            }
        };

        let cfg = match settings {
            Some(cfg) => cfg,
            None => {
                tracing::info!(
                    "EmailService: no SMTP configured — skipping email to={to_email} subject=\"{subject}\""
                );
                return;
            }
        };

        // Build the email message.
        let from_addr = format!("{} <{}>", cfg.from_name, cfg.from_email);
        let email = match Message::builder()
            .from(from_addr.parse().unwrap_or_else(|_| {
                cfg.from_email.parse().expect("Invalid from_email in SMTP settings")
            }))
            .to(to_email.parse().unwrap_or_else(|_| {
                tracing::error!("EmailService: invalid recipient address: {to_email}");
                // Use a dummy address that will cause transport to fail gracefully.
                "invalid@invalid".parse().unwrap()
            }))
            .subject(subject)
            .header(ContentType::TEXT_HTML)
            .body(body_html.to_string())
        {
            Ok(msg) => msg,
            Err(e) => {
                tracing::error!("EmailService: failed to build email: {e}");
                return;
            }
        };

        // Build the SMTP transport.
        let creds = match (cfg.username.as_deref(), cfg.password.as_deref()) {
            (Some(user), Some(pass)) if !user.is_empty() => {
                Some(Credentials::new(user.to_string(), pass.to_string()))
            }
            _ => None,
        };

        let transport_result = if cfg.use_tls {
            let builder = SmtpTransport::relay(&cfg.host);
            match builder {
                Ok(b) => {
                    let b = b.port(cfg.port as u16);
                    Ok(if let Some(creds) = creds {
                        b.credentials(creds).build()
                    } else {
                        b.build()
                    })
                }
                Err(e) => Err(e),
            }
        } else {
            let builder = SmtpTransport::builder_dangerous(&cfg.host).port(cfg.port as u16);
            Ok(if let Some(creds) = creds {
                builder.credentials(creds).build()
            } else {
                builder.build()
            })
        };

        let transport = match transport_result {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(
                    "EmailService: failed to create SMTP transport to {}:{} — {e}",
                    cfg.host,
                    cfg.port,
                );
                return;
            }
        };

        // Send the email.
        match transport.send(&email) {
            Ok(_) => {
                tracing::info!(
                    "EmailService: email sent to={to_email} subject=\"{subject}\" via {}:{}",
                    cfg.host,
                    cfg.port,
                );
            }
            Err(e) => {
                tracing::error!(
                    "EmailService: failed to send email to={to_email} subject=\"{subject}\" via {}:{} — {e}",
                    cfg.host,
                    cfg.port,
                );
            }
        }
    }
}
