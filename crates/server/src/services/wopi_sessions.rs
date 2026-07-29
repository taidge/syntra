use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone)]
struct WopiSession {
    user_id: String,
    file_id: String,
    expires_at: DateTime<Utc>,
}

/// In-memory short-lived WOPI session token store.
pub struct WopiSessionStore {
    sessions: Mutex<HashMap<String, WopiSession>>,
}

impl Default for WopiSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl WopiSessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn create(&self, user_id: &str, file_id: &str, ttl: Duration) -> String {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let session = WopiSession {
            user_id: user_id.to_string(),
            file_id: file_id.to_string(),
            expires_at: Utc::now() + ttl,
        };
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        retain_active(&mut sessions);
        sessions.insert(hash_token(&token), session);
        token
    }

    pub fn validate(&self, token: &str, file_id: &str) -> Option<String> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        retain_active(&mut sessions);
        sessions.get(&hash_token(token)).and_then(|session| {
            if session.file_id == file_id {
                Some(session.user_id.clone())
            } else {
                None
            }
        })
    }
}

fn retain_active(sessions: &mut HashMap<String, WopiSession>) {
    let now = Utc::now();
    sessions.retain(|_, session| session.expires_at > now);
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_token_for_bound_file() {
        let store = WopiSessionStore::new();
        let token = store.create("user-1", "file-1", Duration::minutes(5));

        assert_eq!(store.validate(&token, "file-1").as_deref(), Some("user-1"));
        assert_eq!(store.validate(&token, "file-2"), None);
    }

    #[test]
    fn rejects_expired_token() {
        let store = WopiSessionStore::new();
        let token = store.create("user-1", "file-1", Duration::seconds(-1));

        assert_eq!(store.validate(&token, "file-1"), None);
    }
}
