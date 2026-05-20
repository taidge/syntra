use salvo::prelude::*;

pub struct SecurityHeaders;

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
        headers.insert(
            "content-security-policy",
            "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self' ws: wss:; font-src 'self' data:; frame-ancestors 'self'"
                .parse()
                .unwrap(),
        );
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
