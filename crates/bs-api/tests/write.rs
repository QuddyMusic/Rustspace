#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Интеграционные тесты записи: друзья, поиск, DM.

use bs_api::{
    ApiClient, CreateDmRequest, FriendRequestDecision, MessageSearchQuery, SendFriendRequest,
};
use serde_json::json;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const USER_JSON: &str = r#"{
    "id":"u2","username":"bob","displayName":null,"avatar":null,"banner":null,
    "accentColor":null,"avatarColor":null,"bio":null,"status":"online",
    "customStatus":null,"isAdmin":false,"createdAt":1750000000000,
    "homeInstance":null,"homeUserId":null,"replicatedInstances":[]
}"#;

fn user() -> serde_json::Value {
    serde_json::from_str(USER_JSON).unwrap()
}

fn friend_json() -> serde_json::Value {
    json!({
        "id": "u2",
        "username": "bob",
        "displayName": null,
        "avatar": null,
        "banner": null,
        "accentColor": null,
        "avatarColor": null,
        "bio": null,
        "status": "online",
        "customStatus": null,
        "createdAt": 1_750_000_000_000_i64,
        "addedAt": 1_750_000_100_000_i64,
        "homeUserId": null,
        "homeInstance": null
    })
}

fn request_json() -> serde_json::Value {
    json!({
        "id": "r1",
        "fromId": "u2",
        "toId": "u1",
        "status": "pending",
        "createdAt": 1_750_000_000_000_i64,
        "user": user()
    })
}

fn dm_json() -> serde_json::Value {
    json!({
        "id": "dm1",
        "federatedId": null,
        "ownerId": null,
        "ownerHomeUserId": null,
        "ownerHomeInstance": null,
        "createdAt": 1_750_000_000_000_i64,
        "members": [user()],
        "lastMessage": null,
        "name": null,
        "icon": null,
        "metadataUpdatedAt": 1_750_000_000_000_i64
    })
}

fn dm_message_json(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "dmChannelId": "dm1",
        "userId": "u2",
        "content": "hello",
        "createdAt": 1_750_000_000_000_i64,
        "user": user(),
        "attachments": [],
        "embeds": [],
        "reactions": []
    })
}

fn client(server: &MockServer) -> ApiClient {
    ApiClient::home(server.uri()).unwrap()
}

#[tokio::test]
async fn lists_friends() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/social/friends"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([friend_json()])))
        .mount(&server)
        .await;

    let friends = client(&server).friends().await.unwrap();
    assert_eq!(friends.len(), 1);
    assert_eq!(friends[0].username, "bob");
    assert_eq!(friends[0].added_at, 1_750_000_100_000_i64);
}

#[tokio::test]
async fn lists_friend_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/social/requests"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([request_json()])))
        .mount(&server)
        .await;

    let requests = client(&server).friend_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].from_id, "u2");
    assert_eq!(requests[0].user.as_ref().unwrap().username, "bob");
}

#[tokio::test]
async fn sends_friend_request_by_username() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/social/requests"))
        .and(body_json(json!({"username":"bob"})))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"success": true, "requestId": "r1"})),
        )
        .mount(&server)
        .await;

    let id = client(&server)
        .send_friend_request(&SendFriendRequest::username("bob"))
        .await
        .unwrap();
    assert_eq!(id, "r1");
}

#[tokio::test]
async fn sends_federated_friend_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/social/requests"))
        .and(body_json(json!({
            "homeUserId":"home-u2","homeInstance":"remote.example"
        })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"success": true, "requestId": "r9"})),
        )
        .mount(&server)
        .await;

    let id = client(&server)
        .send_friend_request(&SendFriendRequest::identity("home-u2", "remote.example"))
        .await
        .unwrap();
    assert_eq!(id, "r9");
}

#[tokio::test]
async fn responds_to_friend_request() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/social/requests/r1"))
        .and(body_json(json!({"status":"accepted"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true})))
        .mount(&server)
        .await;

    client(&server)
        .respond_friend_request("r1", FriendRequestDecision::Accept)
        .await
        .unwrap();
}

