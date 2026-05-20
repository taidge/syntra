use std::sync::Arc;

use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{NewNotificationPreference, NotificationPreference};
use contrix_core::schema::notification_preferences::dsl as np;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Default event types
// ---------------------------------------------------------------------------

/// All recognised event types. If the user has no row for an event type,
/// it defaults to enabled.
const DEFAULT_EVENT_TYPES: &[&str] = &[
    "share",
    "comment",
    "mention",
    "system",
    "file_update",
    "room_invite",
];

// ---------------------------------------------------------------------------
// GET /notification-preferences
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_preferences(depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    match np::notification_preferences
        .filter(np::user_id.eq(&uid))
        .load::<NotificationPreference>(&mut conn)
    {
        Ok(rows) => {
            // Build a complete list: stored prefs + defaults for missing types.
            let mut prefs: Vec<serde_json::Value> = Vec::new();
            for event_type in DEFAULT_EVENT_TYPES {
                let enabled = rows
                    .iter()
                    .find(|r| r.event_type == *event_type)
                    .map(|r| r.enabled)
                    .unwrap_or(true); // default enabled
                prefs.push(serde_json::json!({
                    "event_type": event_type,
                    "enabled": enabled,
                }));
            }
            // Also include any stored prefs for non-default event types.
            for row in &rows {
                if !DEFAULT_EVENT_TYPES.contains(&row.event_type.as_str()) {
                    prefs.push(serde_json::json!({
                        "event_type": row.event_type,
                        "enabled": row.enabled,
                    }));
                }
            }
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(prefs)));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}

// ---------------------------------------------------------------------------
// PUT /notification-preferences/:event_type
// ---------------------------------------------------------------------------

#[handler]
pub async fn set_preference(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let event_type_param: String = req.param("event_type").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct SetPrefRequest {
        enabled: bool,
    }

    let body: SetPrefRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let mut conn = match state.db.get() {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    // Check if preference already exists (upsert).
    let existing = np::notification_preferences
        .filter(np::user_id.eq(&uid))
        .filter(np::event_type.eq(&event_type_param))
        .first::<NotificationPreference>(&mut conn)
        .optional();

    match existing {
        Ok(Some(_)) => {
            // Update existing preference.
            match diesel::update(
                np::notification_preferences
                    .filter(np::user_id.eq(&uid))
                    .filter(np::event_type.eq(&event_type_param)),
            )
            .set(np::enabled.eq(body.enabled))
            .execute(&mut conn)
            {
                Ok(_) => {
                    res.status_code(StatusCode::OK);
                    res.render(Json(ApiResponse::ok(serde_json::json!({
                        "event_type": event_type_param,
                        "enabled": body.enabled,
                    }))));
                }
                Err(e) => {
                    res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                    res.render(Json(ApiResponse::<()>::err(
                        AppError::Database(e.to_string()).to_response(),
                    )));
                }
            }
        }
        Ok(None) => {
            // Insert new preference.
            let new_pref = NewNotificationPreference {
                id: Uuid::new_v4().to_string(),
                user_id: uid.clone(),
                event_type: event_type_param.clone(),
                enabled: body.enabled,
            };
            match diesel::insert_into(np::notification_preferences)
                .values(&new_pref)
                .execute(&mut conn)
            {
                Ok(_) => {
                    res.status_code(StatusCode::OK);
                    res.render(Json(ApiResponse::ok(serde_json::json!({
                        "event_type": event_type_param,
                        "enabled": body.enabled,
                    }))));
                }
                Err(e) => {
                    res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
                    res.render(Json(ApiResponse::<()>::err(
                        AppError::Database(e.to_string()).to_response(),
                    )));
                }
            }
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
