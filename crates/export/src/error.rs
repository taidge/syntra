//! Export/import error types.

use thiserror::Error;

/// Errors that can occur during export or import operations.
#[derive(Error, Debug)]
pub enum ExportError {
    /// The requested format is not (yet) supported by this crate.
    #[error("Unsupported format: {0}")]
    UnsupportedFormat(String),

    /// The source document is invalid or cannot be processed.
    #[error("Invalid document: {0}")]
    InvalidDocument(String),

    /// A general export failure.
    #[error("Export failed: {0}")]
    ExportFailed(String),

    /// A general import failure.
    #[error("Import failed: {0}")]
    ImportFailed(String),
}
