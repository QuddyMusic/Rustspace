//! Проверка сквозного шифрования двумя клиентами на настоящих серверах.
//!
//! Тесты помечены `#[ignore]` и идут только с адресами серверов:
//!
//! ```sh
//! # оба на одном сервере (например, оригинал на 3105 из tools/compat/up.sh)
//! BS_E2EE_LIVE_A=http://127.0.0.1:3105 cargo test -p bs-app e2ee_live -- --ignored
//! # через федерацию: два свежих оригинала из tools/compat/fed_up.sh
//! BS_E2EE_LIVE_A=https://x1.test BS_E2EE_LIVE_B=https://y1.test \
//!   BS_E2EE_LIVE_CA=/tmp/fedca/ca.pem cargo test -p bs-app e2ee_live -- --ignored
//! ```
//!
//! Клиенты — [`E2ee`] приложения: объявление ключа, знакомство с ключом
//! собеседника, шифрование исходящих, расшифровка входящих, смена ключа.
//! Сообщения ходят через REST (сервер хранит их так же, как пришедшие по
//! WebSocket); на серверах проверяется, что текст лежит только шифром.

use std::time::Duration;

use bs_types::DmMessageWithUser;
use serde_json::{Value, json};

use super::E2ee;

struct Side {
    http: reqwest::Client,
    base: String,
    token: String,
    id: String,
    name: String,
}

fn http() -> reqwest::Client {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(20));
    if let Ok(path) = std::env::var("BS_E2EE_LIVE_CA")
        && let Ok(pem) = crate::files::read(&path)
        && let Ok(cert) = reqwest::Certificate::from_pem(&pem)
    {
        builder = builder.add_root_certificate(cert);
    }
    builder.build().expect("клиент HTTP")
}

fn suffix() -> String {
    let nanos = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    format!("{nanos:08x}")
}

