//! Личный мост в Matrix: клиентский API от имени самого пользователя
//! (токен доступа или вход по логину и паролю).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Remote, RemoteChat, deliver, save_secret, set_error};
use crate::bridges::{backoff, client};
use crate::state::AppState;

/// Приставка транзакций моста: свои события с ней в `/sync` пропускаются.
const TXN_PREFIX: &str = "rsp";
/// Сколько комнат показывать при выборе.
const MAX_ROOMS: usize = 100;

/// Подключённый аккаунт Matrix.
pub struct Account {
    api: Api,
    task: tokio::task::AbortHandle,
}

#[derive(Clone)]
struct Api {
    http: reqwest::Client,
    base: String,
    token: String,
    txn: Arc<AtomicU64>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("server", &self.api.base)
            .finish_non_exhaustive()
    }
}

impl Account {
    pub(super) async fn send(&self, room: &str, text: &str) -> Result<(), String> {
        let txn = format!(
            "{TXN_PREFIX}{}-{}",
            crate::state::now_ms(),
            self.api.txn.fetch_add(1, Ordering::Relaxed)
        );
        let path = format!(
            "/_matrix/client/v3/rooms/{}/send/m.room.message/{txn}",
            encode(room)
        );
        self.api
            .request(
                reqwest::Method::PUT,
                &path,
                Some(&json!({"msgtype": "m.text", "body": text})),
            )
            .await
            .map(drop)
    }

    pub(super) async fn chats(&self) -> Result<Vec<RemoteChat>, String> {
        let me = self.api.whoami().await?;
        let rooms = self
            .api
            .request(
                reqwest::Method::GET,
                "/_matrix/client/v3/joined_rooms",
                None,
            )
            .await?;
        let mut out = Vec::new();
        for room in rooms["joined_rooms"]
            .as_array()
            .into_iter()
            .flatten()
            .take(MAX_ROOMS)
        {
            let Some(room) = room.as_str() else { continue };
            let members = self
                .api
                .request(
                    reqwest::Method::GET,
                    &format!("/_matrix/client/v3/rooms/{}/joined_members", encode(room)),
                    None,
                )
                .await
                .unwrap_or_default();
            let others: Vec<String> = members["joined"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(id, _)| **id != me)
                .map(|(id, info)| {
                    info["display_name"]
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .map_or_else(|| local_part(id), str::to_owned)
                })
                .collect();
            let name = self
                .api
                .request(
                    reqwest::Method::GET,
                    &format!(
                        "/_matrix/client/v3/rooms/{}/state/m.room.name",
                        encode(room)
                    ),
                    None,
                )
                .await
                .ok()
                .and_then(|value| value["name"].as_str().map(str::to_owned))
                .filter(|name| !name.trim().is_empty());
            let title = name.unwrap_or_else(|| {
                if others.is_empty() {
                    room.to_owned()
                } else {
                    others.join(", ")
                }
            });
            out.push(RemoteChat {
                id: room.to_owned(),
                title,
                group: others.len() > 1,
            });
        }
        Ok(out)
    }

    pub(super) fn stop(&self) {
        self.task.abort();
    }
}

impl Api {
    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, String> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|err| err.without_url().to_string())?;
        let status = response.status();
        let value: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(value)
        } else {
            Err(format!(
                "HTTP {status}: {}",
                value["error"].as_str().unwrap_or_default()
            ))
        }
    }

    async fn whoami(&self) -> Result<String, String> {
        let value = self
            .request(
                reqwest::Method::GET,
                "/_matrix/client/v3/account/whoami",
                None,
            )
            .await?;
        value["user_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "сервер не назвал пользователя".to_owned())
    }
}

/// Адрес домашнего сервера: `https://`, без хвостового `/`.
fn homeserver(text: &str) -> Result<String, String> {
    let text = text.trim().trim_end_matches('/');
    let text = if text.contains("://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let url = url::Url::parse(&text).map_err(|_| "неверный адрес сервера Matrix".to_owned())?;
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
        return Err("неверный адрес сервера Matrix".to_owned());
    }
    Ok(text)
}

/// Вход: токен доступа или логин с паролем. Возвращает (адрес пользователя,
/// секрет для базы).
pub async fn login(
    server: &str,
    token: Option<&str>,
    user: Option<&str>,
    password: Option<&str>,
) -> Result<(String, Value), String> {
    let base = homeserver(server)?;
    let http = client(Duration::from_secs(30));
    let token = match (token.filter(|t| !t.trim().is_empty()), user, password) {
        (Some(token), _, _) => token.trim().to_owned(),
        (None, Some(user), Some(password)) => {
            let response = http
                .post(format!("{base}/_matrix/client/v3/login"))
                .json(&json!({
                    "type": "m.login.password",
                    "identifier": {"type": "m.id.user", "user": user.trim()},
                    "password": password,
                    "initial_device_display_name": "Rustspace",
                }))
                .send()
                .await
                .map_err(|err| err.without_url().to_string())?;
            let status = response.status();
            let value: Value = response.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                return Err(format!(
                    "вход не удался: {}",
                    value["error"].as_str().unwrap_or("HTTP ошибка")
                ));
            }
            value["access_token"]
                .as_str()
                .ok_or_else(|| "сервер не выдал токен".to_owned())?
                .to_owned()
        }
        _ => return Err("укажите токен доступа или логин и пароль".to_owned()),
    };
    let api = Api {
        http,
        base: base.clone(),
        token: token.clone(),
        txn: Arc::default(),
    };
    let me = api.whoami().await?;
    Ok((me, json!({"homeserver": base, "token": token})))
}

