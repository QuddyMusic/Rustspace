//! Мост в Matrix через клиентский API (`/sync` и `send`) от имени
//! пользователя-бота. Приглашения в комнаты бот принимает сам.
//! Зашифрованные комнаты не поддерживаются: в них бот видит только шифр.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{Incoming, Kind, Outgoing, asks_id, backoff, client, id_reply};

/// Отправка в Matrix.
#[derive(Clone)]
pub(super) struct Sender {
    http: reqwest::Client,
    base: String,
    token: String,
    /// Номер транзакции: уникален в пределах запуска.
    txn: Arc<AtomicU64>,
}

impl Sender {
    pub(super) async fn send(&self, room: &str, message: &Outgoing) -> Result<(), String> {
        self.post_text(
            room,
            &format!("{}: {}", message.name, message.text),
            Some(&format!(
                "<b>{}</b>: {}",
                escape(&message.name),
                escape(&message.text).replace('\n', "<br>")
            )),
        )
        .await
    }

    async fn post_text(&self, room: &str, body: &str, html: Option<&str>) -> Result<(), String> {
        let txn = format!(
            "rs{}-{}",
            crate::state::now_ms(),
            self.txn.fetch_add(1, Ordering::Relaxed)
        );
        let mut content = json!({"msgtype": "m.text", "body": body});
        if let Some(html) = html {
            content["format"] = json!("org.matrix.custom.html");
            content["formatted_body"] = json!(html);
        }
        let url = format!(
            "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{txn}",
            self.base,
            encode(room)
        );
        let response = self
            .http
            .put(url)
            .bearer_auth(&self.token)
            .json(&content)
            .send()
            .await
            .map_err(|err| err.without_url().to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("HTTP {}", response.status()))
        }
    }

    async fn get(&self, path: &str) -> Result<Value, String> {
        let response = self
            .http
            .get(format!("{}{path}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|err| err.without_url().to_string())?;
        let status = response.status();
        let value: Value = response
            .json()
            .await
            .map_err(|err| err.without_url().to_string())?;
        if status.is_success() {
            Ok(value)
        } else {
            Err(format!(
                "HTTP {status}: {}",
                value["error"].as_str().unwrap_or_default()
            ))
        }
    }

    async fn join(&self, room: &str) -> Result<(), String> {
        let response = self
            .http
            .post(format!(
                "{}/_matrix/client/v3/join/{}",
                self.base,
                encode(room)
            ))
            .bearer_auth(&self.token)
            .json(&json!({}))
            .send()
            .await
            .map_err(|err| err.without_url().to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("HTTP {}", response.status()))
        }
    }
}

/// Запускает синхронизацию и возвращает отправителя.
pub(super) fn start(
    homeserver: &str,
    token: String,
    incoming: mpsc::UnboundedSender<Incoming>,
) -> Sender {
    let sender = Sender {
        http: client(Duration::from_mins(1)),
        base: homeserver.to_owned(),
        token,
        txn: Arc::new(AtomicU64::new(0)),
    };
    let syncer = sender.clone();
    tokio::spawn(async move { sync(syncer, incoming).await });
    sender
}

async fn sync(sender: Sender, incoming: mpsc::UnboundedSender<Incoming>) {
    let mut attempt = 0;
    let me = loop {
        match sender.get("/_matrix/client/v3/account/whoami").await {
            Ok(value) => break value["user_id"].as_str().unwrap_or_default().to_owned(),
            Err(err) => {
                attempt += 1;
                tracing::warn!(%err, "Matrix: бот не отвечает (whoami)");
                tokio::time::sleep(backoff(attempt)).await;
            }
        }
    };
    tracing::info!(user = me, "Matrix: бот подключён");
    // Первая синхронизация — только чтобы узнать, откуда продолжать:
    // старую историю комнат не пересылаем.
    let mut since: Option<String> = None;
    attempt = 0;
    loop {
        let path = match &since {
            None => "/_matrix/client/v3/sync?timeout=0".to_owned(),
            Some(token) => format!(
                "/_matrix/client/v3/sync?timeout=30000&since={}",
                encode(token)
            ),
        };
        let batch = match sender.get(&path).await {
            Ok(batch) => {
                attempt = 0;
                batch
            }
            Err(err) => {
                attempt += 1;
                tracing::warn!(%err, "Matrix: sync не удался");
                tokio::time::sleep(backoff(attempt)).await;
                continue;
            }
        };
        let first = since.is_none();
        since = batch["next_batch"].as_str().map(str::to_owned).or(since);
        if let Some(invites) = batch["rooms"]["invite"].as_object() {
            for room in invites.keys() {
                match sender.join(room).await {
                    Ok(()) => tracing::info!(room, "Matrix: бот принял приглашение"),
                    Err(err) => tracing::warn!(room, %err, "Matrix: не удалось войти в комнату"),
                }
            }
        }
        if first {
            continue;
        }
        let Some(rooms) = batch["rooms"]["join"].as_object() else {
            continue;
        };
        for (room, data) in rooms {
            for event in data["timeline"]["events"].as_array().into_iter().flatten() {
                if let Some(message) = parse(room, event, &me) {
                    if asks_id(&message.text) {
                        if let Err(err) = sender.post_text(room, &id_reply(room), None).await {
                            tracing::warn!(%err, "Matrix: не удалось ответить на !rustspace_id");
                        }
                        continue;
                    }
                    let _ = incoming.send(message);
                }
            }
        }
        if batch.get("next_batch").is_none() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

/// Текстовое сообщение комнаты от кого-то, кроме самого бота.
fn parse(room: &str, event: &Value, me: &str) -> Option<Incoming> {
    if event["type"].as_str() != Some("m.room.message") {
        return None;
    }
    let sender = event["sender"].as_str()?;
    if sender == me {
        return None;
    }
    let content = &event["content"];
    // Правки приходят новыми событиями — их пропускаем.
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
    // Отображаемого имени в событии нет: локальная часть адреса.
    let name = sender
        .trim_start_matches('@')
        .split(':')
        .next()
        .unwrap_or(sender)
        .to_owned();
    Some(Incoming {
        kind: Kind::Matrix,
        chat: room.to_owned(),
        user: sender.to_owned(),
        name,
        text,
    })
}

/// Процентное кодирование части пути.
fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn room_messages_are_parsed() {
        let event = json!({"type": "m.room.message", "sender": "@oleg:matrix.org",
            "content": {"msgtype": "m.text", "body": "привет"}});
        let message = parse("!r:matrix.org", &event, "@bot:x").expect("сообщение");
        assert_eq!(
            (message.name.as_str(), message.text.as_str()),
            ("oleg", "привет")
        );
        assert!(
            parse(
                "!r:x",
                &json!({"type": "m.room.message", "sender": "@bot:x",
            "content": {"body": "своё"}}),
                "@bot:x"
            )
            .is_none()
        );
        let edit = json!({"type": "m.room.message", "sender": "@a:x",
            "content": {"body": "* x", "m.relates_to": {"rel_type": "m.replace"}}});
        assert!(parse("!r:x", &edit, "@bot:x").is_none());
        assert_eq!(encode("!abc:matrix.org"), "%21abc%3Amatrix.org");
    }
}
