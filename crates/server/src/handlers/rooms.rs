use chrono::Utc;
use diesel::prelude::*;
use horme::OptionalExtension;
use horme::RunQueryDsl;
use salvo::prelude::*;
use uuid::Uuid;

use contrix_core::dto::{
    AddRoomMemberRequest, ApiResponse, CreateRoomRequest, FileResponse, RoomMemberResponse,
    RoomResponse, UpdateRoomRequest,
};
use contrix_core::error::AppError;
use contrix_core::models::{File, NewRoom, NewRoomMember, Room, RoomType};
use contrix_core::schema::files::dsl as f;
use contrix_core::schema::room_members::dsl as rm;
use contrix_core::schema::rooms::dsl as r;
use contrix_core::schema::users::dsl as u;

use crate::middleware::auth::get_user_id;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Whether `user_id` is a member or the owner of `room_id`. Room owners are not
/// necessarily present in `room_members`, so both are checked.
async fn is_room_participant(
    conn: &mut impl horme::GetConnection<Connection = crate::database::DbConnection>,
    room_id: &str,
    user_id: &str,
) -> bool {
    let is_member = rm::room_members
        .filter(rm::room_id.eq(room_id))
        .filter(rm::user_id.eq(user_id))
        .count()
        .get_result::<i64>(conn)
        .await
        .unwrap_or(0)
        > 0;
    if is_member {
        return true;
    }
    r::rooms
        .filter(r::id.eq(room_id))
        .filter(r::owner_id.eq(user_id))
        .count()
        .get_result::<i64>(conn)
        .await
        .unwrap_or(0)
        > 0
}

fn room_model_to_response(row: Room, member_count: u32) -> RoomResponse {
    RoomResponse {
        id: row.id.parse().unwrap_or_default(),
        name: row.name,
        description: row.description,
        room_type: RoomType::parse_lossy(&row.room_type),
        owner_id: row.owner_id.parse().unwrap_or_default(),
        is_archived: row.is_archived,
        member_count,
        created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
        updated_at: row.updated_at.parse().unwrap_or_else(|_| Utc::now()),
    }
}

fn file_model_to_response(row: File) -> FileResponse {
    FileResponse {
        id: row.id.parse().unwrap_or_default(),
        name: row.name,
        mime_type: row.mime_type,
        size: row.size,
        owner_id: row.owner_id.parse().unwrap_or_default(),
        parent_id: row.parent_id.as_deref().and_then(|s| s.parse().ok()),
        is_folder: row.is_folder,
        is_trashed: row.is_trashed,
        trashed_at: row
            .trashed_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc)),
        created_at: row.created_at.parse().unwrap_or_else(|_| Utc::now()),
        updated_at: row.updated_at.parse().unwrap_or_else(|_| Utc::now()),
        shared_with_count: 0,
        document_id: None,
    }
}

// ---------------------------------------------------------------------------
// POST /rooms
// ---------------------------------------------------------------------------

#[handler]
pub async fn create_room(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let body: CreateRoomRequest = match req.parse_json().await {
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

    let room_id_val = Uuid::new_v4();
    let now = Utc::now();
    let room_type_val = body.room_type.unwrap_or(RoomType::Collaboration);

    let new_room = NewRoom {
        id: room_id_val.to_string(),
        name: body.name.clone(),
        description: body.description.clone(),
        room_type: room_type_val.as_str().to_string(),
        owner_id: uid.clone(),
        is_archived: false,
        created_at: now.to_rfc3339(),
        updated_at: now.to_rfc3339(),
        logo_url: body.logo_url.clone(),
        color: body.color.clone(),
        storage_quota: body.storage_quota,
        auto_delete_days: None,
        auto_delete_enabled: false,
    };

    if let Err(e) = diesel::insert_into(r::rooms)
        .values(&new_room)
        .execute(&mut conn)
        .await
    {
        res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Database(e.to_string()).to_response(),
        )));
        return;
    }

    // Add owner as a member with "owner" role.
    let owner_member = NewRoomMember {
        id: Uuid::new_v4().to_string(),
        room_id: room_id_val.to_string(),
        user_id: uid.clone(),
        role: "owner".to_string(),
        joined_at: now.to_rfc3339(),
    };

    let _ = diesel::insert_into(rm::room_members)
        .values(&owner_member)
        .execute(&mut conn)
        .await;

    let resp = RoomResponse {
        id: room_id_val,
        name: body.name,
        description: body.description,
        room_type: room_type_val,
        owner_id: uid.parse().unwrap_or_default(),
        is_archived: false,
        member_count: 1,
        created_at: now,
        updated_at: now,
    };

    res.status_code(StatusCode::CREATED);
    res.render(Json(ApiResponse::ok(resp)));
}

