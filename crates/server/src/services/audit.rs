use chrono::Utc;
use diesel::SqliteConnection;
use diesel::prelude::*;
use uuid::Uuid;

use contrix_core::models::NewActivityLog;
use contrix_core::schema::activity_logs::dsl as al;

pub struct AuditService;

impl AuditService {
    /// Log an activity to the `activity_logs` table.
    ///
    /// `ip_address` and `user_agent` are optional enhanced-audit fields that, when
    /// present, enrich the log entry with request context.
    pub fn log(
        conn: &mut SqliteConnection,
        user_id: &str,
        action: &str,
        resource_type: &str,
        resource_id: &str,
        details: Option<&str>,
        ip_address: Option<&str>,
        user_agent: Option<&str>,
    ) {
        let entry = NewActivityLog {
            id: Uuid::new_v4().to_string(),
            user_id: user_id.to_string(),
            action: action.to_string(),
            resource_type: resource_type.to_string(),
            resource_id: resource_id.to_string(),
            details: details.map(|s| s.to_string()),
            created_at: Utc::now().to_rfc3339(),
            ip_address: ip_address.map(|s| s.to_string()),
            user_agent: user_agent.map(|s| s.to_string()),
        };

        if let Err(e) = diesel::insert_into(al::activity_logs)
            .values(&entry)
            .execute(conn)
        {
            tracing::error!("AuditService: failed to insert activity log: {e}");
        }
    }
}
