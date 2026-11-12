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

// ─── Безопасность транспорта ─────────────────────────────────────────────

#[test]
fn plaintext_http_to_remote_host_is_refused() {
    let err = ApiClient::home("http://chat.example.com").unwrap_err();
    assert!(matches!(err, ApiError::Insecure(_)), "{err}");
    let err = ApiClient::home("http://192.168.1.10:3005").unwrap_err();
    assert!(matches!(err, ApiError::Insecure(_)), "{err}");
}

#[test]
fn plaintext_http_is_allowed_on_loopback_and_with_consent() {
    assert!(ApiClient::home("http://localhost:3005").is_ok());
    assert!(ApiClient::home("http://127.0.0.1:3005").is_ok());
    assert!(ApiClient::home("https://chat.example.com").is_ok());
    let lan = ApiClient::with_policy(
        bs_types::Origin::Home,
        "http://192.168.1.10:3005",
        bs_types::TransportPolicy::AllowInsecure,
    );
    assert!(lan.is_ok());
}

#[test]
fn debug_output_never_contains_secrets() {
    let mut client = ApiClient::home("https://chat.example.com").unwrap();
    client.set_token(Some("super-secret-jwt".to_owned()));
    let shown = format!("{client:?}");
    assert!(!shown.contains("super-secret-jwt"), "{shown}");
    assert!(shown.contains("<redacted>"));

    let response: bs_api::AuthResponse = serde_json::from_value(json!({
        "token": "jwt-in-response",
        "user": serde_json::from_str::<serde_json::Value>(USER_JSON).unwrap()
    }))
    .unwrap();
    assert!(!format!("{response:?}").contains("jwt-in-response"));

    let request = bs_api::auth::LoginRequest::new("alice", "hunter2", Some("123456"));
    let shown = format!("{request:?}");
    assert!(
        shown.contains("alice") && !shown.contains("hunter2"),
        "{shown}"
    );
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let target = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(0)
        .mount(&target)
        .await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/spaces"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", format!("{}/steal", target.uri())),
        )
        .mount(&server)
        .await;

    let mut client = client(&server);
    client.set_token(Some("jwt".to_owned()));
    let err = client.spaces().await.unwrap_err();
    assert_eq!(err.status(), Some(reqwest::StatusCode::FOUND));
    // `expect(0)` проверяется при drop `target`: токен до чужого хоста не дошёл.
}

#[tokio::test]
async fn oversized_response_is_rejected() {
    let server = MockServer::start().await;
    let big = vec![b' '; bs_api::MAX_RESPONSE_BYTES + 1];
    Mock::given(method("GET"))
        .and(path("/api/spaces"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(big))
        .mount(&server)
        .await;

    let err = client(&server).spaces().await.unwrap_err();
    assert!(matches!(err, ApiError::ResponseTooLarge { .. }), "{err}");
}
