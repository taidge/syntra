use std::net::{IpAddr, SocketAddr};

use chrono::Utc;
use diesel::SqliteConnection;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

use contrix_core::models::{NewWebhookDelivery, Webhook};
use contrix_core::schema::webhook_deliveries::dsl as wd;
use contrix_core::schema::webhooks::dsl as w;

/// Backoff delays (in seconds) for each retry attempt after the initial try.
const RETRY_DELAYS: &[u64] = &[5, 30, 180];

/// Maximum number of delivery attempts (1 initial + 3 retries = 4 total).
const MAX_ATTEMPTS: usize = 4;

/// Bound untrusted webhook responses before buffering them in memory and
/// persisting them in SQLite.
const MAX_RESPONSE_BODY_BYTES: usize = 64 * 1024;

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
            let octets = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || octets[0] == 0
                // Shared address space 100.64.0.0/10.
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                // IETF protocol assignments and deprecated 6to4 relay anycast.
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
                // Benchmarking 198.18.0.0/15.
                || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
                // Reserved 240.0.0.0/4.
                || octets[0] >= 240
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Only globally routed unicast space 2000::/3 is eligible.
                || (seg[0] & 0xe000) != 0x2000
                // IETF protocol assignments 2001::/23.
                || (seg[0] == 0x2001 && seg[1] < 0x0200)
                // Documentation 2001:db8::/32 and 3fff::/20.
                || (seg[0] == 0x2001 && seg[1] == 0x0db8)
                || (seg[0] == 0x3fff && (seg[1] & 0xf000) == 0)
                // Deprecated 6to4 2002::/16.
                || seg[0] == 0x2002
                // Unique-local fc00::/7
                || (v6.octets()[0] & 0xfe) == 0xfc
                // Link-local fe80::/10
                || (seg[0] & 0xffc0) == 0xfe80
                // IPv4-mapped — re-check the embedded v4 address
                || v6.to_ipv4_mapped().map(IpAddr::V4).is_some_and(is_blocked_ip)
        }
    }
}

/// Resolve and pin a webhook hostname to a validated public address. Every
/// resolved A/AAAA record must be public; accepting a mixed public/private set
/// would leave room for resolver ordering attacks. The returned client has
/// environment proxies disabled so the pinned destination cannot be bypassed.
async fn client_for_webhook(raw: &str) -> Result<(reqwest::Client, reqwest::Url), String> {
    if !is_safe_webhook_url(raw) {
        return Err("unsafe URL scheme or host".into());
    }

    let url = reqwest::Url::parse(raw).map_err(|e| format!("invalid URL: {e}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| "webhook URL has no host".to_string())?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "webhook URL has no usable port".to_string())?;

    let (pin_host, addresses) = match host.parse::<IpAddr>() {
        Ok(ip) => (None, vec![SocketAddr::new(ip, port)]),
        Err(_) => {
            let resolved: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
                .await
                .map_err(|e| format!("DNS resolution failed: {e}"))?
                .collect();
            (Some(host.to_string()), resolved)
        }
    };

    if addresses.is_empty() {
        return Err("DNS resolution returned no addresses".into());
    }
    if addresses.iter().any(|address| is_blocked_ip(address.ip())) {
        return Err("DNS resolution returned an internal or reserved address".into());
    }

    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none());
    if let Some(host) = pin_host {
        builder = builder.resolve(&host, addresses[0]);
    }
    let client = builder
        .build()
        .map_err(|e| format!("HTTP client setup failed: {e}"))?;
    Ok((client, url))
}

async fn read_bounded_response_body(response: &mut reqwest::Response) -> String {
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(0)
            .min(MAX_RESPONSE_BODY_BYTES as u64) as usize,
    );
    let mut truncated = response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BODY_BYTES as u64);

    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = MAX_RESPONSE_BODY_BYTES.saturating_sub(body.len());
                if chunk.len() > remaining {
                    body.extend_from_slice(&chunk[..remaining]);
                    truncated = true;
                    break;
                }
                body.extend_from_slice(&chunk);
                if body.len() == MAX_RESPONSE_BODY_BYTES {
                    // Stop without requesting another untrusted chunk. An
                    // exact-size body may receive the marker, which is safer
                    // than buffering beyond the hard limit to distinguish it.
                    truncated = true;
                    break;
                }
            }
            Ok(None) => break,
            Err(e) => return format!("Failed to read response body: {e}"),
        }
    }

    let mut text = String::from_utf8_lossy(&body).into_owned();
    if truncated {
        text.push_str("\n[response truncated at 65536 bytes]");
    }
    text
}

