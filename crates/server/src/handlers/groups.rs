use std::sync::Arc;

use chrono::Utc;
use diesel::prelude::*;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{
    AddGroupMemberRequest, ApiResponse, CreateGroupRequest, GroupMemberResponse, GroupResponse,
    UpdateGroupRequest,
};
use contrix_core::error::AppError;
use contrix_core::models::{Group, NewGroup, NewGroupMember};
use contrix_core::schema::group_members::dsl as gm;
use contrix_core::schema::groups::dsl as g;
use contrix_core::schema::users::dsl as u;

use crate::middleware::auth::get_user_id;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Whether `user_id` created `group_id` or is a member of it.
fn is_group_participant(
    conn: &mut diesel::SqliteConnection,
    group_id: &str,
    user_id: &str,
) -> bool {
    let is_creator = g::groups
        .filter(g::id.eq(group_id))
        .filter(g::created_by.eq(user_id))
        .count()
        .get_result::<i64>(conn)
        .unwrap_or(0)
        > 0;
    if is_creator {
        return true;
    }
    gm::group_members
        .filter(gm::group_id.eq(group_id))
        .filter(gm::user_id.eq(user_id))
        .count()
        .get_result::<i64>(conn)
        .unwrap_or(0)
        > 0
}

fn group_model_to_response(row: Group, member_count: u32) -> GroupResponse {
    GroupResponse {
        id: row.id.parse().unwrap_or_default(),
        name: row.name,
        description: row.description,
        created_by: row.created_by.parse().unwrap_or_default(),
        member_count,
        created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
        updated_at: row.updated_at.parse().unwrap_or_else(|_| Utc::now()),
    }
}

// ---------------------------------------------------------------------------
// POST /groups
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_group(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: CreateGroupRequest = match req.parse_json().await {
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

    let group_id_val = Uuid::new_v4();
    let now = Utc::now();

    let new_group = NewGroup {
        id: group_id_val.to_string(),
        name: body.name.clone(),
        description: body.description.clone(),
        created_by: uid.clone(),
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
    };

    if let Err(e) = diesel::insert_into(g::groups)
        .values(&new_group)
        .execute(&mut conn)
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    let resp = GroupResponse {
        id: group_id_val,
        name: body.name,
        description: body.description,
        created_by: uid.parse().unwrap_or_default(),
        member_count: 0,
        created_at: now,
        updated_at: now,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(resp)));
}

