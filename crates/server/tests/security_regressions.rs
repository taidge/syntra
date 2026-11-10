#[allow(dead_code)]
mod common;

use common::TestServer;
use contrix_core::models::NewPasswordResetToken;
use contrix_core::schema::{
    file_locks::dsl as fl, group_members::dsl as gm, password_reset_tokens::dsl as prt,
    room_members::dsl as rm, users::dsl as u,
};
use diesel::prelude::*;
use serde_json::{Value, json};

async fn create(server: &TestServer, token: &str, path: &str, body: Value) -> Value {
    let response = server
        .client
        .post(server.url(path))
        .headers(server.auth_header(token))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 201, "{path}: {body}");
    body["data"].clone()
}

async fn wopi_host(server: &TestServer, jwt: &str, source: &str) -> reqwest::Response {
    server
        .client
        .get(server.url("/api/v1/wopi/host/writer/edit"))
        .query(&[("access_token", jwt), ("WOPISrc", source)])
        .send()
        .await
        .unwrap()
}

async fn wopi_token(server: &TestServer, jwt: &str, file: &str) -> String {
    let response = wopi_host(
        server,
        jwt,
        &server.url(&format!("/api/v1/wopi/files/{file}")),
    )
    .await;
    assert_eq!(response.status(), 200);
    let html = response.text().await.unwrap();
    let start = html.find("const wopiSessionToken = ").unwrap() + "const wopiSessionToken = ".len();
    serde_json::from_str(html[start..].split(';').next().unwrap()).unwrap()
}

