//! Collaboration session management.
//!
//! A [`CollabSession`] represents a single, live editing session for one
//! document.  Multiple users can join the session, apply operations, and
//! see each other's cursor positions in real time via a broadcast channel.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use super::operation::{OpId, Operation};

// ---------------------------------------------------------------------------
// Cursor types
// ---------------------------------------------------------------------------

/// Where a user's cursor sits inside the document tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPosition {
    /// JSON-path-like segments addressing a node (e.g. `["body", "3", "runs", "0"]`).
    pub path: Vec<String>,
    /// Character offset within the addressed node.
    pub offset: usize,
}

/// A contiguous selection (anchor → focus).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionRange {
    pub start: CursorPosition,
    pub end: CursorPosition,
}

/// A user's cursor/selection state plus display metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserCursor {
    pub user_id: Uuid,
    pub username: String,
    /// CSS-style colour string used to render this user's cursor.
    pub color: String,
    pub position: CursorPosition,
    pub selection: Option<SelectionRange>,
    pub last_active: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Collaboration message
// ---------------------------------------------------------------------------

/// An envelope sent over the broadcast channel (and ultimately serialised
/// over WebSocket).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CollabMessage {
    /// A document operation produced by a user.
    Operation(Operation),
    /// Cursor position update.
    CursorUpdate(UserCursor),
    /// A new user joined the session.
    UserJoined {
        user_id: Uuid,
        username: String,
        color: String,
    },
    /// A user left the session.
    UserLeft { user_id: Uuid },
    /// Full document state snapshot (sent when a user first joins).
    DocumentState {
        content: serde_json::Value,
        version: u64,
    },
    /// Acknowledgment that an operation has been accepted.
    Ack { op_id: OpId },
    /// Request to synchronise from a given version.
    SyncRequest { from_version: u64 },
    /// Request to undo the last operation.
    Undo { user_id: Uuid },
    /// Request to redo the last undone operation.
    Redo { user_id: Uuid },
    /// An error message for a single client.
    Error { message: String },
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Pre-defined colours assigned to users in round-robin order.
const USER_COLORS: &[&str] = &[
    "#E57373", "#81C784", "#64B5F6", "#FFD54F", "#BA68C8", "#4FC3F7", "#FF8A65", "#AED581",
    "#F06292", "#4DD0E1",
];

/// A collaboration session for a single document.
pub struct CollabSession {
    /// The document this session is editing.
    pub document_id: Uuid,
    /// Currently connected participants.
    pub participants: HashMap<Uuid, UserCursor>,
    /// Ordered log of all operations applied during this session.
    pub operations: Vec<Operation>,
    /// Monotonically increasing version number (bumped on each accepted op).
    pub version: u64,
    /// Broadcast channel for distributing messages to all participants.
    pub broadcast: broadcast::Sender<CollabMessage>,
    /// When this session was created.
    pub created_at: DateTime<Utc>,
    /// Per-user undo stacks (user_id -> Vec of operations they produced).
    pub undo_stacks: HashMap<Uuid, Vec<Operation>>,
    /// Per-user redo stacks.
    pub redo_stacks: HashMap<Uuid, Vec<Operation>>,
    /// Counter used to cycle through `USER_COLORS`.
    color_index: usize,
}

