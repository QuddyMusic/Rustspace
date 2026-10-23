#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Интеграционные тесты федеративных эндпоинтов (креденшел, re-attach).

use bs_api::{ApiClient, ChangePasswordRequest, FederationCredentialRequest, ReattachResponse};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const USER_JSON: &str = r#"{
    "id":"u1","username":"alice@orbit.ddns.net","displayName":null,"avatar":null,
    "banner":null,"accentColor":null,"avatarColor":null,"bio":null,"status":"online",
    "customStatus":null,"isAdmin":false,"createdAt":1750000000000,
    "homeInstance":"https://orbit.ddns.net","homeUserId":"u9","replicatedInstances":[]
}"#;

fn authed_client(server: &MockServer) -> ApiClient {
    let mut client = ApiClient::home(server.uri()).unwrap();
    client.set_token(Some("home-jwt".to_owned()));
    client
}

#[tokio::test]
async fn federation_credential_sends_bearer_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/@me/federation-credential"))
        .and(header("authorization", "Bearer home-jwt"))
        .and(body_json(json!({ "origin": "https://orbit.ddns.net" })))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"origin":"https://orbit.ddns.net","secret":"s3cr3t","provisioned":false}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let cred = authed_client(&server)
        .federation_credential(&FederationCredentialRequest::new("https://orbit.ddns.net"))
        .await
        .unwrap();
    assert_eq!(cred.origin, "https://orbit.ddns.net");
    assert_eq!(cred.secret, "s3cr3t");
    assert!(!cred.provisioned);
}

#[tokio::test]
async fn federation_credential_marks_provisioned() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/@me/federation-credential"))
        .and(body_json(json!({
            "origin": "https://orbit.ddns.net",
            "markProvisioned": true
        })))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"origin":"https://orbit.ddns.net","secret":"s3cr3t","provisioned":true}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let cred = authed_client(&server)
        .federation_credential(&FederationCredentialRequest::marking_provisioned(
            "https://orbit.ddns.net",
        ))
        .await
        .unwrap();
    assert!(cred.provisioned);
}

#[tokio::test]
async fn federation_credential_surfaces_remote_only_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/@me/federation-credential"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":"remote only","code":"federation_credential_remote_only","statusCode":400}"#,
        ))
        .mount(&server)
        .await;

    let err = authed_client(&server)
        .federation_credential(&FederationCredentialRequest::new(
            "https://home.example.com",
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("federation_credential_remote_only"));
    assert_eq!(err.status().map(|s| s.as_u16()), Some(400));
}

#[tokio::test]
async fn change_password_omits_current_password() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/@me/change-password"))
        .and(header("authorization", "Bearer home-jwt"))
        .and(body_json(json!({ "newPassword": "s3cr3t" })))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"token":"rotated"}"#))
        .expect(1)
        .mount(&server)
        .await;

    let rotated = authed_client(&server)
        .change_password(&ChangePasswordRequest::new("s3cr3t"))
        .await
        .unwrap();
    assert_eq!(rotated.token, "rotated");
}

#[tokio::test]
async fn change_password_keeps_current_when_given() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/@me/change-password"))
        .and(body_json(
            json!({ "currentPassword": "old", "newPassword": "new" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"token":"t"}"#))
        .expect(1)
        .mount(&server)
        .await;

    authed_client(&server)
        .change_password(&ChangePasswordRequest::with_current("old", "new"))
        .await
        .unwrap();
}

#[tokio::test]
async fn attach_proof_sends_target_domain() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/auth/attach-proof"))
        .and(body_json(json!({ "targetDomain": "orbit.ddns.net" })))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"token":"deadbeef"}"#))
        .expect(1)
        .mount(&server)
        .await;

    let proof = authed_client(&server)
        .attach_proof("orbit.ddns.net")
        .await
        .unwrap();
    assert_eq!(proof.token, "deadbeef");
}

#[tokio::test]
async fn reattach_posts_token_and_parses_user() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/@me/reattach"))
        .and(body_json(json!({ "token": "deadbeef" })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!(r#"{{"success":true,"user":{USER_JSON}}}"#)),
        )
        .expect(1)
        .mount(&server)
        .await;

    let res: ReattachResponse = authed_client(&server).reattach("deadbeef").await.unwrap();
    assert!(res.success);
    assert_eq!(res.user.username, "alice@orbit.ddns.net");
    assert_eq!(
        res.user.home_instance.as_deref(),
        Some("https://orbit.ddns.net")
    );
}

#[tokio::test]
async fn reattach_reports_not_detached() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/users/@me/reattach"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_string(r#"{"error":"not detached","statusCode":403}"#),
        )
        .mount(&server)
        .await;

    let err = authed_client(&server)
        .reattach("deadbeef")
        .await
        .unwrap_err();
    assert_eq!(err.status().map(|s| s.as_u16()), Some(403));
}
