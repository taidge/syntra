//! # contrix-collab
//!
//! Real-time collaboration engine for Contrix Office.
//!
//! Implements a simplified CRDT / Operational Transformation (OT) model for
//! concurrent document editing over WebSocket connections.

pub mod manager;
pub mod operation;
pub mod session;

pub use manager::CollabManager;
pub use operation::{OpId, OpType, Operation};
pub use session::{CollabMessage, CollabSession, CursorPosition, SelectionRange, UserCursor};
