use std::sync::Arc;

use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{FileProperty, NewFileProperty};
use contrix_core::schema::file_properties::dsl as fp;
use contrix_core::schema::files::dsl as f;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /files/:file_id/properties
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_properties(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();

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

    // Verify file ownership.
    let file_exists: bool = f::files
        .filter(f::id.eq(&file_id_param))
        .filter(f::owner_id.eq(&uid))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0)
        > 0;

    if !file_exists {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("File not found".into()).to_response(),
        )));
        return;
    }

    match fp::file_properties
        .filter(fp::file_id.eq(&file_id_param))
        .load::<FileProperty>(&mut conn)
    {
        Ok(rows) => {
            let props: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|row| {
                    serde_json::json!({
                        "key": row.prop_key,
                        "value": row.prop_value,
                    })
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(props)));
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
// PUT /files/:file_id/properties/:key
// ---------------------------------------------------------------------------

#[handler]
pub async fn set_property(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();
    let key_param: String = req.param("key").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct SetPropertyRequest {
        value: String,
    }

    let body: SetPropertyRequest = match req.parse_json().await {
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

    // Verify file ownership.
    let file_exists: bool = f::files
        .filter(f::id.eq(&file_id_param))
        .filter(f::owner_id.eq(&uid))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0)
        > 0;

    if !file_exists {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("File not found".into()).to_response(),
        )));
        return;
    }

    // Try to update existing property first (upsert).
    let updated = diesel::update(
        fp::file_properties
            .filter(fp::file_id.eq(&file_id_param))
            .filter(fp::prop_key.eq(&key_param)),
    )
    .set(fp::prop_value.eq(&body.value))
    .execute(&mut conn);

    match updated {
        Ok(0) => {
            // Insert new property.
            let new_prop = NewFileProperty {
                id: Uuid::new_v4().to_string(),
                file_id: file_id_param.clone(),
                prop_key: key_param.clone(),
                prop_value: body.value.clone(),
            };

            match diesel::insert_into(fp::file_properties)
                .values(&new_prop)
                .execute(&mut conn)
            {
                Ok(_) => {
                    res.status_code(StatusCode::OK);
                    res.render(Json(ApiResponse::ok(serde_json::json!({
                        "key": key_param,
                        "value": body.value,
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
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "key": key_param,
                "value": body.value,
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

// ---------------------------------------------------------------------------
// DELETE /files/:file_id/properties/:key
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_property(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let file_id_param: String = req.param("id").unwrap_or_default();
    let key_param: String = req.param("key").unwrap_or_default();

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

    // Verify file ownership.
    let file_exists: bool = f::files
        .filter(f::id.eq(&file_id_param))
        .filter(f::owner_id.eq(&uid))
        .count()
        .get_result::<i64>(&mut conn)
        .unwrap_or(0)
        > 0;

    if !file_exists {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("File not found".into()).to_response(),
        )));
        return;
    }

    match diesel::delete(
        fp::file_properties
            .filter(fp::file_id.eq(&file_id_param))
            .filter(fp::prop_key.eq(&key_param)),
    )
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Property not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Property deleted" }),
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
