use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{
    ApiResponse, CreateTemplateRequest, DocumentResponse, TemplateResponse, UpdateTemplateRequest,
};
use contrix_core::error::AppError;
use contrix_core::models::{DocumentType, NewDocument, NewFile, NewTemplate, Template};
use contrix_core::schema::documents::dsl as d;
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::templates::dsl as tmpl;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn template_to_response(row: Template) -> TemplateResponse {
    let content: Option<serde_json::Value> = row
        .content
        .as_deref()
        .and_then(|v| serde_json::from_str(v).ok());

    TemplateResponse {
        id: row.id.parse().unwrap_or_default(),
        name: row.name,
        description: row.description,
        document_type: DocumentType::from_str(&row.doc_type),
        content,
        thumbnail_url: row.thumbnail_url,
        is_global: row.is_global,
        created_by: row.created_by.parse().unwrap_or_default(),
        created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
        updated_at: row.updated_at.parse().unwrap_or_else(|_| Utc::now()),
    }
}

fn mime_for_doc_type(dt: &DocumentType) -> &'static str {
    match dt {
        DocumentType::Writer => "application/vnd.contrix.writer",
        DocumentType::Calc => "application/vnd.contrix.calc",
        DocumentType::Slides => "application/vnd.contrix.slides",
        DocumentType::Whiteboard => "application/vnd.contrix.whiteboard",
    }
}

// ---------------------------------------------------------------------------
// POST /templates
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_template(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: CreateTemplateRequest = match req.parse_json().await {
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

    let template_id = Uuid::new_v4();
    let now = Utc::now();
    let content_str = body
        .content
        .as_ref()
        .and_then(|v| serde_json::to_string(v).ok());
    let is_global = body.is_global.unwrap_or(false);

    let new_template = NewTemplate {
        id: template_id.to_string(),
        name: body.name.clone(),
        description: body.description.clone(),
        doc_type: body.document_type.as_str().to_string(),
        content: content_str,
        thumbnail_url: None,
        is_global,
        created_by: uid.clone(),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };

    if let Err(e) = diesel::insert_into(tmpl::templates)
        .values(&new_template)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    let resp = TemplateResponse {
        id: template_id,
        name: body.name,
        description: body.description,
        document_type: body.document_type,
        content: body.content,
        thumbnail_url: None,
        is_global,
        created_by: uid.parse().unwrap_or_default(),
        created_at: now,
        updated_at: now,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(resp)));
}

