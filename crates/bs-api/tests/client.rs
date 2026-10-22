#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Интеграционные тесты `ApiClient` против wiremock.

use bs_api::{ApiClient, ApiError};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const USER_JSON: &str = r#"{
    "id":"123456789012345678",
    "username":"alice",
    "displayName":null,
    "avatar":null,
    "banner":null,
    "accentColor":null,
    "avatarColor":"mint",
    "bio":null,
    "status":"online",
    "customStatus":null,
    "isAdmin":false,
    "createdAt":1750000000000,
    "homeInstance":null,
    "homeUserId":null,
    "replicatedInstances":[]
}"#;

fn client(server: &MockServer) -> ApiClient {
    ApiClient::home(server.uri()).unwrap()
}

#[tokio::test]
async fn login_returns_token_and_user() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .and(body_json(json!({"username":"alice","password":"secret"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token":"jwt-abc",
            "user": serde_json::from_str::<serde_json::Value>(USER_JSON).unwrap()
        })))
        .mount(&server)
        .await;

    let response = client(&server)
        .login("alice", "secret")
        .await
        .expect("login should succeed");
    assert_eq!(response.token, "jwt-abc");
    assert_eq!(response.user.username, "alice");
}

#[tokio::test]
async fn login_maps_server_error_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error":"Invalid credentials",
            "code":"invalid_credentials"
        })))
        .mount(&server)
        .await;

    let err = client(&server).login("alice", "bad").await.unwrap_err();
    assert!(err.is_unauthorized(), "expected 401, got {err:?}");
    assert_eq!(err.code(), Some("invalid_credentials"));
    assert_eq!(err.body().unwrap().error, "Invalid credentials");
}

#[tokio::test]
async fn login_attaches_bearer_for_me() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token":"jwt-abc",
            "user": serde_json::from_str::<serde_json::Value>(USER_JSON).unwrap()
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/users/@me"))
        .and(header("authorization", "Bearer jwt-abc"))
        .respond_with(ResponseTemplate::new(200).set_body_string(USER_JSON))
        .mount(&server)
        .await;

    let mut client = client(&server);
    assert!(!client.has_token());
    client
        .login_and_attach("alice", "secret")
        .await
        .expect("login should succeed");
    assert!(client.has_token());
    let me = client.me().await.expect("me should succeed");
    assert_eq!(me.id, "123456789012345678");
}

#[tokio::test]
async fn non_json_error_body_becomes_unexpected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/users/@me"))
        .respond_with(ResponseTemplate::new(502).set_body_string("bad gateway"))
        .mount(&server)
        .await;

    let err = client(&server).me().await.unwrap_err();
    assert!(matches!(err, ApiError::Unexpected { .. }), "got {err:?}");
}

#[tokio::test]
async fn base_url_rejects_relative() {
    assert!(ApiClient::home("not-a-url").is_err());
}
