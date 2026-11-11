use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Application-wide error type used across all services.
///
/// Each variant maps to a specific HTTP status code and provides
/// a human-readable error message for API consumers.
#[derive(Error, Debug)]
pub enum AppError {
    /// 401 - Authentication failure (invalid credentials, expired token, etc.)
    #[error("Authentication error: {0}")]
    Auth(String),

    /// 404 - Requested resource does not exist
    #[error("Not found: {0}")]
    NotFound(String),

    /// 403 - User lacks permission for the requested action
    #[error("Permission denied: {0}")]
    Forbidden(String),

    /// 400 - Request payload failed validation
    #[error("Validation error: {0}")]
    Validation(String),

    /// 408 - The request body stopped making progress or exceeded its deadline.
    #[error("Request timeout: {0}")]
    Timeout(String),

    /// 409 - Conflicting state (e.g., duplicate username, concurrent edit)
    #[error("Conflict: {0}")]
    Conflict(String),

    /// 500/503 - Database failure (503 only for known temporary contention)
    #[error("Database error: {0}")]
    Database(String),

    /// 500 - File storage read/write failure
    #[error("Storage error: {0}")]
    Storage(String),

    /// 500 - Catch-all for unexpected internal errors
    #[error("Internal error: {0}")]
    Internal(String),

    /// 429 - Too many requests (rate limit exceeded)
    #[error("Rate limit exceeded: {0}")]
    RateLimit(String),

    /// 413 - Storage quota exceeded
    #[error("Quota exceeded: {0}")]
    QuotaExceeded(String),
}

/// JSON-serializable error payload returned in API responses.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ErrorResponse {
    /// HTTP status code (e.g., 400, 404, 500)
    pub code: u16,
    /// Short description of the error
    pub message: String,
    /// Optional additional context or field-level details
    pub details: Option<String>,
}

impl AppError {
    /// Temporary admission/checkout pressure and SQLite lock contention are
    /// retryable service failures. Match only canonical diagnostics, never a
    /// broad "database"/"timeout" substring that could hide corruption or SQL
    /// errors. Horme adds one documented prefix for acquisition failures.
    pub fn is_transient_database(&self) -> bool {
        let Self::Database(message) = self else {
            return false;
        };
        let message = message
            .strip_prefix("connection acquisition failed: ")
            .unwrap_or(message);
        matches!(
            message,
            "database admission capacity exceeded"
                | "database checkout timed out"
                | "database operation timed out; cleanup remains admitted"
                | "collaboration database backpressure deadline exceeded"
                | "database is locked"
                | "database is busy"
                | "database table is locked"
                | "database schema is locked"
        ) || ["database table is locked: ", "database schema is locked: "]
            .iter()
            .any(|prefix| {
                message
                    .strip_prefix(prefix)
                    .is_some_and(|table| !table.is_empty() && !table.contains('\n'))
            })
    }

    /// Returns the HTTP status code that corresponds to this error variant.
    pub fn status_code(&self) -> u16 {
        match self {
            AppError::Auth(_) => 401,
            AppError::NotFound(_) => 404,
            AppError::Forbidden(_) => 403,
            AppError::Validation(_) => 400,
            AppError::Timeout(_) => 408,
            AppError::Conflict(_) => 409,
            AppError::RateLimit(_) => 429,
            AppError::QuotaExceeded(_) => 413,
            AppError::Database(_) if self.is_transient_database() => 503,
            AppError::Database(_) | AppError::Storage(_) | AppError::Internal(_) => 500,
        }
    }

    /// Converts this error into a structured [`ErrorResponse`] suitable for
    /// serialization in an API reply.
    pub fn to_response(&self) -> ErrorResponse {
        ErrorResponse {
            code: self.status_code(),
            message: self.to_string(),
            details: None,
        }
    }
}

impl From<diesel::result::Error> for AppError {
    fn from(e: diesel::result::Error) -> Self {
        AppError::Database(e.to_string())
    }
}

#[cfg(feature = "async-db")]
impl From<horme::Error> for AppError {
    fn from(error: horme::Error) -> Self {
        Self::Database(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::AppError;
    #[test]
    fn only_known_temporary_database_contention_is_503() {
        for message in [
            "database admission capacity exceeded",
            "connection acquisition failed: database checkout timed out",
            "database operation timed out; cleanup remains admitted",
            "collaboration database backpressure deadline exceeded",
            "database is locked",
            "database is busy",
            "database table is locked: documents",
            "database schema is locked: main",
        ] {
            let error = AppError::Database(message.into());
            assert!(error.is_transient_database(), "{message}");
            assert_eq!(error.status_code(), 503);
            assert_eq!(error.to_response().code, 503);
        }
        for message in [
            "database disk image is malformed",
            "file is not a database",
            "near INSERT: syntax error",
            "no such table: database is locked",
            "database admission capacity too large",
            "database checkout task failed",
            "UNIQUE constraint failed: documents.id",
            "disk I/O error",
            "other timeout",
            "collaboration database backpressure deadline exceeded: unexpected detail",
        ] {
            let error = AppError::Database(message.into());
            assert!(!error.is_transient_database(), "{message}");
            assert_eq!(error.status_code(), 500);
            assert_eq!(error.to_response().code, 500);
        }
        assert_eq!(
            AppError::Internal("database is locked".into()).status_code(),
            500
        );
    }
}
