#[allow(dead_code)]
mod common;

use common::TestServer;
use diesel::RunQueryDsl;
use diesel::sql_types::Text;
use serde_json::{Value, json};

struct Fixture {
    server: TestServer,
    jwt: String,
    wopi: String,
    room: String,
    source: String,
    target: String,
    target_document: String,
}

async fn create(server: &TestServer, jwt: &str, path: &str, value: Value) -> Value {
    let response = server
        .client
        .post(server.url(path))
        .bearer_auth(jwt)
        .json(&value)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 201, "{path}: {body}");
    body["data"].clone()
}

async fn mint(server: &TestServer, jwt: &str, file: &str) -> String {
    let source = server.url(&format!("/api/v1/wopi/files/{file}"));
    let response = server
        .client
        .get(server.url("/api/v1/wopi/host/writer/edit"))
        .query(&[("access_token", jwt), ("WOPISrc", source.as_str())])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let html = response.text().await.unwrap();
    let marker = "const wopiSessionToken = ";
    let start = html.find(marker).unwrap() + marker.len();
    serde_json::from_str(html[start..].split(';').next().unwrap()).unwrap()
}

async fn fixture() -> Fixture {
    let server = TestServer::new().await;
    let (admin, _) = server
        .register("room_admin", "room_admin@test.invalid", "Password123!")
        .await;
    let (member, member_registration) = server
        .register("room_member", "room_member@test.invalid", "Password123!")
        .await;
    let room = create(
        &server,
        &admin,
        "/api/v1/rooms",
        json!({"name":"shared namespace"}),
    )
    .await;
    let room = room["id"].as_str().unwrap().to_owned();
    create(
        &server,
        &admin,
        &format!("/api/v1/rooms/{room}/members"),
        json!({"user_id":member_registration["data"]["user"]["id"],"role":"editor"}),
    )
    .await;
    let source = create(
        &server,
        &admin,
        "/api/v1/documents",
        json!({"name":"source","document_type":"writer","room_id":room}),
    )
    .await;
    let target = create(
        &server,
        &member,
        "/api/v1/documents",
        json!({"name":"taken","document_type":"writer","room_id":room}),
    )
    .await;
    let source = source["file_id"].as_str().unwrap().to_owned();
    let wopi = mint(&server, &admin, &source).await;
    let fixture = Fixture {
        server,
        jwt: admin,
        wopi,
        room,
        source,
        target: target["file_id"].as_str().unwrap().to_owned(),
        target_document: target["id"].as_str().unwrap().to_owned(),
    };
    assert_ne!(
        file_snapshot(&fixture.server, &fixture.source).await["owner"],
        file_snapshot(&fixture.server, &fixture.target).await["owner"]
    );
    fixture
}

async fn file_snapshot(server: &TestServer, id: &str) -> Value {
    #[derive(diesel::QueryableByName)]
    struct Snapshot {
        #[diesel(sql_type=Text)]
        value: String,
    }
    let id = id.to_owned();
    let value = server.db.run(move |conn| {
        diesel::sql_query("SELECT json_object('id',files.id,'name',files.name,'owner',files.owner_id,'parent',files.parent_id,'room',files.room_id,'is_folder',files.is_folder,'path',files.path,'size',files.size,'updated_at',files.updated_at,'version',documents.version,'content',documents.content) AS value FROM files LEFT JOIN documents ON documents.file_id=files.id WHERE files.id=?").bind::<Text,_>(id).get_result::<Snapshot>(conn)
    }).await.unwrap().value;
    serde_json::from_str(&value).unwrap()
}

async fn room_names(fixture: &Fixture) -> Value {
    #[derive(diesel::QueryableByName)]
    struct Snapshot {
        #[diesel(sql_type=Text)]
        value: String,
    }
    let room = fixture.room.clone();
    let value = fixture.server.db.run(move |conn| {
        diesel::sql_query("SELECT json_group_array(json_array(id,name,owner_id)) AS value FROM (SELECT id,name,owner_id FROM files WHERE room_id=? AND parent_id IS NULL AND is_trashed=0 ORDER BY id)").bind::<Text,_>(room).get_result::<Snapshot>(conn)
    }).await.unwrap().value;
    serde_json::from_str(&value).unwrap()
}

fn operation(fixture: &Fixture, name: &str) -> reqwest::RequestBuilder {
    fixture
        .server
        .client
        .post(
            fixture
                .server
                .url(&format!("/api/v1/wopi/files/{}", fixture.source)),
        )
        .query(&[("access_token", &fixture.wopi)])
        .header("X-WOPI-Override", name)
}