/// Validate that a webhook target URL is safe to call: an http(s) URL whose
/// host is not localhost or an internal/private IP. This is a best-effort SSRF
/// guard for the static URL shape. IP-literal hosts are checked directly and
/// hostnames are checked against obvious internal suffixes; `client_for_webhook`
/// performs the authoritative DNS validation and connection pinning.
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

/// Compute an RFC 2104 HMAC-SHA256 signature for the payload.
fn compute_signature(secret: &str, payload: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts keys of every length");
    mac.update(payload.as_bytes());
    let result = mac.finalize().into_bytes();
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

    for attempt in 0..MAX_ATTEMPTS {
        // Resolve again for every retry and pin the connection to the checked
        // address. This closes the DNS-rebinding gap between validation and
        // connection establishment.
        let (client, target_url) = match client_for_webhook(&hook.url).await {
            Ok(target) => target,
            Err(e) => {
                let message = format!("Refused webhook target: {e}");
                update_delivery(&pool, &delivery_id, None, Some(&message), false);
                tracing::warn!("WebhookService: {message}");
                return;
            }
        };

        tracing::info!(
            "WebhookService: POST attempt {}/{} event={} to url={} (webhook={})",
            attempt + 1,
            MAX_ATTEMPTS,
            event,
            hook.url,
            hook.id,
        );

        let mut request = client
            .post(target_url)
            .header("Content-Type", "application/json")
            .header("X-Webhook-Event", &event);

        // Attach only the HMAC signature derived from the secret. The raw
        // secret is never transmitted — receivers verify via the signature.
        if let Some(ref secret) = hook.secret
            && !secret.is_empty()
        {
            let signature = compute_signature(secret, &payload);
            request = request.header("X-Webhook-Signature", signature);
        }

        let result = request.body(payload.clone()).send().await;

        match result {
            Ok(mut response) => {
                let status = response.status().as_u16() as i32;
                let body = read_bounded_response_body(&mut response).await;
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

#[cfg(test)]
mod tests {
    use super::{MAX_RESPONSE_BODY_BYTES, compute_signature, is_blocked_ip, is_safe_webhook_url};
    use std::net::IpAddr;

    #[test]
    fn webhook_signature_matches_hmac_sha256_test_vector() {
        assert_eq!(
            compute_signature("key", "The quick brown fox jumps over the lazy dog"),
            "sha256=f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn webhook_url_filter_rejects_literal_internal_targets() {
        assert!(!is_safe_webhook_url("http://127.0.0.1/hook"));
        assert!(!is_safe_webhook_url("http://[::1]/hook"));
        assert!(!is_safe_webhook_url(
            "http://169.254.169.254/latest/meta-data"
        ));
        assert!(!is_safe_webhook_url("file:///etc/passwd"));
        assert!(is_safe_webhook_url("https://hooks.example.com/events"));
    }

    #[test]
    fn webhook_ip_filter_rejects_non_public_ranges() {
        for value in [
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "192.0.2.1",
            "192.88.99.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "::1",
            "100::1",
            "2001:db8::1",
            "2002::1",
            "3fff::1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_blocked_ip(value.parse::<IpAddr>().unwrap()), "{value}");
        }
        assert!(!is_blocked_ip("8.8.8.8".parse().unwrap()));
        assert_eq!(MAX_RESPONSE_BODY_BYTES, 65_536);
    }
}
