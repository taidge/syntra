use std::net::IpAddr;

use chrono::Utc;
use diesel::SqliteConnection;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use contrix_core::models::{NewWebhookDelivery, Webhook};
use contrix_core::schema::webhook_deliveries::dsl as wd;
use contrix_core::schema::webhooks::dsl as w;

/// Backoff delays (in seconds) for each retry attempt after the initial try.
const RETRY_DELAYS: &[u64] = &[5, 30, 180];

/// Maximum number of delivery attempts (1 initial + 3 retries = 4 total).
const MAX_ATTEMPTS: usize = 4;

pub struct WebhookService;

impl WebhookService {
    /// Fire webhooks for a given event. This queries all active webhooks that subscribe
    /// to the event, then spawns async tasks to deliver payloads with retry.
    ///
    /// Delivery is best-effort: errors are logged but never propagated to the caller.
    pub fn fire_event(
        pool: &Pool<ConnectionManager<SqliteConnection>>,
        event: &str,
        payload: &serde_json::Value,
    ) {
        let mut conn = match pool.get() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("WebhookService: failed to get db connection: {e}");
                return;
            }
        };

        // Load all active webhooks whose `events` JSON array contains this event.
        let hooks: Vec<Webhook> = match w::webhooks
            .filter(w::is_active.eq(true))
            .load::<Webhook>(&mut conn)
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("WebhookService: failed to load webhooks: {e}");
                return;
            }
        };

        let payload_str = serde_json::to_string(payload).unwrap_or_default();
        let now = Utc::now().to_rfc3339();

        for hook in hooks {
            // The `events` column stores a JSON array of event names.
            // Check whether this webhook subscribes to the fired event.
            let subscribed = match serde_json::from_str::<Vec<String>>(&hook.events) {
                Ok(evts) => evts.iter().any(|e| e == event || e == "*"),
                // If it is a comma-separated string rather than JSON, fall back to contains.
                Err(_) => hook.events.contains(event),
            };

            if !subscribed {
                continue;
            }

            let delivery_id = Uuid::new_v4().to_string();

            // Record the initial delivery attempt as pending.
            let delivery = NewWebhookDelivery {
                id: delivery_id.clone(),
                webhook_id: hook.id.clone(),
                event: event.to_string(),
                payload: payload_str.clone(),
                response_status: None,
                response_body: Some("Delivery in progress".into()),
                delivered_at: now.clone(),
                success: false,
            };

            if let Err(e) = diesel::insert_into(wd::webhook_deliveries)
                .values(&delivery)
                .execute(&mut conn)
            {
                tracing::error!(
                    "WebhookService: failed to record delivery for webhook {}: {e}",
                    hook.id
                );
                continue;
            }

            // Spawn an async task to perform the actual HTTP delivery with retry.
            let pool_clone = pool.clone();
            let event_str = event.to_string();
            let payload_body = payload_str.clone();

            tokio::spawn(async move {
                deliver_with_retry(pool_clone, delivery_id, hook, event_str, payload_body).await;
            });
        }
    }
}

/// Whether an IP address points at a private/internal range that webhook
/// delivery must never reach (SSRF protection).
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                // Unique-local fc00::/7
                || (v6.octets()[0] & 0xfe) == 0xfc
                // Link-local fe80::/10
                || (seg[0] & 0xffc0) == 0xfe80
                // IPv4-mapped — re-check the embedded v4 address
                || v6.to_ipv4_mapped().map(IpAddr::V4).is_some_and(is_blocked_ip)
        }
    }
}