impl Side {
    async fn register(base: &str, prefix: &str) -> Self {
        let http = http();
        let name = format!("{prefix}{}", suffix());
        let reply: Value = http
            .post(format!("{base}/api/auth/register"))
            .json(&json!({"username": name, "password": "password123"}))
            .send()
            .await
            .expect("регистрация")
            .json()
            .await
            .expect("ответ регистрации");
        Self {
            http,
            base: base.to_owned(),
            token: reply["token"].as_str().expect("токен").to_owned(),
            id: reply["user"]["id"].as_str().expect("id").to_owned(),
            name,
        }
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let reply = request.send().await.expect("запрос");
        let status = reply.status().as_u16();
        (status, reply.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        let (status, value) = self.call(reqwest::Method::POST, path, Some(body)).await;
        assert!(status < 300, "POST {path}: {status} {value}");
        value
    }

    async fn get(&self, path: &str) -> Value {
        let (status, value) = self.call(reqwest::Method::GET, path, None).await;
        assert!(status < 300, "GET {path}: {status} {value}");
        value
    }

    async fn send(&self, dm: &str, content: &str) {
        self.post(
            &format!("/api/dm/{dm}/messages"),
            json!({"content": content}),
        )
        .await;
    }

    /// Ждёт переписку с ключом `federated`, а без ключа — единственную новую.
    async fn channel(&self, federated: Option<&str>) -> String {
        for _ in 0..60 {
            let list = self.get("/api/dm").await;
            let found = list
                .as_array()
                .into_iter()
                .flatten()
                .find(|c| federated.is_none_or(|key| c["federatedId"].as_str() == Some(key)));
            if let Some(found) = found {
                return found["id"].as_str().expect("id переписки").to_owned();
            }
            bs_rt::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("переписка не появилась у {}", self.name);
    }

    /// Сообщения переписки, когда их не меньше `count`.
    async fn messages(&self, dm: &str, count: usize) -> Vec<DmMessageWithUser> {
        for _ in 0..80 {
            let page = self.get(&format!("/api/dm/{dm}/messages?limit=100")).await;
            let list: Vec<DmMessageWithUser> =
                serde_json::from_value(page).expect("страница сообщений");
            if list.len() >= count {
                return list;
            }
            bs_rt::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("у {} нет {count} сообщений", self.name);
    }

    /// Пересылает исходящие объявления ключа клиента.
    async fn flush(&self, client: &mut E2ee) {
        for (dm, text) in std::mem::take(&mut client.outbox) {
            self.send(&dm, &text).await;
        }
    }
}

/// Последнее сообщение собеседника после расшифровки клиентом.
async fn read_last(side: &Side, client: &mut E2ee, dm: &str, count: usize, from: &str) -> String {
    let mut page = side.messages(dm, count).await;
    client.incoming_page(&mut page, true);
    page.iter()
        .filter(|m| m.message.user_id.as_str() == from)
        .max_by_key(|m| m.message.created_at)
        .and_then(|m| m.message.content.clone())
        .unwrap_or_default()
}

/// Весь путь: друзья, переписка, обмен ключами, шифр в обе стороны, шифр на
/// серверах, контрольные числа, смена ключа.
#[allow(clippy::too_many_lines, reason = "один сценарий по шагам")]
async fn run(a_base: &str, b_base: &str) {
    let federated = a_base != b_base;
    let admin = Side::register(a_base, "e2eadm").await;
    let a = Side::register(a_base, "e2ea").await;
    let b = Side::register(b_base, "e2eb").await;
    if federated {
        // На свежем стенде первый пользователь — администратор.
        let host = b_base
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let _ = admin
            .call(
                reqwest::Method::POST,
                "/api/federation/peer/initiate",
                Some(json!({"remoteOrigin": b_base})),
            )
            .await;
        bs_rt::time::sleep(Duration::from_secs(2)).await;
        a.post(
            "/api/social/requests",
            json!({"username": format!("{}@{host}", b.name)}),
        )
        .await;
    } else {
        a.post("/api/social/requests", json!({"username": b.name}))
            .await;
    }
    let mut request = None;
    for _ in 0..60 {
        let list = b.get("/api/social/requests").await;
        request = list
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| r["toId"].as_str() == Some(&b.id))
            .and_then(|r| r["id"].as_str().map(str::to_owned));
        if request.is_some() {
            break;
        }
        bs_rt::time::sleep(Duration::from_millis(250)).await;
    }
    let request = request.expect("заявка дошла до b");
    let (status, _) = b
        .call(
            reqwest::Method::PATCH,
            &format!("/api/social/requests/{request}"),
            Some(json!({"status": "accepted"})),
        )
        .await;
    assert!(status < 300, "принятие заявки: {status}");
    let mut friend = None;
    for _ in 0..60 {
        let list = a.get("/api/social/friends").await;
        friend = list
            .as_array()
            .into_iter()
            .flatten()
            .find(|f| {
                f["username"]
                    .as_str()
                    .is_some_and(|n| n.starts_with(&b.name))
            })
            .and_then(|f| f["id"].as_str().map(str::to_owned));
        if friend.is_some() {
            break;
        }
        bs_rt::time::sleep(Duration::from_millis(250)).await;
    }
    let friend = friend.expect("b в друзьях у a");
    let opened = a.post("/api/dm", json!({"userId": friend})).await;
    let a_dm = opened["id"].as_str().expect("переписка a").to_owned();
    let key = opened["federatedId"].as_str().map(str::to_owned);

    let mut alice = E2ee::load(None, &a.id);
    let mut bob = E2ee::load(None, &b.id);

    // a включает шифрование: в переписку уходит её ключ.
    alice.set_enabled(&a_dm, true);
    a.flush(&mut alice).await;
    let b_dm = if federated {
        b.channel(key.as_deref()).await
    } else {
        a_dm.clone()
    };
    // b знакомится с ключом, сам включает шифрование и отвечает ключом и шифром.
    let mut page = b.messages(&b_dm, 1).await;
    bob.incoming_page(&mut page, true);
    assert!(bob.status(&b_dm).has_peer_key, "b узнал ключ a");
    assert!(bob.status(&b_dm).enabled, "у b шифрование включилось само");
    b.flush(&mut bob).await;
    let sealed = bob
        .outgoing(&b_dm, "привет от b".to_owned())
        .expect("шифр b");
    assert!(bs_e2ee::is_sealed(&sealed));
    b.send(&b_dm, &sealed).await;

    // a читает: ключ b и расшифрованный текст.
    let from_b = if federated {
        friend.clone()
    } else {
        b.id.clone()
    };
    let text = read_last(&a, &mut alice, &a_dm, 3, &from_b).await;
    assert_eq!(text, "привет от b");
    let reply = alice.outgoing(&a_dm, "ответ a".to_owned()).expect("шифр a");
    a.send(&a_dm, &reply).await;
    let from_a = if federated {
        let page = b.messages(&b_dm, 4).await;
        page.iter()
            .map(|m| m.message.user_id.clone())
            .find(|id| *id != b.id)
            .expect("копия a у b")
    } else {
        a.id.clone()
    };
    let text = read_last(&b, &mut bob, &b_dm, 4, &from_a).await;
    assert_eq!(text, "ответ a");

    // Контрольные числа совпадают у обоих.
    let (sa, sb) = (
        alice.status(&a_dm).safety_number,
        bob.status(&b_dm).safety_number,
    );
    assert!(
        sa.is_some() && sa == sb,
        "контрольные числа: {sa:?} / {sb:?}"
    );

    // На серверах лежит только шифр: открытого текста нет ни в одном сообщении.
    for (side, dm) in [(&a, &a_dm), (&b, &b_dm)] {
        let raw = side.get(&format!("/api/dm/{dm}/messages?limit=100")).await;
        let raw = raw.to_string();
        assert!(
            !raw.contains("привет от b") && !raw.contains("ответ a"),
            "открытый текст на {}",
            side.base
        );
    }

    // b переходит на новое устройство (новый ключ): a перестаёт шифровать,
    // пока не подтвердит новый ключ, а потом снова читает b.
    let mut bob2 = E2ee::load(None, &b.id);
    bob2.set_enabled(&b_dm, true);
    b.flush(&mut bob2).await;
    let mut page = a.messages(&a_dm, 5).await;
    alice.incoming_page(&mut page, true);
    assert!(alice.status(&a_dm).key_changed, "a видит смену ключа b");
    assert!(
        alice.outgoing(&a_dm, "стоп".to_owned()).is_err(),
        "без подтверждения не шифрует"
    );
    alice.accept_new_key(&a_dm);
    let mut page = b.messages(&b_dm, 5).await;
    bob2.incoming_page(&mut page, true);
    let sealed = bob2
        .outgoing(&b_dm, "с нового устройства".to_owned())
        .expect("шифр b2");
    b.send(&b_dm, &sealed).await;
    let text = read_last(&a, &mut alice, &a_dm, 6, &from_b).await;
    assert_eq!(text, "с нового устройства");
}

#[tokio::test]
#[ignore = "нужны настоящие серверы: BS_E2EE_LIVE_A (и BS_E2EE_LIVE_B для федерации)"]
async fn e2ee_live_two_clients() {
    let Ok(a) = std::env::var("BS_E2EE_LIVE_A") else {
        return;
    };
    let b = std::env::var("BS_E2EE_LIVE_B").unwrap_or_else(|_| a.clone());
    run(a.trim_end_matches('/'), b.trim_end_matches('/')).await;
}
