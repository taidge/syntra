use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{
    ApiResponse, CreateWebhookRequest, UpdateWebhookRequest, WebhookDeliveryResponse,
    WebhookResponse,
};
use contrix_core::error::AppError;
use contrix_core::models::{NewWebhook, Webhook, WebhookDelivery};
use contrix_core::schema::webhook_deliveries::dsl as wd;
use contrix_core::schema::webhooks::dsl as w;

use crate::middleware::auth::{get_user_id, get_user_role};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Verify the caller has admin role; returns `false` and writes a 403 if not.
fn require_admin(depot: &Depot, res: &mut Response) -> bool {
    match get_user_role(depot) {
        Some(role) if role == "admin" => true,
        _ => {
            res.status_code(StatusCode::FORBIDDEN);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Forbidden("Admin access required".into()).to_response(),
            )));
            false
        }
    }
}

/// Convert a comma-separated events string to a Vec<String>.
fn events_to_vec(events: &str) -> Vec<String> {
    events
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Convert a Vec<String> of events to a comma-separated string.
fn events_to_string(events: &[String]) -> String {
    events.join(",")
}

fn webhook_to_response(row: Webhook) -> WebhookResponse {
    WebhookResponse {
        id: row.id.parse().unwrap_or_default(),
        name: row.name,
        url: row.url,
        events: events_to_vec(&row.events),
        is_active: row.is_active,
        created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
        updated_at: row.updated_at.parse().unwrap_or_else(|_| Utc::now()),
    }
}

fn delivery_to_response(row: WebhookDelivery) -> WebhookDeliveryResponse {
    WebhookDeliveryResponse {
        id: row.id.parse().unwrap_or_default(),
        event: row.event,
        response_status: row.response_status,
        success: row.success,
        delivered_at: row.delivered_at.parse().unwrap_or_else(|_| Utc::now()),
    }
}

// ---------------------------------------------------------------------------
// POST /webhooks
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_webhook(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

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

    let body: CreateWebhookRequest = match req.parse_json().await {
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

    let webhook_id = Uuid::new_v4();
    let now = Utc::now();
    let events_str = events_to_string(&body.events);

    let new_webhook = NewWebhook {
        id: webhook_id.to_string(),
        name: body.name.clone(),
        url: body.url.clone(),
        secret: body.secret.clone(),
        events: events_str,
        is_active: true,
        created_by: uid.clone(),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };

    if let Err(e) = diesel::insert_into(w::webhooks)
        .values(&new_webhook)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    let resp = WebhookResponse {
        id: webhook_id,
        name: body.name,
        url: body.url,
        events: body.events,
        is_active: true,
        created_at: now,
        updated_at: now,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(resp)));
}

// ---------------------------------------------------------------------------
// GET /webhooks
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_webhooks(depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let _uid = match get_user_id(depot) {
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

    match w::webhooks
        .order(w::created_at.desc())
        .load::<Webhook>(&mut conn)
    {
        Ok(rows) => {
            let webhook_list: Vec<WebhookResponse> =
                rows.into_iter().map(webhook_to_response).collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(webhook_list)));
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
// GET /webhooks/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_webhook(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let _uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let webhook_id_param: String = req.param("id").unwrap_or_default();

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

    match w::webhooks
        .filter(w::id.eq(&webhook_id_param))
        .first::<Webhook>(&mut conn)
        .optional()
    {
        Ok(Some(row)) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(webhook_to_response(row))));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Webhook not found".into()).to_response(),
            )));
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
// PUT /webhooks/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_webhook(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let _uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let webhook_id_param: String = req.param("id").unwrap_or_default();

    let body: UpdateWebhookRequest = match req.parse_json().await {
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

    // Verify the webhook exists.
    let existing = w::webhooks
        .filter(w::id.eq(&webhook_id_param))
        .first::<Webhook>(&mut conn)
        .optional();

    match existing {
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Webhook not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
        Ok(Some(_)) => {}
    }

    let now = Utc::now();

    // Apply updates field by field.
    if let Some(ref name) = body.name {
        let _ = diesel::update(w::webhooks.filter(w::id.eq(&webhook_id_param)))
            .set(w::name.eq(name))
            .execute(&mut conn);
    }
    if let Some(ref url) = body.url {
        let _ = diesel::update(w::webhooks.filter(w::id.eq(&webhook_id_param)))
            .set(w::url.eq(url))
            .execute(&mut conn);
    }
    if let Some(ref events) = body.events {
        let events_str = events_to_string(events);
        let _ = diesel::update(w::webhooks.filter(w::id.eq(&webhook_id_param)))
            .set(w::events.eq(events_str))
            .execute(&mut conn);
    }
    if let Some(is_active) = body.is_active {
        let _ = diesel::update(w::webhooks.filter(w::id.eq(&webhook_id_param)))
            .set(w::is_active.eq(is_active))
            .execute(&mut conn);
    }

    // Always update the updated_at timestamp.
    let _ = diesel::update(w::webhooks.filter(w::id.eq(&webhook_id_param)))
        .set(w::updated_at.eq(now.to_rfc3339()))
        .execute(&mut conn);

    // Return the updated webhook.
    match w::webhooks
        .filter(w::id.eq(&webhook_id_param))
        .first::<Webhook>(&mut conn)
    {
        Ok(row) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(webhook_to_response(row))));
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
// DELETE /webhooks/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_webhook(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    if !require_admin(depot, res) {
        return;
    }

    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();

    let webhook_id_param: String = req.param("id").unwrap_or_default();

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

    // Delete associated deliveries first.
    let _ = diesel::delete(wd::webhook_deliveries.filter(wd::webhook_id.eq(&webhook_id_param)))
        .execute(&mut conn);

    match diesel::delete(w::webhooks.filter(w::id.eq(&webhook_id_param))).execute(&mut conn) {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Webhook not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Webhook deleted" }),
            )));
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
// GET /webhooks/:id/deliveries
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_deliveries(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();
    let _uid = match get_user_id(depot) {
        Some(val) => val,
        None => {
            res.status_code(StatusCode::UNAUTHORIZED);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Auth("Not authenticated".into()).to_response(),
            )));
            return;
        }
    };

    let webhook_id_param: String = req.param("id").unwrap_or_default();

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

    // Verify the webhook exists.
    let count: i64 = match w::webhooks
        .filter(w::id.eq(&webhook_id_param))
        .count()
        .get_result(&mut conn)
    {
        Ok(c) => c,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    if count == 0 {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Webhook not found".into()).to_response(),
        )));
        return;
    }

    match wd::webhook_deliveries
        .filter(wd::webhook_id.eq(&webhook_id_param))
        .order(wd::delivered_at.desc())
        .limit(50)
        .load::<WebhookDelivery>(&mut conn)
    {
        Ok(rows) => {
            let delivery_list: Vec<WebhookDeliveryResponse> =
                rows.into_iter().map(delivery_to_response).collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(delivery_list)));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
