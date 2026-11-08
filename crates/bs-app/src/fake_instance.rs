//! Мини-сервер Backspace для интеграционных тестов `run_session`.
//!
//! Умеет ровно две вещи: `POST /api/auth/login` и WebSocket `/ws`.
//! Этого достаточно, чтобы пройти happy-path: HTTP-логин → `auth` →
//! `ready` → обмен событиями. Внешние крейты (hyper и т.п.) не нужны —
//! HTTP/1.1 здесь разбирается руками, потому что запросов всего два.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;

use bs_types::{ClientEvent, ServerEvent};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;

/// Запускает сервер на случайном порту `127.0.0.1`.
///
/// Возвращает базовый URL (`http://127.0.0.1:PORT`) и приёмник событий,
/// которые клиент прислал по WebSocket.
pub(crate) async fn spawn(ready: ServerEvent) -> (String, mpsc::UnboundedReceiver<ClientEvent>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let (tx, rx) = mpsc::unbounded_channel();
    let ready_text = serde_json::to_string(&ready).expect("serialize ready");

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let events = tx.clone();
            let ready_text = ready_text.clone();
            tokio::spawn(async move {
                let _ = handle_conn(stream, events, ready_text).await;
            });
        }
    });

    (format!("http://{addr}"), rx)
}

/// Обслуживает одно TCP-соединение: либо REST-логин, либо WebSocket.
async fn handle_conn(
    mut stream: TcpStream,
    events: mpsc::UnboundedSender<ClientEvent>,
    ready_text: String,
) -> std::io::Result<()> {
    let (head, leftover) = read_head(&mut stream).await?;
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default().to_owned();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }

    if request_line.starts_with("POST /api/auth/login") {
        drain_body(&mut stream, &headers, leftover).await?;
        let body = login_response();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await?;
        stream.flush().await?;
        return Ok(());
    }

    if request_line.starts_with("GET /api/uploads/") {
        // Любой файл отдаётся 3000 байтами «x», кроме `gone.bin`.
        let (status, body) = if request_line.contains("gone.bin") {
            ("404 Not Found", Vec::new())
        } else {
            ("200 OK", vec![b'x'; 3000])
        };
        let head = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(&body).await?;
        stream.flush().await?;
        return Ok(());
    }

    if request_line.starts_with("GET /api/users/@me") {
        // Проверка сохранённого токена: годится только `jwt-abc`.
        let authorised = headers.get("authorization").map(String::as_str) == Some("Bearer jwt-abc");
        let (status, body) = if authorised {
            let user = serde_json::from_str::<serde_json::Value>(&login_response()).unwrap();
            ("200 OK", user["user"].to_string())
        } else {
            ("401 Unauthorized", r#"{"error":"Unauthorized","code":"unauthorized"}"#.to_owned())
        };
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await?;
        stream.flush().await?;
        return Ok(());
    }

    if headers.get("upgrade").map(String::as_str) == Some("websocket") {
        let key = headers
            .get("sec-websocket-key")
            .cloned()
            .unwrap_or_default();
        let accept = derive_accept_key(key.as_bytes());
        let handshake = format!(
            "HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: {accept}\r\n\r\n"
        );
        stream.write_all(handshake.as_bytes()).await?;
        stream.flush().await?;

        let mut ws = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
        while let Some(message) = ws.next().await {
            match message {
                Ok(WsMessage::Text(text)) => {
                    let Ok(event) = serde_json::from_str::<ClientEvent>(&text) else {
                        continue;
                    };
                    if matches!(event, ClientEvent::Auth { .. }) {
                        ws.send(WsMessage::Text(ready_text.clone().into()))
                            .await
                            .map_err(std::io::Error::other)?;
                    } else if events.send(event).is_err() {
                        return Ok(());
                    }
                }
                Ok(WsMessage::Close(_)) | Err(_) => return Ok(()),
                Ok(_) => {}
            }
        }
    }

    Ok(())
}

/// Читает HTTP-заголовки до пустой строки и возвращает их текстом.
async fn read_head(stream: &mut TcpStream) -> std::io::Result<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok((String::new(), Vec::new()));
        }
        buf.extend_from_slice(&chunk[..read]);
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
            // Тело запроса могло приехать тем же пакетом — сохраняем его.
            let leftover = buf[pos + 4..].to_vec();
            return Ok((head, leftover));
        }
    }
}

/// Дочитывает тело запроса по `Content-Length`, чтобы не сбить поток.
async fn drain_body(
    stream: &mut TcpStream,
    headers: &HashMap<String, String>,
    already_read: Vec<u8>,
) -> std::io::Result<()> {
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut left = length.saturating_sub(already_read.len());
    let mut chunk = [0u8; 4096];
    while left > 0 {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        left = left.saturating_sub(read);
    }
    Ok(())
}

/// Тело успешного ответа `POST /api/auth/login`.
fn login_response() -> String {
    serde_json::json!({
        "token": "jwt-abc",
        "user": {
            "id": "123456789012345678",
            "username": "alice",
            "displayName": null,
            "avatar": null,
            "banner": null,
            "accentColor": null,
            "avatarColor": "mint",
            "bio": null,
            "status": "online",
            "customStatus": null,
            "isAdmin": false,
            "createdAt": 1_750_000_000_000i64,
            "homeInstance": null,
            "homeUserId": null,
            "replicatedInstances": []
        }
    })
    .to_string()
}

/// Позиция подпоследовательности в буфере.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
