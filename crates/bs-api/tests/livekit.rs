//! Интеграционные тесты получения токенов LiveKit.

use bs_api::ApiClient;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn authed_client(server: &MockServer) -> Result<ApiClient, Box<dyn std::error::Error>> {
    let mut client = ApiClient::home(server.uri())?;
    client.set_token(Some("voice-jwt".to_owned()));
    Ok(client)
}

#[tokio::test]
async fn channel_token_sends_channel_id_and_bearer() -> Result<(), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/livekit/token"))
        .and(header("authorization", "Bearer voice-jwt"))
        .and(body_json(json!({ "channelId": "channel-1" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "channel-token",
            "url": "wss://livekit.example.test"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let response = authed_client(&server)?
        .livekit_token_channel("channel-1")
        .await?;
    assert_eq!(response.token, "channel-token");
    assert_eq!(response.url, "wss://livekit.example.test");
    Ok(())
}

#[tokio::test]
async fn dm_token_sends_dm_channel_id() -> Result<(), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/livekit/token"))
        .and(header("authorization", "Bearer voice-jwt"))
        .and(body_json(json!({ "dmChannelId": "dm-1" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "dm-token",
            "url": "wss://livekit.example.test"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let response = authed_client(&server)?.livekit_token_dm("dm-1").await?;
    assert_eq!(response.token, "dm-token");
    assert_eq!(response.url, "wss://livekit.example.test");
    Ok(())
}
