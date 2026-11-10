use chrono::Utc;
use diesel::prelude::*;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{File, FileVersion, NewFileVersion};
use contrix_core::schema::file_versions::dsl as fv;
use contrix_core::schema::files::dsl as f;

use crate::middleware::auth::get_user_id;

// ---------------------------------------------------------------------------
// GET /files/:file_id/versions
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_versions(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    match fv::file_versions
        .filter(fv::file_id.eq(&file_id_param))
        .order(fv::version_number.desc())
        .load::<FileVersion>(&mut conn)
        .await
    {
        Ok(rows) => {
            let versions: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|v| {
                    serde_json::json!({
                        "id": v.id,
                        "file_id": v.file_id,
                        "version_number": v.version_number,
                        "path": v.path,
                        "size": v.size,
                        "created_by": v.created_by,
                        "created_at": v.created_at,
                    })
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(versions)));
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
// POST /files/:file_id/versions
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_version(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let result = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                let file = diesel::RunQueryDsl::first::<File>(f::files.find(&file_id_param), conn)?;
                crate::services::persistence::authorize(conn, &file, &uid, "editor")?;
                if file.is_folder {
                    return Err(AppError::Validation("Cannot snapshot a folder".into()));
                }
                let max: Option<i32> = diesel::RunQueryDsl::first(
                    fv::file_versions
                        .filter(fv::file_id.eq(&file_id_param))
                        .select(diesel::dsl::max(fv::version_number)),
                    conn,
                )?;
                let next = max
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or_else(|| AppError::Conflict("Version exhausted".into()))?;
                let version = NewFileVersion {
                    id: Uuid::new_v4().to_string(),
                    file_id: file_id_param,
                    version_number: next,
                    path: file.path,
                    size: file.size,
                    created_by: uid,
                    created_at: Utc::now().to_rfc3339(),
                };
                diesel::RunQueryDsl::execute(
                    diesel::insert_into(fv::file_versions).values(&version),
                    conn,
                )?;
                Ok(diesel::RunQueryDsl::first::<FileVersion>(
                    fv::file_versions.find(version.id),
                    conn,
                )?)
            })
        })
        .await;
    match result {
        Ok(version) => {
            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(version)));
        }
        Err(error) => {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
        }
    }
}
