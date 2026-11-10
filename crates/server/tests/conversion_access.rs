// Each integration binary uses a different subset of the shared server fixture.
#[allow(dead_code)]
mod common;
use common::TestServer;

#[tokio::test]
async fn room_conversion_requires_current_source_membership_for_creation_status_and_download() {
    let server = TestServer::new().await;
    let (admin, _) = server
        .register("room_admin", "room_admin@review.test", "password123")
        .await;
    let (creator, creator_body) = server
        .register("creator", "creator@review.test", "password123")
        .await;
    let (editor, editor_body) = server
        .register("editor", "editor@review.test", "password123")
        .await;
    let response = server
        .client
        .post(server.url("/api/v1/rooms"))
        .headers(server.auth_header(&admin))
        .json(&serde_json::json!({"name":"Conversion ACL"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let room: serde_json::Value = response.json().await.unwrap();
    let room_id = room["data"]["id"].as_str().unwrap();
    for body in [&creator_body, &editor_body] {
        let response = server
            .client
            .post(server.url(&format!("/api/v1/rooms/{room_id}/members")))
            .headers(server.auth_header(&admin))
            .json(&serde_json::json!({"user_id":body["data"]["user"]["id"],"role":"editor"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
    }
    let response = server
        .client
        .post(server.url("/api/v1/documents"))
        .headers(server.auth_header(&creator))
        .json(
            &serde_json::json!({"name":"Room document","document_type":"writer","room_id":room_id}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let document: serde_json::Value = response.json().await.unwrap();
    let file_id = document["data"]["file_id"].as_str().unwrap();
    let mut jobs = Vec::new();
    for token in [&creator, &editor] {
        let response = server
            .client
            .post(server.url("/api/v1/convert"))
            .headers(server.auth_header(token))
            .json(&serde_json::json!({"file_id":file_id,"target_format":"html"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            201,
            "current room editor can convert someone else's document"
        );
        let job: serde_json::Value = response.json().await.unwrap();
        let id = job["data"]["id"].as_str().unwrap().to_owned();
        let mut completed = false;
        for _ in 0..100 {
            let response = server
                .client
                .get(server.url(&format!("/api/v1/convert/{id}")))
                .headers(server.auth_header(token))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let body: serde_json::Value = response.json().await.unwrap();
            if body["data"]["status"] == "completed" {
                completed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(completed);
        jobs.push(id);
    }
    for (index, (token, body)) in [(&creator, &creator_body), (&editor, &editor_body)]
        .into_iter()
        .enumerate()
    {
        let uid = body["data"]["user"]["id"].as_str().unwrap();
        let response = server
            .client
            .delete(server.url(&format!("/api/v1/rooms/{room_id}/members/{uid}")))
            .headers(server.auth_header(&admin))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let response = server
            .client
            .post(server.url("/api/v1/convert"))
            .headers(server.auth_header(token))
            .json(&serde_json::json!({"file_id":file_id,"target_format":"html"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            403,
            "original creator privilege cannot bypass removal"
        );
        for suffix in ["", "/download"] {
            let response = server
                .client
                .get(server.url(&format!("/api/v1/convert/{}{suffix}", jobs[index])))
                .headers(server.auth_header(token))
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                403,
                "old conversion cannot bypass revocation"
            );
        }
    }
}
