use chrono::Utc;
use diesel::SqliteConnection;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, Pool};
use uuid::Uuid;

use contrix_core::models::NewNotification;
use contrix_core::schema::notifications::dsl as n;

use crate::services::NotificationBus;

pub struct NotificationService;

impl NotificationService {
    pub async fn create(
        pool: &Pool<ConnectionManager<SqliteConnection>>,
        bus: &NotificationBus,
        user_id: &str,
        notification_type: &str,
        title: &str,
        message: &str,
        link: Option<&str>,
    ) {
        let mut conn = match pool.get() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("NotificationService: failed to get db connection: {e}");
                return;
            }
        };

        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now().to_rfc3339();
        let link_owned = link.map(|s| s.to_string());

        let row = NewNotification {
            id: id.clone(),
            user_id: user_id.to_string(),
            notification_type: notification_type.to_string(),
            title: title.to_string(),
            message: Some(message.to_string()),
            link: link_owned.clone(),
            is_read: false,
            created_at: created_at.clone(),
        };

        if let Err(e) = diesel::insert_into(n::notifications)
            .values(&row)
            .execute(&mut conn)
        {
            tracing::error!("NotificationService: failed to insert notification: {e}");
            return;
        }

        let payload = serde_json::json!({
            "id": id,
            "notification_type": notification_type,
            "title": title,
            "message": message,
            "link": link_owned,
            "is_read": false,
            "created_at": created_at,
        });

        let payload_str = match serde_json::to_string(&payload) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("NotificationService: failed to serialize payload: {e}");
                return;
            }
        };

        bus.publish(user_id, &payload_str).await;
    }
}
