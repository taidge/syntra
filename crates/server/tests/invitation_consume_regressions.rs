#[allow(dead_code)]
mod common;

use common::TestServer;
use diesel::RunQueryDsl;
use diesel::sql_types::Text;
use serde_json::{Value, json};

async fn setup() -> (TestServer, String, String) {
    let server = TestServer::new().await;
    let (_, body) = server
        .register("issuer", "issuer@test.invalid", "Password123!")
        .await;
    server
        .make_admin(body["data"]["user"]["id"].as_str().unwrap())
        .await;
    let (admin, _) = server.login("issuer@test.invalid", "Password123!").await;
    server
        .register("existing", "existing@test.invalid", "Original123!")
        .await;
    let response = server
        .client
        .post(server.url("/api/v1/invitations"))
        .bearer_auth(&admin)
        .json(&json!({"email":"existing@test.invalid","role":"admin"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let body: Value = response.json().await.unwrap();
    (
        server,
        body["data"]["id"].as_str().unwrap().to_owned(),
        body["data"]["token"].as_str().unwrap().to_owned(),
    )
}

async fn accept(server: &TestServer, token: &str) -> reqwest::Response {
    server.client.post(server.url(&format!("/api/v1/invitations/accept/{token}"))).json(&json!({"username":"ignored_existing_name","email":"existing@test.invalid","password":"Different123!"})).send().await.unwrap()
}

async fn invitation_state(server: &TestServer, id: &str) -> Value {
    #[derive(diesel::QueryableByName)]
    struct Snapshot {
        #[diesel(sql_type = Text)]
        value: String,
    }
    let id = id.to_owned();
    let row = server.db.run(move |conn| {
        diesel::sql_query("SELECT json_object('accepted',is_accepted,'accepted_at',accepted_at,'users',(SELECT json_group_array(json_array(id,username,email,password_hash,role,tenant_id)) FROM (SELECT * FROM users ORDER BY id))) AS value FROM invitations WHERE id=?").bind::<Text,_>(id).get_result::<Snapshot>(conn)
    }).await.unwrap();
    serde_json::from_str(&row.value).unwrap()
}

#[tokio::test]
async fn existing_user_invitation_write_failure_is_reported_without_consumption_or_account_changes()
{
    let (server, id, token) = setup().await;
    let before = invitation_state(&server, &id).await;
    server.db.run(|conn| diesel::sql_query("CREATE TRIGGER fail_existing_invitation BEFORE UPDATE OF is_accepted ON invitations BEGIN SELECT RAISE(ABORT, 'injected invitation consume failure'); END").execute(conn)).await.unwrap();
    let response = accept(&server, &token).await;
    let status = response.status();
    let body = response.text().await.unwrap();
    assert_eq!(
        status, 500,
        "necessary invitation write must not be reported as success: {body}"
    );
    assert_eq!(invitation_state(&server, &id).await, before);
    server
        .db
        .run(|conn| diesel::sql_query("DROP TRIGGER fail_existing_invitation").execute(conn))
        .await
        .unwrap();
    assert_eq!(accept(&server, &token).await.status(), 200);
    let after = invitation_state(&server, &id).await;
    assert_eq!(after["accepted"], 1);
    assert!(after["accepted_at"].is_string());
    assert_eq!(
        after["users"], before["users"],
        "existing account role, password and tenant must not be replaced"
    );
    assert!(matches!(
        accept(&server, &token).await.status().as_u16(),
        404 | 409
    ));
    assert_eq!(invitation_state(&server, &id).await, after);
    let (_, login) = server.login("existing@test.invalid", "Original123!").await;
    assert_eq!(login["data"]["user"]["username"], "existing");
}

#[tokio::test]
async fn concurrent_existing_user_acceptance_consumes_the_invitation_once() {
    let (server, id, token) = setup().await;
    let before = invitation_state(&server, &id).await;
    let (first, second) = tokio::join!(accept(&server, &token), accept(&server, &token));
    let mut statuses = [first.status().as_u16(), second.status().as_u16()];
    statuses.sort();
    assert!(
        matches!(statuses, [200, 404] | [200, 409]),
        "exactly one existing-user acceptance may succeed: {statuses:?}"
    );
    let after = invitation_state(&server, &id).await;
    assert_eq!(after["accepted"], 1);
    assert_eq!(after["users"], before["users"]);
}
