#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Загрузка файлов (tus), GIF, стикеры и отправка с вложениями.

use std::sync::{Arc, Mutex};

use bs_api::{ApiClient, MAX_UPLOAD_CHUNK, SendMessageBody};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> ApiClient {
    let mut client = ApiClient::home(server.uri()).unwrap();
    client.set_token(Some("tok".to_owned()));
    client
}

fn attachment() -> serde_json::Value {
    json!({
        "id":"att1","messageId":"","filename":"f.bin","originalName":"a.bin",
        "mimetype":"application/octet-stream","size":3,"thumbnailFilename":null,
        "width":null,"height":null,"duration":null,"playable":null,"createdAt":1
    })
}

#[tokio::test]
async fn upload_creates_session_sends_chunks_and_returns_attachment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/files/"))
        .and(header("authorization", "Bearer tok"))
        .and(header("upload-length", "3"))
        .respond_with(ResponseTemplate::new(201).insert_header("Location", "/api/files/u1"))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/files/u1"))
        .and(header("upload-offset", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(attachment()))
        .mount(&server)
        .await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let result = client(&server)
        .upload_file(
            "a.bin",
            "application/octet-stream",
            &[1, 2, 3],
            move |sent, total| {
                sink.lock().unwrap().push((sent, total));
            },
        )
        .await
        .unwrap();
    assert_eq!(result.id, "att1");
    assert_eq!(*seen.lock().unwrap(), vec![(0, 3), (3, 3)]);
}

#[tokio::test]
async fn upload_splits_big_files_into_chunks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/files/"))
        .respond_with(ResponseTemplate::new(201).insert_header("Location", "/api/files/u2"))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/files/u2"))
        .and(header("upload-offset", "0"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/files/u2"))
        .and(header(
            "upload-offset",
            MAX_UPLOAD_CHUNK.to_string().as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(attachment()))
        .expect(1)
        .mount(&server)
        .await;
    let bytes = vec![7_u8; MAX_UPLOAD_CHUNK + 10];
    client(&server)
        .upload_file("big.bin", "application/octet-stream", &bytes, |_, _| {})
        .await
        .unwrap();
}

#[tokio::test]
async fn upload_refuses_foreign_location() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/files/"))
        .respond_with(
            ResponseTemplate::new(201).insert_header("Location", "https://evil.example/x"),
        )
        .mount(&server)
        .await;
    let err = client(&server)
        .upload_file("a", "text/plain", &[1], |_, _| {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("адрес загрузки"), "{err}");
}

#[tokio::test]
async fn upload_reports_server_refusal() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/files/"))
        .respond_with(ResponseTemplate::new(413).set_body_string("File too large"))
        .mount(&server)
        .await;
    let err = client(&server)
        .upload_file("a", "text/plain", &[1], |_, _| {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("413"), "{err}");
}

#[tokio::test]
async fn gif_search_and_enabled() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/gif/enabled"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"enabled": true})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/gif/search"))
        .and(query_param("q", "кот"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results":[{"id":"1","title":"t","previewUrl":"https://static.klipy.com/p.webp",
                "url":"https://static.klipy.com/f.gif","width":10,"height":20}],
            "next":"n2"
        })))
        .mount(&server)
        .await;
    let client = client(&server);
    assert!(client.gif_enabled().await.unwrap());
    let page = client.gifs("кот", 30, "").await.unwrap();
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.next, "n2");
}

#[tokio::test]
async fn sticker_upload_sends_base64() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/stickers"))
        .and(body_json(json!({"name":"s","image":"Zm9v"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"id":"1","name":"s","token":"sticker:/api/stickers/assets/x.webp"}),
        ))
        .mount(&server)
        .await;
    let sticker = client(&server).upload_sticker("s", b"foo").await.unwrap();
    assert_eq!(sticker.token, "sticker:/api/stickers/assets/x.webp");
}

#[tokio::test]
async fn message_with_attachments_goes_through_rest() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/channels/c1/messages"))
        .and(body_json(json!({"content":"","attachments":["att1"]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id":"m1","channelId":"c1","userId":"u1","content":null,"createdAt":1,
            "user":{"id":"u1","username":"a","displayName":null,"avatar":null,"banner":null,
              "accentColor":null,"avatarColor":null,"bio":null,"status":"online",
              "customStatus":null,"isAdmin":false,"createdAt":1,"homeInstance":null,
              "homeUserId":null,"replicatedInstances":[]},
            "attachments":[],"embeds":[],"reactions":[]
        })))
        .mount(&server)
        .await;
    let body = SendMessageBody {
        content: String::new(),
        attachments: vec!["att1".to_owned()],
        reply_to_id: None,
    };
    client(&server)
        .send_channel_message("c1", &body)
        .await
        .unwrap();
}
