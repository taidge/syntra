use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use salvo::prelude::*;

/// In-memory rate limiter middleware for Salvo.
///
/// Tracks request counts per client key (user id or IP address) within a
/// sliding window. Returns 429 Too Many Requests when the limit is exceeded.
pub struct RateLimiter {
    max_requests: u32,
    window_seconds: u64,
    enabled: bool,
    trust_forwarded_headers: bool,
    clients: Mutex<HashMap<String, (u32, Instant)>>,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// * `max_requests` - maximum number of requests allowed per window.
    /// * `window_seconds` - length of the sliding window in seconds.
    /// * `enabled` - when `false`, the middleware is a no-op pass-through.
    /// * `trust_forwarded_headers` - whether proxy-provided client IP headers
    ///   are trusted for unauthenticated requests.
    pub fn new(
        max_requests: u32,
        window_seconds: u64,
        enabled: bool,
        trust_forwarded_headers: bool,
    ) -> Self {
        Self {
            max_requests,
            window_seconds,
            enabled,
            trust_forwarded_headers,
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn client_key(&self, req: &Request, depot: &Depot) -> String {
        depot
            .get::<String>("current_user_id")
            .ok()
            .cloned()
            .or_else(|| {
                if self.trust_forwarded_headers {
                    forwarded_client_ip(req)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| {
                req.remote_addr()
                    .ip()
                    .map(|ip| ip.to_string())
                    .unwrap_or_else(|| req.remote_addr().to_string())
            })
    }
}

fn forwarded_client_ip(req: &Request) -> Option<String> {
    req.headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .or_else(|| {
            req.headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        })
}

#[handler]
impl RateLimiter {
    async fn handle(
        &self,
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        ctrl: &mut FlowCtrl,
    ) {
        if !self.enabled {
            return;
        }

        let key = self.client_key(req, depot);

        let now = Instant::now();
        let window = std::time::Duration::from_secs(self.window_seconds);

        let mut clients = match self.clients.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        clients.retain(|_, (_, started_at)| now.duration_since(*started_at) < window);

        let entry = clients.entry(key).or_insert((0, now));

        // If the window has elapsed, reset the counter.
        if now.duration_since(entry.1) >= window {
            entry.0 = 0;
            entry.1 = now;
        }

        entry.0 += 1;

        if entry.0 > self.max_requests {
            res.status_code(StatusCode::TOO_MANY_REQUESTS);
            res.render(Json(serde_json::json!({
                "success": false,
                "error": {
                    "code": 429,
                    "message": "Rate limit exceeded. Please try again later."
                }
            })));
            ctrl.skip_rest();
        }
    }
}
