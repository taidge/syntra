//! Per-document collaboration channels and gates. The registry lock is never
//! held while applying an operation or waiting for a document's mutex.
use super::operation::Operation;
use super::session::{CollabMessage, CollabSession, UserCursor};
use std::collections::HashMap;
use std::sync::{Arc, Weak};
use tokio::sync::{Mutex, RwLock, broadcast};
use uuid::Uuid;

type Session = Arc<Mutex<CollabSession>>;
type DocumentGates = Arc<Mutex<HashMap<Uuid, Weak<Mutex<()>>>>>;

#[derive(Clone, Default)]
pub struct CollabManager {
    sessions: Arc<RwLock<HashMap<Uuid, Session>>>,
    gates: DocumentGates,
}
impl CollabManager {
    pub fn new() -> Self {
        Self::default()
    }

    async fn session(&self, document_id: Uuid) -> Session {
        if let Some(session) = self.sessions.read().await.get(&document_id).cloned() {
            return session;
        }
        self.sessions
            .write()
            .await
            .entry(document_id)
            .or_insert_with(|| Arc::new(Mutex::new(CollabSession::new(document_id))))
            .clone()
    }

    /// Serialize read/apply/commit for one document without blocking other documents.
    /// Weak entries do not retain gates after callers finish.
    pub async fn document_gate(&self, document_id: Uuid) -> Arc<Mutex<()>> {
        let mut gates = self.gates.lock().await;
        if let Some(gate) = gates.get(&document_id).and_then(Weak::upgrade) {
            return gate;
        }
        if gates.len() >= 1024 {
            gates.retain(|_, gate| gate.strong_count() != 0);
        }
        let gate = Arc::new(Mutex::new(()));
        gates.insert(document_id, Arc::downgrade(&gate));
        gate
    }

    pub async fn get_or_create_session(
        &self,
        document_id: Uuid,
    ) -> broadcast::Receiver<CollabMessage> {
        self.session(document_id)
            .await
            .lock()
            .await
            .broadcast
            .subscribe()
    }
    pub async fn join_session(
        &self,
        document_id: Uuid,
        user_id: Uuid,
        username: String,
    ) -> Result<broadcast::Receiver<CollabMessage>, String> {
        Ok(self
            .session(document_id)
            .await
            .lock()
            .await
            .join(user_id, username))
    }
    pub async fn leave_session(&self, document_id: Uuid, user_id: Uuid) {
        let session = self.sessions.read().await.get(&document_id).cloned();
        if let Some(session) = session {
            session.lock().await.leave(user_id);
        }
        self.cleanup_empty_sessions().await;
    }
    pub async fn apply_operation(
        &self,
        document_id: Uuid,
        op: Operation,
    ) -> Result<(u64, Operation), String> {
        let session = self
            .sessions
            .read()
            .await
            .get(&document_id)
            .cloned()
            .ok_or_else(|| format!("no active session for document {document_id}"))?;
        session.lock().await.apply_operation(op)
    }
    pub async fn undo(&self, document_id: Uuid, user_id: Uuid) -> Result<u64, String> {
        let session = self
            .sessions
            .read()
            .await
            .get(&document_id)
            .cloned()
            .ok_or_else(|| format!("no active session for document {document_id}"))?;
        session.lock().await.undo(user_id)
    }
    pub async fn redo(&self, document_id: Uuid, user_id: Uuid) -> Result<u64, String> {
        let session = self
            .sessions
            .read()
            .await
            .get(&document_id)
            .cloned()
            .ok_or_else(|| format!("no active session for document {document_id}"))?;
        session.lock().await.redo(user_id)
    }
    pub async fn update_cursor(&self, document_id: Uuid, cursor: UserCursor) {
        let session = self.sessions.read().await.get(&document_id).cloned();
        if let Some(session) = session {
            session.lock().await.update_cursor(cursor);
        }
    }
    /// Publish only committed content and its database version. An older commit's
    /// delayed cleanup/publish cannot roll another client's version backwards.
    pub async fn publish_snapshot(
        &self,
        document_id: Uuid,
        content: serde_json::Value,
        version: u64,
    ) {
        let session = self.sessions.read().await.get(&document_id).cloned();
        if let Some(session) = session {
            let mut session = session.lock().await;
            if version < session.version {
                return;
            }
            session.reset_history(version);
            let _ = session
                .broadcast
                .send(CollabMessage::DocumentState { content, version });
        }
    }
    pub async fn cleanup_empty_sessions(&self) {
        let candidates: Vec<_> = self
            .sessions
            .read()
            .await
            .iter()
            .map(|(id, s)| (*id, s.clone()))
            .collect();
        for (id, candidate) in candidates {
            if !candidate.lock().await.is_empty() {
                continue;
            }
            let mut sessions = self.sessions.write().await;
            // Any concurrent caller holding this session prevents removal.
            if Arc::strong_count(&candidate) == 2
                && sessions
                    .get(&id)
                    .is_some_and(|current| Arc::ptr_eq(current, &candidate))
            {
                sessions.remove(&id);
            }
        }
    }
    pub async fn active_session_count(&self) -> usize {
        self.sessions.read().await.len()
    }
    pub async fn session_version(&self, document_id: Uuid) -> Option<u64> {
        let session = self.sessions.read().await.get(&document_id).cloned()?;
        Some(session.lock().await.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OpType;
    #[tokio::test]
    async fn one_document_gate_does_not_block_another_and_empty_sessions_retire() {
        let manager = CollabManager::new();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let gate = manager.document_gate(a).await;
        let _held = gate.lock().await;
        let user = Uuid::new_v4();
        let _rx = manager.join_session(b, user, "B".into()).await.unwrap();
        let op = Operation::new(
            user,
            Uuid::new_v4(),
            1,
            OpType::Insert {
                path: vec!["body".into()],
                index: 0,
                content: "x".into(),
            },
        );
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            manager.apply_operation(b, op),
        )
        .await
        .unwrap()
        .unwrap();
        manager.leave_session(b, user).await;
        assert_eq!(manager.active_session_count().await, 0);
    }
    #[tokio::test]
    async fn delayed_old_commit_cannot_publish_over_newer_state() {
        let manager = CollabManager::new();
        let doc = Uuid::new_v4();
        let mut rx = manager.get_or_create_session(doc).await;
        manager
            .publish_snapshot(doc, serde_json::json!({"new":true}), 3)
            .await;
        manager
            .publish_snapshot(doc, serde_json::json!({"old":true}), 2)
            .await;
        assert!(matches!(
            rx.recv().await.unwrap(),
            CollabMessage::DocumentState { version: 3, .. }
        ));
        assert!(rx.try_recv().is_err());
    }
}
