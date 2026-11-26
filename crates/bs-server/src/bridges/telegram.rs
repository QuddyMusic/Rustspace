//! Мост в Telegram через Bot API: длинный опрос `getUpdates` и
//! `sendMessage`.
//!
//! Боту нужно выключить «privacy mode» (`/setprivacy` у @BotFather),
//! иначе в группах он видит только команды.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{Incoming, Kind, Outgoing, asks_id, backoff, id_reply, proxied_client};

/// Отправка в Telegram.
#[derive(Clone)]
pub(super) struct Sender {
    http: reqwest::Client,
    base: String,
}

impl Sender {
    pub(super) async fn send(&self, chat: &str, message: &Outgoing) -> Result<(), String> {
        let text = format!(
            "<b>{}</b>\n{}",
            escape(&message.name),
            escape(&message.text)
        );
        self.call(
            "sendMessage",
            &json!({
                "chat_id": chat,
                "text": text,
                "parse_mode": "HTML",
                "disable_web_page_preview": true,
            }),
        )
        .await
        .map(drop)
    }

    async fn call(&self, method: &str, body: &Value) -> Result<Value, String> {
        let response = self
            .http
            .post(format!("{}/{method}", self.base))
            .json(body)
            .send()
            .await
            .map_err(|err| err.without_url().to_string())?;
        let value: Value = response
            .json()
            .await
            .map_err(|err| err.without_url().to_string())?;
        if value["ok"].as_bool() == Some(true) {
            Ok(value["result"].clone())
        } else {
            Err(value["description"]
                .as_str()
                .unwrap_or("ответ без ok")
                .to_owned())
        }
    }
}

/// Запускает опрос и возвращает отправителя.
pub(super) fn start(
    api: &str,
    token: String,
    proxy: Option<&str>,
    incoming: mpsc::UnboundedSender<Incoming>,
) -> Sender {
    let sender = Sender {
        http: proxied_client(Duration::from_secs(70), proxy),
        base: format!("{api}/bot{token}"),
    };
    let poller = sender.clone();
    tokio::spawn(async move { poll(poller, incoming).await });
    sender
}

async fn poll(sender: Sender, incoming: mpsc::UnboundedSender<Incoming>) {
    let mut attempt = 0;
    let me = loop {
        match sender.call("getMe", &json!({})).await {
            Ok(me) => break me,
            Err(err) => {
                attempt += 1;
                tracing::warn!(%err, "Telegram: бот не отвечает (getMe)");
                tokio::time::sleep(backoff(attempt)).await;
            }
        }
    };
    let own_id = me["id"].as_i64();
    tracing::info!(
        bot = me["username"].as_str().unwrap_or("?"),
        "Telegram: бот подключён"
    );
    let mut offset: i64 = 0;
    attempt = 0;
    loop {
        let updates = sender
            .call(
                "getUpdates",
                &json!({"offset": offset, "timeout": 50, "allowed_updates": ["message"]}),
            )
            .await;
        let updates = match updates {
            Ok(Value::Array(list)) => {
                attempt = 0;
                list
            }
            Ok(_) => Vec::new(),
            Err(err) => {
                attempt += 1;
                tracing::warn!(%err, "Telegram: getUpdates не удался");
                tokio::time::sleep(backoff(attempt)).await;
                continue;
            }
        };
        if updates.is_empty() {
            // Настоящий Telegram держит запрос до 50 с; на случай, если
            // ответ пришёл сразу, — короткая пауза.
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        for update in updates {
            if let Some(id) = update["update_id"].as_i64() {
                offset = offset.max(id + 1);
            }
            let message = &update["message"];
            let Some(chat) = message["chat"]["id"].as_i64() else {
                continue;
            };
            let chat = chat.to_string();
            let from = &message["from"];
            if from["id"].as_i64() == own_id {
                continue;
            }
            let Some(text) = text_of(message) else {
                continue;
            };
            if asks_id(&text) {
                let reply = json!({"chat_id": chat, "text": id_reply(&chat)});
                if let Err(err) = sender.call("sendMessage", &reply).await {
                    tracing::warn!(%err, "Telegram: не удалось ответить на /rustspace_id");
                }
                continue;
            }
            let Some(user) = from["id"].as_i64() else {
                continue;
            };
            let _ = incoming.send(Incoming {
                kind: Kind::Telegram,
                chat,
                user: user.to_string(),
                name: name_of(from),
                text,
            });
        }
    }
}

/// Текст сообщения; у фото и файлов — подпись и пометка.
fn text_of(message: &Value) -> Option<String> {
    let body = message["text"]
        .as_str()
        .or_else(|| message["caption"].as_str())
        .unwrap_or_default()
        .trim()
        .to_owned();
    let attachment = if message["photo"].is_array() {
        Some("📷 фото")
    } else if message["video"].is_object() {
        Some("🎬 видео")
    } else if message["voice"].is_object() || message["audio"].is_object() {
        Some("🎙 аудио")
    } else if message["sticker"].is_object() {
        Some("стикер")
    } else if message["document"].is_object() {
        Some("📎 файл")
    } else {
        None
    };
    match (body.is_empty(), attachment) {
        (true, None) => None,
        (true, Some(note)) => Some(format!("[{note}]")),
        (false, Some(note)) => Some(format!("[{note}] {body}")),
        (false, None) => Some(body),
    }
}

/// Имя и фамилия, иначе @username.
fn name_of(from: &Value) -> String {
    let full = [from["first_name"].as_str(), from["last_name"].as_str()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    if full.trim().is_empty() {
        from["username"].as_str().unwrap_or("Telegram").to_owned()
    } else {
        full
    }
}

/// Экранирование для `parse_mode: HTML`.
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
    fn telegram_messages_become_text() {
        assert_eq!(
            text_of(&json!({"text": " привет "})).as_deref(),
            Some("привет")
        );
        assert_eq!(
            text_of(&json!({"photo": [], "caption": "дача"})).as_deref(),
            Some("[📷 фото] дача")
        );
        assert_eq!(text_of(&json!({"new_chat_members": []})), None);
        assert_eq!(
            name_of(&json!({"first_name": "Олег", "last_name": "К."})),
            "Олег К."
        );
        assert_eq!(name_of(&json!({"username": "oleg"})), "oleg");
        assert_eq!(escape("a<b>&"), "a&lt;b&gt;&amp;");
    }
}
