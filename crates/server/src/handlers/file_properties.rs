use diesel::prelude::*;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use syntra_core::dto::ApiResponse;
use syntra_core::error::AppError;
use syntra_core::models::{FileProperty, NewFileProperty};
use syntra_core::schema::file_properties::dsl as fp;
use syntra_core::schema::files::dsl as f;

use crate::middleware::auth::get_user_id;

// ---------------------------------------------------------------------------
// GET /files/:file_id/properties
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_properties(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
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

    match crate::handlers::access::load_accessible_file(&mut conn, &file_id_param, &uid, "viewer")
        .await
    {
        Ok(Some(file)) if !file.is_trashed => {}
        Ok(_) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
            )));
            return;
        }
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
            return;
        }
    }

    match fp::file_properties
        .filter(fp::file_id.eq(&file_id_param))
        .load::<FileProperty>(&mut conn)
        .await
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
    let state = crate::require_app_state!(depot, res);
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

    let response_key = key_param.clone();
    let response_value = body.value.clone();
    let result = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                let file = diesel::RunQueryDsl::first::<syntra_core::models::File>(
                    f::files.find(&file_id_param),
                    conn,
                )?;
                crate::services::persistence::authorize(conn, &file, &uid, "editor")?;
                let property = NewFileProperty {
                    id: Uuid::new_v4().to_string(),
                    file_id: file_id_param,
                    prop_key: key_param,
                    prop_value: body.value,
                };
                diesel::RunQueryDsl::execute(
                    diesel::insert_into(fp::file_properties)
                        .values(&property)
                        .on_conflict((fp::file_id, fp::prop_key))
                        .do_update()
                        .set(fp::prop_value.eq(&property.prop_value)),
                    conn,
                )?;
                Ok(())
            })
        })
        .await;
    match result {
        Ok(()) => res.render(Json(ApiResponse::ok(
            serde_json::json!({"key":response_key,"value":response_value}),
        ))),
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
        }
    }
}

// ---------------------------------------------------------------------------
// DELETE /files/:file_id/properties/:key
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_property(req: &mut Request, depot: &mut Depot, res: &mut Response) {
    let state = crate::require_app_state!(depot, res);
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

    let result = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                let file = diesel::RunQueryDsl::first::<syntra_core::models::File>(
                    f::files.find(&file_id_param),
                    conn,
                )?;
                crate::services::persistence::authorize(conn, &file, &uid, "editor")?;
                let count = diesel::RunQueryDsl::execute(
                    diesel::delete(
                        fp::file_properties
                            .filter(fp::file_id.eq(&file_id_param))
                            .filter(fp::prop_key.eq(key_param)),
                    ),
                    conn,
                )?;
                if count == 0 {
                    return Err(AppError::NotFound("Property not found".into()));
                }
                Ok(())
            })
        })
        .await;
    match result {
        Ok(()) => res.render(Json(ApiResponse::ok(
            serde_json::json!({"message":"Property deleted"}),
        ))),
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
        }
    }
}