impl CollabSession {
    /// Create a new session for the given document.
    pub fn new(document_id: Uuid) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            document_id,
            participants: HashMap::new(),
            operations: Vec::new(),
            version: 0,
            broadcast: tx,
            created_at: Utc::now(),
            undo_stacks: HashMap::new(),
            redo_stacks: HashMap::new(),
            color_index: 0,
        }
    }

    /// Add a user to the session, returning a broadcast receiver so the
    /// caller can listen for future messages.
    pub fn join(&mut self, user_id: Uuid, username: String) -> broadcast::Receiver<CollabMessage> {
        let color = USER_COLORS[self.color_index % USER_COLORS.len()].to_string();
        self.color_index += 1;

        let cursor = UserCursor {
            user_id,
            username: username.clone(),
            color: color.clone(),
            position: CursorPosition {
                path: vec![],
                offset: 0,
            },
            selection: None,
            last_active: Utc::now(),
        };

        self.participants.insert(user_id, cursor);

        // Notify existing participants.
        let _ = self.broadcast.send(CollabMessage::UserJoined {
            user_id,
            username,
            color,
        });

        self.broadcast.subscribe()
    }

    /// Remove a user from the session and notify others.
    pub fn leave(&mut self, user_id: Uuid) {
        self.participants.remove(&user_id);
        let _ = self.broadcast.send(CollabMessage::UserLeft { user_id });
    }

    /// Apply an operation, bump the version, broadcast it, and return the
    /// new version number.
    ///
    /// Before acceptance the operation is transformed against every operation
    /// that was applied *after* the client's last known version (inferred
    /// from the operation's counter).
    pub fn apply_operation(&mut self, mut op: Operation) -> Result<u64, String> {
        // Transform against every operation applied after the version this op
        // was generated against. `base_version` is the server version the
        // client had seen — NOT the per-client Lamport counter, which bears no
        // relation to positions in the global `operations` log.
        let base = (op.base_version as usize).min(self.operations.len());
        let ops_to_transform = &self.operations[base..];

        for concurrent in ops_to_transform {
            op = op.transform(concurrent);
        }

        self.version += 1;
        self.operations.push(op.clone());

        // Track the operation on the user's undo stack and clear redo.
        self.undo_stacks
            .entry(op.user_id)
            .or_default()
            .push(op.clone());
        self.redo_stacks.entry(op.user_id).or_default().clear();

        // Broadcast the (transformed) operation and its ack.
        let _ = self.broadcast.send(CollabMessage::Operation(op.clone()));
        let _ = self.broadcast.send(CollabMessage::Ack {
            op_id: op.id.clone(),
        });

        // Warn if there are participants but no active broadcast receivers.
        if !self.participants.is_empty() && self.broadcast.receiver_count() == 0 {
            tracing::warn!(
                document_id = %self.document_id,
                "broadcast has no receivers despite {} active participant(s)",
                self.participants.len(),
            );
        }

        Ok(self.version)
    }

    /// Undo the last operation by the given user.
    pub fn undo(&mut self, user_id: Uuid) -> Result<u64, String> {
        let stack = self.undo_stacks.entry(user_id).or_default();
        let last_op = stack.pop().ok_or_else(|| "Nothing to undo".to_string())?;
        let inverse_type = last_op.op_type.inverse();
        let inverse_op = Operation::new(user_id, last_op.id.client_id, self.version, inverse_type);

        self.redo_stacks.entry(user_id).or_default().push(last_op);

        self.version += 1;
        self.operations.push(inverse_op.clone());
        let _ = self
            .broadcast
            .send(CollabMessage::Operation(inverse_op.clone()));
        let _ = self.broadcast.send(CollabMessage::Ack {
            op_id: inverse_op.id,
        });
        Ok(self.version)
    }

    /// Redo the last undone operation by the given user.
    pub fn redo(&mut self, user_id: Uuid) -> Result<u64, String> {
        let stack = self.redo_stacks.entry(user_id).or_default();
        let redo_op = stack.pop().ok_or_else(|| "Nothing to redo".to_string())?;
        let redo_type = redo_op.op_type.clone();
        let new_op = Operation::new(user_id, redo_op.id.client_id, self.version, redo_type);

        self.undo_stacks.entry(user_id).or_default().push(redo_op);

        self.version += 1;
        self.operations.push(new_op.clone());
        let _ = self
            .broadcast
            .send(CollabMessage::Operation(new_op.clone()));
        let _ = self.broadcast.send(CollabMessage::Ack { op_id: new_op.id });
        Ok(self.version)
    }

    /// Update a user's cursor position and broadcast the change.
    pub fn update_cursor(&mut self, cursor: UserCursor) {
        let user_id = cursor.user_id;
        self.participants.insert(user_id, cursor.clone());
        let _ = self.broadcast.send(CollabMessage::CursorUpdate(cursor));
    }

    /// Return a snapshot of all currently connected participants.
    pub fn get_participants(&self) -> Vec<&UserCursor> {
        self.participants.values().collect()
    }

    /// `true` when there are no participants.
    pub fn is_empty(&self) -> bool {
        self.participants.is_empty()
    }
}
