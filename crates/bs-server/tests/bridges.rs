//! Мост в Telegram от начала до конца: настоящий сервер, поддельный Bot API.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "тесты")]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use bs_server::config::Config;
use bs_server::db::Db;
use bs_server::state::AppState;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "123:secret";

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

async fn start(telegram: &str) -> Server {
    start_with(&[
        ("TELEGRAM_BOT_TOKEN", TOKEN),
        ("TELEGRAM_API_URL", telegram),
    ])
    .await
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

fn ok(result: &Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"ok": true, "result": result}))
}

#[tokio::test]
#[allow(clippy::too_many_lines, reason = "один сквозной сценарий")]
async fn telegram_bridge_relays_both_ways() {
    let telegram = MockServer::start().await;
    let bot = format!("/bot{TOKEN}");
    Mock::given(method("POST"))
        .and(path(format!("{bot}/getMe")))
        .respond_with(ok(
            &json!({"id": 999, "is_bot": true, "username": "rustspace_bot"}),
        ))
        .mount(&telegram)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{bot}/getUpdates")))
        .respond_with(ok(&json!([])))
        .mount(&telegram)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{bot}/sendMessage")))
        .respond_with(ok(&json!({})))
        .mount(&telegram)
        .await;
    let server = start(&telegram.uri()).await;

    // Пользователь, пространство и его текстовый канал.
    let (status, me) = server
        .call(
            reqwest::Method::POST,
            "/api/auth/register",
            "",
            Some(json!({"username": "quddy", "password": "password123"})),
        )
        .await;
    assert_eq!(status, 201, "{me}");
    let token = me["token"].as_str().unwrap().to_owned();
    let (_, space) = server
        .call(
            reqwest::Method::POST,
            "/api/spaces",
            &token,
            Some(json!({"name": "Клуб"})),
        )
        .await;
    let space = space["id"].as_str().unwrap().to_owned();
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

    // Сеть доступна, связь заводится, повтор — конфликт, мусор — отказ.
    let bridges = format!("/api/rustspace/channels/{channel}/bridges");
    let (status, info) = server
        .call(reqwest::Method::GET, &bridges, &token, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(info["available"], json!(["telegram"]));
    let link = json!({"kind": "telegram", "externalId": "-100123"});
    let (status, created) = server
        .call(reqwest::Method::POST, &bridges, &token, Some(link.clone()))
        .await;
    assert_eq!(status, 201, "{created}");
    let (status, _) = server
        .call(reqwest::Method::POST, &bridges, &token, Some(link))
        .await;
    assert_eq!(status, 409);
    let (status, _) = server
        .call(
            reqwest::Method::POST,
            &bridges,
            &token,
            Some(json!({"kind": "discord", "externalId": "1"})),
        )
        .await;
    assert_eq!(status, 400, "Discord на этом сервере не настроен");
    let (status, _) = server
        .call(
            reqwest::Method::POST,
            &bridges,
            &token,
            Some(json!({"kind": "telegram", "externalId": "@chat"})),
        )
        .await;
    assert_eq!(status, 400);
    // Чужой пользователь связи не видит.
    let (_, other) = server
        .call(
            reqwest::Method::POST,
            "/api/auth/register",
            "",
            Some(json!({"username": "stranger", "password": "password123"})),
        )
        .await;
    let (status, _) = server
        .call(
            reqwest::Method::GET,
            &bridges,
            other["token"].as_str().unwrap(),
            None,
        )
        .await;
    assert_ne!(status, 200);

    // Внутрь: сообщение из группы Telegram появляется в канале от марионетки.
    Mock::given(method("POST"))
        .and(path(format!("{bot}/getUpdates")))
        .respond_with(ok(&json!([{
            "update_id": 7,
            "message": {
                "message_id": 1,
                "chat": {"id": -100_123, "type": "supergroup"},
                "from": {"id": 42, "is_bot": false, "first_name": "Олег"},
                "text": "Привет из Telegram",
            }
        }])))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&telegram)
        .await;
    let mut found = Value::Null;
    for _ in 0..50 {
        let (_, messages) = server
            .call(
                reqwest::Method::GET,
                &format!("/api/channels/{channel}/messages"),
                &token,
                None,
            )
            .await;
        if let Some(message) = messages
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["content"] == "Привет из Telegram")
        {
            found = message.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(found["user"]["displayName"], "Олег · Telegram", "{found}");
    assert_eq!(found["user"]["username"], "tg_42");
    // Под марионеткой войти нельзя.
    let (status, _) = server
        .call(
            reqwest::Method::POST,
            "/api/auth/login",
            "",
            Some(json!({"username": "tg_42", "password": "!"})),
        )
        .await;
    assert_eq!(status, 401);

    // Наружу: сообщение из канала уходит в группу, своё — не возвращается.
    let (status, _) = server
        .call(
            reqwest::Method::POST,
            &format!("/api/channels/{channel}/messages"),
            &token,
            Some(json!({"content": "Привет <всем> из Rustspace"})),
        )
        .await;
    assert_eq!(status, 201);
    let mut sent: Vec<Value> = Vec::new();
    for _ in 0..50 {
        sent = telegram
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| request.url.path().ends_with("/sendMessage"))
            .map(|request| serde_json::from_slice(&request.body).unwrap())
            .collect();
        if !sent.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        sent.len(),
        1,
        "сообщение марионетки обратно не уходит: {sent:?}"
    );
    assert_eq!(sent[0]["chat_id"], "-100123");
    assert_eq!(sent[0]["parse_mode"], "HTML");
    assert_eq!(
        sent[0]["text"],
        "<b>quddy</b>\nПривет &lt;всем&gt; из Rustspace"
    );

    // Развязка.
    let link_id = created["id"].as_str().unwrap();
    let (status, _) = server
        .call(
            reqwest::Method::DELETE,
            &format!("/api/rustspace/bridges/{link_id}"),
            &token,
            None,
        )
        .await;
    assert_eq!(status, 200);
    let (_, info) = server
        .call(reqwest::Method::GET, &bridges, &token, None)
        .await;
    assert_eq!(info["links"], json!([]));
}

