//! # contrix-doc-engine
//!
//! Document processing models and operations for the Contrix Office suite.
//!
//! This crate provides the core data structures for the four document types
//! supported by Contrix Office:
//!
//! - **Writer** — rich text documents (similar to Word / Google Docs)
//! - **Calc** — spreadsheets (similar to Excel / Google Sheets)
//! - **Slides** — presentations (similar to PowerPoint / Google Slides)
//! - **Whiteboard** — freeform drawing canvases

pub mod calc;
pub mod slides;
pub mod whiteboard;
pub mod writer;

// Re-export the top-level document types for convenience.
pub use calc::{CellRef, CellValue, Selection, Sheet, Workbook};
pub use slides::{Presentation, Slide, SlideElement, SlideLayout};
pub use whiteboard::{DrawElement, Layer, Whiteboard};
pub use writer::{Block, TextRun, WriterDocument};
