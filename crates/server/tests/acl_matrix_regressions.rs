#[allow(dead_code)]
mod common;

use common::TestServer;
use diesel::prelude::*;
use serde_json::{Value, json};
use syntra_core::models::{Document, File, FileLock, FileVersion, Share, Version};
use syntra_core::schema::{
    documents::dsl as d, file_locks::dsl as locks, file_versions::dsl as fv, files::dsl as f,
    shares::dsl as shares, versions::dsl as v,
};

struct Actor {
    role: &'static str,
    jwt: String,
    wopi: String,
    read: bool,
    write: bool,
    admin: bool,
    // An outsider has a valid token for a different file: binding rejects it
    // with 401. The removed creator retains a valid target-file token, so the
    // current room ACL must reject that capability with 403 instead.
    denied_wopi_status: u16,
}

struct Matrix<'a> {
    server: &'a TestServer,
    document: String,
    file: String,
}

impl Matrix<'_> {
    async fn snapshot(&self) -> Value {
        let document = self.document.clone();
        let file = self.file.clone();
        self.server
            .db
            .run(move |conn| {
                // Only the target's business rows are compared. JWT activity,
                // unrelated fixture documents and asynchronous GC are excluded.
                conn.transaction::<_, diesel::result::Error, _>(|conn| {
                    Ok(json!({
                        "document": d::documents.find(&document).first::<Document>(conn)?,
                        "file": f::files.find(&file).first::<File>(conn)?,
                        "history": v::versions.filter(v::document_id.eq(&document))
                            .order(v::id.asc()).load::<Version>(conn)?,
                        "file_history": fv::file_versions.filter(fv::file_id.eq(&file))
                            .order(fv::id.asc()).load::<FileVersion>(conn)?,
                        "shares": shares::shares.filter(shares::file_id.eq(&file))
                            .order(shares::id.asc()).load::<Share>(conn)?,
                        "locks": locks::file_locks.filter(locks::file_id.eq(&file))
                            .order(locks::id.asc()).load::<FileLock>(conn)?,
                    }))
                })
            })
            .await
            .unwrap()
    }

    async fn expect(
        &self,
        role: &str,
        action: &str,
        expected: u16,
        request: reqwest::RequestBuilder,
    ) -> reqwest::Response {
        let before = if expected >= 400 {
            Some(self.snapshot().await)
        } else {
            None
        };
        let response = request.send().await.unwrap();
        let actual = response.status().as_u16();
        assert_eq!(actual, expected, "role={role} action={action}");
        if let Some(before) = before {
            assert_eq!(
                self.snapshot().await,
                before,
                "denial changed target business state: role={role} action={action}"
            );
        }
        response
    }

    async fn document(&self, jwt: &str) -> Value {
        let response = self
            .expect(
                "room owner",
                "verify committed document",
                200,
                self.server
                    .client
                    .get(
                        self.server
                            .url(&format!("/api/v1/documents/{}", self.document)),
                    )
                    .bearer_auth(jwt),
            )
            .await;
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["success"], true);
        body["data"].clone()
    }
}

async fn create(server: &TestServer, jwt: &str, path: &str, body: Value) -> Value {
    let response = server
        .client
        .post(server.url(path))
        .bearer_auth(jwt)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 201, "fixture POST {path}: {body}");
    body["data"].clone()
}

fn host_request(server: &TestServer, jwt: &str, file: &str) -> reqwest::RequestBuilder {
    server
        .client
        .get(server.url("/api/v1/wopi/host/writer/edit"))
        .query(&[
            ("access_token", jwt.to_owned()),
            ("WOPISrc", server.url(&format!("/api/v1/wopi/files/{file}"))),
        ])
}

async fn mint_wopi(server: &TestServer, jwt: &str, file: &str) -> String {
    let response = host_request(server, jwt, file).send().await.unwrap();
    assert_eq!(response.status(), 200, "positive-control WOPI mint");
    let html = response.text().await.unwrap();
    let marker = "const wopiSessionToken = ";
    let start = html.find(marker).unwrap() + marker.len();
    serde_json::from_str(html[start..].split(';').next().unwrap()).unwrap()
}

