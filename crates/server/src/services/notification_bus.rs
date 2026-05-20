use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast};

const CHANNEL_CAPACITY: usize = 32;

pub struct NotificationBus {
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<String>>>>,
}

impl NotificationBus {
    pub fn new() -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn subscribe(&self, user_id: &str) -> broadcast::Receiver<String> {
        let mut channels = self.channels.write().await;
        let sender = channels
            .entry(user_id.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0);
        sender.subscribe()
    }

    pub async fn publish(&self, user_id: &str, json_payload: &str) {
        let channels = self.channels.read().await;
        if let Some(sender) = channels.get(user_id) {
            let _ = sender.send(json_payload.to_string());
        }
    }
}

impl Default for NotificationBus {
    fn default() -> Self {
        Self::new()
    }
}