// ---------------------------------------------------------------------------
// GET /groups
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_groups(depot: &mut Depot, res: &mut Response) {
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

    match g::groups
        .filter(g::created_by.eq(&uid))
        .order(g::updated_at.desc())
        .load::<Group>(&mut conn)
    {
        Ok(rows) => {
            let group_list: Vec<GroupResponse> = rows
                .into_iter()
                .map(|group| {
                    let count: i64 = gm::group_members
                        .filter(gm::group_id.eq(&group.id))
                        .count()
                        .get_result(&mut conn)
                        .unwrap_or(0);
                    group_model_to_response(group, count as u32)
                })
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(group_list)));
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
// GET /groups/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_group(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let group_id_param: String = req.param("id").unwrap_or_default();

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

    match g::groups
        .filter(g::id.eq(&group_id_param))
        .first::<Group>(&mut conn)
        .optional()
    {
        Ok(Some(group)) => {
            if !is_group_participant(&mut conn, &group_id_param, &uid) {
                res.status_code(StatusCode::FORBIDDEN);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Forbidden("Not a member of this group".into()).to_response(),
                )));
                return;
            }
            let count: i64 = gm::group_members
                .filter(gm::group_id.eq(&group.id))
                .count()
                .get_result(&mut conn)
                .unwrap_or(0);
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(group_model_to_response(
                group,
                count as u32,
            ))));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Group not found".into()).to_response(),
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
// PUT /groups/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_group(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let group_id_param: String = req.param("id").unwrap_or_default();

    let body: UpdateGroupRequest = match req.parse_json().await {
        Ok(b) => b,
        Err(e) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation(e.to_string()).to_response(),
            )));
            return;
        }
    };

    let now = Utc::now();

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

    let result = match (&body.name, &body.description) {
        (Some(name), Some(desc)) => diesel::update(
            g::groups
                .filter(g::id.eq(&group_id_param))
                .filter(g::created_by.eq(&uid)),
        )
        .set((
            g::name.eq(name),
            g::description.eq(Some(desc)),
            g::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn),
        (Some(name), None) => diesel::update(
            g::groups
                .filter(g::id.eq(&group_id_param))
                .filter(g::created_by.eq(&uid)),
        )
        .set((g::name.eq(name), g::updated_at.eq(now.to_rfc3339())))
        .execute(&mut conn),
        (None, Some(desc)) => diesel::update(
            g::groups
                .filter(g::id.eq(&group_id_param))
                .filter(g::created_by.eq(&uid)),
        )
        .set((
            g::description.eq(Some(desc)),
            g::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn),
        (None, None) => {
            res.status_code(StatusCode::BAD_REQUEST);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Validation("No fields to update".into()).to_response(),
            )));
            return;
        }
    };

    match result {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Group not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Group updated" }),
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
// DELETE /groups/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_group(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let group_id_param: String = req.param("id").unwrap_or_default();

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

    // Delete group members first.
    let _ = diesel::delete(gm::group_members.filter(gm::group_id.eq(&group_id_param)))
        .execute(&mut conn);

    match diesel::delete(
        g::groups
            .filter(g::id.eq(&group_id_param))
            .filter(g::created_by.eq(&uid)),
    )
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Group not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Group deleted" }),
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
// POST /groups/:id/members
// ---------------------------------------------------------------------------

#[handler]
pub async fn add_member(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let group_id_param: String = req.param("id").unwrap_or_default();

    let body: AddGroupMemberRequest = match req.parse_json().await {
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

    // Verify group ownership.
    let group_count: i64 = match g::groups
        .filter(g::id.eq(&group_id_param))
        .filter(g::created_by.eq(&uid))
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

    if group_count == 0 {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Group not found or not owned by you".into()).to_response(),
        )));
        return;
    }

    let member_id_val = Uuid::new_v4();
    let now = Utc::now();

    let new_member = NewGroupMember {
        id: member_id_val.to_string(),
        group_id: group_id_param.clone(),
        user_id: body.user_id.to_string(),
        added_at: now.to_rfc3339(),
    };

    match diesel::insert_into(gm::group_members)
        .values(&new_member)
        .execute(&mut conn)
    {
        Ok(_) => {
            // Fetch username for response.
            let (uname, dname) = u::users
                .filter(u::id.eq(body.user_id.to_string()))
                .select((u::username, u::display_name))
                .first::<(String, Option<String>)>(&mut conn)
                .unwrap_or(("unknown".to_string(), None));

            let resp = GroupMemberResponse {
                id: member_id_val,
                group_id: group_id_param.parse().unwrap_or_default(),
                user_id: body.user_id,
                username: uname,
                display_name: dname,
                added_at: now,
            };
            res.status_code(StatusCode::CREATED);
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

// ---------------------------------------------------------------------------
// DELETE /groups/:id/members/:user_id
// ---------------------------------------------------------------------------

#[handler]
pub async fn remove_member(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let group_id_param: String = req.param("id").unwrap_or_default();
    let user_id_param: String = req.param("user_id").unwrap_or_default();

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

    // Verify group ownership.
    let group_count: i64 = match g::groups
        .filter(g::id.eq(&group_id_param))
        .filter(g::created_by.eq(&uid))
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

    if group_count == 0 {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Group not found or not owned by you".into()).to_response(),
        )));
        return;
    }

    match diesel::delete(
        gm::group_members
            .filter(gm::group_id.eq(&group_id_param))
            .filter(gm::user_id.eq(&user_id_param)),
    )
    .execute(&mut conn)
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Member not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Member removed" }),
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
// GET /groups/:id/members
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_members(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let group_id_param: String = req.param("id").unwrap_or_default();

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

    if !is_group_participant(&mut conn, &group_id_param, &uid) {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Not a member of this group".into()).to_response(),
        )));
        return;
    }

    match gm::group_members
        .inner_join(u::users.on(gm::user_id.eq(u::id)))
        .filter(gm::group_id.eq(&group_id_param))
        .select((
            gm::id,
            gm::group_id,
            gm::user_id,
            u::username,
            u::display_name,
            gm::added_at,
        ))
        .load::<(String, String, String, String, Option<String>, String)>(&mut conn)
    {
        Ok(rows) => {
            let members: Vec<GroupMemberResponse> = rows
                .into_iter()
                .map(
                    |(mid, gid, muid, uname, dname, added)| GroupMemberResponse {
                        id: mid.parse().unwrap_or_default(),
                        group_id: gid.parse().unwrap_or_default(),
                        user_id: muid.parse().unwrap_or_default(),
                        username: uname,
                        display_name: dname,
                        added_at: added.parse().unwrap_or_else(|_| Utc::now()),
                    },
                )
                .collect();
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(members)));
        }
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
        }
    }
}
