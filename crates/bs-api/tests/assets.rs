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

fn temp_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bs-dl-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("file.bin")
}

#[tokio::test]
async fn downloads_to_disk_with_progress_and_replaces_the_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/uploads/big.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 5000]))
        .mount(&server)
        .await;
    let client = ApiClient::home(server.uri()).unwrap();
    let dest = temp_file("ok");
    std::fs::write(&dest, b"old").unwrap();
    let mut last = (0, None);
    let size = client
        .download_asset(&asset("/api/uploads/big.bin"), &dest, |got, total| {
            last = (got, total);
        })
        .await
        .unwrap();
    assert_eq!(size, 5000);
    assert_eq!(last, (5000, Some(5000)));
    assert_eq!(std::fs::read(&dest).unwrap(), vec![7u8; 5000]);
    assert!(!dest.with_extension("bin.part").exists());
}

#[tokio::test]
async fn failed_download_leaves_no_files() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/uploads/gone.bin"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let client = ApiClient::home(server.uri()).unwrap();
    let dest = temp_file("gone");
    let err = client
        .download_asset(&asset("/api/uploads/gone.bin"), &dest, |_, _| {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404"), "{err}");
    assert!(!dest.exists());
    assert!(!dest.with_extension("bin.part").exists());
}

#[tokio::test]
async fn download_into_a_missing_folder_reports_an_io_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/uploads/a.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8]))
        .mount(&server)
        .await;
    let client = ApiClient::home(server.uri()).unwrap();
    let dest = temp_file("nofolder")
        .parent()
        .unwrap()
        .join("нет/такой/папки.bin");
    let err = client
        .download_asset(&asset("/api/uploads/a.bin"), &dest, |_, _| {})
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Io(_)), "{err}");
}
