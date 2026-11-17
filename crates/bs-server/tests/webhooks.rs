//! Вебхуки, карточки и битрейт голоса — расширения Rustspace.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "тесты")]

use std::collections::HashMap;
use std::net::SocketAddr;

use bs_server::config::Config;
use bs_server::db::Db;
use bs_server::state::AppState;
use serde_json::{Value, json};

struct Server {
    base: String,
    http: reqwest::Client,
}

impl Server {
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("запрос");
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }
}

async fn start_with(bridges: &[(&str, &str)]) -> Server {
    let vars: HashMap<String, String> = [
        ("JWT_SECRET", "0123456789abcdef0123456789abcdef"),
        ("BS_PASSWORD_HASH_COST", "4"),
        ("DIRECTORY_ENDPOINT", ""),
        ("TRUSTED_PROXY_HOPS", "0"),
        ("DISABLE_RATE_LIMITS", "true"),
    ]
    .into_iter()
    .chain(bridges.iter().copied())
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let config = Config::from_map(&vars).expect("конфигурация");
    let state = AppState::new(config, Db::open_memory().expect("база")).expect("состояние");
    bs_server::bridges::spawn(&state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("порт");
    let base = format!("http://{}", listener.local_addr().expect("адрес"));
    let app = bs_server::app(state);
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("сервер");
    });
    Server {
        base,
        http: reqwest::Client::new(),
    }
}

async fn user_with_channel(server: &Server) -> (String, String) {
    let (_, me) = server
        .call(
            reqwest::Method::POST,
            "/api/auth/register",
            "",
            Some(json!({"username": "quddy", "password": "password123"})),
        )
        .await;
    let token = me["token"].as_str().unwrap().to_owned();
    let (_, space) = server
        .call(
            reqwest::Method::POST,
            "/api/spaces",
            &token,
            Some(json!({"name": "Клуб"})),
        )
        .await;
    let space = space["id"].as_str().unwrap();
    let (_, channels) = server
        .call(
            reqwest::Method::GET,
            &format!("/api/spaces/{space}/channels"),
            &token,
            None,
        )
        .await;
    let channel = channels
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["type"] == "text")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (token, channel)
}

