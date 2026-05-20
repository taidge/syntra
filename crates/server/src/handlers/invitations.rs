use std::sync::Arc;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};
use chrono::{Duration, Utc};
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, CreateInvitationRequest, InvitationResponse};
use contrix_core::error::AppError;
use contrix_core::models::{Invitation, NewInvitation, UserRole};
use contrix_core::schema::invitations::dsl as inv;

use crate::middleware::auth::{get_user_id, get_user_role};
use crate::services::password::PasswordService;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn invitation_to_response(row: Invitation) -> InvitationResponse {
    InvitationResponse {
        id: row.id.parse().unwrap_or_default(),
        email: row.email,
        role: UserRole::from_str(&row.role),
        invited_by: row.invited_by.parse().unwrap_or_default(),
        token: row.token,
        message: row.message,
        is_accepted: row.is_accepted,
        expires_at: row.expires_at.parse().unwrap_or_else(|_| Utc::now()),
        created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
    }
}

/// Generate a random invitation token (URL-safe).
fn generate_token() -> String {
    Uuid::new_v4().to_string().replace('-', "")
}

// ---------------------------------------------------------------------------
// POST /invitations
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_invitation(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: CreateInvitationRequest = match req.parse_json().await {
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

    let invitation_id = Uuid::new_v4();
    let now = Utc::now();
    let expires_at = now + Duration::days(7);
    let token = generate_token();
    let role = body.role.unwrap_or(UserRole::Editor);

    let new_invitation = NewInvitation {
        id: invitation_id.to_string(),
        email: body.email.clone(),
        role: role.as_str().to_string(),
        invited_by: uid.clone(),
        token: token.clone(),
        message: body.message.clone(),
        is_accepted: false,
        accepted_at: None,
        expires_at: expires_at.to_rfc3339(),
        created_at: now.to_rfc3339(),
    };

    if let Err(e) = diesel::insert_into(inv::invitations)
        .values(&new_invitation)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    let resp = InvitationResponse {
        id: invitation_id,
        email: body.email,
        role,
        invited_by: uid.parse().unwrap_or_default(),
        token,
        message: body.message,
        is_accepted: false,
        expires_at,
        created_at: now,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(resp)));
}

// ---------------------------------------------------------------------------
// GET /invitations
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_invitations(depot: &mut Depot, res: &mut Response) {
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

    // Admins see all invitations; regular users see only their own.
    let is_admin = get_user_role(depot) == Some("admin".to_string());

    let result = if is_admin {
        inv::invitations
            .order(inv::created_at.desc())
            .load::<Invitation>(&mut conn)
    } else {
        inv::invitations
            .filter(inv::invited_by.eq(&uid))
            .order(inv::created_at.desc())
            .load::<Invitation>(&mut conn)
    };

    match result {
        Ok(rows) => {
            let invitation_list: Vec<InvitationResponse> =
                rows.into_iter().map(invitation_to_response).collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(invitation_list)));
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
// GET /invitations/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_invitation(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let invitation_id_param: String = req.param("id").unwrap_or_default();

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

    match inv::invitations
        .filter(inv::id.eq(&invitation_id_param))
        .first::<Invitation>(&mut conn)
        .optional()
    {
        Ok(Some(row)) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(invitation_to_response(row))));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Invitation not found".into()).to_response(),
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
// DELETE /invitations/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn cancel_invitation(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let invitation_id_param: String = req.param("id").unwrap_or_default();

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

    // Allow the inviter or an admin to cancel.
    let is_admin = get_user_role(depot) == Some("admin".to_string());

    let result = if is_admin {
        diesel::delete(inv::invitations.filter(inv::id.eq(&invitation_id_param))).execute(&mut conn)
    } else {
        diesel::delete(
            inv::invitations
                .filter(inv::id.eq(&invitation_id_param))
                .filter(inv::invited_by.eq(&uid)),
        )
        .execute(&mut conn)
    };

    match result {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Invitation not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Invitation cancelled" }),
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
// POST /invitations/accept/:token
// ---------------------------------------------------------------------------

#[handler]
pub async fn accept_invitation_with_depot(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
) {
    let state = depot.obtain::<Arc<AppState>>().unwrap().clone();

    let token_param: String = req.param("token").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct AcceptRequest {
        username: String,
        email: String,
        password: String,
    }

    let body: AcceptRequest = match req.parse_json().await {
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

    // Find the invitation by token.
    let invitation = match inv::invitations
        .filter(inv::token.eq(&token_param))
        .filter(inv::is_accepted.eq(false))
        .first::<Invitation>(&mut conn)
        .optional()
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Invitation not found or already accepted".into()).to_response(),
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

    // Check if the invitation has expired.
    let expires_at = invitation
        .expires_at
        .parse::<chrono::DateTime<Utc>>()
        .unwrap_or_else(|_| Utc::now());

    if Utc::now() > expires_at {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Invitation has expired".into()).to_response(),
        )));
        return;
    }

    // Verify email matches.
    if body.email != invitation.email {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Validation("Email does not match invitation".into()).to_response(),
        )));
        return;
    }

    if let Err(e) = PasswordService::validate_password(&mut conn, &body.password) {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(e.to_response())));
        return;
    }

    // Check if a user with this email already exists.
    use contrix_core::schema::users::dsl as u_dsl;

    let existing_user_count: i64 = match u_dsl::users
        .filter(u_dsl::email.eq(&body.email))
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

    if existing_user_count > 0 {
        // User already exists - just mark invitation as accepted.
        let now = Utc::now();
        let _ = diesel::update(inv::invitations.filter(inv::id.eq(&invitation.id)))
            .set((
                inv::is_accepted.eq(true),
                inv::accepted_at.eq(Some(now.to_rfc3339())),
            ))
            .execute(&mut conn);

        res.status_code(StatusCode::OK);
        res.render(Json(ApiResponse::ok(serde_json::json!({
            "message": "Invitation accepted, existing user linked",
        }))));
        return;
    }

    let existing_username_count: i64 = match u_dsl::users
        .filter(u_dsl::username.eq(&body.username))
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

    if existing_username_count > 0 {
        res.status_code(StatusCode::CONFLICT);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Conflict("Username already taken".into()).to_response(),
        )));
        return;
    }

    let salt = SaltString::generate(&mut OsRng);
    let password_hash = match Argon2::default().hash_password(body.password.as_bytes(), &salt) {
        Ok(hash) => hash.to_string(),
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Internal(format!("Password hashing failed: {e}")).to_response(),
            )));
            return;
        }
    };

    // Create a new user.
    let user_id = Uuid::new_v4();
    let now = Utc::now();
    let default_quota = (state.config.storage.default_quota_mb as i64) * 1024 * 1024;

    let new_user = contrix_core::models::NewUser {
        id: user_id.to_string(),
        username: body.username.clone(),
        email: body.email.clone(),
        password_hash,
        display_name: None,
        avatar_url: None,
        role: invitation.role.clone(),
        storage_used: 0,
        storage_quota: Some(default_quota),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        last_login_at: Some(now.to_rfc3339()),
    };

    if let Err(e) = diesel::insert_into(u_dsl::users)
        .values(&new_user)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    // Mark the invitation as accepted.
    let _ = diesel::update(inv::invitations.filter(inv::id.eq(&invitation.id)))
        .set((
            inv::is_accepted.eq(true),
            inv::accepted_at.eq(Some(now.to_rfc3339())),
        ))
        .execute(&mut conn);

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Invitation accepted, user created",
        "user_id": user_id.to_string(),
    }))));
}
