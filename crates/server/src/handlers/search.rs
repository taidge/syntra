use chrono::Utc;
use diesel::prelude::*;
use diesel::sql_types::{Bool, Text};
use horme::RunQueryDsl;
use salvo::prelude::*;

use contrix_core::dto::{ApiResponse, SearchResponse, SearchResult};
use contrix_core::error::AppError;
use contrix_core::models::DocumentType;
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;

use crate::middleware::auth::get_user_id;

// ---------------------------------------------------------------------------
// GET /search
// ---------------------------------------------------------------------------

#[handler]
pub async fn search(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let q: String = req.query("q").unwrap_or_default();
    let q = q.trim();
    let doc_type_filter: Option<String> = req.query("document_type");
    let page: u32 = req.query("page").unwrap_or(1);
    let per_page: u32 = req.query::<u32>("per_page").unwrap_or(20).clamp(1, 100);

    if q.is_empty() {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Query parameter 'q' is required".into()).to_response(),
        )));
        return;
    }
    if q.chars().count() > 500 {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Search query must not exceed 500 characters".into())
                .to_response(),
        )));
        return;
    }

    let like_pattern = literal_like_pattern(q);
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
        let count_matches = diesel::dsl::sql::<Bool>("(files.name LIKE ")
            .bind::<Text, _>(&like_pattern)
            .sql(" ESCAPE '\\' OR COALESCE(documents.content, '') LIKE ")
            .bind::<Text, _>(&like_pattern)
            .sql(" ESCAPE '\\')");
        let count_val: i64 = d::documents
            .inner_join(f::files.on(d::file_id.eq(f::id)))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_trashed.eq(false))
            .filter(d::doc_type.eq(dt))
            .filter(count_matches)
            .count()
            .get_result(&mut conn)
            .await
            .unwrap_or(0);

        let row_matches = diesel::dsl::sql::<Bool>("(files.name LIKE ")
            .bind::<Text, _>(&like_pattern)
            .sql(" ESCAPE '\\' OR COALESCE(documents.content, '') LIKE ")
            .bind::<Text, _>(&like_pattern)
            .sql(" ESCAPE '\\')");
        let results = d::documents
            .inner_join(f::files.on(d::file_id.eq(f::id)))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_trashed.eq(false))
            .filter(d::doc_type.eq(dt))
            .filter(row_matches)
            .order(d::updated_at.desc())
            .limit(limit_val)
            .offset(offset_val)
            .select((d::id, f::name, d::doc_type, d::content, d::updated_at))
            .load::<(String, String, String, Option<String>, String)>(&mut conn)
            .await;

        (results, count_val as u64)
    } else {
        let count_matches = diesel::dsl::sql::<Bool>("(files.name LIKE ")
            .bind::<Text, _>(&like_pattern)
            .sql(" ESCAPE '\\' OR COALESCE(documents.content, '') LIKE ")
            .bind::<Text, _>(&like_pattern)
            .sql(" ESCAPE '\\')");
        let count_val: i64 = d::documents
            .inner_join(f::files.on(d::file_id.eq(f::id)))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_trashed.eq(false))
            .filter(count_matches)
            .count()
            .get_result(&mut conn)
            .await
            .unwrap_or(0);

        let row_matches = diesel::dsl::sql::<Bool>("(files.name LIKE ")
            .bind::<Text, _>(&like_pattern)
            .sql(" ESCAPE '\\' OR COALESCE(documents.content, '') LIKE ")
            .bind::<Text, _>(&like_pattern)
            .sql(" ESCAPE '\\')");
        let results = d::documents
            .inner_join(f::files.on(d::file_id.eq(f::id)))
            .filter(f::owner_id.eq(&uid))
            .filter(f::is_trashed.eq(false))
            .filter(row_matches)
            .order(d::updated_at.desc())
            .limit(limit_val)
            .offset(offset_val)
            .select((d::id, f::name, d::doc_type, d::content, d::updated_at))
            .load::<(String, String, String, Option<String>, String)>(&mut conn)
            .await;

        (results, count_val as u64)
    };

    match rows {
        Ok(rows) => {
            let results: Vec<SearchResult> = rows
                .into_iter()
                .map(
                    |(id_val, name_val, doc_type_str, content, updated_val)| SearchResult {
                        id: id_val.parse().unwrap_or_default(),
                        name: name_val.clone(),
                        document_type: DocumentType::parse_lossy(&doc_type_str),
                        snippet: search_snippet(&name_val, content.as_deref(), q),
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

fn search_snippet(name: &str, content: Option<&str>, query: &str) -> String {
    if name.to_lowercase().contains(&query.to_lowercase()) {
        return name.to_string();
    }
    let Some(content) = content else {
        return name.to_string();
    };
    let byte_index = content.find(query).or_else(|| {
        query
            .is_ascii()
            .then(|| {
                content
                    .to_ascii_lowercase()
                    .find(&query.to_ascii_lowercase())
            })
            .flatten()
    });
    let Some(byte_index) = byte_index else {
        return name.to_string();
    };
    let start = content[..byte_index]
        .char_indices()
        .rev()
        .nth(40)
        .map_or(0, |(index, _)| index);
    let end = content[byte_index..]
        .char_indices()
        .nth(120)
        .map_or(content.len(), |(index, _)| byte_index + index);
    let excerpt = &content[start..end];
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        excerpt,
        if end < content.len() { "…" } else { "" }
    )
}

fn literal_like_pattern(query: &str) -> String {
    let mut pattern = String::with_capacity(query.len() + 2);
    pattern.push('%');
    for character in query.chars() {
        if matches!(character, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}

#[cfg(test)]
mod tests {
    use super::{literal_like_pattern, search_snippet};

    #[test]
    fn snippets_use_content_matches_without_breaking_utf8() {
        let content = format!("{}关键字{}", "前".repeat(80), "后".repeat(160));
        let snippet = search_snippet("Untitled", Some(&content), "关键字");
        assert!(snippet.contains("关键字"));
        assert!(snippet.starts_with('…'));
        assert!(snippet.ends_with('…'));
    }

    #[test]
    fn like_wildcards_are_escaped_as_literal_text() {
        assert_eq!(
            literal_like_pattern(r"50%_off\today"),
            r"%50\%\_off\\today%"
        );
    }
}
