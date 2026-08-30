use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use contrix_core::dto::ApiResponse;
use contrix_core::error::AppError;
use contrix_core::models::{DocumentSignature, NewDocumentSignature};
use contrix_core::schema::document_signatures::dsl as ds;
use contrix_core::schema::documents::dsl as d;

use crate::handlers::access::load_accessible_document;
use crate::middleware::auth::get_user_id;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a document id and authorize the caller for `required_role`.
///
/// Returns `Ok(None)` when the document does not exist *or* the caller is not
/// permitted to access it (forbidden is folded into not-found so existence is
/// not leaked). Only genuine database errors propagate as `Err`.
fn resolve_document_id(
    conn: &mut diesel::SqliteConnection,
    identifier: &str,
    user_id: &str,
    required_role: &str,
) -> Result<Option<String>, AppError> {
    match load_accessible_document(conn, identifier, user_id, required_role) {
        Ok(Some(_)) => Ok(Some(identifier.to_string())),
        Ok(None) | Err(AppError::Forbidden(_)) | Err(AppError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn compute_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// POST /documents/:id/sign
// ---------------------------------------------------------------------------

#[handler]
pub async fn sign_document(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct SignBody {
        signature_data: String,
    }

    let body: SignBody = match req.parse_json().await {
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

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param, &uid, "editor") {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    // Fetch the current document content.
    let content_opt: Option<String> = d::documents
        .filter(d::id.eq(&resolved_doc_id))
        .select(d::content)
        .first::<Option<String>>(&mut conn)
        .unwrap_or(None);

    let content_str = content_opt.unwrap_or_default();
    let signed_hash = compute_content_hash(&content_str);

    let sig_id = Uuid::new_v4();
    let now = Utc::now();

    let new_sig = NewDocumentSignature {
        id: sig_id.to_string(),
        document_id: resolved_doc_id.clone(),
        user_id: uid.clone(),
        signature_data: body.signature_data,
        signed_hash: signed_hash.clone(),
        signed_at: now.to_rfc3339(),
        is_valid: true,
    };

    match diesel::insert_into(ds::document_signatures)
        .values(&new_sig)
        .execute(&mut conn)
    {
        Ok(_) => {
            res.status_code(StatusCode::CREATED);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "id": sig_id.to_string(),
                "document_id": resolved_doc_id,
                "user_id": uid,
                "signed_hash": signed_hash,
                "signed_at": now.to_rfc3339(),
                "is_valid": true,
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
// GET /documents/:id/signatures
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_signatures(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

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

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param, &uid, "viewer") {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    match ds::document_signatures
        .filter(ds::document_id.eq(&resolved_doc_id))
        .order(ds::signed_at.desc())
        .load::<DocumentSignature>(&mut conn)
    {
        Ok(rows) => {
            let sigs: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|sig| {
                    serde_json::json!({
                        "id": sig.id,
                        "document_id": sig.document_id,
                        "user_id": sig.user_id,
                        "signature_data": sig.signature_data,
                        "signed_hash": sig.signed_hash,
                        "signed_at": sig.signed_at,
                        "is_valid": sig.is_valid,
                    })
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(sigs)));
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
// GET /documents/:id/signatures/verify
// ---------------------------------------------------------------------------

#[handler]
pub async fn verify_signatures(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let doc_id_param: String = req.param("id").unwrap_or_default();

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

    let resolved_doc_id = match resolve_document_id(&mut conn, &doc_id_param, &uid, "viewer") {
        Ok(Some(id)) => id,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Document not found".into()).to_response(),
            )));
            return;
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
            return;
        }
    };

    // Fetch the current document content and compute its hash.
    let content_opt: Option<String> = d::documents
        .filter(d::id.eq(&resolved_doc_id))
        .select(d::content)
        .first::<Option<String>>(&mut conn)
        .unwrap_or(None);

    let content_str = content_opt.unwrap_or_default();
    let current_hash = compute_content_hash(&content_str);

    // Fetch all signatures.
    let signatures = ds::document_signatures
        .filter(ds::document_id.eq(&resolved_doc_id))
        .order(ds::signed_at.desc())
        .load::<DocumentSignature>(&mut conn)
        .unwrap_or_default();

    let results: Vec<serde_json::Value> = signatures
        .into_iter()
        .map(|sig| {
            let valid = sig.signed_hash == current_hash;
            serde_json::json!({
                "id": sig.id,
                "user_id": sig.user_id,
                "signed_at": sig.signed_at,
                "signed_hash": sig.signed_hash,
                "current_hash": current_hash,
                "is_valid": valid,
            })
        })
        .collect();

    res.status_code(StatusCode::OK);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "document_id": resolved_doc_id,
        "current_hash": current_hash,
        "signatures": results,
    }))));
}
