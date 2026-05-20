use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{File, FileVersion, NewFileVersion};
use contrix_core::schema::file_versions::dsl as fv;
use contrix_core::schema::files::dsl as f;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /files/:file_id/versions
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_versions(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    // Verify the file exists and belongs to the user.
    match f::files
        .filter(f::id.eq(&file_id_param))
        .filter(f::owner_id.eq(&uid))
        .first::<File>(&mut conn)
        .optional()
    {
        Ok(Some(_)) => {}
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
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
    }

    match fv::file_versions
        .filter(fv::file_id.eq(&file_id_param))
        .order(fv::version_number.desc())
        .load::<FileVersion>(&mut conn)
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

    // Fetch the file (must belong to user and not be a folder).
    let file_row = match f::files
        .filter(f::id.eq(&file_id_param))
        .filter(f::owner_id.eq(&uid))
        .filter(f::is_folder.eq(false))
        .first::<File>(&mut conn)
        .optional()
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("File not found".into()).to_response(),
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
    };

    // Determine the next version number.
    let max_version: Option<i32> = fv::file_versions
        .filter(fv::file_id.eq(&file_id_param))
        .select(diesel::dsl::max(fv::version_number))
        .first::<Option<i32>>(&mut conn)
        .unwrap_or(None);

    let next_version = max_version.unwrap_or(0) + 1;
    let now = Utc::now();

    let new_version = NewFileVersion {
        id: Uuid::new_v4().to_string(),
        file_id: file_id_param.clone(),
        version_number: next_version,
        path: file_row.path.clone(),
        size: file_row.size,
        created_by: uid.clone(),
        created_at: now.to_rfc3339(),
    };

    match diesel::insert_into(fv::file_versions)
        .values(&new_version)
        .execute(&mut conn)
    {
        Ok(_) => {
            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "id": new_version.id,
                "file_id": new_version.file_id,
                "version_number": new_version.version_number,
                "path": new_version.path,
                "size": new_version.size,
                "created_by": new_version.created_by,
                "created_at": new_version.created_at,
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
