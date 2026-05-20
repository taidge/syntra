//! # contrix-frontend
//!
//! Dioxus-based WebAssembly frontend for Contrix Office.
//!
//! This crate provides the complete browser UI including:
//!
//! - Authentication pages (login, register)
//! - Dashboard with recent documents and quick actions
//! - File manager with folder navigation
//! - Rich-text editor (Writer)
//! - Spreadsheet editor (Calc)
//! - Presentation editor (Slides)
//! - Whiteboard / freeform canvas
//! - Admin panel with user management
//!
//! The application entry point is in `main.rs` which sets up the Dioxus
//! router and launches the WASM application.  All modules are declared
//! there to keep `crate::Route` in scope for page components.

pub mod state;
