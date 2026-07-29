use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;

use contrix_core::dto::{ApiResponse, SearchResponse, SearchResult};
use contrix_core::error::AppError;
use contrix_core::models::DocumentType;
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// GET /search
// ---------------------------------------------------------------------------

#[handler]
pub async fn search(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let q: String = req.query("q").unwrap_or_default();
    let doc_type_filter: Option<String> = req.query("document_type");
    let page: u32 = req.query("page").unwrap_or(1);
    let per_page: u32 = req.query("per_page").unwrap_or(20);

    if q.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Query parameter 'q' is required".into()).to_response(),
        )));
        return;
    }

    // -------------------------------------------------------------------
    // Try Tantivy first, fall back to SQL LIKE
    // -------------------------------------------------------------------
    if let Some(ref search_svc) = state.search {
        match search_svc.search(&q, Some(&uid), 1000) {
            Ok(tantivy_results) => {
                let filtered: Vec<_> = tantivy_results
                    .into_iter()
                    .filter(|(_id, _name, dt, _score)| {
                        if let Some(ref filter) = doc_type_filter {
                            dt == filter
                        } else {
                            true
                        }
                    })
                    .collect();
                let total = filtered.len() as u64;
                let offset = ((page.saturating_sub(1)) * per_page) as usize;

                let results: Vec<SearchResult> = filtered
                    .into_iter()
                    .skip(offset)
                    .take(per_page as usize)
                    .map(|(id_val, name_val, doc_type_str, score)| SearchResult {
                        id: id_val.parse().unwrap_or_default(),
                        name: name_val.clone(),
                        document_type: DocumentType::parse_lossy(&doc_type_str),
                        snippet: name_val,
                        score,
                        updated_at: Utc::now(),
                    })
                    .collect();

                let resp = SearchResponse {
                    results,
                    total,
                    page,
                    per_page,
                };
                res.status_code(StatusCode::OK);
                res.render(Json(ApiResponse::ok(resp)));
                return;
            }
            Err(e) => {
                tracing::warn!("Tantivy search failed, falling back to SQL: {e}");
            }
        }
    }

    // -------------------------------------------------------------------
    // SQL LIKE fallback
    // -------------------------------------------------------------------
    let like_pattern = format!("%{q}%");
    let offset_val = ((page.saturating_sub(1)) * per_page) as i64;
    let limit_val = per_page as i64;

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

    let (rows, total) = if let Some(ref dt) = doc_type_filter {
        let count_val: i64 = d::documents
            .inner_join(f::files.on(d::file_id.eq(f::id)))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_trashed.eq(false))
            .filter(d::doc_type.eq(dt))
            .filter(f::name.like(&like_pattern))
            .count()
            .get_result(&mut conn)
            .unwrap_or(0);

        let results = d::documents
            .inner_join(f::files.on(d::file_id.eq(f::id)))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_trashed.eq(false))
            .filter(d::doc_type.eq(dt))
            .filter(f::name.like(&like_pattern))
            .order(d::updated_at.desc())
            .limit(limit_val)
            .offset(offset_val)
            .select((d::id, f::name, d::doc_type, d::updated_at))
            .load::<(String, String, String, String)>(&mut conn);

        (results, count_val as u64)
    } else {
        let count_val: i64 = d::documents
            .inner_join(f::files.on(d::file_id.eq(f::id)))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_trashed.eq(false))
            .filter(f::name.like(&like_pattern))
            .count()
            .get_result(&mut conn)
            .unwrap_or(0);

        let results = d::documents
            .inner_join(f::files.on(d::file_id.eq(f::id)))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_trashed.eq(false))
            .filter(f::name.like(&like_pattern))
            .order(d::updated_at.desc())
            .limit(limit_val)
            .offset(offset_val)
            .select((d::id, f::name, d::doc_type, d::updated_at))
            .load::<(String, String, String, String)>(&mut conn);

        (results, count_val as u64)
    };

    match rows {
        Ok(rows) => {
            let results: Vec<SearchResult> = rows
                .into_iter()
                .map(
                    |(id_val, name_val, doc_type_str, updated_val)| SearchResult {
                        id: id_val.parse().unwrap_or_default(),
                        name: name_val.clone(),
                        document_type: DocumentType::parse_lossy(&doc_type_str),
                        snippet: name_val,
                        score: 1.0,
                        updated_at: updated_val.parse().unwrap_or_else(|_| Utc::now()),
                    },
                )
                .collect();

            let resp = SearchResponse {
                results,
                total,
                page,
                per_page,
            };
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(resp)));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