#[tokio::test]
#[allow(clippy::too_many_lines, reason = "один сквозной сценарий")]
async fn webhook_posts_messages_with_cards() {
    let server = start_with(&[]).await;
    let (token, channel) = user_with_channel(&server).await;
    let (status, hook) = server
        .call(
            reqwest::Method::POST,
            &format!("/api/rustspace/channels/{channel}/webhooks"),
            &token,
            Some(json!({"name": "CI"})),
        )
        .await;
    assert_eq!(status, 201, "{hook}");
    let url = hook["url"].as_str().unwrap();
    let path = &url[url.find("/api/webhooks/").unwrap()..];
    let (status, listed) = server
        .call(
            reqwest::Method::GET,
            &format!("/api/rustspace/channels/{channel}/webhooks"),
            &token,
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(listed.as_array().unwrap().len(), 1);

    // Как Discord: content, username и карточка; ?wait=true вернёт сообщение.
    let (status, message) = server
        .call(
            reqwest::Method::POST,
            &format!("{path}?wait=true"),
            "",
            Some(json!({
                "content": "Сборка **готова**",
                "username": "GitHub",
                "embeds": [{"title": "main", "description": "3 коммита", "color": 3_066_993,
                            "fields": [{"name": "Время", "value": "2 мин"}]}]
            })),
        )
        .await;
    assert_eq!(status, 200, "{message}");
    assert_eq!(message["user"]["displayName"], "GitHub");
    assert_eq!(message["embeds"][0]["title"], "main");
    assert_eq!(message["embeds"][0]["color"], "#2ecc71");
    assert_eq!(
        message["embeds"][0]["description"],
        "3 коммита\n**Время**\n2 мин"
    );
    let (_, history) = server
        .call(
            reqwest::Method::GET,
            &format!("/api/channels/{channel}/messages"),
            &token,
            None,
        )
        .await;
    let last = history.as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["content"], "Сборка **готова**");
    assert_eq!(last["embeds"][0]["embedType"], "generic");

    // Чужой секрет — 404, пустое тело — 400.
    let wrong = format!("{path}x");
    let (status, _) = server
        .call(
            reqwest::Method::POST,
            &wrong,
            "",
            Some(json!({"content": "hi"})),
        )
        .await;
    assert_eq!(status, 404);
    let (status, _) = server
        .call(
            reqwest::Method::POST,
            path,
            "",
            Some(json!({"content": "  "})),
        )
        .await;
    assert_eq!(status, 400);

    // Карточка от своего имени.
    let (status, card) = server
        .call(
            reqwest::Method::POST,
            &format!("/api/rustspace/channels/{channel}/cards"),
            &token,
            Some(json!({"embeds": [{"title": "Правила", "description": "- раз\n- два", "color": "#ff5500"}]})),
        )
        .await;
    assert_eq!(status, 201, "{card}");
    assert_eq!(card["user"]["username"], "quddy");
    assert_eq!(card["embeds"][0]["color"], "#ff5500");

    // Удалённый вебхук больше не пишет.
    let id = hook["id"].as_str().unwrap();
    let (status, _) = server
        .call(
            reqwest::Method::DELETE,
            &format!("/api/rustspace/webhooks/{id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(status, 200);
    let (status, _) = server
        .call(
            reqwest::Method::POST,
            path,
            "",
            Some(json!({"content": "hi"})),
        )
        .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn voice_bitrate_can_only_go_down() {
    let server = start_with(&[]).await;
    let (token, channel) = user_with_channel(&server).await;
    let get = |path: String| {
        let server = &server;
        let token = token.clone();
        async move { server.call(reqwest::Method::GET, &path, &token, None).await }
    };
    let (status, instance) = get("/api/rustspace/admin/voice-bitrate".to_owned()).await;
    assert_eq!(status, 200);
    assert_eq!(instance["kbps"], 96);
    // Первый пользователь — администратор инстанса.
    let (status, _) = server
        .call(
            reqwest::Method::PUT,
            "/api/rustspace/admin/voice-bitrate",
            &token,
            Some(json!({"kbps": 128})),
        )
        .await;
    assert_eq!(status, 200);
    let path = format!("/api/rustspace/voice-bitrate/{channel}");
    let (_, info) = get(path.clone()).await;
    assert_eq!(info["effectiveKbps"], 128);
    assert_eq!(info["canEdit"], true);
    let (status, _) = server
        .call(
            reqwest::Method::PUT,
            &path,
            &token,
            Some(json!({"kbps": 256})),
        )
        .await;
    assert_eq!(status, 400, "выше инстанса нельзя");
    let (status, info) = server
        .call(
            reqwest::Method::PUT,
            &path,
            &token,
            Some(json!({"kbps": 32})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(info["effectiveKbps"], 32);
    let (_, info) = server
        .call(
            reqwest::Method::PUT,
            &path,
            &token,
            Some(json!({"kbps": null})),
        )
        .await;
    assert_eq!(info["effectiveKbps"], 128);
}

/// Отправляет сообщение и возвращает его id.
async fn say(server: &Server, token: &str, path: &str, text: &str) -> String {
    let (status, message) = server
        .call(
            reqwest::Method::POST,
            path,
            token,
            Some(json!({"content": text})),
        )
        .await;
    assert!(status == 200 || status == 201, "{status} {message}");
    message["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
#[allow(clippy::too_many_lines, reason = "один сквозной сценарий")]
async fn saved_messages_pins_and_bookmarks() {
    let server = start_with(&[]).await;
    let (token, channel) = user_with_channel(&server).await;
    let get = |path: String| {
        let server = &server;
        let token = token.clone();
        async move { server.call(reqwest::Method::GET, &path, &token, None).await }
    };

    // «Избранное»: одна и та же переписка, писать в неё можно, звать — нет.
    let (status, saved) = get("/api/rustspace/saved".to_owned()).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["savedMessages"], true);
    let saved_id = saved["id"].as_str().unwrap().to_owned();
    let (_, again) = get("/api/rustspace/saved".to_owned()).await;
    assert_eq!(again["id"], saved_id.as_str());
    let note = say(
        &server,
        &token,
        &format!("/api/dm/{saved_id}/messages"),
        "идея",
    )
    .await;
    let (status, _) = server
        .call(
            reqwest::Method::POST,
            &format!("/api/dm/{saved_id}/members"),
            &token,
            Some(json!({"userId": "1"})),
        )
        .await;
    assert!(status == 400 || status == 404);

    // Закрепы: общий в канале и личный в «Избранном».
    let first = say(
        &server,
        &token,
        &format!("/api/channels/{channel}/messages"),
        "правила",
    )
    .await;
    let pin_path = format!("/api/rustspace/pins/channel/{channel}/{first}");
    let (status, _) = server
        .call(
            reqwest::Method::PUT,
            &pin_path,
            &token,
            Some(json!({"shared": true})),
        )
        .await;
    assert_eq!(status, 200);
    let (_, pins) = get(format!("/api/rustspace/pins/channel/{channel}")).await;
    assert_eq!(pins[0]["shared"], true);
    assert_eq!(pins[0]["message"]["content"], "правила");
    let (_, _) = server
        .call(
            reqwest::Method::PUT,
            &format!("/api/rustspace/pins/dm/{saved_id}/{note}"),
            &token,
            Some(json!({"shared": false})),
        )
        .await;
    let (_, pins) = get(format!("/api/rustspace/pins/dm/{saved_id}")).await;
    assert_eq!(pins[0]["shared"], false);
    let (status, _) = server
        .call(reqwest::Method::DELETE, &pin_path, &token, None)
        .await;
    assert_eq!(status, 200);
    let (_, pins) = get(format!("/api/rustspace/pins/channel/{channel}")).await;
    assert_eq!(pins.as_array().unwrap().len(), 0);
    // Сообщение из другого чата закрепить нельзя.
    let (status, _) = server
        .call(
            reqwest::Method::PUT,
            &format!("/api/rustspace/pins/channel/{channel}/{note}"),
            &token,
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 404);

    // Закладки (#474): 201, повтор — 200, ключи, страницы, правка оригинала.
    let path = format!("/api/bookmarks/channel/{first}");
    let (status, mark) = server.call(reqwest::Method::PUT, &path, &token, None).await;
    assert_eq!(status, 201, "{mark}");
    assert_eq!(mark["status"], "ok");
    assert_eq!(mark["channelName"], "general");
    let (status, _) = server.call(reqwest::Method::PUT, &path, &token, None).await;
    assert_eq!(status, 200);
    let (status, _) = server
        .call(
            reqwest::Method::PUT,
            &format!("/api/bookmarks/dm/{note}"),
            &token,
            None,
        )
        .await;
    assert_eq!(status, 201);
    let (_, keys) = get(format!(
        "/api/bookmarks/keys?kind=channel&channelId={channel}"
    ))
    .await;
    assert_eq!(keys, json!([first.clone()]));
    let (_, page) = get("/api/bookmarks?limit=1".to_owned()).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let cursor = page["nextCursor"].as_str().unwrap().to_owned();
    let (_, rest) = get(format!("/api/bookmarks?limit=1&cursor={cursor}")).await;
    assert_eq!(rest["items"].as_array().unwrap().len(), 1);
    assert!(rest["nextCursor"].is_null());
    let (status, _) = get("/api/bookmarks?cursor=zzz".to_owned()).await;
    assert_eq!(status, 400);
    let (_, _) = server
        .call(
            reqwest::Method::PATCH,
            &format!("/api/messages/{first}"),
            &token,
            Some(json!({"content": "правила v2"})),
        )
        .await;
    let (_, page) = get("/api/bookmarks".to_owned()).await;
    let edited = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["messageId"] == first.as_str())
        .unwrap()
        .clone();
    assert_eq!(edited["status"], "edited");
    assert_eq!(edited["content"], "правила");
    let (status, _) = server
        .call(reqwest::Method::DELETE, &path, &token, None)
        .await;
    assert_eq!(status, 200);
    let (_, keys) = get(format!(
        "/api/bookmarks/keys?kind=channel&channelId={channel}"
    ))
    .await;
    assert_eq!(keys, json!([]));
}
