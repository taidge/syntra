use chrono::{Duration, Utc};
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{ApiResponse, CreateInvitationRequest, InvitationResponse};
use contrix_core::error::AppError;
use contrix_core::models::{Invitation, NewInvitation, UserRole};
use contrix_core::schema::invitations::dsl as inv;

use crate::middleware::auth::{get_user_id, get_user_role};
use crate::services::password::PasswordService;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn invitation_to_response(row: Invitation) -> InvitationResponse {
    InvitationResponse {
        id: row.id.parse().unwrap_or_default(),
        email: row.email,
        role: UserRole::parse_lossy(&row.role),
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

    // Only admins may invite users; otherwise any logged-in user could mint an
    // admin invitation and escalate privileges by accepting it.
    if get_user_role(depot).as_deref() != Some("admin") {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Admin access required".into()).to_response(),
        )));
        return;
    }

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
        .await
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    // Email the invitation (best-effort, off the async runtime).
    {
        let pool = state.db.clone();
        let to = body.email.clone();
        let invite_token = token.clone();
        let custom = body.message.clone().unwrap_or_default();
        let body_html = format!(
            "<p>You have been invited to join the workspace.</p>\
             {}\
             <p>Use this invitation token to create your account:</p>\
             <p><code>{invite_token}</code></p>",
            if custom.is_empty() {
                String::new()
            } else {
                format!("<p>{custom}</p>")
            }
        );
        tokio::spawn(async move {
            crate::services::EmailService::send_notification(
                &pool,
                &to,
                "You have been invited",
                &body_html,
            )
            .await;
        });
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
            .await
    } else {
        inv::invitations
            .filter(inv::invited_by.eq(&uid))
            .order(inv::created_at.desc())
            .load::<Invitation>(&mut conn)
            .await
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

    let is_admin = get_user_role(depot).as_deref() == Some("admin");
    let mut query = inv::invitations
        .filter(inv::id.eq(&invitation_id_param))
        .into_boxed();
    if !is_admin {
        query = query.filter(inv::invited_by.eq(&uid));
    }
    match query.first::<Invitation>(&mut conn).await.optional() {
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
        diesel::delete(inv::invitations.filter(inv::id.eq(&invitation_id_param)))
            .execute(&mut conn)
            .await
    } else {
        diesel::delete(
            inv::invitations
                .filter(inv::id.eq(&invitation_id_param))
                .filter(inv::invited_by.eq(&uid)),
        )
        .execute(&mut conn)
        .await
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
    let state = crate::require_app_state!(depot, res);

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
        .await
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

    // Invitations inherit their issuer's namespace and cannot reactivate a
    // disabled tenant or a capability issued by an administrator since demoted.
    if let Err(error) =
        crate::services::tenancy::user_tenant_async(&mut conn, &invitation.invited_by).await
    {
        res.status_code(
            StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        );
        res.render(Json(ApiResponse::<()>::err(error.to_response())));
        return;
    }
    let issuer_role = contrix_core::schema::users::table
        .find(&invitation.invited_by)
        .select(contrix_core::schema::users::role)
        .first::<String>(&mut conn)
        .await;
    if !matches!(issuer_role.as_deref(), Ok("admin")) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Invitation issuer no longer has administrator access".into())
                .to_response(),
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

    if let Err(e) = PasswordService::validate_password(&mut conn, &body.password).await {
        res.status_code(StatusCode::BAD_REQUEST);
        res.render(Json(ApiResponse::<()>::err(e.to_response())));
        return;
    }

    // Check if a user with this email already exists.
    use contrix_core::schema::users::dsl as u_dsl;

    let existing_user: Option<String> = match u_dsl::users
        .filter(u_dsl::email.eq(&body.email))
        .select(u_dsl::id)
        .first::<String>(&mut conn)
        .await
        .optional()
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

    if let Some(existing_user) = existing_user {
        if let Err(error) = crate::services::tenancy::ensure_same_tenant_async(
            &mut conn,
            &invitation.invited_by,
            &existing_user,
        )
        .await
        {
            res.status_code(
                StatusCode::from_u16(error.status_code())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            );
            res.render(Json(ApiResponse::<()>::err(error.to_response())));
            return;
        }
        // User already exists - just mark invitation as accepted.
        let now = Utc::now();
        let _ = diesel::update(inv::invitations.filter(inv::id.eq(&invitation.id)))
            .set((
                inv::is_accepted.eq(true),
                inv::accepted_at.eq(Some(now.to_rfc3339())),
            ))
            .execute(&mut conn)
            .await;

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
        .await
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

    let password_hash = match crate::services::password_crypto::hash_password(body.password).await {
        Ok(hash) => hash,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(e.to_response())));
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

    let accepted = state
        .db
        .run(move |conn| {
            conn.immediate_transaction::<_, AppError, _>(|conn| {
                let tenant = crate::services::tenancy::user_tenant(conn, &invitation.invited_by)?;
                let issuer_role = diesel::RunQueryDsl::first::<String>(
                    u_dsl::users
                        .find(&invitation.invited_by)
                        .select(u_dsl::role),
                    conn,
                )?;
                if issuer_role != "admin" {
                    return Err(AppError::Forbidden(
                        "Invitation issuer no longer has administrator access".into(),
                    ));
                }
                let claimed = diesel::RunQueryDsl::execute(
                    diesel::update(
                        inv::invitations
                            .filter(inv::id.eq(&invitation.id))
                            .filter(inv::is_accepted.eq(false))
                            .filter(inv::expires_at.gt(now.to_rfc3339())),
                    )
                    .set((
                        inv::is_accepted.eq(true),
                        inv::accepted_at.eq(Some(now.to_rfc3339())),
                    )),
                    conn,
                )?;
                if claimed != 1 {
                    return Err(AppError::Conflict(
                        "Invitation was already accepted or expired".into(),
                    ));
                }
                diesel::RunQueryDsl::execute(
                    diesel::insert_into(u_dsl::users).values(&new_user),
                    conn,
                )?;
                diesel::RunQueryDsl::execute(
                    diesel::sql_query("UPDATE users SET tenant_id = ? WHERE id = ?")
                        .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(tenant)
                        .bind::<diesel::sql_types::Text, _>(&new_user.id),
                    conn,
                )?;
                Ok(())
            })
        })
        .await;
    if let Err(error) = accepted {
        res.status_code(
            StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        );
        res.render(Json(ApiResponse::<()>::err(error.to_response())));
        return;
    }

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(serde_json::json!({
        "message": "Invitation accepted, user created",
        "user_id": user_id.to_string(),
    }))));
}
