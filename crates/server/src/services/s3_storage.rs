use contrix_core::error::AppError;

/// S3-compatible storage backend.
///
/// Uses `reqwest` to make HTTP requests to an S3-compatible API (simple
/// PUT / GET / DELETE).  This is a minimal implementation that avoids pulling
/// in a full AWS SDK dependency.
pub struct S3Storage {
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    endpoint: String,
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
        }
    }

    /// Build an `S3Storage` from an [`contrix_core::config::StorageConfig`].
    ///
    /// Returns `None` if the required S3 fields are not configured.
    pub fn from_config(cfg: &contrix_core::config::StorageConfig) -> Option<Self> {
        let bucket = cfg.s3_bucket.as_deref()?;
        let region = cfg.s3_region.as_deref().unwrap_or("us-east-1");
        let access_key = cfg.s3_access_key.as_deref().unwrap_or("");
        let secret_key = cfg.s3_secret_key.as_deref().unwrap_or("");
        let endpoint = cfg.s3_endpoint.as_deref().unwrap_or("");
        Some(Self::new(bucket, region, access_key, secret_key, endpoint))
    }

    // -- accessors -----------------------------------------------------------

    /// The configured bucket name.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// The configured region.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Whether this storage points at a custom (non-AWS) endpoint.
    pub fn is_custom_endpoint(&self) -> bool {
        !self.endpoint.is_empty()
    }

    // -- internal helpers ----------------------------------------------------

    /// Generate the full URL for an object key.
    fn object_url(&self, key: &str) -> String {
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
            format!(
                "https://{}.s3.{}.amazonaws.com",
                self.bucket, self.region
            )
        } else {
            format!("{}/{}", self.endpoint, self.bucket)
        }
    }

    /// Build a shared `reqwest::Client`.
    fn client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }

    // -- public API ----------------------------------------------------------

    /// Upload data to S3 under the given key.
    pub async fn put_object(
        &self,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<(), AppError> {
        let url = self.object_url(key);
        let resp = self
            .client()
            .put(&url)
            .header("Content-Type", content_type)
            .header("x-amz-content-sha256", "UNSIGNED-PAYLOAD")
            .body(data.to_vec())
            .send()
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
            .client()
            .get(&url)
            .send()
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
        let _ = self
            .client()
            .delete(&url)
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 DELETE failed: {e}")))?;
        Ok(())
    }

    /// Check whether an object exists (HEAD request).
    pub async fn head_object(&self, key: &str) -> Result<bool, AppError> {
        let url = self.object_url(key);
        let resp = self
            .client()
            .head(&url)
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 HEAD failed: {e}")))?;

        Ok(resp.status().is_success())
    }

    /// Copy an object within the same bucket.
    pub async fn copy_object(&self, source_key: &str, dest_key: &str) -> Result<(), AppError> {
        let url = self.object_url(dest_key);
        let copy_source = format!("{}/{}", self.bucket, source_key);

        let resp = self
            .client()
            .put(&url)
            .header("x-amz-copy-source", copy_source)
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 COPY failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::Storage(format!(
                "S3 COPY returned {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// List object keys under a given prefix.
    ///
    /// Returns a simplified list of keys (no pagination / continuation token
    /// handling — this is intentionally minimal).
    pub async fn list_objects(&self, prefix: &str) -> Result<Vec<String>, AppError> {
        let url = format!("{}?list-type=2&prefix={}", self.bucket_url(), prefix);

        let resp = self
            .client()
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 LIST failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::Storage(format!(
                "S3 LIST returned {}",
                resp.status()
            )));
        }

        // Very simple XML key extraction — good enough for the minimal
        // backend; a production implementation would use a proper XML parser.
        let body = resp
            .text()
            .await
            .map_err(|e| AppError::Storage(format!("S3 LIST read failed: {e}")))?;

        let keys: Vec<String> = body
            .split("<Key>")
            .skip(1)
            .filter_map(|fragment| fragment.split("</Key>").next())
            .map(|s| s.to_string())
            .collect();

        Ok(keys)
    }

    /// Generate a storage key for a user-uploaded file.
    ///
    /// The key layout is: `uploads/<owner_id>/<file_id>/<filename>`
    pub fn generate_key(owner_id: &str, file_id: &str, filename: &str) -> String {
        format!("uploads/{}/{}/{}", owner_id, file_id, filename)
    }

    /// Generate a storage key for a file version.
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

    /// Generate a storage key for a thumbnail.
    pub fn generate_thumbnail_key(file_id: &str) -> String {
        format!("thumbnails/{}.png", file_id)
    }
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
        let s3 = S3Storage::new("my-bucket", "eu-west-1", "ak", "sk", "http://localhost:9000");
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
        assert_eq!(
            vkey,
            "uploads/user-1/file-abc/versions/v3/report.pdf"
        );

        let tkey = S3Storage::generate_thumbnail_key("file-abc");
        assert_eq!(tkey, "thumbnails/file-abc.png");
    }

    #[test]
    fn from_config_returns_none_when_no_bucket() {
        let cfg = contrix_core::config::StorageConfig {
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
}