// ---------------------------------------------------------------------------
// GET /templates
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_templates(depot: &mut Depot, res: &mut Response) {
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

    // Return user's own templates plus global templates.
    match tmpl::templates
        .filter(tmpl::created_by.eq(&uid).or(tmpl::is_global.eq(true)))
        .order(tmpl::created_at.desc())
        .load::<Template>(&mut conn)
    {
        Ok(rows) => {
            let template_list: Vec<TemplateResponse> =
                rows.into_iter().map(template_to_response).collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(template_list)));
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
// GET /templates/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_template(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let template_id_param: String = req.param("id").unwrap_or_default();

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

    // Allow access to own templates or global templates.
    match tmpl::templates
        .filter(tmpl::id.eq(&template_id_param))
        .filter(tmpl::created_by.eq(&uid).or(tmpl::is_global.eq(true)))
        .first::<Template>(&mut conn)
        .optional()
    {
        Ok(Some(row)) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(template_to_response(row))));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Template not found".into()).to_response(),
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
// PUT /templates/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_template(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let template_id_param: String = req.param("id").unwrap_or_default();

    let body: UpdateTemplateRequest = match req.parse_json().await {
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

    // Verify ownership.
    let existing = tmpl::templates
        .filter(tmpl::id.eq(&template_id_param))
        .filter(tmpl::created_by.eq(&uid))
        .first::<Template>(&mut conn)
        .optional();

    match existing {
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Template not found".into()).to_response(),
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

    if let Some(ref name) = body.name {
        let _ = diesel::update(tmpl::templates.filter(tmpl::id.eq(&template_id_param)))
            .set(tmpl::name.eq(name))
            .execute(&mut conn);
    }
    if let Some(ref description) = body.description {
        let _ = diesel::update(tmpl::templates.filter(tmpl::id.eq(&template_id_param)))
            .set(tmpl::description.eq(Some(description)))
            .execute(&mut conn);
    }
    if let Some(ref content) = body.content {
        let content_str = serde_json::to_string(content).unwrap_or_default();
        let _ = diesel::update(tmpl::templates.filter(tmpl::id.eq(&template_id_param)))
            .set(tmpl::content.eq(Some(content_str)))
            .execute(&mut conn);
    }

    // Always update updated_at.
    let _ = diesel::update(tmpl::templates.filter(tmpl::id.eq(&template_id_param)))
        .set(tmpl::updated_at.eq(now.to_rfc3339()))
        .execute(&mut conn);

    // Return the updated template.
    match tmpl::templates
        .filter(tmpl::id.eq(&template_id_param))
        .first::<Template>(&mut conn)
    {
        Ok(row) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(template_to_response(row))));
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
// DELETE /templates/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_template(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let template_id_param: String = req.param("id").unwrap_or_default();

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

    // Only the owner can delete the template.
    match diesel::delete(
        tmpl::templates
            .filter(tmpl::id.eq(&template_id_param))
            .filter(tmpl::created_by.eq(&uid)),
    )
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Template not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Template deleted" }),
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
// POST /templates/:id/create-document
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_document_from_template(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
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

    let template_id_param: String = req.param("id").unwrap_or_default();

    // Optionally accept a name and parent_id for the new document.
    #[derive(serde::Deserialize)]
    struct CreateFromTemplateRequest {
        name: Option<String>,
        parent_id: Option<String>,
    }

    let body: CreateFromTemplateRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(_) => CreateFromTemplateRequest {
            name: None,
            parent_id: None,
        },
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

    // Fetch the template (own or global).
    let template = match tmpl::templates
        .filter(tmpl::id.eq(&template_id_param))
        .filter(tmpl::created_by.eq(&uid).or(tmpl::is_global.eq(true)))
        .first::<Template>(&mut conn)
        .optional()
    {
        Ok(Some(t)) => t,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Template not found".into()).to_response(),
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

    // Validate the optional parent folder and inherit its room.
    let room_id_for_new_file = if let Some(parent_id) = body.parent_id.as_deref() {
        match crate::handlers::access::load_accessible_target_folder(
            &mut conn, parent_id, &uid, "editor",
        ) {
            Ok(Some(parent)) => parent.room_id,
            Ok(None) => {
                res.status_code(StatusCode::NOT_FOUND);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::NotFound("Parent folder not found".into()).to_response(),
                )));
                return;
            }
            Err(e) => {
                res.status_code(StatusCode::FORBIDDEN);
                res.render(Json(ApiResponse::<()>::err(e.to_response())));
                return;
            }
        }
    } else {
        None
    };

    let doc_type = DocumentType::from_str(&template.doc_type);
    let doc_name = body
        .name
        .unwrap_or_else(|| format!("{} (copy)", template.name));
    let content_str = template.content.unwrap_or_default();
    let content: serde_json::Value =
        serde_json::from_str(&content_str).unwrap_or(serde_json::Value::Object(Default::default()));

    let file_id_val = Uuid::new_v4();
    let doc_id_val = Uuid::new_v4();
    let now = Utc::now();
    let path_val = format!("/{}.contrix", doc_name);

    // Create the file record.
    let new_file = NewFile {
        id: file_id_val.to_string(),
        name: doc_name.clone(),
        path: path_val,
        mime_type: Some(mime_for_doc_type(&doc_type).to_string()),
        size: content_str.len() as i64,
        owner_id: uid.clone(),
        parent_id: body.parent_id,
        is_folder: false,
        is_trashed: false,
        trashed_at: None,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        room_id: room_id_for_new_file,
        thumbnail_path: None,
        sort_order: 0,
    };

    if let Err(e) = diesel::insert_into(f::files)
        .values(&new_file)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    // Create the document record.
    let new_doc = NewDocument {
        id: doc_id_val.to_string(),
        file_id: file_id_val.to_string(),
        doc_type: doc_type.as_str().to_string(),
        content: Some(content_str),
        version: 1,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };

    if let Err(e) = diesel::insert_into(d::documents)
        .values(&new_doc)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    let resp = DocumentResponse {
        id: doc_id_val,
        file_id: file_id_val,
        name: doc_name,
        document_type: doc_type,
        content,
        version: 1,
        owner_id: uid.parse().unwrap_or_default(),
        created_at: now,
        updated_at: now,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(resp)));
}
