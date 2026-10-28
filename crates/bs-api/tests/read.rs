#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Интеграционные тесты чтения (spaces/channels/messages).

use bs_api::{ApiClient, PagingCursor};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SPACE_JSON: &str = r#"{
    "id":"s1","name":"Orbit","icon":null,"banner":null,"avatarColor":"mint",
    "ownerId":"u1","ownerTitle":null,"inviteCode":"abc","visibility":"public",
    "directoryListed":true,"description":null,"createdAt":1750000000000
}"#;

const CHANNEL_JSON: &str = r#"{
    "id":"c1","spaceId":"s1","name":"general","type":"text","topic":null,
    "position":0,"categoryId":null,"createdAt":1750000000000
}"#;

const USER_JSON: &str = r#"{
    "id":"u1","username":"alice","displayName":null,"avatar":null,"banner":null,
    "accentColor":null,"avatarColor":null,"bio":null,"status":"online",
    "customStatus":null,"isAdmin":false,"createdAt":1750000000000,
    "homeInstance":null,"homeUserId":null,"replicatedInstances":[]
}"#;

fn message_json(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "channelId": "c1",
        "userId": "u1",
        "replyToId": null,
        "content": "hi",
        "editedAt": null,
        "createdAt": 1_750_000_000_000_i64,
        "user": serde_json::from_str::<serde_json::Value>(USER_JSON).unwrap(),
        "attachments": [],
        "embeds": [],
        "reactions": []
    })
}

fn client(server: &MockServer) -> ApiClient {
    ApiClient::home(server.uri()).unwrap()
}

#[tokio::test]
async fn lists_spaces() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/spaces"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!("[{SPACE_JSON}]")))
        .mount(&server)
        .await;

    let spaces = client(&server).spaces().await.unwrap();
    assert_eq!(spaces.len(), 1);
    assert_eq!(spaces[0].id, "s1");
    assert_eq!(spaces[0].avatar_color.as_deref(), Some("mint"));
}

#[tokio::test]
async fn lists_channels() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/spaces/s1/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!("[{CHANNEL_JSON}]")))
        .mount(&server)
        .await;

    let channels = client(&server).channels(&"s1".to_owned()).await.unwrap();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].name, "general");
    assert_eq!(channels[0].channel_type, bs_types::ChannelType::Text);
}

#[tokio::test]
async fn messages_sends_limit_and_before() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/channels/c1/messages"))
        .and(query_param("limit", "2"))
        .and(query_param("before", "m9"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([message_json("m1"), message_json("m2")])),
        )
        .mount(&server)
        .await;

    let page = client(&server)
        .messages("c1", Some(PagingCursor::Before("m9".into())), Some(2))
        .await
        .unwrap();
    assert_eq!(page.messages.len(), 2);
    assert!(!page.paged_after);
}

#[tokio::test]
async fn messages_detects_paging_after_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/channels/c1/messages"))
        .and(query_param("after", "m1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-backspace-paging", "after")
                .set_body_json(json!([message_json("m2")])),
        )
        .mount(&server)
        .await;

    let page = client(&server)
        .messages("c1", Some(PagingCursor::After("m1".into())), None)
        .await
        .unwrap();
    assert!(page.paged_after);
}

#[tokio::test]
async fn messages_clamps_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/channels/c1/messages"))
        .and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let page = client(&server)
        .messages("c1", None, Some(500))
        .await
        .unwrap();
    assert!(page.messages.is_empty());
}

#[tokio::test]
async fn space_not_found_maps_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/spaces/nope"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error":"Space not found","code":"space_not_found"
        })))
        .mount(&server)
        .await;

    let err = client(&server).space("nope").await.unwrap_err();
    assert_eq!(err.status(), Some(reqwest::StatusCode::NOT_FOUND));
    assert_eq!(err.code(), Some("space_not_found"));
}

fn dm_message_json(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "dmChannelId": "d1",
        "userId": "u1",
        "content": "привет",
        "createdAt": 1_750_000_000_000_i64,
        "user": serde_json::from_str::<serde_json::Value>(USER_JSON).unwrap(),
        "attachments": [],
        "embeds": [],
        "reactions": []
    })
}

#[tokio::test]
async fn dm_messages_requests_dm_route_with_limit_and_before() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/dm/d1/messages"))
        .and(query_param("limit", "2"))
        .and(query_param("before", "m9"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([dm_message_json("m1"), dm_message_json("m2")])),
        )
        .expect(1)
        .mount(&server)
        .await;

    let page = client(&server)
        .dm_messages("d1", Some(PagingCursor::Before("m9".into())), Some(2))
        .await
        .unwrap();
    assert_eq!(page.messages.len(), 2);
    assert_eq!(page.messages[0].message.dm_channel_id, "d1");
    assert!(!page.paged_after);
}

#[tokio::test]
async fn dm_messages_detects_paging_after_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/dm/d1/messages"))
        .and(query_param("after", "m1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-backspace-paging", "after")
                .set_body_json(json!([dm_message_json("m2")])),
        )
        .mount(&server)
        .await;

    let page = client(&server)
        .dm_messages("d1", Some(PagingCursor::After("m1".into())), None)
        .await
        .unwrap();
    assert!(page.paged_after);
}

#[tokio::test]
async fn dm_messages_clamps_limit_and_maps_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/dm/d1/messages"))
        .and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/dm/nope/messages"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error":"Not a member","code":"not_dm_member"
        })))
        .mount(&server)
        .await;

    let page = client(&server)
        .dm_messages("d1", None, Some(500))
        .await
        .unwrap();
    assert!(page.messages.is_empty());

    let err = client(&server)
        .dm_messages("nope", None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("not_dm_member"));
}
