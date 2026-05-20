use std::sync::Arc;

use diesel::prelude::*;
use salvo::prelude::*;

use contrix_core::schema::ip_restrictions::dsl as ip;

use crate::state::AppState;

/// Middleware that enforces IP-based access restrictions stored in the
/// `ip_restrictions` database table.
///
/// Behaviour:
/// - If there are any "allow" rules, only IPs that match an allow rule are
///   permitted (allowlist mode).
/// - If there are only "deny" rules, IPs that match a deny rule are blocked
///   (denylist mode).
/// - If there are no rules, all traffic is permitted.
///
/// Matching is performed as an exact string comparison on `ip_range`
/// (not CIDR).
pub struct IpFilter;

#[handler]
impl IpFilter {
    async fn handle(
        &self,
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        ctrl: &mut FlowCtrl,
    ) {
        let state = match depot.obtain::<Arc<AppState>>() {
            Ok(s) => s.clone(),
            Err(_) => {
                // If state is unavailable, let the request through rather
                // than blocking all traffic due to a misconfiguration.
                return;
            }
        };

        let mut conn = match state.db.get() {
            Ok(c) => c,
            Err(_) => {
                // Cannot verify IP restrictions without a DB connection.
                // Fail open to avoid a full outage.
                return;
            }
        };

        // Resolve client IP address from headers or remote addr.
        let client_ip = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(',').next().unwrap_or("").trim().to_string())
            .or_else(|| {
                req.headers()
                    .get("x-real-ip")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| format!("{}", req.remote_addr()));

        if client_ip.is_empty() {
            // No IP detected — allow the request through.
            return;
        }

        // Load all restriction rules.
        let rules: Vec<(String, String)> = match ip::ip_restrictions
            .select((ip::ip_range, ip::restriction_type))
            .load::<(String, String)>(&mut conn)
        {
            Ok(rows) => rows,
            Err(_) => {
                // On query failure, fail open.
                return;
            }
        };

        if rules.is_empty() {
            // No rules configured — allow everything.
            return;
        }

        let has_allow_rules = rules.iter().any(|(_, rtype)| rtype == "allow");

        if has_allow_rules {
            // Allowlist mode: the client IP must match at least one "allow" rule.
            let is_allowed = rules
                .iter()
                .any(|(range, rtype)| rtype == "allow" && range == &client_ip);

            if !is_allowed {
                res.status_code(StatusCode::FORBIDDEN);
                res.render(Json(serde_json::json!({
                    "success": false,
                    "error": {
                        "code": 403,
                        "message": "Access denied: your IP address is not allowed"
                    }
                })));
                ctrl.skip_rest();
                return;
            }
        } else {
            // Denylist mode: block the client if it matches a "deny" rule.
            let is_denied = rules
                .iter()
                .any(|(range, rtype)| rtype == "deny" && range == &client_ip);

            if is_denied {
                res.status_code(StatusCode::FORBIDDEN);
                res.render(Json(serde_json::json!({
                    "success": false,
                    "error": {
                        "code": 403,
                        "message": "Access denied: your IP address is blocked"
                    }
                })));
                ctrl.skip_rest();
                return;
            }
        }
    }
}
