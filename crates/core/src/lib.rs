//! # contrix-core
//!
//! Shared types, models, data-transfer objects, configuration structures,
//! and error types used across all crates in the Contrix Office workspace.

pub mod config;
pub mod dto;
pub mod error;
pub mod models;
pub mod schema;

// Re-export the most commonly used items at the crate root for convenience.
pub use config::AppConfig;
pub use error::{AppError, ErrorResponse};