/// Запускает синхронизацию сохранённого аккаунта.
pub fn resume(state: &AppState, account: &str, secret: &Value) -> Result<Account, String> {
    let base = secret["homeserver"]
        .as_str()
        .ok_or_else(|| "нет адреса сервера".to_owned())?
        .to_owned();
    let token = secret["token"]
        .as_str()
        .ok_or_else(|| "нет токена".to_owned())?
        .to_owned();
    let api = Api {
        http: client(Duration::from_mins(1)),
        base,
        token,
        txn: Arc::default(),
    };
    let since = secret["since"].as_str().map(str::to_owned);
    let task = tokio::spawn(sync(
        state.clone(),
        account.to_owned(),
        api.clone(),
        secret.clone(),
        since,
    ));
    Ok(Account {
        api,
        task: task.abort_handle(),
    })
}

async fn sync(
    state: AppState,
    account: String,
    api: Api,
    mut secret: Value,
    mut since: Option<String>,
) {
    let mut attempt = 0;
    let me = loop {
        match api.whoami().await {
            Ok(me) => break me,
            Err(err) if err.contains("401") => {
                set_error(
                    &state,
                    &account,
                    "Matrix отозвал вход: подключите аккаунт заново",
                )
                .await;
                return;
            }
            Err(err) => {
                attempt += 1;
                tracing::warn!(account, %err, "личный мост Matrix: сервер не отвечает");
                tokio::time::sleep(backoff(attempt)).await;
            }
        }
    };
    attempt = 0;
    let mut saved = since.clone();
    loop {
        let path = match &since {
            // Первый запуск: только узнать, откуда продолжать.
            None => "/_matrix/client/v3/sync?timeout=0".to_owned(),
            Some(token) => format!(
                "/_matrix/client/v3/sync?timeout=30000&since={}",
                encode(token)
            ),
        };
        let batch = match api.request(reqwest::Method::GET, &path, None).await {
            Ok(batch) => {
                attempt = 0;
                batch
            }
            Err(err) => {
                if err.contains("401") {
                    set_error(
                        &state,
                        &account,
                        "Matrix отозвал вход: подключите аккаунт заново",
                    )
                    .await;
                    return;
                }
                attempt += 1;
                tracing::warn!(account, %err, "личный мост Matrix: sync не удался");
                tokio::time::sleep(backoff(attempt)).await;
                continue;
            }
        };
        let first = since.is_none();
        since = batch["next_batch"].as_str().map(str::to_owned).or(since);
        if !first {
            for message in parse_batch(&account, &batch, &me) {
                deliver(&state, message).await;
            }
        }
        // Позиция синхронизации переживает перезапуск сервера.
        if since != saved {
            if let Some(token) = &since {
                secret["since"] = json!(token);
                save_secret(&state, &account, &secret).await;
            }
            saved.clone_from(&since);
        }
    }
}

/// Сообщения из ответа `/sync`.
fn parse_batch(account: &str, batch: &Value, me: &str) -> Vec<Remote> {
    let mut out = Vec::new();
    let Some(rooms) = batch["rooms"]["join"].as_object() else {
        return out;
    };
    for (room, data) in rooms {
        for event in data["timeline"]["events"].as_array().into_iter().flatten() {
            if let Some(message) = parse(account, room, event, me) {
                out.push(message);
            }
        }
    }
    out
}

fn parse(account: &str, room: &str, event: &Value, me: &str) -> Option<Remote> {
    if event["type"].as_str() != Some("m.room.message") {
        return None;
    }
    let sender = event["sender"].as_str()?;
    let mine = sender == me;
    // Своё, отправленное мостом: уже есть в переписке.
    if mine
        && event["unsigned"]["transaction_id"]
            .as_str()
            .is_some_and(|txn| txn.starts_with(TXN_PREFIX))
    {
        return None;
    }
    let content = &event["content"];
    if content["m.relates_to"]["rel_type"].as_str() == Some("m.replace") {
        return None;
    }
    let body = content["body"].as_str()?.trim();
    let text = match content["msgtype"].as_str() {
        Some("m.image") => format!("[📷 {body}]"),
        Some("m.file" | "m.video" | "m.audio") => format!("[📎 {body}]"),
        Some("m.emote") => format!("* {body}"),
        _ => body.to_owned(),
    };
    if text.is_empty() {
        return None;
    }
    Some(Remote {
        account: account.to_owned(),
        chat: room.to_owned(),
        sender: local_part(sender),
        text,
        mine,
    })
}

fn local_part(id: &str) -> String {
    id.trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or(id)
        .to_owned()
}

fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn own_bridge_echo_is_skipped_but_phone_messages_are_kept() {
        let batch = json!({"rooms": {"join": {"!r:x": {"timeline": {"events": [
            {"type": "m.room.message", "sender": "@anna:x", "content": {"msgtype": "m.text", "body": "привет"}},
            {"type": "m.room.message", "sender": "@me:x", "content": {"body": "из моста"},
             "unsigned": {"transaction_id": "rsp1-0"}},
            {"type": "m.room.message", "sender": "@me:x", "content": {"body": "с телефона"}},
            {"type": "m.room.member", "sender": "@anna:x", "content": {}},
        ]}}}}});
        let messages = parse_batch("a1", &batch, "@me:x");
        assert_eq!(messages.len(), 2);
        assert_eq!(
            (messages[0].sender.as_str(), messages[0].mine),
            ("anna", false)
        );
        assert_eq!(
            (messages[1].text.as_str(), messages[1].mine),
            ("с телефона", true)
        );
    }

    #[test]
    fn homeserver_gets_a_scheme() {
        assert_eq!(
            homeserver("matrix.org/").expect("адрес"),
            "https://matrix.org"
        );
        assert!(homeserver("ftp://x").is_err());
    }
}