async fn revoke_current_session(server: &TestServer, jwt: &str) {
    // DELETE /sessions intentionally preserves this session and only revokes
    // other devices. Revoke the exact source session to exercise logout.
    let current = server
        .client
        .get(server.url("/api/v1/sessions/current"))
        .headers(server.auth_header(jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(current.status(), 200);
    let current: Value = current.json().await.unwrap();
    let id = current["data"]["id"].as_str().unwrap();
    let revoked = server
        .client
        .delete(server.url(&format!("/api/v1/sessions/{id}")))
        .headers(server.auth_header(jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);
}

#[tokio::test]
async fn wopi_host_escapes_both_html_and_script_contexts() {
    let server = TestServer::new().await;
    let (token, _) = server
        .register("xssowner", "xss@test.com", "password123")
        .await;
    let (_, file, _) = server.create_document(&token, "safe", "writer", None).await;
    let attack =
        "</script><script>window.attack=true</script><img src=x onerror=window.attack=true>";
    let source = format!(
        "{}?label={attack}",
        server.url(&format!("/api/v1/wopi/files/{file}"))
    );
    let response = wopi_host(&server, &token, &source).await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let policy = response.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    let html = response.text().await.unwrap();
    assert!(!html.contains(attack));
    assert!(!html.contains("<img src=x"));
    assert_eq!(html.matches("<script").count(), 1);
    assert_eq!(html.matches("</script>").count(), 1);
    let nonce_marker = "<script nonce=\"";
    let start = html.find(nonce_marker).unwrap() + nonce_marker.len();
    let nonce = html[start..].split('"').next().unwrap();
    assert_eq!(nonce.len(), 64);
    assert!(nonce.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(policy.contains(&format!("'nonce-{nonce}'")));
    let script_policy = policy
        .split(';')
        .find(|directive| directive.trim_start().starts_with("script-src "))
        .unwrap();
    assert!(!script_policy.contains("'unsafe-inline'"));
    assert!(!script_policy.contains("'unsafe-eval'"));
    assert!(script_policy.contains("'wasm-unsafe-eval'"));
    assert!(html.contains("&lt;/script&gt;"));
    let start = html.find("const initialWopiSrc = ").unwrap() + "const initialWopiSrc = ".len();
    let script_value = html[start..].split(';').next().unwrap();
    assert!(!script_value.contains('<'));
    assert_eq!(
        serde_json::from_str::<String>(script_value).unwrap(),
        source
    );
    let another = wopi_host(&server, &token, &source).await;
    let next_policy = another.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        !next_policy.contains(&format!("'nonce-{nonce}'")),
        "each host response needs a fresh nonce"
    );
    let another = another.text().await.unwrap();
    let start = another.find(nonce_marker).unwrap() + nonce_marker.len();
    let next_nonce = another[start..].split('"').next().unwrap();
    assert!(next_policy.contains(&format!("'nonce-{next_nonce}'")));
}

#[tokio::test]
async fn default_csp_only_allows_known_shell_scripts_and_wasm_compilation() {
    let server = TestServer::new().await;
    let response = server
        .client
        .get(server.url("/api/v1/capabilities"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let policy = response.headers()["content-security-policy"]
        .to_str()
        .unwrap();
    let script = policy
        .split(';')
        .find(|directive| directive.trim_start().starts_with("script-src "))
        .unwrap();
    assert!(!script.contains("'unsafe-inline'"));
    assert!(!script.contains("'unsafe-eval'"));
    assert!(script.contains("'wasm-unsafe-eval'"));
    assert_eq!(script.matches("'sha256-").count(), 2);
    assert!(!script.contains("'nonce-"));
    assert!(policy.contains("style-src 'self' 'unsafe-inline'"));
}

#[tokio::test]
async fn unauthorized_room_and_group_delete_preserves_members_and_failure_rolls_back() {
    let server = TestServer::new().await;
    let (owner, _) = server
        .register("owner", "owner@test.com", "password123")
        .await;
    let (stranger, other) = server
        .register("stranger", "stranger@test.com", "password123")
        .await;
    let other_id = other["data"]["user"]["id"].as_str().unwrap();
    let room = create(
        &server,
        &owner,
        "/api/v1/rooms",
        json!({"name":"private-room"}),
    )
    .await;
    let group = create(
        &server,
        &owner,
        "/api/v1/groups",
        json!({"name":"private-group"}),
    )
    .await;
    let room_id = room["id"].as_str().unwrap().to_owned();
    let group_id = group["id"].as_str().unwrap().to_owned();
    create(
        &server,
        &owner,
        &format!("/api/v1/groups/{group_id}/members"),
        json!({"user_id":other_id}),
    )
    .await;
    for (resource, id) in [("rooms", &room_id), ("groups", &group_id)] {
        let response = server
            .client
            .delete(server.url(&format!("/api/v1/{resource}/{id}")))
            .headers(server.auth_header(&stranger))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
    }
    let (room_for_count, group_for_count) = (room_id.clone(), group_id.clone());
    let counts = server
        .db
        .run(move |conn| {
            Ok::<_, diesel::result::Error>((
                rm::room_members
                    .filter(rm::room_id.eq(room_for_count))
                    .count()
                    .get_result::<i64>(conn)?,
                gm::group_members
                    .filter(gm::group_id.eq(group_for_count))
                    .count()
                    .get_result::<i64>(conn)?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(counts, (1, 1));
    server.db.run(|conn| diesel::sql_query(
        "CREATE TRIGGER refuse_room_delete BEFORE DELETE ON rooms BEGIN SELECT RAISE(ABORT, 'injected-delete-failure'); END"
    ).execute(conn)).await.unwrap();
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/rooms/{room_id}")))
        .headers(server.auth_header(&owner))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    let count = server
        .db
        .run(move |conn| {
            rm::room_members
                .filter(rm::room_id.eq(room_id))
                .count()
                .get_result::<i64>(conn)
        })
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "failed parent deletion must roll back member deletion"
    );
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/groups/{group_id}")))
        .headers(server.auth_header(&owner))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn invitation_details_do_not_disclose_other_users_capabilities() {
    let server = TestServer::new().await;
    let (admin, registration) = server
        .register("inviter", "inviter@test.com", "password123")
        .await;
    server
        .make_admin(registration["data"]["user"]["id"].as_str().unwrap())
        .await;
    let (stranger, _) = server
        .register("outsider", "outsider@test.com", "password123")
        .await;
    let invitation = create(
        &server,
        &admin,
        "/api/v1/invitations",
        json!({"email":"future-admin@test.com","role":"admin"}),
    )
    .await;
    let id = invitation["id"].as_str().unwrap();
    let secret = invitation["token"].as_str().unwrap();
    let response = server
        .client
        .get(server.url(&format!("/api/v1/invitations/{id}")))
        .headers(server.auth_header(&stranger))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let body = response.text().await.unwrap();
    assert!(!body.contains(secret));
    assert!(!body.contains("future-admin@test.com"));
    let response = server
        .client
        .get(server.url(&format!("/api/v1/invitations/{id}")))
        .headers(server.auth_header(&admin))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn revoked_or_disabled_sessions_cannot_use_or_mint_wopi_tokens() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("revoked", "revoked@test.com", "password123")
        .await;
    let (_, file, _) = server
        .create_document(&jwt, "revoked-file", "writer", None)
        .await;
    let token = wopi_token(&server, &jwt, &file).await;
    revoke_current_session(&server, &jwt).await;
    let response = server
        .client
        .get(server.url(&format!("/api/v1/wopi/files/{file}/contents")))
        .query(&[("access_token", &token)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        wopi_host(
            &server,
            &jwt,
            &server.url(&format!("/api/v1/wopi/files/{file}"))
        )
        .await
        .status(),
        401
    );

    let (jwt, registration) = server
        .register("disabled", "disabled@test.com", "password123")
        .await;
    let (_, file, _) = server
        .create_document(&jwt, "disabled-file", "writer", None)
        .await;
    let token = wopi_token(&server, &jwt, &file).await;
    let user = registration["data"]["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    server
        .db
        .run(move |conn| {
            diesel::update(u::users.find(user))
                .set(u::role.eq("disabled"))
                .execute(conn)
        })
        .await
        .unwrap();
    let response = server
        .client
        .get(server.url(&format!("/api/v1/wopi/files/{file}")))
        .query(&[("access_token", &token)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    assert_eq!(
        wopi_host(
            &server,
            &jwt,
            &server.url(&format!("/api/v1/wopi/files/{file}"))
        )
        .await
        .status(),
        401
    );
}

#[tokio::test]
async fn a_new_login_never_reactivates_a_previously_revoked_token() {
    let server = TestServer::new().await;
    let (old, _) = server
        .register("nonceowner", "nonceowner@test.com", "password123")
        .await;
    revoke_current_session(&server, &old).await;
    let (new, _) = server.login("nonceowner@test.com", "password123").await;
    assert_ne!(old, new);
    let response = server
        .client
        .get(server.url("/api/v1/auth/me"))
        .headers(server.auth_header(&old))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let response = server
        .client
        .get(server.url("/api/v1/auth/me"))
        .headers(server.auth_header(&new))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn bulk_session_revocation_keeps_the_caller_and_revokes_other_devices() {
    let server = TestServer::new().await;
    let (other, _) = server
        .register("sessiondevices", "sessiondevices@test.com", "password123")
        .await;
    let (current, _) = server.login("sessiondevices@test.com", "password123").await;
    assert_ne!(other, current);
    let response = server
        .client
        .delete(server.url("/api/v1/sessions"))
        .headers(server.auth_header(&current))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    for (credential, expected) in [(&current, 200), (&other, 401)] {
        let response = server
            .client
            .get(server.url("/api/v1/auth/me"))
            .headers(server.auth_header(credential))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}

#[tokio::test]
async fn removed_room_creator_loses_wopi_and_public_share_access() {
    let server = TestServer::new().await;
    let (owner, _) = server
        .register("roomowner", "roomowner@test.com", "password123")
        .await;
    let (editor, registration) = server
        .register("creator", "creator@test.com", "password123")
        .await;
    let user = registration["data"]["user"]["id"].as_str().unwrap();
    let room = create(&server, &owner, "/api/v1/rooms", json!({"name":"team"})).await;
    let room_id = room["id"].as_str().unwrap();
    create(
        &server,
        &owner,
        &format!("/api/v1/rooms/{room_id}/members"),
        json!({"user_id":user,"role":"editor"}),
    )
    .await;
    let doc = create(
        &server,
        &editor,
        "/api/v1/documents",
        json!({"name":"room-doc","document_type":"writer","room_id":room_id}),
    )
    .await;
    let file = doc["file_id"].as_str().unwrap();
    let token = wopi_token(&server, &editor, file).await;
    let share = create(
        &server,
        &editor,
        "/api/v1/shares",
        json!({"file_id":file,"permission":"read"}),
    )
    .await;
    let public_path = format!(
        "/api/v1/public/shares/{}",
        share["public_token"].as_str().unwrap()
    );
    assert_eq!(
        server
            .client
            .get(server.url(&public_path))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/rooms/{room_id}/members/{user}")))
        .headers(server.auth_header(&owner))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = server
        .client
        .get(server.url(&format!("/api/v1/wopi/files/{file}/contents")))
        .query(&[("access_token", &token)])
        .send()
        .await
        .unwrap();
    assert!(matches!(response.status().as_u16(), 403 | 404));
    assert!(matches!(
        wopi_host(
            &server,
            &editor,
            &server.url(&format!("/api/v1/wopi/files/{file}"))
        )
        .await
        .status()
        .as_u16(),
        403 | 404
    ));
    assert_eq!(
        server
            .client
            .get(server.url(&public_path))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let response = server
        .client
        .post(server.url("/api/v1/shares"))
        .headers(server.auth_header(&editor))
        .json(&json!({"file_id":file,"permission":"read"}))
        .send()
        .await
        .unwrap();
    assert!(matches!(response.status().as_u16(), 403 | 404));
}

#[tokio::test]
async fn group_shares_do_not_mint_public_download_capabilities() {
    let server = TestServer::new().await;
    let (owner, _) = server
        .register("groupowner", "groupowner@test.com", "password123")
        .await;
    let group = create(&server, &owner, "/api/v1/groups", json!({"name":"private"})).await;
    let (_, file, _) = server
        .create_document(&owner, "private-file", "writer", None)
        .await;
    let share = create(
        &server,
        &owner,
        "/api/v1/shares",
        json!({"file_id":file,"permission":"read","shared_with_group_id":group["id"]}),
    )
    .await;
    assert!(share["public_token"].is_null());
}

#[tokio::test]
async fn room_viewer_can_read_wopi_without_writing_or_locking() {
    let server = TestServer::new().await;
    let (owner, _) = server
        .register("viewowner", "viewowner@test.com", "password123")
        .await;
    let (viewer, registration) = server
        .register("viewer", "viewer@test.com", "password123")
        .await;
    let room = create(
        &server,
        &owner,
        "/api/v1/rooms",
        json!({"name":"view-team"}),
    )
    .await;
    let room_id = room["id"].as_str().unwrap();
    create(
        &server,
        &owner,
        &format!("/api/v1/rooms/{room_id}/members"),
        json!({
            "user_id":registration["data"]["user"]["id"], "role":"viewer"
        }),
    )
    .await;
    let doc = create(
        &server,
        &owner,
        "/api/v1/documents",
        json!({
            "name":"view-only", "document_type":"writer", "room_id":room_id
        }),
    )
    .await;
    let file = doc["file_id"].as_str().unwrap();
    let token = wopi_token(&server, &viewer, file).await;
    let url = server.url(&format!("/api/v1/wopi/files/{file}"));
    let response = server
        .client
        .get(&url)
        .query(&[("access_token", &token)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let info: Value = response.json().await.unwrap();
    assert_eq!(info["ReadOnly"], true);
    assert_eq!(info["UserCanWrite"], false);
    assert_eq!(info["UserCanRename"], false);
    for operation in ["LOCK", "RENAME_FILE", "PUT_RELATIVE"] {
        let response = server
            .client
            .post(&url)
            .query(&[("access_token", &token)])
            .header("X-WOPI-Override", operation)
            .header("X-WOPI-Lock", "viewer-lock")
            .header("X-WOPI-RequestedName", "forbidden-name")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{operation}");
    }
    let response = server
        .client
        .get(format!("{url}/contents"))
        .query(&[("access_token", &token)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = server
        .client
        .post(format!("{url}/contents"))
        .query(&[("access_token", &token)])
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn batch_and_folder_routes_cannot_bypass_removed_creator_permissions() {
    let server = TestServer::new().await;
    let (owner, _) = server
        .register("batchleader", "batchleader@test.com", "password123")
        .await;
    let (creator, registration) = server
        .register("batchcreator", "batchcreator@test.com", "password123")
        .await;
    let user = registration["data"]["user"]["id"].as_str().unwrap();
    let room = create(
        &server,
        &owner,
        "/api/v1/rooms",
        json!({"name":"batch-team"}),
    )
    .await;
    let room_id = room["id"].as_str().unwrap();
    create(
        &server,
        &owner,
        &format!("/api/v1/rooms/{room_id}/members"),
        json!({"user_id":user,"role":"editor"}),
    )
    .await;
    let folder = create(
        &server,
        &creator,
        "/api/v1/folders",
        json!({"name":"room-folder","room_id":room_id}),
    )
    .await;
    let folder_id = folder["id"].as_str().unwrap();
    let doc = create(
        &server,
        &creator,
        "/api/v1/documents",
        json!({"name":"room-source","document_type":"writer","parent_id":folder_id}),
    )
    .await;
    let file = doc["file_id"].as_str().unwrap();
    let (private_folder, _) = server.create_folder(&creator, "private-target", None).await;
    // Another authorized room editor can download and duplicate the file even
    // though they are not its original file.owner_id.
    let response = server
        .client
        .post(server.url("/api/v1/files/batch/download"))
        .headers(server.auth_header(&owner))
        .json(&json!({"file_ids":[file]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let archive =
        zip::ZipArchive::new(std::io::Cursor::new(response.bytes().await.unwrap())).unwrap();
    assert_eq!(archive.len(), 1);
    let response = server
        .client
        .post(server.url("/api/v1/files/batch/duplicate"))
        .headers(server.auth_header(&owner))
        .json(&json!({"file_ids":[file]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["duplicated_count"],
        1
    );

    let response = server
        .client
        .delete(server.url(&format!("/api/v1/rooms/{room_id}/members/{user}")))
        .headers(server.auth_header(&owner))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    for (operation, body, count_field) in [
        ("duplicate", json!({"file_ids":[file]}), "duplicated_count"),
        ("delete", json!({"file_ids":[file]}), "deleted_count"),
        (
            "move",
            json!({"file_ids":[file],"target_folder_id":private_folder}),
            "moved_count",
        ),
        ("mark-read", json!({"file_ids":[file]}), "marked_count"),
    ] {
        let response = server
            .client
            .post(server.url(&format!("/api/v1/files/batch/{operation}")))
            .headers(server.auth_header(&creator))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "batch {operation}");
        assert_eq!(
            response.json::<Value>().await.unwrap()["data"][count_field],
            0,
            "batch {operation}"
        );
    }
    let response = server
        .client
        .post(server.url("/api/v1/files/batch/download"))
        .headers(server.auth_header(&creator))
        .json(&json!({"file_ids":[file]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let archive =
        zip::ZipArchive::new(std::io::Cursor::new(response.bytes().await.unwrap())).unwrap();
    assert!(archive.is_empty());
    let response = server
        .client
        .put(server.url(&format!("/api/v1/folders/{folder_id}")))
        .headers(server.auth_header(&creator))
        .json(&json!({"name":"forbidden-rename"}))
        .send()
        .await
        .unwrap();
    assert!(matches!(response.status().as_u16(), 403 | 404));
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/folders/{folder_id}")))
        .headers(server.auth_header(&creator))
        .send()
        .await
        .unwrap();
    assert!(matches!(response.status().as_u16(), 403 | 404));
    let still_present = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}")))
        .headers(server.auth_header(&owner))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(still_present["data"]["parent_id"], folder_id);
    assert_eq!(still_present["data"]["is_trashed"], false);
}

#[tokio::test]
async fn batch_move_uses_its_static_route_and_missing_single_items_return_not_found() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("routingowner", "routingowner@test.com", "password123")
        .await;
    let (target, _) = server.create_folder(&jwt, "move-target", None).await;
    let missing = uuid::Uuid::new_v4().to_string();
    let response = server
        .client
        .post(server.url(&format!("/api/v1/files/{missing}/move")))
        .headers(server.auth_header(&jwt))
        .json(&json!({"target_folder_id":target}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let response = server
        .client
        .post(server.url("/api/v1/files/batch/move"))
        .headers(server.auth_header(&jwt))
        .json(&json!({"file_ids":[missing],"target_folder_id":target}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["moved_count"],
        0
    );
    let (_, file, _) = server
        .create_document(&jwt, "movable", "writer", None)
        .await;
    let response = server
        .client
        .post(server.url("/api/v1/files/batch/move"))
        .headers(server.auth_header(&jwt))
        .json(&json!({"file_ids":[file],"target_folder_id":target}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["moved_count"],
        1
    );
    let response = server
        .client
        .get(server.url(&format!("/api/v1/files/{file}")))
        .headers(server.auth_header(&jwt))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["parent_id"],
        target
    );
}

async fn assign_tenant(server: &TestServer, user: &str, tenant: &str) {
    let (user, tenant) = (user.to_owned(), tenant.to_owned());
    server
        .db
        .run(move |conn| {
            diesel::sql_query("UPDATE users SET tenant_id = ? WHERE id = ?")
                .bind::<diesel::sql_types::Text, _>(tenant)
                .bind::<diesel::sql_types::Text, _>(user)
                .execute(conn)
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn tenant_memberships_shares_and_legacy_memberships_do_not_cross_namespaces() {
    use contrix_core::models::{NewGroupMember, NewRoomMember};
    let server = TestServer::new().await;
    let (platform, platform_user) = server
        .register("platform", "platform@test.com", "password123")
        .await;
    server
        .make_admin(platform_user["data"]["user"]["id"].as_str().unwrap())
        .await;
    let a = create(
        &server,
        &platform,
        "/api/v1/admin/tenants",
        json!({"name":"tenant-a"}),
    )
    .await;
    let b = create(
        &server,
        &platform,
        "/api/v1/admin/tenants",
        json!({"name":"tenant-b"}),
    )
    .await;
    let (owner, owner_user) = server
        .register("tenantowner", "tenantowner@test.com", "password123")
        .await;
    let (outsider, outsider_user) = server
        .register("tenantoutsider", "tenantoutsider@test.com", "password123")
        .await;
    let owner_id = owner_user["data"]["user"]["id"].as_str().unwrap();
    let outsider_id = outsider_user["data"]["user"]["id"].as_str().unwrap();
    let room = create(
        &server,
        &owner,
        "/api/v1/rooms",
        json!({"name":"tenant-room"}),
    )
    .await;
    let group = create(
        &server,
        &owner,
        "/api/v1/groups",
        json!({"name":"tenant-group"}),
    )
    .await;
    let foreign_group = create(
        &server,
        &outsider,
        "/api/v1/groups",
        json!({"name":"foreign-group"}),
    )
    .await;
    let (_, file, _) = server
        .create_document(&owner, "tenant-doc", "writer", None)
        .await;
    assign_tenant(&server, owner_id, a["id"].as_str().unwrap()).await;
    assign_tenant(&server, outsider_id, b["id"].as_str().unwrap()).await;
    let room_id = room["id"].as_str().unwrap();
    let group_id = group["id"].as_str().unwrap();
    for path in [
        format!("/api/v1/rooms/{room_id}/members"),
        format!("/api/v1/groups/{group_id}/members"),
    ] {
        let response = server
            .client
            .post(server.url(&path))
            .headers(server.auth_header(&owner))
            .json(&json!({"user_id":outsider_id,"role":"editor"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403, "{path}");
    }
    for target in [
        json!({"shared_with_user_id":outsider_id}),
        json!({"shared_with_group_id":foreign_group["id"]}),
    ] {
        let mut body = target;
        body["file_id"] = json!(file);
        body["permission"] = json!("read");
        let response = server
            .client
            .post(server.url("/api/v1/shares"))
            .headers(server.auth_header(&owner))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
    }
    // Legacy rows or tenant reassignment must not turn a stale membership into
    // a permanent bypass of the tenant boundary.
    let stale_room = NewRoomMember {
        id: uuid::Uuid::new_v4().to_string(),
        room_id: room_id.into(),
        user_id: outsider_id.into(),
        role: "editor".into(),
        joined_at: chrono::Utc::now().to_rfc3339(),
    };
    let stale_group = NewGroupMember {
        id: uuid::Uuid::new_v4().to_string(),
        group_id: group_id.into(),
        user_id: outsider_id.into(),
        added_at: chrono::Utc::now().to_rfc3339(),
    };
    server
        .db
        .run(move |conn| {
            diesel::insert_into(rm::room_members)
                .values(stale_room)
                .execute(conn)?;
            diesel::insert_into(gm::group_members)
                .values(stale_group)
                .execute(conn)?;
            Ok::<_, diesel::result::Error>(())
        })
        .await
        .unwrap();
    for path in [
        format!("/api/v1/rooms/{room_id}"),
        format!("/api/v1/rooms/{room_id}/files"),
        format!("/api/v1/rooms/{room_id}/members"),
        format!("/api/v1/groups/{group_id}"),
        format!("/api/v1/groups/{group_id}/members"),
    ] {
        let response = server
            .client
            .get(server.url(&path))
            .headers(server.auth_header(&outsider))
            .send()
            .await
            .unwrap();
        assert!(matches!(response.status().as_u16(), 403 | 404), "{path}");
    }
    let rooms = server
        .client
        .get(server.url("/api/v1/rooms"))
        .headers(server.auth_header(&outsider))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(
        rooms["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["id"] != room_id)
    );
}

#[tokio::test]
async fn accepted_invitation_inherits_issuer_tenant_and_inactive_tenant_revokes_wopi() {
    use contrix_core::models::NewInvitation;
    let server = TestServer::new().await;
    let (platform, platform_user) = server
        .register("tenantplatform", "tenantplatform@test.com", "password123")
        .await;
    server
        .make_admin(platform_user["data"]["user"]["id"].as_str().unwrap())
        .await;
    let tenant = create(
        &server,
        &platform,
        "/api/v1/admin/tenants",
        json!({"name":"invite-tenant"}),
    )
    .await;
    let tenant_id = tenant["id"].as_str().unwrap();
    let (issuer, issuer_user) = server
        .register("issuer", "issuer@test.com", "password123")
        .await;
    let issuer_id = issuer_user["data"]["user"]["id"].as_str().unwrap();
    server.make_admin(issuer_id).await;
    assign_tenant(&server, issuer_id, tenant_id).await;
    let now = chrono::Utc::now();
    let invitation = NewInvitation {
        id: uuid::Uuid::new_v4().to_string(),
        email: "invited-admin@test.com".into(),
        role: "admin".into(),
        invited_by: issuer_id.into(),
        token: "tenant-invitation-capability".into(),
        message: None,
        is_accepted: false,
        accepted_at: None,
        expires_at: (now + chrono::Duration::hours(1)).to_rfc3339(),
        created_at: now.to_rfc3339(),
    };
    server
        .db
        .run(move |conn| {
            diesel::insert_into(contrix_core::schema::invitations::table)
                .values(invitation)
                .execute(conn)
        })
        .await
        .unwrap();
    let response = server.client.post(server.url("/api/v1/invitations/accept/tenant-invitation-capability"))
        .json(&json!({"username":"invited-admin","email":"invited-admin@test.com","password":"password123"}))
        .send().await.unwrap();
    assert_eq!(response.status(), 201);
    let invited = response.json::<Value>().await.unwrap()["data"]["user_id"]
        .as_str()
        .unwrap()
        .to_owned();
    #[derive(diesel::QueryableByName)]
    struct Tenant {
        #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
        tenant_id: Option<String>,
    }
    let assigned = server
        .db
        .run(move |conn| {
            diesel::sql_query("SELECT tenant_id FROM users WHERE id = ?")
                .bind::<diesel::sql_types::Text, _>(invited)
                .get_result::<Tenant>(conn)
        })
        .await
        .unwrap();
    assert_eq!(assigned.tenant_id.as_deref(), Some(tenant_id));
    let (invited_jwt, _) = server.login("invited-admin@test.com", "password123").await;
    assert_eq!(
        server
            .client
            .get(server.url("/api/v1/admin/stats"))
            .headers(server.auth_header(&invited_jwt))
            .send()
            .await
            .unwrap()
            .status(),
        403,
        "tenant admin cannot become platform admin"
    );
    let (_, file, _) = server
        .create_document(&issuer, "tenant-wopi", "writer", None)
        .await;
    let token = wopi_token(&server, &issuer, &file).await;
    let response = server
        .client
        .delete(server.url(&format!("/api/v1/admin/tenants/{tenant_id}")))
        .headers(server.auth_header(&platform))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response = server
        .client
        .get(server.url(&format!("/api/v1/wopi/files/{file}")))
        .query(&[("access_token", &token)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    assert_eq!(
        wopi_host(
            &server,
            &issuer,
            &server.url(&format!("/api/v1/wopi/files/{file}"))
        )
        .await
        .status(),
        403
    );
}

#[tokio::test]
async fn concurrent_wopi_locks_have_one_winner_and_replacement_requires_current_token() {
    let server = TestServer::new().await;
    let (jwt, _) = server
        .register("locker", "locker@test.com", "password123")
        .await;
    let (_, file, _) = server
        .create_document(&jwt, "locked-file", "writer", None)
        .await;
    let token = wopi_token(&server, &jwt, &file).await;
    let url = server.url(&format!("/api/v1/wopi/files/{file}"));
    let request = |lock: &str| {
        server
            .client
            .post(&url)
            .query(&[("access_token", &token)])
            .header("X-WOPI-Override", "LOCK")
            .header("X-WOPI-Lock", lock)
    };
    let (a, b) = tokio::join!(request("lock-a").send(), request("lock-b").send());
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(
        usize::from(a.status() == 200) + usize::from(b.status() == 200),
        1
    );
    assert_eq!(
        usize::from(a.status() == 409) + usize::from(b.status() == 409),
        1
    );
    let winner = if a.status() == 200 {
        "lock-a"
    } else {
        "lock-b"
    };
    let count_id = file.clone();
    assert_eq!(
        server
            .db
            .run(move |conn| fl::file_locks
                .filter(fl::file_id.eq(count_id))
                .count()
                .get_result::<i64>(conn))
            .await
            .unwrap(),
        1
    );
    let response = request("replacement")
        .header("X-WOPI-OldLock", "stale-lock")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 409);
    let response = request("replacement")
        .header("X-WOPI-OldLock", winner)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    for (lock, expected) in [(winner, 409), ("replacement", 200)] {
        let response = server
            .client
            .post(&url)
            .query(&[("access_token", &token)])
            .header("X-WOPI-Override", "UNLOCK")
            .header("X-WOPI-Lock", lock)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}

async fn insert_reset_token(server: &TestServer, user: &str, token: &str) {
    let now = chrono::Utc::now();
    let reset = NewPasswordResetToken {
        id: uuid::Uuid::new_v4().to_string(),
        user_id: user.into(),
        token: token.into(),
        used: false,
        created_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::minutes(10)).to_rfc3339(),
    };
    server
        .db
        .run(move |conn| {
            diesel::insert_into(prt::password_reset_tokens)
                .values(reset)
                .execute(conn)
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn reset_token_is_consumed_once_and_db_failure_rolls_back_credentials() {
    let server = TestServer::new().await;
    let (jwt, registration) = server
        .register("resetowner", "reset@test.com", "password123")
        .await;
    let user = registration["data"]["user"]["id"].as_str().unwrap();
    insert_reset_token(&server, user, "single-use-reset").await;
    let url = server.url("/api/v1/auth/reset-password");
    let (a, b) = tokio::join!(
        server
            .client
            .post(&url)
            .json(&json!({"token":"single-use-reset","password":"NewPassword123A"}))
            .send(),
        server
            .client
            .post(&url)
            .json(&json!({"token":"single-use-reset","password":"NewPassword123B"}))
            .send(),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(
        usize::from(a.status() == 200) + usize::from(b.status() == 200),
        1
    );
    assert_eq!(
        usize::from(a.status() == 400) + usize::from(b.status() == 400),
        1
    );
    let winner = if a.status() == 200 {
        "NewPassword123A"
    } else {
        "NewPassword123B"
    };
    let (new_jwt, _) = server.login("reset@test.com", winner).await;
    assert_eq!(
        server
            .client
            .get(server.url("/api/v1/auth/me"))
            .headers(server.auth_header(&jwt))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    insert_reset_token(&server, user, "rollback-reset").await;
    server.db.run(|conn| diesel::sql_query(
        "CREATE TRIGGER reject_reset_session_revoke BEFORE UPDATE OF is_active ON sessions WHEN NEW.is_active = 0 BEGIN SELECT RAISE(ABORT, 'injected-revoke-failure'); END"
    ).execute(conn)).await.unwrap();
    let response = server
        .client
        .post(&url)
        .json(&json!({"token":"rollback-reset","password":"ShouldNotCommit123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 500);
    let unused = server
        .db
        .run(|conn| {
            prt::password_reset_tokens
                .filter(prt::token.eq("rollback-reset"))
                .select(prt::used)
                .first::<bool>(conn)
        })
        .await
        .unwrap();
    assert!(
        !unused,
        "the token claim must roll back with credential changes"
    );
    assert_eq!(
        server
            .client
            .get(server.url("/api/v1/auth/me"))
            .headers(server.auth_header(&new_jwt))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let _ = server.login("reset@test.com", winner).await;
}
