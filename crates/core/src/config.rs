use serde::Deserialize;

/// Top-level application configuration.
///
/// Loaded from layered sources (defaults, TOML files, environment variables)
/// via the `figment` crate.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    /// HTTP server settings
    pub server: ServerConfig,
    /// Database connection settings
    pub database: DatabaseConfig,
    /// Authentication / JWT settings
    pub auth: AuthConfig,
    /// File storage settings
    pub storage: StorageConfig,
    /// Rate limiting settings
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
    /// Optional OpenAI-compatible assistant provider.
    #[serde(default)]
    pub ai: AiConfig,
    /// Trash retention settings
    #[serde(default)]
    pub trash: TrashConfig,
}

/// HTTP server bind address and CORS configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Bind host (e.g., `"0.0.0.0"`)
    pub host: String,
    /// Bind port (e.g., `3000`)
    pub port: u16,
    /// Allowed CORS origins (e.g., `["http://localhost:8080"]`)
    pub cors_origins: Vec<String>,
}

/// Database connection pool configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// Connection URL (`sqlite://data.db` or `postgres://...`)
    pub url: String,
    /// Maximum number of connections in the pool
    pub max_connections: u32,
    /// Maximum wait for a short-lived query lease.
    #[serde(default = "database_checkout_timeout_ms")]
    pub checkout_timeout_ms: u64,
    /// Deadline for a query's blocking work; cleanup keeps its admission until done.
    #[serde(default = "database_operation_timeout_ms")]
    pub operation_timeout_ms: u64,
    /// SQLite's maximum wait for a database lock.
    #[serde(default = "database_busy_timeout_ms")]
    pub busy_timeout_ms: u64,
}

fn database_checkout_timeout_ms() -> u64 {
    3_000
}
fn database_operation_timeout_ms() -> u64 {
    15_000
}
fn database_busy_timeout_ms() -> u64 {
    3_000
}

/// JWT authentication parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// Secret key used to sign and verify JWT tokens
    pub jwt_secret: String,
    /// Access token lifetime in hours
    pub jwt_expiry_hours: u64,
    /// Refresh token lifetime in days
    pub refresh_expiry_days: u64,
}

/// Local (or remote) file storage configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    /// Root directory for stored files
    pub base_path: String,
    /// Maximum single-upload size in megabytes
    pub max_upload_size_mb: u64,
    /// Default per-user storage quota in megabytes
    pub default_quota_mb: u64,
    /// Storage backend type: "local" or "s3"
    #[serde(default)]
    pub backend: Option<String>,
    /// S3 bucket name (only used when backend = "s3")
    #[serde(default)]
    pub s3_bucket: Option<String>,
    /// S3 region (only used when backend = "s3")
    #[serde(default)]
    pub s3_region: Option<String>,
    /// S3 access key (only used when backend = "s3")
    #[serde(default)]
    pub s3_access_key: Option<String>,
    /// S3 secret key (only used when backend = "s3")
    #[serde(default)]
    pub s3_secret_key: Option<String>,
    /// S3-compatible endpoint URL (only used when backend = "s3")
    #[serde(default)]
    pub s3_endpoint: Option<String>,
}

/// Rate limiting configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum requests per window
    pub max_requests: u32,
    /// Window duration in seconds
    pub window_seconds: u64,
    /// Whether rate limiting is enabled
    pub enabled: bool,
    /// Whether to trust X-Forwarded-For/X-Real-IP as the client address.
    #[serde(default)]
    pub trust_forwarded_headers: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: 100,
            window_seconds: 60,
            enabled: false,
            trust_forwarded_headers: false,
        }
    }
}

/// Configuration for an OpenAI-compatible chat-completions endpoint.
#[derive(Clone, Deserialize)]
pub struct AiConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub timeout_seconds: u64,
    pub max_requests_per_hour: u32,
    pub max_prompt_chars: usize,
    pub max_document_chars: usize,
    pub max_output_chars: usize,
}

impl std::fmt::Debug for AiConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiConfig")
            .field("enabled", &self.enabled)
            .field("endpoint", &self.endpoint)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("timeout_seconds", &self.timeout_seconds)
            .field("max_requests_per_hour", &self.max_requests_per_hour)
            .field("max_prompt_chars", &self.max_prompt_chars)
            .field("max_document_chars", &self.max_document_chars)
            .field("max_output_chars", &self.max_output_chars)
            .finish()
    }
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            api_key: String::new(),
            model: String::new(),
            timeout_seconds: 30,
            max_requests_per_hour: 20,
            max_prompt_chars: 4_000,
            max_document_chars: 40_000,
            max_output_chars: 12_000,
        }
    }
}

/// Trash retention configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct TrashConfig {
    /// Automatically delete trashed items after this many days
    pub auto_delete_days: u32,
    /// Whether auto-deletion is enabled
    pub auto_delete_enabled: bool,
}

impl Default for TrashConfig {
    fn default() -> Self {
        Self {
            auto_delete_days: 30,
            auto_delete_enabled: false,
        }
    }
}
