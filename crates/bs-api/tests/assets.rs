#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Скачивание файлов инстанса.

use bs_api::{ApiClient, ApiError, AssetPath};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn asset(p: &str) -> AssetPath {
    AssetPath::parse(p).unwrap()
}

#[tokio::test]
async fn fetches_bytes_without_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/uploads/a.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8, 2, 3]))
        .mount(&server)
        .await;
    let mut client = ApiClient::home(server.uri()).unwrap();
    client.set_token(Some("секретный-токен".to_owned()));
    let bytes = client
        .fetch_asset(&asset("/api/uploads/a.png"), 1024)
        .await
        .unwrap();
    assert_eq!(bytes, vec![1, 2, 3]);
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests
            .iter()
            .all(|r| !r.headers.contains_key("authorization")),
        "токен не должен уходить с запросом файла"
    );
}

#[tokio::test]
async fn refuses_files_over_the_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/uploads/big.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 4096]))
        .mount(&server)
        .await;
    let client = ApiClient::home(server.uri()).unwrap();
    let err = client
        .fetch_asset(&asset("/api/uploads/big.png"), 1024)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::ResponseTooLarge { limit: 1024 }));
}

#[tokio::test]
async fn missing_file_is_an_error_and_redirects_are_not_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/uploads/gone.png"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/uploads/moved.png"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "http://127.0.0.1:1/x"))
        .mount(&server)
        .await;
    let client = ApiClient::home(server.uri()).unwrap();
    for name in ["gone.png", "moved.png"] {
        let err = client
            .fetch_asset(&asset(&format!("/api/uploads/{name}")), 1024)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ApiError::Unexpected { .. }),
            "{name}: {err:?}"
        );
    }
}
