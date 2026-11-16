use std::collections::BTreeMap;

use chrono::Utc;
use contrix_core::error::AppError;
use reqwest::{Method, StatusCode, Url};
use sha2::{Digest, Sha256};

/// S3-compatible storage backend.
///
/// Uses `reqwest` to make HTTP requests to an S3-compatible API (simple
/// PUT / GET / DELETE).  This is a minimal implementation that avoids pulling
/// in a full AWS SDK dependency.
#[derive(Clone)]
pub struct S3Storage {
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    endpoint: String,
    #[cfg(test)]
    bypass_proxy_for_tests: bool,
    pub(crate) registry: std::sync::OnceLock<crate::state::DbPool>,
}

impl S3Storage {
    /// Create a new `S3Storage` instance.
    ///
    /// When `endpoint` is empty the default AWS S3 endpoint for the given
    /// region is used.
    pub fn new(
        bucket: &str,
        region: &str,
        access_key: &str,
        secret_key: &str,
        endpoint: &str,
    ) -> Self {
        Self {
            bucket: bucket.to_string(),
            region: region.to_string(),
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            endpoint: endpoint.to_string(),
            #[cfg(test)]
            bypass_proxy_for_tests: false,
            registry: std::sync::OnceLock::new(),
        }
    }

    /// Build an `S3Storage` from an [`contrix_core::config::StorageConfig`].
    ///
    /// Returns `None` if the required S3 fields are not configured.
    pub fn from_config(cfg: &contrix_core::config::StorageConfig) -> Option<Self> {
        let bucket = cfg.s3_bucket.as_deref()?;
        let region = cfg.s3_region.as_deref().unwrap_or("us-east-1");
        let access_key = cfg.s3_access_key.as_deref()?;
        let secret_key = cfg.s3_secret_key.as_deref()?;
        if bucket.trim().is_empty() || access_key.trim().is_empty() || secret_key.trim().is_empty()
        {
            return None;
        }
        let endpoint = cfg.s3_endpoint.as_deref().unwrap_or("");
        Some(Self::new(bucket, region, access_key, secret_key, endpoint))
    }

    // -- accessors -----------------------------------------------------------

    /// The configured bucket name.
    #[allow(dead_code)] // accessor retained for diagnostics / tests
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// The configured region.
    #[allow(dead_code)] // accessor retained for diagnostics / tests
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Whether this storage points at a custom (non-AWS) endpoint.
    #[allow(dead_code)] // accessor retained for diagnostics / tests
    pub fn is_custom_endpoint(&self) -> bool {
        !self.endpoint.is_empty()
    }

    // -- internal helpers ----------------------------------------------------

    /// Keep a loopback unit-test fixture independent of the host's proxy setup.
    #[cfg(test)]
    pub(crate) fn without_proxy_for_tests(mut self) -> Self {
        self.bypass_proxy_for_tests = true;
        self
    }

    /// Generate the full URL for an object key.
    fn object_url(&self, key: &str) -> String {
        let key = encode_path(key);
        if self.endpoint.is_empty() {
            format!(
                "https://{}.s3.{}.amazonaws.com/{}",
                self.bucket, self.region, key
            )
        } else {
            format!("{}/{}/{}", self.endpoint, self.bucket, key)
        }
    }

    /// Generate the base URL for listing objects (the bucket root).
    fn bucket_url(&self) -> String {
        if self.endpoint.is_empty() {
            format!("https://{}.s3.{}.amazonaws.com", self.bucket, self.region)
        } else {
            format!("{}/{}", self.endpoint, self.bucket)
        }
    }

    /// Build a shared `reqwest::Client`.
    fn client(&self) -> reqwest::Client {
        #[cfg(test)]
        if self.bypass_proxy_for_tests {
            return reqwest::Client::builder()
                .no_proxy()
                .build()
                .expect("build direct client for loopback unit-test fixture");
        }
        reqwest::Client::new()
    }

