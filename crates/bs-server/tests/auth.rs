//! Регистрация, вход и профиль поверх настоящего HTTP.

#![allow(clippy::expect_used, clippy::unwrap_used, reason = "тесты")]

use std::collections::HashMap;
use std::net::SocketAddr;

use bs_server::config::Config;
use bs_server::db::Db;
use bs_server::params;
use bs_server::state::AppState;
use serde_json::{Value, json};

struct Server {
    base: String,
    state: AppState,
    http: reqwest::Client,
}

async fn start(extra: &[(&str, &str)]) -> Server {
    let mut vars: HashMap<String, String> = [
        ("JWT_SECRET", "0123456789abcdef0123456789abcdef"),
        ("BS_PASSWORD_HASH_COST", "4"),
        ("DIRECTORY_ENDPOINT", ""),
    ]
    .into_iter()
    .chain(extra.iter().copied())
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    vars.entry("TRUSTED_PROXY_HOPS".into())
        .or_insert_with(|| "0".into());
    let config = Config::from_map(&vars).expect("конфигурация");
    let state = AppState::new(config, Db::open_memory().expect("база")).expect("состояние");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("порт");
    let base = format!("http://{}", listener.local_addr().expect("адрес"));
    let app = bs_server::app(state.clone());
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
        state,
        http: reqwest::Client::new(),
    }
}

impl Server {
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .expect("запрос");
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str, token: Option<&str>) -> (u16, Value) {
        let mut request = self.http.get(format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.expect("запрос");
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn register(&self, name: &str) -> Value {
        let (status, body) = self
            .post(
                "/api/auth/register",
                json!({ "username": name, "password": "password123" }),
            )
            .await;
        assert_eq!(status, 201, "{body}");
        body
    }
}

#[tokio::test]
async fn register_login_and_profile() {
    let server = start(&[]).await;
    let first = server.register("Alice").await;
    assert_eq!(first["user"]["username"], "alice");
    assert_eq!(
        first["user"]["isAdmin"], true,
        "первый пользователь — администратор"
    );
    assert_eq!(first["user"]["showActivity"], true);
    assert_eq!(first["user"]["detachedHomeInstance"], Value::Null);
    assert!(first["user"].get("passwordHash").is_none());
    let second = server.register("bob").await;
    assert_eq!(second["user"]["isAdmin"], false);

    let (status, body) = server
        .post(
            "/api/auth/login",
            json!({ "username": " ALICE ", "password": "password123" }),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let token = body["token"].as_str().expect("токен");
    let (status, me) = server.get("/api/users/@me", Some(token)).await;
    assert_eq!(status, 200);
    assert_eq!(me["id"], first["user"]["id"]);

    let (status, body) = server
        .post(
            "/api/auth/login",
            json!({ "username": "alice", "password": "wrong-password" }),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("invalid_credentials"))
    );
    let (status, body) = server
        .post(
            "/api/auth/login",
            json!({ "username": "nobody", "password": "password123" }),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("invalid_credentials"))
    );
}

#[tokio::test]
async fn validation_errors_match_original() {
    let server = start(&[]).await;
    server.register("alice").await;
    let cases = [
        (
            json!({ "password": "password123" }),
            400,
            "username_required",
        ),
        (json!({ "username": "carol" }), 400, "password_required"),
        (
            json!({ "username": "ab", "password": "password123" }),
            400,
            "username_length_invalid",
        ),
        (
            json!({ "username": "bad name", "password": "password123" }),
            400,
            "username_characters_invalid",
        ),
        (
            json!({ "username": "carol", "password": "short" }),
            400,
            "password_too_short",
        ),
        (
            json!({ "username": "alice", "password": "password123" }),
            409,
            "username_taken",
        ),
        (
            json!({ "username": "carol", "password": "password123", "homeInstance": "a.example" }),
            400,
            "replicated_username_format_required",
        ),
    ];
    for (body, status, code) in cases {
        let (got, reply) = server.post("/api/auth/register", body.clone()).await;
        assert_eq!(
            (got, reply["code"].as_str()),
            (status, Some(code)),
            "{body}"
        );
        assert_eq!(reply["statusCode"], status);
    }
    let (_, reply) = server
        .post(
            "/api/auth/register",
            json!({ "username": "carol", "password": "short" }),
        )
        .await;
    assert_eq!(reply["error"], "Password must be at least 8 characters");
    assert_eq!(reply["details"]["min"], 8);
}

