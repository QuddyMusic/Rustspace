#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Подключение к другому инстансу.

use bs_api::{ApiClient, ApiError, RegisterRequest, ReplicatedEntry};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn info_is_public_and_tolerates_missing_fields() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/instance/info"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "name": "Orbit", "registrationOpen": true
        })))
        .mount(&server)
        .await;
    let info = ApiClient::home(server.uri())
        .unwrap()
        .instance_info()
        .await
        .unwrap();
    assert_eq!(info.name, "Orbit");
    assert!(info.registration_open);
    // Старый сервер не знает про федеративную регистрацию: считаем её открытой.
    assert!(info.federated_registration_open);
}

#[tokio::test]
async fn closed_federated_registration_is_a_distinct_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/register"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(serde_json::json!({"error": "federated_registration_closed"})),
        )
        .mount(&server)
        .await;
    let request = RegisterRequest {
        username: "alice@home.example".into(),
        password: "секрет".into(),
        display_name: None,
        home_instance: Some("home.example".into()),
        home_user_id: Some("7".into()),
    };
    let err = ApiClient::home(server.uri())
        .unwrap()
        .register(&request)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ApiError::Server { status, .. } if status.as_u16() == 403),
        "{err:?}"
    );
    assert!(!format!("{request:?}").contains("секрет"));
}

#[tokio::test]
async fn verify_password_reports_mismatch_without_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/@me/verify-password"))
        .and(header("authorization", "Bearer t"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"valid": false})))
        .mount(&server)
        .await;
    let mut client = ApiClient::home(server.uri()).unwrap();
    client.set_token(Some("t".to_owned()));
    assert!(!client.verify_password("x").await.unwrap());
}

#[tokio::test]
async fn ensure_peered_states_the_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/federation/peer/ensure"))
        .and(body_json(serde_json::json!({
            "remoteOrigin": "https://orbit.example", "reason": "instance_connect"
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"peeringStatus": "pending"})),
        )
        .mount(&server)
        .await;
    let mut client = ApiClient::home(server.uri()).unwrap();
    client.set_token(Some("t".to_owned()));
    let answer = client.ensure_peered("https://orbit.example").await.unwrap();
    assert_eq!(answer.peering_status, "pending");
}

#[tokio::test]
async fn replicated_instances_are_sent_in_camel_case() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/users/@me"))
        .and(body_json(serde_json::json!({
            "replicatedInstances": [{"origin": "https://orbit.example", "username": "alice@home.example"}]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "1", "username": "alice", "displayName": null, "avatar": null, "banner": null,
            "accentColor": null, "avatarColor": null, "bio": null, "status": "online",
            "customStatus": null, "isAdmin": false, "createdAt": 0, "homeInstance": null,
            "homeUserId": null, "replicatedInstances": []
        })))
        .mount(&server)
        .await;
    let mut client = ApiClient::home(server.uri()).unwrap();
    client.set_token(Some("t".to_owned()));
    client
        .set_replicated_instances(&[ReplicatedEntry {
            origin: "https://orbit.example".into(),
            username: "alice@home.example".into(),
        }])
        .await
        .unwrap();
}