/// Пользователь, его пространство и текстовый канал: (токен, канал).
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

/// Тела запросов к поддельному API, путь которых кончается на `suffix`.
async fn bodies(mock: &MockServer, suffix: &str) -> Vec<Value> {
    for _ in 0..50 {
        let found: Vec<Value> = mock
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| {
                request.url.path().ends_with(suffix) || request.url.path().contains(suffix)
            })
            .filter_map(|request| serde_json::from_slice(&request.body).ok())
            .collect();
        if !found.is_empty() {
            return found;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Vec::new()
}

#[tokio::test]
async fn matrix_bridge_relays_both_ways() {
    use wiremock::matchers::{path_regex, query_param};
    let matrix = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/_matrix/client/v3/account/whoami"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"user_id": "@bot:hs.test"})))
        .mount(&matrix)
        .await;
    Mock::given(method("GET"))
        .and(path("/_matrix/client/v3/sync"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"next_batch": "s1"})))
        .mount(&matrix)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(
            "^/_matrix/client/v3/rooms/.+/send/m.room.message/.+$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"event_id": "$e"})))
        .mount(&matrix)
        .await;
    let server = start_with(&[
        ("MATRIX_HOMESERVER", &matrix.uri()),
        ("MATRIX_ACCESS_TOKEN", "mx-token"),
    ])
    .await;
    let (token, channel) = user_with_channel(&server).await;
    let (status, _) = server
        .call(
            reqwest::Method::POST,
            &format!("/api/rustspace/channels/{channel}/bridges"),
            &token,
            Some(json!({"kind": "matrix", "externalId": "!room:hs.test"})),
        )
        .await;
    assert_eq!(status, 201);
    Mock::given(method("GET"))
        .and(path("/_matrix/client/v3/sync"))
        .and(query_param("since", "s1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "next_batch": "s2",
            "rooms": {"join": {"!room:hs.test": {"timeline": {"events": [
                {"type": "m.room.message", "sender": "@anna:hs.test",
                 "content": {"msgtype": "m.text", "body": "Привет из Matrix"}},
                {"type": "m.room.message", "sender": "@bot:hs.test",
                 "content": {"msgtype": "m.text", "body": "своё — не пересылается"}}
            ]}}}}
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&matrix)
        .await;
    let mut contents = Vec::new();
    for _ in 0..50 {
        let (_, messages) = server
            .call(
                reqwest::Method::GET,
                &format!("/api/channels/{channel}/messages"),
                &token,
                None,
            )
            .await;
        contents = messages
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| (m["content"].clone(), m["user"]["displayName"].clone()))
            .collect();
        if !contents.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        contents,
        vec![(json!("Привет из Matrix"), json!("anna · Matrix"))]
    );
    server
        .call(
            reqwest::Method::POST,
            &format!("/api/channels/{channel}/messages"),
            &token,
            Some(json!({"content": "Ответ"})),
        )
        .await;
    let sent = bodies(&matrix, "/send/m.room.message/").await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["body"], "quddy: Ответ");
    assert_eq!(sent[0]["formatted_body"], "<b>quddy</b>: Ответ");
}

#[tokio::test]
async fn discord_messages_go_out_through_a_webhook() {
    let discord = MockServer::start().await;
    // Gateway недоступен — приём не работает, отправка всё равно идёт.
    Mock::given(method("GET"))
        .and(path("/gateway/bot"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&discord)
        .await;
    Mock::given(method("GET"))
        .and(path("/channels/111/webhooks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&discord)
        .await;
    Mock::given(method("POST"))
        .and(path("/channels/111/webhooks"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "77", "token": "hook", "name": "Rustspace"})),
        )
        .mount(&discord)
        .await;
    Mock::given(method("POST"))
        .and(path("/webhooks/77/hook"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&discord)
        .await;
    let server = start_with(&[
        ("DISCORD_BOT_TOKEN", "dc"),
        ("DISCORD_API_URL", &discord.uri()),
    ])
    .await;
    let (token, channel) = user_with_channel(&server).await;
    let (status, _) = server
        .call(
            reqwest::Method::POST,
            &format!("/api/rustspace/channels/{channel}/bridges"),
            &token,
            Some(json!({"kind": "discord", "externalId": "111"})),
        )
        .await;
    assert_eq!(status, 201);
    for text in ["Первое", "Второе"] {
        server
            .call(
                reqwest::Method::POST,
                &format!("/api/channels/{channel}/messages"),
                &token,
                Some(json!({"content": text})),
            )
            .await;
    }
    let mut sent = Vec::new();
    for _ in 0..50 {
        sent = bodies(&discord, "/webhooks/77/hook").await;
        if sent.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let texts: Vec<_> = sent.iter().map(|body| body["content"].clone()).collect();
    assert_eq!(
        texts,
        vec![json!("Первое"), json!("Второе")],
        "порядок сохраняется"
    );
    assert_eq!(sent[0]["username"], "quddy");
    assert_eq!(sent[0]["allowed_mentions"], json!({"parse": []}));
    // Вебхук создан один раз и дальше берётся из памяти.
    let created = bodies(&discord, "/channels/111/webhooks").await;
    assert_eq!(created, vec![json!({"name": "Rustspace"})]);
}