#[tokio::test]
async fn tokens_are_checked() {
    let server = start(&[]).await;
    let alice = server.register("alice").await;
    let token = alice["token"].as_str().expect("токен").to_owned();
    let (status, body) = server.get("/api/users/@me", None).await;
    assert_eq!((status, body["code"].as_str()), (401, Some("unauthorized")));
    let (status, _) = server.get("/api/users/@me", Some("not.a.token")).await;
    assert_eq!(status, 401);

    // Смена пароля отзывает выданные раньше токены.
    let id = alice["user"]["id"].as_str().expect("id").to_owned();
    let later = bs_server::state::now_ms() + 5_000;
    server
        .state
        .db
        .transaction(move |tx| {
            tx.execute(
                "UPDATE users SET password_changed_at = ? WHERE id = ?",
                params![later, &id],
            )
        })
        .await
        .expect("обновление");
    let (status, body) = server.get("/api/users/@me", Some(&token)).await;
    assert_eq!(status, 401);
    assert_eq!(
        body["error"],
        "Token has been revoked — please log in again"
    );

    // Удалённый аккаунт не входит и не проходит по старому токену.
    let bob = server.register("bob").await;
    let bob_id = bob["user"]["id"].as_str().expect("id").to_owned();
    server
        .state
        .db
        .transaction(move |tx| {
            tx.execute(
                "UPDATE users SET is_deleted = 1 WHERE id = ?",
                params![&bob_id],
            )
        })
        .await
        .expect("обновление");
    let (status, body) = server.get("/api/users/@me", bob["token"].as_str()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("account_deleted"))
    );
    let (status, body) = server
        .post(
            "/api/auth/login",
            json!({ "username": "bob", "password": "password123" }),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("account_deleted"))
    );
}

#[tokio::test]
async fn closed_registration_needs_a_live_invite() {
    let server = start(&[("REGISTRATION_OPEN", "false")]).await;
    let (status, body) = server
        .post(
            "/api/auth/register",
            json!({ "username": "alice", "password": "password123" }),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (403, Some("invite_required"))
    );
    let (status, body) = server
        .get("/api/auth/check-username?username=alice", None)
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (403, Some("registration_closed"))
    );

    // Приглашение на одно использование от существующего администратора.
    let token = "AAAAAAAAAAAAAAAAAAAAAA";
    let now = bs_server::state::now_ms();
    server
        .state
        .db
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO users (id, username, password_hash, is_admin, created_at) VALUES ('1', 'root', 'x', 1, ?)",
                params![now],
            )?;
            tx.execute(
                "INSERT INTO invite_links (id, token, name, created_by, created_at, max_uses) VALUES ('10', ?, 'друзьям', '1', ?, 1)",
                params![token, now],
            )
        })
        .await
        .expect("приглашение");
    let (_, check) = server
        .get(&format!("/api/auth/check-invite?token={token}"), None)
        .await;
    assert_eq!(check, json!({ "valid": true, "name": "друзьям" }));
    let (status, body) = server
        .post(
            "/api/auth/register",
            json!({ "username": "alice", "password": "password123", "inviteToken": "BBBBBBBBBBBBBBBBBBBBBB" }),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (403, Some("invite_invalid"))
    );
    let (status, body) = server
        .post(
            "/api/auth/register",
            json!({ "username": "alice", "password": "password123", "inviteToken": token }),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["user"]["isAdmin"], false);
    let (_, check) = server
        .get(&format!("/api/auth/check-invite?token={token}"), None)
        .await;
    assert_eq!(check, json!({ "valid": false, "reason": "exhausted" }));
    let (status, body) = server
        .post(
            "/api/auth/register",
            json!({ "username": "bob", "password": "password123", "inviteToken": token }),
        )
        .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (403, Some("invite_invalid"))
    );
}

#[tokio::test]
async fn login_is_rate_limited_per_client() {
    let server = start(&[]).await;
    let mut last = 0;
    for _ in 0..16 {
        let (status, _) = server
            .post(
                "/api/auth/login",
                json!({ "username": "nobody", "password": "password123" }),
            )
            .await;
        last = status;
    }
    assert_eq!(last, 429);
    let (status, body) = server.get("/api/health", None).await;
    assert_eq!((status, body["status"].as_str()), (200, Some("ok")));
}

#[tokio::test]
async fn instance_info_and_unknown_routes() {
    let server = start(&[]).await;
    let (status, info) = server.get("/api/instance/info", None).await;
    assert_eq!(status, 200);
    assert_eq!(info["name"], "Rustspace");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["compatibleWith"], "1.11.0");
    assert_eq!(info["registrationOpen"], true);
    assert_eq!(info["directoryConfigured"], false);
    assert_eq!(info["supportCardEnabled"], true);
    assert_eq!(info["instanceId"].as_str().map(str::len), Some(36));
    let (status, body) = server.get("/api/nope", None).await;
    assert_eq!(status, 404);
    assert_eq!(body["message"], "Route GET:/api/nope not found");
}