#[tokio::test]
async fn relative_new_save_rejects_another_owners_existing_room_name() {
    let fixture = fixture().await;
    let source = file_snapshot(&fixture.server, &fixture.source).await;
    let target = file_snapshot(&fixture.server, &fixture.target).await;
    let names = room_names(&fixture).await;
    let response = operation(&fixture, "PUT_RELATIVE")
        .header("X-WOPI-RelativeTarget", "taken")
        .body("new bytes must not create a duplicate")
        .send()
        .await
        .unwrap();
    let status = response.status();
    let suggestion = response
        .headers()
        .get("X-WOPI-ValidRelativeTarget")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.text().await.unwrap();
    assert_eq!(
        status, 409,
        "all room members share one sibling namespace: {body}"
    );
    assert_ne!(suggestion.as_deref(), Some("taken"));
    let suggestion = suggestion.expect("a naming conflict must suggest an available sibling name");
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.source).await,
        source
    );
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.target).await,
        target
    );
    assert_eq!(room_names(&fixture).await, names);

    let response = operation(&fixture, "PUT_RELATIVE")
        .header("X-WOPI-RelativeTarget", &suggestion)
        .body("legitimate new sibling")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["Name"], suggestion);
    assert_eq!(room_names(&fixture).await.as_array().unwrap().len(), 3);
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.source).await,
        source
    );
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.target).await,
        target
    );
}

#[tokio::test]
async fn relative_overwrite_updates_the_authorized_existing_room_target_without_a_duplicate() {
    let fixture = fixture().await;
    let source = file_snapshot(&fixture.server, &fixture.source).await;
    let target = file_snapshot(&fixture.server, &fixture.target).await;
    let replacement = json!({"text":"authorized cross-owner room overwrite"});
    let response = operation(&fixture, "PUT_RELATIVE")
        .header("X-WOPI-RelativeTarget", "taken")
        .header("X-WOPI-OverwriteRelativeTarget", "true")
        .body(replacement.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert!(
        body["Url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/api/v1/wopi/files/{}", fixture.target)),
        "overwrite must address the existing target: {body}"
    );
    let after = file_snapshot(&fixture.server, &fixture.target).await;
    assert_eq!(after["owner"], target["owner"]);
    assert_eq!(
        after["version"].as_i64().unwrap(),
        target["version"].as_i64().unwrap() + 1
    );
    assert_eq!(
        serde_json::from_str::<Value>(after["content"].as_str().unwrap()).unwrap(),
        replacement
    );
    assert_eq!(room_names(&fixture).await.as_array().unwrap().len(), 2);
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.source).await,
        source
    );
    let response = fixture
        .server
        .client
        .get(
            fixture
                .server
                .url(&format!("/api/v1/documents/{}", fixture.target_document)),
        )
        .bearer_auth(&fixture.jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"]["content"],
        replacement
    );
}

#[tokio::test]
async fn rename_rejects_another_owners_room_sibling_but_allows_an_available_name() {
    let fixture = fixture().await;
    let source = file_snapshot(&fixture.server, &fixture.source).await;
    let target = file_snapshot(&fixture.server, &fixture.target).await;
    let names = room_names(&fixture).await;
    let response = operation(&fixture, "RENAME_FILE")
        .header("X-WOPI-RequestedName", "taken")
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(
        status, 409,
        "rename must use the same room namespace as REST: {body}"
    );
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.source).await,
        source
    );
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.target).await,
        target
    );
    assert_eq!(room_names(&fixture).await, names);
    assert_eq!(
        operation(&fixture, "RENAME_FILE")
            .header("X-WOPI-RequestedName", "available")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.source).await["name"],
        "available"
    );
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.target).await,
        target
    );
    assert_eq!(room_names(&fixture).await.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn private_and_separate_room_names_do_not_block_each_others_wopi_renames() {
    let fixture = fixture().await;
    let private = create(
        &fixture.server,
        &fixture.jwt,
        "/api/v1/documents",
        json!({"name":"private_taken","document_type":"writer"}),
    )
    .await;
    let private_file = private["file_id"].as_str().unwrap();
    let private_before = file_snapshot(&fixture.server, private_file).await;
    let other_room = create(
        &fixture.server,
        &fixture.jwt,
        "/api/v1/rooms",
        json!({"name":"separate room"}),
    )
    .await;
    let other = create(
        &fixture.server,
        &fixture.jwt,
        "/api/v1/documents",
        json!({"name":"otherroom_taken","document_type":"writer","room_id":other_room["id"]}),
    )
    .await;
    let other_file = other["file_id"].as_str().unwrap();
    let other_before = file_snapshot(&fixture.server, other_file).await;
    let target_before = file_snapshot(&fixture.server, &fixture.target).await;

    // All three files have the same owner and no parent, but their room/private
    // namespaces differ. A room rename must ignore both unrelated names.
    for name in ["private_taken", "otherroom_taken"] {
        let response = operation(&fixture, "RENAME_FILE")
            .header("X-WOPI-RequestedName", name)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(
            status, 200,
            "unrelated namespaces must not block {name}: {body}"
        );
        let source = file_snapshot(&fixture.server, &fixture.source).await;
        assert_eq!(source["name"], name);
        assert_eq!(source["room"], fixture.room);
        assert_eq!(room_names(&fixture).await.as_array().unwrap().len(), 2);
    }

    // The reverse direction also permits an available private name even when
    // that name is occupied by the same owner in two separate rooms.
    let private_source = create(
        &fixture.server,
        &fixture.jwt,
        "/api/v1/documents",
        json!({"name":"private_source","document_type":"writer"}),
    )
    .await;
    let private_source = private_source["file_id"].as_str().unwrap();
    let token = mint(&fixture.server, &fixture.jwt, private_source).await;
    let response = fixture
        .server
        .client
        .post(
            fixture
                .server
                .url(&format!("/api/v1/wopi/files/{private_source}")),
        )
        .query(&[("access_token", &token)])
        .header("X-WOPI-Override", "RENAME_FILE")
        .header("X-WOPI-RequestedName", "otherroom_taken")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let private_source = file_snapshot(&fixture.server, private_source).await;
    assert_eq!(private_source["name"], "otherroom_taken");
    assert!(private_source["room"].is_null());
    assert_eq!(
        file_snapshot(&fixture.server, private_file).await,
        private_before
    );
    assert_eq!(
        file_snapshot(&fixture.server, other_file).await,
        other_before
    );
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.target).await,
        target_before
    );
}