/// Validate that a webhook target URL is safe to call: an http(s) URL whose
/// host is not localhost or an internal/private IP. This is a best-effort SSRF
/// guard; IP-literal hosts are checked directly, hostnames are checked against
/// obvious internal suffixes (it does not resolve DNS, so DNS-rebinding is out
/// of scope).
fn is_safe_webhook_url(raw: &str) -> bool {
    let url = match reqwest::Url::parse(raw) {
        Ok(u) => u,
        Err(_) => return false,
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let host = match url.host_str() {
        Some(h) => h,
        None => return false,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return !is_blocked_ip(ip);
    }
    let lower = host.to_ascii_lowercase();
    !(lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower.ends_with(".internal"))
}

/// Compute a signature for the payload using SHA-256(secret + payload).
fn compute_signature(secret: &str, payload: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(payload.as_bytes());
    let result = hasher.finalize();
    let hex: String = result.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

/// Attempt to deliver the webhook payload with exponential backoff retry.
async fn deliver_with_retry(
    pool: Pool<ConnectionManager<SqliteConnection>>,
    delivery_id: String,
    hook: Webhook,
    event: String,
    payload: String,
) {
    // Reject internal/private targets before issuing any request (SSRF guard),
    // and never follow redirects that could bounce into internal hosts.
    if !is_safe_webhook_url(&hook.url) {
        let msg = format!("Refused delivery to unsafe webhook URL: {}", hook.url);
        tracing::warn!("WebhookService: {msg}");
        update_delivery(&pool, &delivery_id, None, Some(&msg), false);
        return;
    }

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    for attempt in 0..MAX_ATTEMPTS {
        tracing::info!(
            "WebhookService: POST attempt {}/{} event={} to url={} (webhook={})",
            attempt + 1,
            MAX_ATTEMPTS,
            event,
            hook.url,
            hook.id,
        );

        let mut request = client
            .post(&hook.url)
            .header("Content-Type", "application/json")
            .header("X-Webhook-Event", &event);

        // Attach only the HMAC signature derived from the secret. The raw
        // secret is never transmitted — receivers verify via the signature.
        if let Some(ref secret) = hook.secret {
            if !secret.is_empty() {
                let signature = compute_signature(secret, &payload);
                request = request.header("X-Webhook-Signature", signature);
            }
        }

        let result = request.body(payload.clone()).send().await;

        match result {
            Ok(response) => {
                let status = response.status().as_u16() as i32;
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|e| format!("Failed to read response body: {e}"));
                let success = (200..300).contains(&status);

                update_delivery(&pool, &delivery_id, Some(status), Some(&body), success);

                if success {
                    tracing::info!(
                        "WebhookService: delivery {} succeeded (status={status})",
                        delivery_id,
                    );
                    return;
                }

                tracing::warn!(
                    "WebhookService: delivery {} attempt {}/{} failed (status={status})",
                    delivery_id,
                    attempt + 1,
                    MAX_ATTEMPTS,
                );
            }
            Err(e) => {
                let error_msg = format!("Request error: {e}");
                update_delivery(&pool, &delivery_id, None, Some(&error_msg), false);

                tracing::warn!(
                    "WebhookService: delivery {} attempt {}/{} error: {e}",
                    delivery_id,
                    attempt + 1,
                    MAX_ATTEMPTS,
                );
            }
        }

        // Wait before retrying (skip sleep after the last attempt).
        if attempt < MAX_ATTEMPTS - 1 {
            let delay_secs = RETRY_DELAYS.get(attempt).copied().unwrap_or(180);
            tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
        }
    }

    tracing::error!(
        "WebhookService: delivery {} exhausted all {} attempts",
        delivery_id,
        MAX_ATTEMPTS,
    );
}

/// Update the webhook_delivery record with the latest attempt results.
fn update_delivery(
    pool: &Pool<ConnectionManager<SqliteConnection>>,
    delivery_id: &str,
    response_status: Option<i32>,
    response_body: Option<&str>,
    success: bool,
) {
    match pool.get() {
        Ok(mut conn) => {
            let now = Utc::now().to_rfc3339();
            let _ = diesel::update(wd::webhook_deliveries.filter(wd::id.eq(delivery_id)))
                .set((
                    wd::response_status.eq(response_status),
                    wd::response_body.eq(response_body),
                    wd::success.eq(success),
                    wd::delivered_at.eq(&now),
                ))
                .execute(&mut conn);
        }
        Err(e) => {
            tracing::error!("WebhookService: failed to update delivery {delivery_id}: {e}");
        }
    }
}