    async fn send_signed(
        &self,
        method: Method,
        url: String,
        body: Vec<u8>,
        content_type: Option<&str>,
        extra_headers: BTreeMap<String, String>,
    ) -> Result<reqwest::Response, AppError> {
        let url_parts = Url::parse(&url).map_err(|e| AppError::Storage(e.to_string()))?;
        let amz_date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let date = Utc::now().format("%Y%m%d").to_string();
        let payload_hash = sha256_hex(&body);
        let host = host_header(&url_parts)?;

        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), host);
        headers.insert("x-amz-content-sha256".to_string(), payload_hash.clone());
        headers.insert("x-amz-date".to_string(), amz_date.clone());
        if let Some(content_type) = content_type {
            headers.insert("content-type".to_string(), content_type.to_string());
        }
        for (name, value) in extra_headers {
            headers.insert(name.to_ascii_lowercase(), value);
        }

        let canonical_headers = headers
            .iter()
            .map(|(name, value)| format!("{name}:{}\n", value.trim()))
            .collect::<String>();
        let signed_headers = headers.keys().cloned().collect::<Vec<_>>().join(";");
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method.as_str(),
            canonical_uri(&url_parts),
            canonical_query(&url_parts),
            canonical_headers,
            signed_headers,
            payload_hash
        );
        let credential_scope = format!("{date}/{}/s3/aws4_request", self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{}",
            sha256_hex(canonical_request.as_bytes())
        );
        let signing_key = signing_key(&self.secret_key, &date, &self.region);
        let signature = hex_bytes(&hmac_sha256(&signing_key, string_to_sign.as_bytes()));
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
            self.access_key
        );

        let mut request = self
            .client()
            .request(method, url)
            .header("Authorization", authorization);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if !body.is_empty() {
            request = request.body(body);
        }

        request
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 request failed: {e}")))
    }

    // -- public API ----------------------------------------------------------

    /// Verify authenticated bucket metadata access without creating an object.
    pub async fn head_bucket(&self) -> Result<(), AppError> {
        let response = self
            .send_signed(
                Method::HEAD,
                self.bucket_url(),
                Vec::new(),
                None,
                BTreeMap::new(),
            )
            .await?;
        if !response.status().is_success() {
            return Err(AppError::Storage(format!(
                "S3 bucket HEAD returned {}",
                response.status()
            )));
        }
        Ok(())
    }

    /// S3 ListObjectsV2 page, signed with the same credentials as blob I/O.
    /// Bound bytes as they arrive, including endpoints returning HTTP 200 with
    /// invalid XML. Continuation tokens are opaque and never used as paths.
    pub async fn list_objects_page(
        &self,
        continuation: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<super::storage::ObjectEntry>, Option<String>), AppError> {
        let limit = limit.clamp(1, 1000);
        let mut url =
            Url::parse(&self.bucket_url()).map_err(|e| AppError::Storage(e.to_string()))?;
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("list-type", "2")
                .append_pair("max-keys", &limit.to_string())
                .append_pair("encoding-type", "url");
            if let Some(token) = continuation {
                query.append_pair("continuation-token", token);
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let mut response = self
                .send_signed(
                    Method::GET,
                    url.to_string(),
                    Vec::new(),
                    None,
                    BTreeMap::new(),
                )
                .await?;
            if !response.status().is_success() {
                return Err(AppError::Storage(format!(
                    "S3 LIST returned {}",
                    response.status()
                )));
            }
            let mut data = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| AppError::Storage(e.to_string()))?
            {
                if data.len().saturating_add(chunk.len()) > 4 * 1024 * 1024 {
                    return Err(AppError::Storage(
                        "S3 listing exceeds page byte budget".into(),
                    ));
                }
                data.extend_from_slice(&chunk);
            }
            let xml = std::str::from_utf8(&data).map_err(|e| AppError::Storage(e.to_string()))?;
            parse_list_page(xml, limit)
        })
        .await
        .map_err(|_| AppError::Storage("S3 listing timed out".into()))?
    }

    /// Upload data to S3 under the given key.
    pub async fn put_object(
        &self,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<(), AppError> {
        let url = self.object_url(key);
        let resp = self
            .send_signed(
                Method::PUT,
                url,
                data.to_vec(),
                Some(content_type),
                BTreeMap::new(),
            )
            .await
            .map_err(|e| AppError::Storage(format!("S3 PUT failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::Storage(format!(
                "S3 PUT returned {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// Download the contents of an object from S3.
    pub async fn get_object(&self, key: &str) -> Result<Vec<u8>, AppError> {
        let url = self.object_url(key);
        let resp = self
            .send_signed(Method::GET, url, Vec::new(), None, BTreeMap::new())
            .await
            .map_err(|e| AppError::Storage(format!("S3 GET failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::Storage(format!(
                "S3 GET returned {}",
                resp.status()
            )));
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| AppError::Storage(format!("S3 read failed: {e}")))
    }

    /// Delete an object from S3.
    pub async fn delete_object(&self, key: &str) -> Result<(), AppError> {
        let url = self.object_url(key);
        let resp = self
            .send_signed(Method::DELETE, url, Vec::new(), None, BTreeMap::new())
            .await
            .map_err(|e| AppError::Storage(format!("S3 DELETE failed: {e}")))?;
        if resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(AppError::Storage(format!(
            "S3 DELETE returned {}",
            resp.status()
        )))
    }

    /// Check whether an object exists (HEAD request).
    pub async fn head_object(&self, key: &str) -> Result<bool, AppError> {
        let url = self.object_url(key);
        let resp = self
            .send_signed(Method::HEAD, url, Vec::new(), None, BTreeMap::new())
            .await
            .map_err(|e| AppError::Storage(format!("S3 HEAD failed: {e}")))?;

        if resp.status().is_success() {
            Ok(true)
        } else if resp.status() == StatusCode::NOT_FOUND {
            Ok(false)
        } else {
            Err(AppError::Storage(format!(
                "S3 HEAD returned {}",
                resp.status()
            )))
        }
    }

    /// Generate a storage key for a user-uploaded file.
    ///
    /// The key layout is: `uploads/<owner_id>/<file_id>/<filename>`
    #[allow(dead_code)] // key-layout helper retained for tests / future wiring
    pub fn generate_key(owner_id: &str, file_id: &str, filename: &str) -> String {
        format!("uploads/{}/{}/{}", owner_id, file_id, filename)
    }

    /// Generate a storage key for a file version.
    #[allow(dead_code)] // key-layout helper retained for tests / future wiring
    pub fn generate_version_key(
        owner_id: &str,
        file_id: &str,
        version: i32,
        filename: &str,
    ) -> String {
        format!(
            "uploads/{}/{}/versions/v{}/{}",
            owner_id, file_id, version, filename
        )
    }

    /// Generate a storage key for a thumbnail. Thumbnails are encoded as JPEG
    /// (see `ThumbnailService`), so the key uses a `.jpg` extension to match.
    #[allow(dead_code)] // key-layout helper retained for tests / future wiring
    pub fn generate_thumbnail_key(file_id: &str) -> String {
        format!("thumbnails/{}.jpg", file_id)
    }
}

fn parse_list_page(
    xml: &str,
    limit: usize,
) -> Result<(Vec<super::storage::ObjectEntry>, Option<String>), AppError> {
    use quick_xml::{Reader, events::Event};
    let failure = |message: &str| AppError::Storage(format!("Invalid S3 listing: {message}"));
    let mut reader = Reader::from_str(xml);
    let mut entries = Vec::new();
    let mut key = None;
    let mut modified = None;
    let mut contents = false;
    let mut root = false;
    let mut closed = false;
    let mut truncated = None;
    let mut next = None;
    loop {
        match reader.read_event().map_err(|e| failure(&e.to_string()))? {
            Event::Start(event) => {
                let name = event.local_name();
                match name.as_ref() {
                    b"ListBucketResult" => {
                        if root {
                            return Err(failure("duplicate root"));
                        }
                        root = true;
                    }
                    b"Contents" => {
                        if contents {
                            return Err(failure("nested contents"));
                        }
                        contents = true;
                        key = None;
                        modified = None;
                    }
                    b"Key" | b"LastModified" | b"IsTruncated" | b"NextContinuationToken" => {
                        let text = reader
                            .read_text(event.name())
                            .map_err(|e| failure(&e.to_string()))?;
                        // quick-xml 0.41 returns BytesText, not a string. Decode
                        // strictly, then resolve XML entities exactly once before
                        // percent-decoding S3 keys (tokens stay opaque strings).
                        let text = text.decode().map_err(|e| failure(&e.to_string()))?;
                        let text = quick_xml::escape::unescape(&text)
                            .map_err(|e| failure(&e.to_string()))?;
                        match name.as_ref() {
                            b"Key" if contents => key = Some(decode_key(&text)?),
                            b"LastModified" if contents => {
                                modified = Some(
                                    chrono::DateTime::parse_from_rfc3339(&text)
                                        .map_err(|e| failure(&e.to_string()))?
                                        .timestamp(),
                                )
                            }
                            b"IsTruncated" => {
                                truncated = Some(match text.trim() {
                                    "true" => true,
                                    "false" => false,
                                    _ => return Err(failure("invalid truncation flag")),
                                })
                            }
                            b"NextContinuationToken" => {
                                if text.len() > 65536 {
                                    return Err(failure("oversized continuation token"));
                                }
                                next = Some(text.into_owned());
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            Event::End(event) => match event.local_name().as_ref() {
                b"Contents" => {
                    let path = key.take().ok_or_else(|| failure("missing key"))?;
                    let modified_at = modified
                        .take()
                        .ok_or_else(|| failure("missing last modification"))?;
                    entries.push(super::storage::ObjectEntry { path, modified_at });
                    contents = false;
                    if entries.len() > limit {
                        return Err(failure("more objects than requested"));
                    }
                }
                b"ListBucketResult" => closed = true,
                _ => {}
            },
            Event::DocType(_) => return Err(failure("DOCTYPE is not permitted")),
            Event::Eof => break,
            _ => {}
        }
    }
    if !root || !closed || contents {
        return Err(failure("incomplete response"));
    }
    let truncated = truncated.ok_or_else(|| failure("missing truncation flag"))?;
    if truncated && next.as_ref().is_none_or(String::is_empty) {
        return Err(failure("missing continuation token"));
    }
    Ok((entries, if truncated { next } else { None }))
}
fn decode_key(value: &str) -> Result<String, AppError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(AppError::Storage("Invalid S3 key percent escape".into()));
            }
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            let (Some(high), Some(low)) = (high, low) else {
                return Err(AppError::Storage("Invalid S3 key percent escape".into()));
            };
            decoded.push((high * 16 + low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    if decoded.len() > 1024 {
        return Err(AppError::Storage("S3 object key exceeds 1024 bytes".into()));
    }
    String::from_utf8(decoded).map_err(|e| AppError::Storage(e.to_string()))
}

fn host_header(url: &Url) -> Result<String, AppError> {
    let host = url
        .host_str()
        .ok_or_else(|| AppError::Storage("S3 endpoint is missing a host".into()))?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn canonical_uri(url: &Url) -> String {
    let path = url.path();
    if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    }
}

fn canonical_query(url: &Url) -> String {
    let mut pairs = url
        .query_pairs()
        .map(|(key, value)| (encode_query_component(&key), encode_query_component(&value)))
        .collect::<Vec<_>>();
    pairs.sort();
    pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn encode_path(value: &str) -> String {
    percent_encode(value, true)
}

fn encode_query_component(value: &str) -> String {
    percent_encode(value, false)
}

fn percent_encode(value: &str, keep_slash: bool) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (keep_slash && *byte == b'/');
        if keep {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    hex_bytes(&digest)
}

fn signing_key(secret_key: &str, date: &str, region: &str) -> [u8; 32] {
    let date_key = hmac_sha256(format!("AWS4{secret_key}").as_bytes(), date.as_bytes());
    let region_key = hmac_sha256(&date_key, region.as_bytes());
    let service_key = hmac_sha256(&region_key, b"s3");
    hmac_sha256(&service_key, b"aws4_request")
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0_u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let digest = Sha256::digest(key);
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut inner_pad = [0x36_u8; BLOCK_SIZE];
    let mut outer_pad = [0x5c_u8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        inner_pad[i] ^= key_block[i];
        outer_pad[i] ^= key_block[i];
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_hash);
    let digest = outer.finalize();
    let mut output = [0_u8; 32];
    output.copy_from_slice(&digest);
    output
}

fn hex_bytes(data: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(data.len() * 2);
    for byte in data {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_url_default_endpoint() {
        let s3 = S3Storage::new("my-bucket", "us-east-1", "ak", "sk", "");
        assert_eq!(
            s3.object_url("path/to/file.txt"),
            "https://my-bucket.s3.us-east-1.amazonaws.com/path/to/file.txt"
        );
    }

    #[test]
    fn object_url_custom_endpoint() {
        let s3 = S3Storage::new("my-bucket", "us-east-1", "ak", "sk", "http://minio:9000");
        assert_eq!(
            s3.object_url("path/to/file.txt"),
            "http://minio:9000/my-bucket/path/to/file.txt"
        );
    }

    #[test]
    fn bucket_url_default_endpoint() {
        let s3 = S3Storage::new("my-bucket", "eu-west-1", "ak", "sk", "");
        assert_eq!(
            s3.bucket_url(),
            "https://my-bucket.s3.eu-west-1.amazonaws.com"
        );
    }

    #[test]
    fn bucket_url_custom_endpoint() {
        let s3 = S3Storage::new(
            "my-bucket",
            "eu-west-1",
            "ak",
            "sk",
            "http://localhost:9000",
        );
        assert_eq!(s3.bucket_url(), "http://localhost:9000/my-bucket");
    }

    #[test]
    fn is_custom_endpoint() {
        let s3_default = S3Storage::new("b", "r", "a", "s", "");
        assert!(!s3_default.is_custom_endpoint());

        let s3_custom = S3Storage::new("b", "r", "a", "s", "http://minio:9000");
        assert!(s3_custom.is_custom_endpoint());
    }

    #[test]
    fn generate_key_formats() {
        let key = S3Storage::generate_key("user-1", "file-abc", "report.pdf");
        assert_eq!(key, "uploads/user-1/file-abc/report.pdf");

        let vkey = S3Storage::generate_version_key("user-1", "file-abc", 3, "report.pdf");
        assert_eq!(vkey, "uploads/user-1/file-abc/versions/v3/report.pdf");

        let tkey = S3Storage::generate_thumbnail_key("file-abc");
        assert_eq!(tkey, "thumbnails/file-abc.jpg");
    }

    #[test]
    fn from_config_returns_none_when_no_bucket() {
        let cfg = contrix_core::config::StorageConfig {
            upload_idle_timeout_seconds: 30,
            upload_total_timeout_seconds: 900,
            base_path: "data/files".to_string(),
            max_upload_size_mb: 100,
            default_quota_mb: 5120,
            backend: Some("s3".to_string()),
            s3_bucket: None,
            s3_region: None,
            s3_access_key: None,
            s3_secret_key: None,
            s3_endpoint: None,
        };
        assert!(S3Storage::from_config(&cfg).is_none());
    }

    #[test]
    fn from_config_returns_some_with_bucket() {
        let cfg = contrix_core::config::StorageConfig {
            upload_idle_timeout_seconds: 30,
            upload_total_timeout_seconds: 900,
            base_path: "data/files".to_string(),
            max_upload_size_mb: 100,
            default_quota_mb: 5120,
            backend: Some("s3".to_string()),
            s3_bucket: Some("test-bucket".to_string()),
            s3_region: Some("us-west-2".to_string()),
            s3_access_key: Some("AKID".to_string()),
            s3_secret_key: Some("SECRET".to_string()),
            s3_endpoint: None,
        };
        let s3 = S3Storage::from_config(&cfg).unwrap();
        assert_eq!(s3.bucket(), "test-bucket");
        assert_eq!(s3.region(), "us-west-2");
        assert!(!s3.is_custom_endpoint());
    }

    #[test]
    fn accessors() {
        let s3 = S3Storage::new("b", "r", "a", "s", "http://ep");
        assert_eq!(s3.bucket(), "b");
        assert_eq!(s3.region(), "r");
        assert!(s3.is_custom_endpoint());
    }
    #[test]
    fn listing_parses_encoded_keys_and_rejects_malformed_or_unbounded_pages() {
        let xml = r#"<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><IsTruncated>true</IsTruncated><Contents><Key>owner%2FHello%20%E4%B8%96%E7%95%8C%2B%25%23%3F</Key><LastModified>2026-11-10T00:00:00Z</LastModified></Contents><NextContinuationToken>opaque&amp;token</NextContinuationToken></ListBucketResult>"#;
        let (objects, next) = parse_list_page(xml, 1).unwrap();
        assert_eq!(objects[0].path, "owner/Hello 世界+%#?");
        assert_eq!(next.as_deref(), Some("opaque&token"));
        let (_, numeric) = parse_list_page(
            &xml.replace("opaque&amp;token", "opaque&#x4E16;&#x754C;&amp;token"),
            1,
        )
        .unwrap();
        assert_eq!(numeric.as_deref(), Some("opaque世界&token"));
        let (_, once) =
            parse_list_page(&xml.replace("opaque&amp;token", "opaque&amp;amp;token"), 1).unwrap();
        assert_eq!(once.as_deref(), Some("opaque&amp;token"));
        let (literal, _) = parse_list_page(
            &xml.replace(
                "owner%2FHello%20%E4%B8%96%E7%95%8C%2B%25%23%3F",
                "owner%2FA%26amp%3B",
            ),
            1,
        )
        .unwrap();
        assert_eq!(literal[0].path, "owner/A&amp;");
        assert!(parse_list_page(&xml.replace("opaque&amp;token", "opaque&unknown;"), 1).is_err());
        assert!(parse_list_page(&xml.replace("opaque&amp;token", "opaque&#xD800;"), 1).is_err());
        assert!(parse_list_page(&xml.replace("owner%2F", "owner%2F%FF"), 1).is_err());
        assert!(
            parse_list_page(
                &xml.replace(
                    "<NextContinuationToken>opaque&amp;token</NextContinuationToken>",
                    ""
                ),
                1
            )
            .is_err()
        );
        assert!(parse_list_page(&xml.replace("</ListBucketResult>", ""), 1).is_err());
        assert!(parse_list_page(&xml.replace("2026-11-10T00:00:00Z", "not-a-date"), 1).is_err());
        assert!(parse_list_page(&xml.replace("owner%2F", "bad%QZ"), 1).is_err());
        assert!(
            parse_list_page(
                &xml.replace("<IsTruncated>", "<!DOCTYPE x><IsTruncated>"),
                1
            )
            .is_err()
        );
        assert!(parse_list_page(xml, 0).is_err());
    }
}
