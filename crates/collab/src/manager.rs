//! Session manager — the top-level API consumed by the server crate.
//!
//! [`CollabManager`] owns all active collaboration sessions and serialises
//! access to them via a `RwLock`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

use super::operation::Operation;
use super::session::{CollabMessage, CollabSession, UserCursor};

/// Manages all active collaboration sessions.
///
/// This type is cheaply cloneable (the inner state is behind an `Arc`) and
/// is designed to be shared across Tokio tasks.
#[derive(Clone)]
pub struct CollabManager {
    sessions: Arc<RwLock<HashMap<Uuid, CollabSession>>>,
}

impl CollabManager {
    /// Create a new, empty manager.
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Return a broadcast receiver for the given document, creating a new
    /// session if one does not already exist.
    pub async fn get_or_create_session(
        &self,
        document_id: Uuid,
    ) -> broadcast::Receiver<CollabMessage> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(document_id)
            .or_insert_with(|| CollabSession::new(document_id));
        session.broadcast.subscribe()
    }

    /// Join an existing (or newly created) session for `document_id`.
    ///
    /// Returns a broadcast receiver the caller can use to listen for future
    /// collaboration messages.
    pub async fn join_session(
        &self,
        document_id: Uuid,
        user_id: Uuid,
        username: String,
    ) -> Result<broadcast::Receiver<CollabMessage>, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .entry(document_id)
            .or_insert_with(|| CollabSession::new(document_id));
        Ok(session.join(user_id, username))
    }

    /// Remove a user from the session.  If the session becomes empty it is
    /// **not** automatically removed — call [`cleanup_empty_sessions`] for
    /// that.
    pub async fn leave_session(&self, document_id: Uuid, user_id: Uuid) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(&document_id) {
            session.leave(user_id);
        }
    }

    /// Apply an operation to the document's session, returning the new
    /// version number on success.
    pub async fn apply_operation(&self, document_id: Uuid, op: Operation) -> Result<u64, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&document_id)
            .ok_or_else(|| format!("no active session for document {document_id}"))?;
        session.apply_operation(op)
    }

    /// Undo the last operation by the given user in the specified session.
    pub async fn undo(&self, document_id: Uuid, user_id: Uuid) -> Result<u64, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&document_id)
            .ok_or_else(|| format!("no active session for document {document_id}"))?;
        session.undo(user_id)
    }

    /// Redo the last undone operation by the given user in the specified session.
    pub async fn redo(&self, document_id: Uuid, user_id: Uuid) -> Result<u64, String> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(&document_id)
            .ok_or_else(|| format!("no active session for document {document_id}"))?;
        session.redo(user_id)
    }

    /// Broadcast a cursor update for a participant.
    pub async fn update_cursor(&self, document_id: Uuid, cursor: UserCursor) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(&document_id) {
            session.update_cursor(cursor);
        }
    }

    /// Remove all sessions that have no remaining participants.
    pub async fn cleanup_empty_sessions(&self) {
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_id, session| !session.is_empty());
    }

    /// Return the number of currently active sessions.
    pub async fn active_session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Return the current version of a session, or `None` if the session
    /// does not exist.
    pub async fn session_version(&self, document_id: Uuid) -> Option<u64> {
        self.sessions
            .read()
            .await
            .get(&document_id)
            .map(|s| s.version)
    }
}

impl Default for CollabManager {
    fn default() -> Self {
        Self::new()
    }
}