#[tokio::test]
async fn relative_overwrite_cannot_turn_a_sibling_folder_into_a_billable_blob() {
    let fixture = fixture().await;
    let folder = create(
        &fixture.server,
        &fixture.jwt,
        "/api/v1/folders",
        json!({"name":"foldertarget","room_id":fixture.room}),
    )
    .await;
    let folder = folder["id"].as_str().unwrap();
    let child = create(&fixture.server, &fixture.jwt, "/api/v1/documents", json!({"name":"preserved child","document_type":"writer","parent_id":folder,"room_id":fixture.room})).await;
    let child = child["file_id"].as_str().unwrap();
    let folder_before = file_snapshot(&fixture.server, folder).await;
    let child_before = file_snapshot(&fixture.server, child).await;
    let source_before = file_snapshot(&fixture.server, &fixture.source).await;
    let target_before = file_snapshot(&fixture.server, &fixture.target).await;
    let names_before = room_names(&fixture).await;
    #[derive(diesel::QueryableByName)]
    struct Accounting {
        #[diesel(sql_type=Text)]
        value: String,
    }
    async fn accounting(server: &TestServer) -> String {
        server.db.run(|conn| diesel::sql_query("SELECT json_object('quota',(SELECT json_group_array(json_array(id,storage_used)) FROM (SELECT id,storage_used FROM users ORDER BY id)),'files',(SELECT COUNT(*) FROM files),'documents',(SELECT COUNT(*) FROM documents),'versions',(SELECT COUNT(*) FROM versions)) AS value").get_result::<Accounting>(conn)).await.unwrap().value
    }
    let accounting_before = accounting(&fixture.server).await;
    assert_eq!(folder_before["is_folder"], 1);
    let response = operation(&fixture, "PUT_RELATIVE")
        .header("X-WOPI-RelativeTarget", "foldertarget")
        .header("X-WOPI-OverwriteRelativeTarget", "true")
        .body("must not become folder content")
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(
        status, 409,
        "a folder is a name conflict, never an overwriteable file: {body}"
    );
    assert_eq!(file_snapshot(&fixture.server, folder).await, folder_before);
    assert_eq!(file_snapshot(&fixture.server, child).await, child_before);
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.source).await,
        source_before
    );
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.target).await,
        target_before
    );
    assert_eq!(room_names(&fixture).await, names_before);
    assert_eq!(accounting(&fixture.server).await, accounting_before);

    assert_eq!(
        operation(&fixture, "PUT_RELATIVE")
            .header("X-WOPI-RelativeTarget", "legitimate file")
            .body("legitimate new sibling")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(file_snapshot(&fixture.server, folder).await, folder_before);
    assert_eq!(file_snapshot(&fixture.server, child).await, child_before);
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.source).await,
        source_before
    );
    assert_eq!(
        file_snapshot(&fixture.server, &fixture.target).await,
        target_before
    );
}
