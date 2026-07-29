use std::net::IpAddr;
use std::sync::Arc;

use diesel::prelude::*;
use salvo::prelude::*;

use contrix_core::schema::ip_restrictions::dsl as ip;

use crate::state::AppState;

/// Whether `client` matches an `ip_range` rule. The rule may be a single IP
/// (exact match) or CIDR notation (`10.0.0.0/8`, `2001:db8::/32`).
fn ip_matches(range: &str, client: IpAddr) -> bool {
    let range = range.trim();
    if let Some((base, prefix)) = range.split_once('/') {
        let prefix: u32 = match prefix.trim().parse() {
            Ok(p) => p,
            Err(_) => return false,
        };
        match (base.trim().parse::<IpAddr>(), client) {
            (Ok(IpAddr::V4(b)), IpAddr::V4(c)) => {
                if prefix > 32 {
                    return false;
                }
                let mask: u32 = if prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - prefix)
                };
                (u32::from(b) & mask) == (u32::from(c) & mask)
            }
            (Ok(IpAddr::V6(b)), IpAddr::V6(c)) => {
                if prefix > 128 {
                    return false;
                }
                let mask: u128 = if prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - prefix)
                };
                (u128::from(b) & mask) == (u128::from(c) & mask)
            }
            _ => false,
        }
    } else {
        match range.parse::<IpAddr>() {
            Ok(r) => r == client,
            Err(_) => false,
        }
    }
}

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
/// Each `ip_range` rule may be a single IP address (exact match) or CIDR
/// notation such as `10.0.0.0/8` or `2001:db8::/32`.
pub struct IpFilter;

#[cfg(test)]
mod ip_filter_tests {
    use super::*;

    #[test]
    fn cidr_and_exact_matching() {
        let c: IpAddr = "10.1.2.3".parse().unwrap();
        assert!(ip_matches("10.0.0.0/8", c));
        assert!(!ip_matches("10.2.0.0/16", c));
        assert!(ip_matches("10.1.2.3", c));
        assert!(!ip_matches("10.1.2.4", c));
        assert!(ip_matches("0.0.0.0/0", c));
        // Malformed rules never match.
        assert!(!ip_matches("not-an-ip", c));
        assert!(!ip_matches("10.0.0.0/40", c));
    }

    #[test]
    fn cidr_v6_matching() {
        let c: IpAddr = "2001:db8::1".parse().unwrap();
        assert!(ip_matches("2001:db8::/32", c));
        assert!(!ip_matches("2001:dead::/32", c));
    }
}

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

        let client_ip = match req.remote_addr().ip() {
            Some(addr) => addr,
            None => {
                // No IP detected — allow the request through.
                return;
            }
        };

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
                .any(|(range, rtype)| rtype == "allow" && ip_matches(range, client_ip));

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
            }
        } else {
            // Denylist mode: block the client if it matches a "deny" rule.
            let is_denied = rules
                .iter()
                .any(|(range, rtype)| rtype == "deny" && ip_matches(range, client_ip));

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
            }
        }
    }
}
