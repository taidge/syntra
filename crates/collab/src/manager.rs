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

/// Retry retirement deferred by an in-flight session owner, without new traffic.
pub const SESSION_REAPER_INTERVAL_SECONDS: u64 = 60;

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
    /// Explicitly spawned by the server, independent of SQL/storage cleanup.
    /// The first retry is after 60 seconds; delayed ticks do not cause a burst.
    pub async fn run_session_reaper(self) {
        let period = std::time::Duration::from_secs(SESSION_REAPER_INTERVAL_SECONDS);
        let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            self.cleanup_empty_sessions().await;
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
            // Keep the emptiness check atomic with the registry decision: a
            // rejoin must not finish in between and then lose its active entry.
            // No registry lock is held while awaiting this per-session lock.
            let session_guard = candidate.lock().await;
            if !session_guard.is_empty() {
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

    async fn poll_once<F: std::future::Future>(
        mut future: std::pin::Pin<&mut F>,
    ) -> std::task::Poll<F::Output> {
        std::future::poll_fn(|context| {
            std::task::Poll::Ready(std::future::Future::poll(future.as_mut(), context))
        })
        .await
    }

    #[tokio::test(start_paused = true)]
    async fn periodic_reaper_retries_after_last_external_session_owner_drops() {
        let manager = CollabManager::new();
        let document = Uuid::new_v4();
        let user = Uuid::new_v4();
        let _receiver = manager
            .join_session(document, user, "before".into())
            .await
            .unwrap();
        manager
            .apply_operation(
                document,
                Operation::new(
                    user,
                    Uuid::new_v4(),
                    1,
                    OpType::Insert {
                        path: vec!["body".into()],
                        index: 0,
                        content: "x".repeat(8192),
                    },
                ),
            )
            .await
            .unwrap();
        let held = manager.session(document).await;
        let weak = Arc::downgrade(&held);
        manager.leave_session(document, user).await;
        assert_eq!(manager.active_session_count().await, 1);
        assert_eq!(held.lock().await.operations.len(), 1);

        // This empty entry has no extra session owner, so it detects an
        // accidentally immediate first tick independently of the held entry.
        let empty_document = Uuid::new_v4();
        let _empty_receiver = manager.get_or_create_session(empty_document).await;
        let empty_weak = {
            let sessions = manager.sessions.read().await;
            Arc::downgrade(sessions.get(&empty_document).unwrap())
        };
        let mut reaper = Box::pin(manager.clone().run_session_reaper());
        assert!(poll_once(reaper.as_mut()).await.is_pending());
        assert_eq!(manager.active_session_count().await, 2);
        tokio::time::advance(std::time::Duration::from_secs(59)).await;
        assert!(poll_once(reaper.as_mut()).await.is_pending());
        assert_eq!(manager.active_session_count().await, 2);
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert!(poll_once(reaper.as_mut()).await.is_pending());
        assert_eq!(manager.active_session_count().await, 1);
        assert!(empty_weak.upgrade().is_none());
        for _ in 0..2 {
            tokio::time::advance(std::time::Duration::from_secs(
                SESSION_REAPER_INTERVAL_SECONDS,
            ))
            .await;
            assert!(poll_once(reaper.as_mut()).await.is_pending());
            assert_eq!(manager.active_session_count().await, 1);
            assert!(weak.upgrade().is_some());
        }
        drop(held);
        // No request or leave triggers this final cleanup; only the next tick.
        tokio::time::advance(std::time::Duration::from_secs(
            SESSION_REAPER_INTERVAL_SECONDS,
        ))
        .await;
        assert!(poll_once(reaper.as_mut()).await.is_pending());
        assert_eq!(manager.active_session_count().await, 0);
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn periodic_reaper_keeps_a_rejoined_session_active() {
        let manager = CollabManager::new();
        let document = Uuid::new_v4();
        let before = Uuid::new_v4();
        let after = Uuid::new_v4();
        let _old_receiver = manager
            .join_session(document, before, "before".into())
            .await
            .unwrap();
        let held = manager.session(document).await;
        let weak = Arc::downgrade(&held);
        manager.leave_session(document, before).await;

        let mut reaper = Box::pin(manager.clone().run_session_reaper());
        assert!(poll_once(reaper.as_mut()).await.is_pending());
        tokio::time::advance(std::time::Duration::from_secs(
            SESSION_REAPER_INTERVAL_SECONDS,
        ))
        .await;
        assert!(poll_once(reaper.as_mut()).await.is_pending());
        let _new_receiver = manager
            .join_session(document, after, "after".into())
            .await
            .unwrap();
        {
            let registered = manager.session(document).await;
            assert!(Arc::ptr_eq(&held, &registered));
        }
        drop(held);
        for _ in 0..2 {
            tokio::time::advance(std::time::Duration::from_secs(
                SESSION_REAPER_INTERVAL_SECONDS,
            ))
            .await;
            assert!(poll_once(reaper.as_mut()).await.is_pending());
            assert_eq!(manager.active_session_count().await, 1);
            let live = weak.upgrade().expect("the rejoined session remains owned");
            assert!(live.lock().await.participants.contains_key(&after));
        }
        manager.leave_session(document, after).await;
        assert_eq!(manager.active_session_count().await, 0);
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn cleanup_does_not_detach_a_session_that_rejoins_before_registry_write() {
        let manager = CollabManager::new();
        let document = Uuid::new_v4();
        let user = Uuid::new_v4();
        let _old_receiver = manager
            .join_session(document, user, "before".into())
            .await
            .unwrap();
        let held = manager.session(document).await;
        let weak = Arc::downgrade(&held);
        manager.leave_session(document, user).await;
        assert_eq!(manager.active_session_count().await, 1);

        // Arrange the lock queue without sleeps or production hooks. Cleanup
        // waits first for this session mutex; rejoin clones the same Arc and
        // waits second. The registry reader later blocks cleanup's write.
        let session_guard = held.lock().await;
        let registry_guard = manager.sessions.read().await;
        let mut cleanup = Box::pin(manager.cleanup_empty_sessions());
        assert!(poll_once(cleanup.as_mut()).await.is_pending());
        let mut joining = Box::pin(manager.join_session(document, user, "after".into()));
        assert!(poll_once(joining.as_mut()).await.is_pending());
        drop(session_guard);
        assert!(poll_once(cleanup.as_mut()).await.is_pending());

        // A correct cleanup retains the session guard until its registry
        // decision. The old implementation lets this join finish, drops its
        // temporary Arc, then removes the now-active session at strong_count2.
        let joined = match poll_once(joining.as_mut()).await {
            std::task::Poll::Ready(result) => Some(result.unwrap()),
            std::task::Poll::Pending => None,
        };
        drop(held);
        drop(registry_guard);
        cleanup.await;
        let _new_receiver = match joined {
            Some(receiver) => receiver,
            None => joining.await.unwrap(),
        };
        assert_eq!(
            manager.active_session_count().await,
            1,
            "cleanup detached a rejoined active session"
        );
        let live = weak
            .upgrade()
            .expect("the rejoined session's original owner must survive");
        let registered = manager.session(document).await;
        assert!(Arc::ptr_eq(&live, &registered));
        assert!(live.lock().await.participants.contains_key(&user));
    }

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
    async fn last_connection_releases_session_history_and_undo_ownership() {
        let manager = CollabManager::new();
        let document = Uuid::new_v4();
        let user = Uuid::new_v4();
        let client = Uuid::new_v4();
        // Keep both receivers alive through retirement: neither may retain the
        // CollabSession itself after its last connection leaves.
        let receiver_a = manager
            .join_session(document, user, "A".into())
            .await
            .unwrap();
        let receiver_b = manager
            .join_session(document, user, "A again".into())
            .await
            .unwrap();
        let weak = {
            let sessions = manager.sessions.read().await;
            Arc::downgrade(sessions.get(&document).unwrap())
        };
        for index in 0..2 {
            let operation = Operation::new(
                user,
                client,
                10 + index,
                OpType::Insert {
                    path: vec!["body".into()],
                    index: index as usize * 8192,
                    content: if index == 0 { "a" } else { "b" }.repeat(8192),
                },
            )
            .with_base_version(index);
            manager.apply_operation(document, operation).await.unwrap();
        }
        // undo derives its id counter from version2; author counters10/11 keep
        // that real inverse operation distinct from both accepted operations.
        assert_eq!(manager.undo(document, user).await.unwrap(), 3);
        manager.leave_session(document, user).await;
        assert_eq!(manager.active_session_count().await, 1);
        {
            let live = weak
                .upgrade()
                .expect("the second connection still owns the session");
            let session = live.lock().await;
            assert!(session.participants.contains_key(&user));
            assert_eq!(session.version, 3);
            assert_eq!(session.operations.len(), 3);
            assert_eq!(session.undo_stacks[&user].len(), 1);
            assert_eq!(session.redo_stacks[&user].len(), 1);
            assert!(matches!(&session.undo_stacks[&user][0].op_type,
                OpType::Insert { content, .. } if content == &"a".repeat(8192)));
            assert!(matches!(&session.redo_stacks[&user][0].op_type,
                OpType::Insert { content, .. } if content == &"b".repeat(8192)));
        }
        // No upgraded Arc survives this call. Registry removal must release
        // the session and its history/undo/redo owners, not just hide its id.
        manager.leave_session(document, user).await;
        assert_eq!(manager.active_session_count().await, 0);
        assert_eq!(manager.session_version(document).await, None);
        assert!(weak.upgrade().is_none());
        // Receivers can temporarily retain broadcast payload clones; dropping
        // those is separate from the session ownership assertion above.
        drop((receiver_a, receiver_b));
    }

    #[tokio::test]
    async fn document_gate_registry_does_not_retain_the_last_callers_lock() {
        let manager = CollabManager::new();
        let document = Uuid::new_v4();
        let gate = manager.document_gate(document).await;
        let another_caller = manager.document_gate(document).await;
        assert!(Arc::ptr_eq(&gate, &another_caller));
        let weak = Arc::downgrade(&gate);
        drop(gate);
        assert!(weak.upgrade().is_some());
        drop(another_caller);
        assert!(weak.upgrade().is_none());
        // The map may retain a Weak entry; it must not own the lock's payload.
        assert_eq!(manager.gates.lock().await[&document].strong_count(), 0);
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