#[tokio::test]
async fn canonical_room_acl_matrix_covers_rest_and_wopi_without_creator_bypass() {
    let server = TestServer::new().await;
    let (owner, owner_registration) = server
        .register("matrixowner", "matrixowner@test.com", "password123")
        .await;
    let (creator, creator_registration) = server
        .register("matrixcreator", "matrixcreator@test.com", "password123")
        .await;
    let (editor, editor_registration) = server
        .register("matrixeditor", "matrixeditor@test.com", "password123")
        .await;
    let (viewer, viewer_registration) = server
        .register("matrixviewer", "matrixviewer@test.com", "password123")
        .await;
    let (outsider, _) = server
        .register("matrixoutsider", "matrixoutsider@test.com", "password123")
        .await;
    let room = create(
        &server,
        &owner,
        "/api/v1/rooms",
        json!({"name":"ACL matrix"}),
    )
    .await;
    let room_id = room["id"].as_str().unwrap();
    for (registration, role) in [
        (&creator_registration, "editor"),
        (&editor_registration, "editor"),
        (&viewer_registration, "viewer"),
    ] {
        create(
            &server,
            &owner,
            &format!("/api/v1/rooms/{room_id}/members"),
            json!({"user_id":registration["data"]["user"]["id"], "role":role}),
        )
        .await;
    }
    let document = create(
        &server,
        &creator,
        "/api/v1/documents",
        json!({"name":"room-owned policy", "document_type":"writer", "room_id":room_id}),
    )
    .await;
    let matrix = Matrix {
        server: &server,
        document: document["id"].as_str().unwrap().to_owned(),
        file: document["file_id"].as_str().unwrap().to_owned(),
    };
    let creator_id = creator_registration["data"]["user"]["id"].as_str().unwrap();
    let before_removal = matrix.snapshot().await;
    assert_eq!(before_removal["file"]["owner_id"], creator_id);
    assert_ne!(
        before_removal["file"]["owner_id"], owner_registration["data"]["user"]["id"],
        "the room owner must not be the document creator"
    );

    // Establish genuine capabilities while the creator is still an editor.
    let creator_wopi = mint_wopi(&server, &creator, &matrix.file).await;
    let wopi_url = server.url(&format!("/api/v1/wopi/files/{}", matrix.file));
    matrix
        .expect(
            "creator before removal",
            "WOPI get positive control",
            200,
            server
                .client
                .get(format!("{wopi_url}/contents"))
                .query(&[("access_token", &creator_wopi)]),
        )
        .await;
    let old_share = create(
        &server,
        &creator,
        "/api/v1/shares",
        json!({"file_id":matrix.file, "permission":"read"}),
    )
    .await;
    let old_public_url = server.url(&format!(
        "/api/v1/public/shares/{}",
        old_share["public_token"].as_str().unwrap()
    ));
    assert_eq!(
        server
            .client
            .get(&old_public_url)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let removed = server
        .client
        .delete(server.url(&format!("/api/v1/rooms/{room_id}/members/{creator_id}")))
        .bearer_auth(&owner)
        .send()
        .await
        .unwrap();
    assert_eq!(removed.status(), 200);
    matrix
        .expect(
            "removed creator",
            "previously usable public share",
            404,
            server.client.get(&old_public_url),
        )
        .await;

    // The outsider never joins this room. Their own-file token tests binding
    // without constructing an impossible authorized target-file capability.
    let (_, outsider_file, _) = server
        .create_document(&outsider, "outsider private", "writer", None)
        .await;
    let outsider_wopi = mint_wopi(&server, &outsider, &outsider_file).await;
    let actors = [
        Actor {
            role: "owner (not creator)",
            jwt: owner.clone(),
            wopi: mint_wopi(&server, &owner, &matrix.file).await,
            read: true,
            write: true,
            admin: true,
            denied_wopi_status: 403,
        },
        Actor {
            role: "editor",
            jwt: editor.clone(),
            wopi: mint_wopi(&server, &editor, &matrix.file).await,
            read: true,
            write: true,
            admin: false,
            denied_wopi_status: 403,
        },
        Actor {
            role: "viewer",
            jwt: viewer.clone(),
            wopi: mint_wopi(&server, &viewer, &matrix.file).await,
            read: true,
            write: false,
            admin: false,
            denied_wopi_status: 403,
        },
        Actor {
            role: "removed creator",
            jwt: creator,
            wopi: creator_wopi,
            read: false,
            write: false,
            admin: false,
            denied_wopi_status: 403,
        },
        Actor {
            role: "outsider",
            jwt: outsider,
            wopi: outsider_wopi,
            read: false,
            write: false,
            admin: false,
            denied_wopi_status: 401,
        },
    ];
    let document_url = server.url(&format!("/api/v1/documents/{}", matrix.document));
    let mut current = matrix.document(&owner).await;
    for actor in &actors {
        // Viewer: read/export/history. Editor: save/read-share/WOPI write+lock.
        // Admin: delete/admin-share. These expectations do not follow responses.
        let read_status = if actor.read { 200 } else { 403 };
        let write_status = if actor.write { 200 } else { 403 };
        let response = matrix
            .expect(
                actor.role,
                "read",
                read_status,
                server.client.get(&document_url).bearer_auth(&actor.jwt),
            )
            .await;
        if actor.read {
            let body: Value = response.json().await.unwrap();
            assert_eq!(
                body["data"]["content"], current["content"],
                "{} read",
                actor.role
            );
            assert_eq!(
                body["data"]["version"], current["version"],
                "{} read",
                actor.role
            );
        }
        let mut next = current["content"].clone();
        next["title"] = json!(format!("REST save by {}", actor.role));
        matrix
            .expect(
                actor.role,
                "save",
                write_status,
                server
                    .client
                    .put(&document_url)
                    .bearer_auth(&actor.jwt)
                    .json(&json!({"expected_version":current["version"], "content":next})),
            )
            .await;
        if actor.write {
            let saved = matrix.document(&owner).await;
            assert_eq!(saved["content"], next, "{} save content", actor.role);
            assert_eq!(
                saved["version"].as_i64().unwrap(),
                current["version"].as_i64().unwrap() + 1
            );
            current = saved;
        }
        let exported = matrix
            .expect(
                actor.role,
                "export",
                read_status,
                server
                    .client
                    .post(format!("{document_url}/export/json"))
                    .bearer_auth(&actor.jwt),
            )
            .await;
        if actor.read {
            assert_eq!(
                exported.json::<Value>().await.unwrap(),
                current["content"],
                "{} export",
                actor.role
            );
        }
        let history = matrix
            .expect(
                actor.role,
                "history",
                read_status,
                server
                    .client
                    .get(format!("{document_url}/versions"))
                    .bearer_auth(&actor.jwt),
            )
            .await;
        if actor.read {
            let body: Value = history.json().await.unwrap();
            let rows = body["data"].as_array().unwrap();
            let snapshot = matrix.snapshot().await;
            let stored = snapshot["history"].as_array().unwrap();
            assert!(
                !stored.is_empty(),
                "{} history positive control",
                actor.role
            );
            assert_eq!(rows.len(), stored.len(), "{} history count", actor.role);
            for row in stored {
                let visible = rows.iter().find(|visible| visible["id"] == row["id"]);
                let visible =
                    visible.unwrap_or_else(|| panic!("{} missing history row", actor.role));
                assert_eq!(visible["version_number"], row["version_number"]);
                assert_eq!(visible["created_by"], row["created_by"]);
            }
        }
        for (permission, allowed) in [("read", actor.write), ("admin", actor.admin)] {
            let response = matrix
                .expect(
                    actor.role,
                    &format!("share/{permission}"),
                    if allowed { 201 } else { 403 },
                    server
                        .client
                        .post(server.url("/api/v1/shares"))
                        .bearer_auth(&actor.jwt)
                        .json(&json!({"file_id":matrix.file, "permission":permission})),
                )
                .await;
            if allowed {
                let body: Value = response.json().await.unwrap();
                let id = body["data"]["id"].as_str().unwrap();
                let state = matrix.snapshot().await;
                let share = state["shares"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|share| share["id"] == id)
                    .unwrap();
                assert_eq!(share["file_id"], matrix.file);
                assert_eq!(share["permission"], permission);
            }
        }
        matrix
            .expect(
                actor.role,
                "WOPI mint",
                read_status,
                host_request(&server, &actor.jwt, &matrix.file),
            )
            .await;
        let wopi_read = if actor.read {
            200
        } else {
            actor.denied_wopi_status
        };
        let wopi_write = if actor.write {
            200
        } else {
            actor.denied_wopi_status
        };
        let info = matrix
            .expect(
                actor.role,
                "WOPI info",
                wopi_read,
                server
                    .client
                    .get(&wopi_url)
                    .query(&[("access_token", &actor.wopi)]),
            )
            .await;
        if actor.read {
            let info: Value = info.json().await.unwrap();
            assert_eq!(info["UserCanWrite"], actor.write);
            assert_eq!(info["ReadOnly"], !actor.write);
        }
        let contents = matrix
            .expect(
                actor.role,
                "WOPI get",
                wopi_read,
                server
                    .client
                    .get(format!("{wopi_url}/contents"))
                    .query(&[("access_token", &actor.wopi)]),
            )
            .await;
        if actor.read {
            assert_eq!(contents.json::<Value>().await.unwrap(), current["content"]);
        }
        let mut next = current["content"].clone();
        next["title"] = json!(format!("WOPI save by {}", actor.role));
        matrix
            .expect(
                actor.role,
                "WOPI put",
                wopi_write,
                server
                    .client
                    .post(format!("{wopi_url}/contents"))
                    .query(&[("access_token", &actor.wopi)])
                    .body(serde_json::to_vec(&next).unwrap()),
            )
            .await;
        if actor.write {
            let saved = matrix.document(&owner).await;
            assert_eq!(saved["content"], next, "{} WOPI content", actor.role);
            assert_eq!(
                saved["version"].as_i64().unwrap(),
                current["version"].as_i64().unwrap() + 1
            );
            current = saved;
        }
        let lock = format!("matrix-lock-{}", actor.role);
        matrix
            .expect(
                actor.role,
                "WOPI lock",
                wopi_write,
                server
                    .client
                    .post(&wopi_url)
                    .query(&[("access_token", &actor.wopi)])
                    .header("X-WOPI-Override", "LOCK")
                    .header("X-WOPI-Lock", &lock),
            )
            .await;
        if actor.write {
            let state = matrix.snapshot().await;
            assert_eq!(state["locks"].as_array().unwrap().len(), 1);
            assert_eq!(state["locks"][0]["lock_token"], lock);
            matrix
                .expect(
                    actor.role,
                    "WOPI unlock",
                    200,
                    server
                        .client
                        .post(&wopi_url)
                        .query(&[("access_token", &actor.wopi)])
                        .header("X-WOPI-Override", "UNLOCK")
                        .header("X-WOPI-Lock", &lock),
                )
                .await;
            assert!(
                matrix.snapshot().await["locks"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
        if !actor.admin {
            matrix
                .expect(
                    actor.role,
                    "delete",
                    403,
                    server.client.delete(&document_url).bearer_auth(&actor.jwt),
                )
                .await;
        }
    }
    // Successful deletion is last so all other matrix cells see the live file.
    matrix
        .expect(
            actors[0].role,
            "delete",
            200,
            server.client.delete(&document_url).bearer_auth(&owner),
        )
        .await;
    let final_state = matrix.snapshot().await;
    assert_eq!(final_state["file"]["is_trashed"], true);
    assert_eq!(final_state["document"]["version"], current["version"]);
    assert_eq!(final_state["file"]["owner_id"], creator_id);
}