// ---------------------------------------------------------------------------
// GET /rooms
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_rooms(depot: &mut Depot, res: &mut Response) {
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

    // Get room IDs where user is a member.
    let member_room_ids: Vec<String> = match rm::room_members
        .filter(rm::user_id.eq(&uid))
        .select(rm::room_id)
        .load::<String>(&mut conn)
        .await
    {
        Ok(ids) => ids,
        Err(e) => {
            res.status_code(StatusCode::INTERNAL_SERVER_ERROR);
            res.render(Json(ApiResponse::<()>::err(
                AppError::Database(e.to_string()).to_response(),
            )));
            return;
        }
    };

    match r::rooms
        .filter(r::id.eq_any(&member_room_ids).or(r::owner_id.eq(&uid)))
        .order(r::updated_at.desc())
        .load::<Room>(&mut conn)
        .await
    {
        Ok(rows) => {
            let mut room_list = Vec::with_capacity(rows.len());
            for room in rows {
                let count: i64 = rm::room_members
                    .filter(rm::room_id.eq(&room.id))
                    .count()
                    .get_result(&mut conn)
                    .await
                    .unwrap_or(0);
                room_list.push(room_model_to_response(room, count as u32));
            }
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(room_list)));
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
// GET /rooms/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn get_room(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

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

    match r::rooms
        .filter(r::id.eq(&room_id_param))
        .first::<Room>(&mut conn)
        .await
        .optional()
    {
        Ok(Some(room)) => {
            if !is_room_participant(&mut conn, &room_id_param, &uid).await {
                res.status_code(StatusCode::FORBIDDEN);
                res.render(Json(ApiResponse::<()>::err(
                    AppError::Forbidden("Not a member of this room".into()).to_response(),
                )));
                return;
            }
            let count: i64 = rm::room_members
                .filter(rm::room_id.eq(&room.id))
                .count()
                .get_result(&mut conn)
                .await
                .unwrap_or(0);
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(room_model_to_response(
                room,
                count as u32,
            ))));
        }
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Room not found".into()).to_response(),
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
// PUT /rooms/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_room(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

    let body: UpdateRoomRequest = match req.parse_json().await {
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

    // Build update depending on which fields are provided.
    let result = match (&body.name, &body.description) {
        (Some(name), Some(desc)) => {
            diesel::update(
                r::rooms
                    .filter(r::id.eq(&room_id_param))
                    .filter(r::owner_id.eq(&uid)),
            )
            .set((
                r::name.eq(name),
                r::description.eq(Some(desc)),
                r::updated_at.eq(now.to_rfc3339()),
            ))
            .execute(&mut conn)
            .await
        }
        (Some(name), None) => {
            diesel::update(
                r::rooms
                    .filter(r::id.eq(&room_id_param))
                    .filter(r::owner_id.eq(&uid)),
            )
            .set((r::name.eq(name), r::updated_at.eq(now.to_rfc3339())))
            .execute(&mut conn)
            .await
        }
        (None, Some(desc)) => {
            diesel::update(
                r::rooms
                    .filter(r::id.eq(&room_id_param))
                    .filter(r::owner_id.eq(&uid)),
            )
            .set((
                r::description.eq(Some(desc)),
                r::updated_at.eq(now.to_rfc3339()),
            ))
            .execute(&mut conn)
            .await
        }
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
                AppError::NotFound("Room not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Room updated" }),
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
// DELETE /rooms/:id
// ---------------------------------------------------------------------------

#[handler]
pub async fn delete_room(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

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

    // Delete room members first.
    let _ = diesel::delete(rm::room_members.filter(rm::room_id.eq(&room_id_param)))
        .execute(&mut conn)
        .await;

    match diesel::delete(
        r::rooms
            .filter(r::id.eq(&room_id_param))
            .filter(r::owner_id.eq(&uid)),
    )
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Room not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Room deleted" }),
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
// POST /rooms/:id/archive
// ---------------------------------------------------------------------------

#[handler]
pub async fn toggle_archive(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

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

    // Fetch current archive status.
    let room = match r::rooms
        .filter(r::id.eq(&room_id_param))
        .filter(r::owner_id.eq(&uid))
        .first::<Room>(&mut conn)
        .await
        .optional()
    {
        Ok(Some(room)) => room,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Room not found".into()).to_response(),
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

    let new_status = !room.is_archived;
    let now = Utc::now();

    match diesel::update(
        r::rooms
            .filter(r::id.eq(&room_id_param))
            .filter(r::owner_id.eq(&uid)),
    )
    .set((
        r::is_archived.eq(new_status),
        r::updated_at.eq(now.to_rfc3339()),
    ))
    .execute(&mut conn)
    .await
    {
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "is_archived": new_status }),
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
// POST /rooms/:id/members
// ---------------------------------------------------------------------------

#[handler]
pub async fn add_member(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

    let body: AddRoomMemberRequest = match req.parse_json().await {
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

    // Verify room ownership.
    let room_count: i64 = match r::rooms
        .filter(r::id.eq(&room_id_param))
        .filter(r::owner_id.eq(&uid))
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

    if room_count == 0 {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Room not found or not owned by you".into()).to_response(),
        )));
        return;
    }

    let member_id_val = Uuid::new_v4();
    let now = Utc::now();
    let role_val = body.role.unwrap_or_else(|| "member".to_string());

    let new_member = NewRoomMember {
        id: member_id_val.to_string(),
        room_id: room_id_param.clone(),
        user_id: body.user_id.to_string(),
        role: role_val.clone(),
        joined_at: now.to_rfc3339(),
    };

    match diesel::insert_into(rm::room_members)
        .values(&new_member)
        .execute(&mut conn)
        .await
    {
        Ok(_) => {
            // Fetch username for response.
            let (uname, dname) = u::users
                .filter(u::id.eq(body.user_id.to_string()))
                .select((u::username, u::display_name))
                .first::<(String, Option<String>)>(&mut conn)
                .await
                .unwrap_or(("unknown".to_string(), None));

            let resp = RoomMemberResponse {
                id: member_id_val,
                room_id: room_id_param.parse().unwrap_or_default(),
                user_id: body.user_id,
                username: uname,
                display_name: dname,
                role: role_val,
                joined_at: now,
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
// DELETE /rooms/:id/members/:user_id
// ---------------------------------------------------------------------------

#[handler]
pub async fn remove_member(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();
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

    // Verify room ownership.
    let room_count: i64 = match r::rooms
        .filter(r::id.eq(&room_id_param))
        .filter(r::owner_id.eq(&uid))
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

    if room_count == 0 {
        res.status_code(StatusCode::NOT_FOUND);
        res.render(Json(ApiResponse::<()>::err(
            AppError::NotFound("Room not found or not owned by you".into()).to_response(),
        )));
        return;
    }

    match diesel::delete(
        rm::room_members
            .filter(rm::room_id.eq(&room_id_param))
            .filter(rm::user_id.eq(&user_id_param)),
    )
    .execute(&mut conn)
    .await
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
// GET /rooms/:id/members
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_members(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

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

    if !is_room_participant(&mut conn, &room_id_param, &uid).await {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Not a member of this room".into()).to_response(),
        )));
        return;
    }

    match rm::room_members
        .inner_join(u::users.on(rm::user_id.eq(u::id)))
        .filter(rm::room_id.eq(&room_id_param))
        .select((
            rm::id,
            rm::room_id,
            rm::user_id,
            u::username,
            u::display_name,
            rm::role,
            rm::joined_at,
        ))
        .load::<(
            String,
            String,
            String,
            String,
            Option<String>,
            String,
            String,
        )>(&mut conn)
        .await
    {
        Ok(rows) => {
            let members: Vec<RoomMemberResponse> = rows
                .into_iter()
                .map(
                    |(mid, rid, muid, uname, dname, role, joined)| RoomMemberResponse {
                        id: mid.parse().unwrap_or_default(),
                        room_id: rid.parse().unwrap_or_default(),
                        user_id: muid.parse().unwrap_or_default(),
                        username: uname,
                        display_name: dname,
                        role,
                        joined_at: joined.parse().unwrap_or_else(|_| Utc::now()),
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

// ---------------------------------------------------------------------------
// GET /rooms/:id/files
// ---------------------------------------------------------------------------

#[handler]
pub async fn list_room_files(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

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

    let is_member = rm::room_members
        .filter(rm::room_id.eq(&room_id_param))
        .filter(rm::user_id.eq(&uid))
        .count()
        .get_result::<i64>(&mut conn)
        .await
        .unwrap_or(0)
        > 0;
    let is_owner = r::rooms
        .filter(r::id.eq(&room_id_param))
        .filter(r::owner_id.eq(&uid))
        .count()
        .get_result::<i64>(&mut conn)
        .await
        .unwrap_or(0)
        > 0;

    if !is_member && !is_owner {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Not a member of this room".into()).to_response(),
        )));
        return;
    }

    match f::files
        .filter(f::room_id.eq(&room_id_param))
        .filter(f::is_trashed.eq(false))
        .order((f::is_folder.desc(), f::name.asc()))
        .load::<File>(&mut conn)
        .await
    {
        Ok(rows) => {
            let mut file_list: Vec<FileResponse> =
                rows.into_iter().map(file_model_to_response).collect();
            for r in &mut file_list {
                r.shared_with_count =
                    crate::handlers::access::count_shares_for_file(&mut conn, &r.id.to_string())
                        .await;
            }
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(file_list)));
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
// PUT /rooms/:id/logo
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_room_logo(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct LogoBody {
        logo_url: Option<String>,
    }

    let body: LogoBody = match req.parse_json().await {
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

    let now = Utc::now();

    match diesel::update(
        r::rooms
            .filter(r::id.eq(&room_id_param))
            .filter(r::owner_id.eq(&uid)),
    )
    .set((
        r::logo_url.eq(&body.logo_url),
        r::updated_at.eq(now.to_rfc3339()),
    ))
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Room not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Room logo updated", "logo_url": body.logo_url }),
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
// PUT /rooms/:id/color
// ---------------------------------------------------------------------------

#[handler]
pub async fn update_room_color(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct ColorBody {
        color: Option<String>,
    }

    let body: ColorBody = match req.parse_json().await {
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

    let now = Utc::now();

    match diesel::update(
        r::rooms
            .filter(r::id.eq(&room_id_param))
            .filter(r::owner_id.eq(&uid)),
    )
    .set((r::color.eq(&body.color), r::updated_at.eq(now.to_rfc3339())))
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Room not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Room color updated", "color": body.color }),
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
// PUT /rooms/:id/quota
// ---------------------------------------------------------------------------

#[handler]
pub async fn set_room_quota(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct QuotaBody {
        storage_quota: Option<i64>,
    }

    let body: QuotaBody = match req.parse_json().await {
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

    // Check if user is admin or room owner.
    let room = match r::rooms
        .filter(r::id.eq(&room_id_param))
        .first::<Room>(&mut conn)
        .await
        .optional()
    {
        Ok(Some(room)) => room,
        Ok(None) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Room not found".into()).to_response(),
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

    // Fetch user role.
    let user_role: String = u::users
        .filter(u::id.eq(&uid))
        .select(u::role)
        .first::<String>(&mut conn)
        .await
        .unwrap_or_default();

    if user_role != "admin" && room.owner_id != uid {
        res.status_code(StatusCode::FORBIDDEN);
        res.render(Json(ApiResponse::<()>::err(
            AppError::Forbidden("Only admin or room owner can set room quota".into()).to_response(),
        )));
        return;
    }

    let now = Utc::now();

    match diesel::update(r::rooms.filter(r::id.eq(&room_id_param)))
        .set((
            r::storage_quota.eq(body.storage_quota),
            r::updated_at.eq(now.to_rfc3339()),
        ))
        .execute(&mut conn)
        .await
    {
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(
                serde_json::json!({ "message": "Room quota updated", "storage_quota": body.storage_quota }),
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
// PUT /rooms/:id/lifetime
// ---------------------------------------------------------------------------

#[handler]
pub async fn set_room_lifetime(req: &mut Request, depot: &mut Depot, res: &mut Response) {
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

    let room_id_param: String = req.param("id").unwrap_or_default();

    #[derive(serde::Deserialize)]
    struct LifetimeBody {
        auto_delete_days: Option<i32>,
        auto_delete_enabled: bool,
    }

    let body: LifetimeBody = match req.parse_json().await {
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

    let now = Utc::now();

    match diesel::update(
        r::rooms
            .filter(r::id.eq(&room_id_param))
            .filter(r::owner_id.eq(&uid)),
    )
    .set((
        r::auto_delete_days.eq(body.auto_delete_days),
        r::auto_delete_enabled.eq(body.auto_delete_enabled),
        r::updated_at.eq(now.to_rfc3339()),
    ))
    .execute(&mut conn)
    .await
    {
        Ok(0) => {
            res.status_code(StatusCode::NOT_FOUND);
            res.render(Json(ApiResponse::<()>::err(
                AppError::NotFound("Room not found".into()).to_response(),
            )));
        }
        Ok(_) => {
            res.status_code(StatusCode::OK);
            res.render(Json(ApiResponse::ok(serde_json::json!({
                "message": "Room lifetime updated",
                "auto_delete_days": body.auto_delete_days,
                "auto_delete_enabled": body.auto_delete_enabled,
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
