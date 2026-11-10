use salvo::prelude::*;

pub struct SecurityHeaders;

/// Exact inline script bodies from the shipped shell, after HTML newline
/// normalization. Keep these stable for service-worker-cached copies of it.
const SHELL_SCRIPT_HASHES: [&str; 2] = [
    "GgCxvz/iHAN/FagJv/QLYLE1qit5uadocOPLhnVRX7Y=",
    "Sf1Ebf5y5Deqg5+Zj02cFWKMRu4UO3NKHeat9xtMpdE=",
];

pub fn content_security_policy(nonce: Option<&str>) -> String {
    let inline_sources = if let Some(nonce) = nonce {
        format!("'nonce-{nonce}'")
    } else {
        SHELL_SCRIPT_HASHES
            .iter()
            .map(|hash| format!("'sha256-{hash}'"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval' {inline_sources}; script-src-attr 'none'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self' ws: wss:; font-src 'self' data:; frame-ancestors 'self'"
    )
}

#[handler]
impl SecurityHeaders {
    async fn handle(&self, res: &mut Response) {
        let headers = res.headers_mut();
        headers.insert("x-content-type-options", "nosniff".parse().unwrap());
        headers.insert("x-frame-options", "SAMEORIGIN".parse().unwrap());
        headers.insert("x-xss-protection", "1; mode=block".parse().unwrap());
        headers.insert(
            "referrer-policy",
            "strict-origin-when-cross-origin".parse().unwrap(),
        );
        // A specialized HTML handler can install a response-specific nonce
        // policy. Preserve it regardless of this hoop's invocation order.
        if !headers.contains_key("content-security-policy") {
            headers.insert(
                "content-security-policy",
                content_security_policy(None).parse().unwrap(),
            );
        }
        headers.insert(
            "strict-transport-security",
            "max-age=31536000; includeSubDomains".parse().unwrap(),
        );
        headers.insert(
            "permissions-policy",
            "camera=(), microphone=(), geolocation=()".parse().unwrap(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use sha2::{Digest, Sha256};

    #[test]
    fn the_shipped_shell_scripts_match_the_explicit_hash_allowlist() {
        let html = include_str!("../../../../static/index.html")
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        let mut rest = html.as_str();
        let mut hashes = Vec::new();
        while let Some(start) = rest.find("<script") {
            rest = &rest[start + "<script".len()..];
            let tag_end = rest.find('>').unwrap();
            let attributes = &rest[..tag_end];
            let body = &rest[tag_end + 1..];
            let end = body.find("</script>").unwrap();
            if !attributes.contains("src=") {
                hashes.push(
                    base64::engine::general_purpose::STANDARD
                        .encode(Sha256::digest(&body.as_bytes()[..end])),
                );
            }
            rest = &body[end + "</script>".len()..];
        }
        assert_eq!(hashes, SHELL_SCRIPT_HASHES);
        let policy = content_security_policy(None);
        let script = policy
            .split(';')
            .find(|directive| directive.trim_start().starts_with("script-src "))
            .unwrap();
        assert!(!script.contains("'unsafe-inline'"));
        assert!(!script.contains("'unsafe-eval'"));
        assert!(script.contains("'wasm-unsafe-eval'"));
        for hash in hashes {
            assert!(script.contains(&format!("'sha256-{hash}'")));
        }
    }

    #[test]
    fn nonce_policy_only_allows_its_response_script_and_retains_wasm_support() {
        let policy = content_security_policy(Some("a-random-response-nonce"));
        assert!(policy.contains("'nonce-a-random-response-nonce'"));
        assert!(!policy.contains("'sha256-"));
        assert!(!policy.contains("'unsafe-eval'"));
        assert!(policy.contains("'wasm-unsafe-eval'"));
        assert!(policy.contains("script-src-attr 'none'"));
    }
}