#[tokio::test]
async fn declines_friend_request() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/social/requests/r1"))
        .and(body_json(json!({"status":"declined"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true})))
        .mount(&server)
        .await;

    client(&server)
        .respond_friend_request("r1", FriendRequestDecision::Decline)
        .await
        .unwrap();
}

#[tokio::test]
async fn cancels_and_removes() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/social/requests/r1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/social/friends/u2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"success":true})))
        .mount(&server)
        .await;

    let api = client(&server);
    api.cancel_friend_request("r1").await.unwrap();
    api.remove_friend("u2").await.unwrap();
}

#[tokio::test]
async fn remove_friend_surfaces_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/social/friends/u9"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error":"Not friends","code":"not_friends"
        })))
        .mount(&server)
        .await;

    let err = client(&server).remove_friend("u9").await.unwrap_err();
    assert_eq!(err.code(), Some("not_friends"));
}

#[tokio::test]
async fn searches_users_with_query_param() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/social/search"))
        .and(query_param("q", "bo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([user()])))
        .mount(&server)
        .await;

    let users = client(&server).search_users("bo").await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "bob");
}

#[tokio::test]
async fn discovers_users() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/social/discover"))
        .and(query_param("q", "bo"))
        .and(query_param("limit", "5"))
        .and(query_param("offset", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "users":[{
                "id":"u2","username":"bob","displayName":null,
                "avatar":null,"bio":null,"status":"online",
                "createdAt":1_750_000_000_000_i64,
                "mutualFriendCount":2,"mutualSpaceCount":1,
                "relationship":"none"
            }],
            "total":1
        })))
        .mount(&server)
        .await;

    let found = client(&server)
        .discover_users("bo", Some(5), Some(10))
        .await
        .unwrap();
    assert_eq!(found.total, 1);
    assert_eq!(found.users[0].mutual_friend_count, 2);
}

#[tokio::test]
async fn searches_channel_messages() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/channels/c1/search"))
        .and(query_param("q", "hello world"))
        .and(query_param("from", "bob"))
        .and(query_param("limit", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results":[],"totalCount":0
        })))
        .mount(&server)
        .await;

    let query = MessageSearchQuery {
        text: Some("hello world".to_owned()),
        from: Some("bob".to_owned()),
        limit: Some(500),
        ..MessageSearchQuery::default()
    };
    let found = client(&server)
        .search_channel_messages("c1", &query)
        .await
        .unwrap();
    assert_eq!(found.total_count, 0);
}

#[tokio::test]
async fn searches_dm_messages() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/dm/dm1/search"))
        .and(query_param("q", "hello"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results":[dm_message_json("m1")],"totalCount":1
        })))
        .mount(&server)
        .await;

    let found = client(&server)
        .search_dm_messages("dm1", &MessageSearchQuery::text("hello"))
        .await
        .unwrap();
    assert_eq!(found.results.len(), 1);
    assert_eq!(found.results[0].message.id, "m1");
}

#[tokio::test]
async fn lists_and_opens_dm() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/dm"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([dm_json()])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/dm"))
        .and(body_json(json!({"userId":"u2"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(dm_json()))
        .mount(&server)
        .await;

    let api = client(&server);
    let list = api.dm_channels().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "dm1");

    let opened = api.open_dm(&CreateDmRequest::user("u2")).await.unwrap();
    assert_eq!(opened.id, "dm1");
}

#[tokio::test]
async fn opens_federated_dm() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/dm"))
        .and(body_json(json!({
            "homeUserId":"home-u2","homeInstance":"remote.example"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(dm_json()))
        .mount(&server)
        .await;

    let opened = client(&server)
        .open_dm(&CreateDmRequest::identity("home-u2", "remote.example"))
        .await
        .unwrap();
    assert_eq!(opened.members.len(), 1);
}
