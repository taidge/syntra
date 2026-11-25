//! # syntra-doc-engine
//!
//! Document processing models and operations for the syntra suite.
//!
//! This crate provides the core data structures for the four document types
//! supported by syntra:
//!
//! - **Writer** — rich text documents (similar to Word / Google Docs)
//! - **Calc** — spreadsheets (similar to Excel / Google Sheets)
//! - **Slides** — presentations (similar to PowerPoint / Google Slides)
//! - **Whiteboard** — freeform drawing canvases

pub mod calc;
pub mod chart_render;
pub mod slides;
pub mod whiteboard;
pub mod writer;

// Re-export the top-level document types for convenience.
pub use calc::{CellRef, CellValue, Selection, Sheet, Workbook};
pub use slides::{Presentation, Slide, SlideElement, SlideLayout};
pub use whiteboard::{DrawElement, Layer, Whiteboard};
pub use writer::{Block, TextRun, WriterDocument};

/// Maximum persisted document JSON content, shared by uploads and editor saves.
pub const MAX_DOCUMENT_CONTENT_BYTES: usize = 20 * 1024 * 1024;
