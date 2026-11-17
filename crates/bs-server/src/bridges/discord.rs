//! Мост в Discord: события — через Gateway (WebSocket), отправка — через
//! вебхук канала, чтобы в Discord было видно имя автора. Если права
//! «Управлять вебхуками» нет, бот пишет сам: «**Имя**: текст».
//!
//! У бота должен быть включён привилегированный intent «Message Content»
//! (Developer Portal → Bot), иначе текст сообщений приходит пустым.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::{Incoming, Kind, Outgoing, asks_id, backoff, client, id_reply};

/// Имя вебхука, через который мост пишет в канал.
const WEBHOOK_NAME: &str = "Rustspace";
/// GUILDS | GUILD_MESSAGES | MESSAGE_CONTENT.
const INTENTS: u64 = 1 | (1 << 9) | (1 << 15);

/// Отправка в Discord.
#[derive(Clone)]
pub(super) struct Sender {
    http: reqwest::Client,
    api: String,
    token: String,
    /// Адрес вебхука по каналу (`None` — вебхука нет и не будет).
    webhooks: Arc<Mutex<HashMap<String, Option<String>>>>,
}

impl Sender {
    pub(super) async fn send(&self, channel: &str, message: &Outgoing) -> Result<(), String> {
        if let Some(url) = self.webhook(channel).await {
            let body = json!({
                "content": clip(&message.text, 2000),
                "username": webhook_username(&message.name),
                "allowed_mentions": {"parse": []},
            });
            let response = self
                .http
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|err| err.without_url().to_string())?;
            if response.status().is_success() {
                return Ok(());
            }
            // Вебхук удалили — в следующий раз создать заново.
            self.webhooks.lock().await.remove(channel);
            tracing::debug!(status = %response.status(), "Discord: вебхук не принял сообщение");
        }
        self.post_as_bot(
            channel,
            &clip(&format!("**{}**: {}", message.name, message.text), 2000),
        )
        .await
    }

    async fn post_as_bot(&self, channel: &str, content: &str) -> Result<(), String> {
        let response = self
            .http
            .post(format!("{}/channels/{channel}/messages", self.api))
            .header("Authorization", format!("Bot {}", self.token))
            .json(&json!({"content": content, "allowed_mentions": {"parse": []}}))
            .send()
            .await
            .map_err(|err| err.without_url().to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("HTTP {}", response.status()))
        }
    }

    /// Вебхук моста в канале: найденный, созданный или `None`.
    async fn webhook(&self, channel: &str) -> Option<String> {
        if let Some(known) = self.webhooks.lock().await.get(channel) {
            return known.clone();
        }
        let found = self.find_or_create_webhook(channel).await;
        if found.is_none() {
            tracing::info!(
                channel,
                "Discord: вебхука нет (нужно право «Управлять вебхуками»), пишет бот"
            );
        }
        self.webhooks
            .lock()
            .await
            .insert(channel.to_owned(), found.clone());
        found
    }

    async fn find_or_create_webhook(&self, channel: &str) -> Option<String> {
        let auth = format!("Bot {}", self.token);
        let list: Value = self
            .http
            .get(format!("{}/channels/{channel}/webhooks", self.api))
            .header("Authorization", &auth)
            .send()
            .await
            .ok()
            .filter(|response| response.status().is_success())?
            .json()
            .await
            .ok()?;
        let url_of = |hook: &Value| {
            Some(format!(
                "{}/webhooks/{}/{}",
                self.api,
                hook["id"].as_str()?,
                hook["token"].as_str()?
            ))
        };
        if let Some(url) = list
            .as_array()
            .into_iter()
            .flatten()
            .filter(|hook| hook["name"].as_str() == Some(WEBHOOK_NAME))
            .find_map(url_of)
        {
            return Some(url);
        }
        let created: Value = self
            .http
            .post(format!("{}/channels/{channel}/webhooks", self.api))
            .header("Authorization", &auth)
            .json(&json!({"name": WEBHOOK_NAME}))
            .send()
            .await
            .ok()
            .filter(|response| response.status().is_success())?
            .json()
            .await
            .ok()?;
        url_of(&created)
    }
}

/// Запускает Gateway и возвращает отправителя.
pub(super) fn start(api: &str, token: String, incoming: mpsc::UnboundedSender<Incoming>) -> Sender {
    let sender = Sender {
        http: client(Duration::from_secs(30)),
        api: api.to_owned(),
        token,
        webhooks: Arc::new(Mutex::new(HashMap::new())),
    };
    let gateway = sender.clone();
    tokio::spawn(async move {
        let mut attempt = 0;
        loop {
            match session(&gateway, &incoming).await {
                Ok(()) => attempt = 0,
                Err(err) => {
                    attempt += 1;
                    tracing::warn!(%err, "Discord: соединение с Gateway прервано");
                }
            }
            tokio::time::sleep(backoff(attempt.max(1))).await;
        }
    });
    sender
}

/// Один сеанс Gateway: до разрыва или просьбы переподключиться.
async fn session(
    sender: &Sender,
    incoming: &mpsc::UnboundedSender<Incoming>,
) -> Result<(), String> {
    let info: Value = sender
        .http
        .get(format!("{}/gateway/bot", sender.api))
        .header("Authorization", format!("Bot {}", sender.token))
        .send()
        .await
        .map_err(|err| err.without_url().to_string())?
        .json()
        .await
        .map_err(|err| err.without_url().to_string())?;
    let url = info["url"].as_str().ok_or_else(|| {
        format!(
            "нет адреса Gateway: {}",
            info["message"].as_str().unwrap_or("?")
        )
    })?;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("{url}/?v=10&encoding=json"))
        .await
        .map_err(|err| err.to_string())?;
    let hello = next_json(&mut socket).await?;
    let interval = hello["d"]["heartbeat_interval"]
        .as_u64()
        .ok_or("Gateway не прислал HELLO")?;
    socket
        .send(WsMessage::text(
            json!({"op": 2, "d": {
                "token": sender.token,
                "intents": INTENTS,
                "properties": {"os": std::env::consts::OS, "browser": "rustspace", "device": "rustspace"},
            }})
            .to_string(),
        ))
        .await
        .map_err(|err| err.to_string())?;
    let mut heartbeat = tokio::time::interval(Duration::from_millis(interval));
    heartbeat.tick().await;
    let mut seq: Option<u64> = None;
    let mut me: Option<String> = None;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                socket
                    .send(WsMessage::text(json!({"op": 1, "d": seq}).to_string()))
                    .await
                    .map_err(|err| err.to_string())?;
            }
            frame = next_json(&mut socket) => {
                let frame = frame?;
                if let Some(s) = frame["s"].as_u64() {
                    seq = Some(s);
                }
                match frame["op"].as_u64() {
                    Some(1) => {
                        socket
                            .send(WsMessage::text(json!({"op": 1, "d": seq}).to_string()))
                            .await
                            .map_err(|err| err.to_string())?;
                    }
                    // RECONNECT и INVALID_SESSION: новый сеанс.
                    Some(7 | 9) => return Ok(()),
                    Some(0) => match frame["t"].as_str() {
                        Some("READY") => {
                            me = frame["d"]["user"]["id"].as_str().map(str::to_owned);
                            tracing::info!(
                                bot = frame["d"]["user"]["username"].as_str().unwrap_or("?"),
                                "Discord: бот подключён"
                            );
                        }
                        Some("MESSAGE_CREATE") => {
                            if let Some(message) = parse(&frame["d"], me.as_deref()) {
                                if asks_id(&message.text) {
                                    if let Err(err) =
                                        sender.post_as_bot(&message.chat, &id_reply(&message.chat)).await
                                    {
                                        tracing::warn!(%err, "Discord: не удалось ответить на !rustspace_id");
                                    }
                                } else {
                                    let _ = incoming.send(message);
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Следующий JSON-кадр; закрытие — ошибка с кодом.
async fn next_json(socket: &mut Socket) -> Result<Value, String> {
    loop {
        match socket.next().await {
            Some(Ok(WsMessage::Text(text))) => {
                return serde_json::from_str(&text).map_err(|err| err.to_string());
            }
            Some(Ok(WsMessage::Close(frame))) => {
                return Err(format!(
                    "Gateway закрыл соединение: {}",
                    frame
                        .map(|f| format!("{} {}", f.code, f.reason))
                        .unwrap_or_default()
                ));
            }
            Some(Ok(_)) => {}
            Some(Err(err)) => return Err(err.to_string()),
            None => return Err("Gateway закрыл соединение".to_owned()),
        }
    }
}

/// Сообщение канала от человека или чужого бота; свои и вебхуки — нет.
fn parse(data: &Value, me: Option<&str>) -> Option<Incoming> {
    if data["webhook_id"].is_string() {
        return None;
    }
    let author = &data["author"];
    let user = author["id"].as_str()?;
    if Some(user) == me {
        return None;
    }
    let mut text = data["content"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned();
    for file in data["attachments"].as_array().into_iter().flatten() {
        if let (Some(name), Some(url)) = (file["filename"].as_str(), file["url"].as_str()) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("📎 {name}: {url}"));
        }
    }
    if text.is_empty() {
        return None;
    }
    let name = data["member"]["nick"]
        .as_str()
        .or_else(|| author["global_name"].as_str())
        .or_else(|| author["username"].as_str())
        .unwrap_or("Discord")
        .to_owned();
    Some(Incoming {
        kind: Kind::Discord,
        chat: data["channel_id"].as_str()?.to_owned(),
        user: user.to_owned(),
        name,
        text,
    })
}

/// Имя для вебхука: 1–80 символов, без «discord» (его Discord не пускает).
fn webhook_username(name: &str) -> String {
    let cleaned = name.replace(['@', '#', ':', '`'], "");
    let lower = cleaned.to_lowercase();
    let safe = if lower.contains("discord") || lower.trim() == "clyde" || cleaned.trim().is_empty()
    {
        format!("{cleaned} (Rustspace)").replace("iscord", "ısсord")
    } else {
        cleaned
    };
    clip(safe.trim(), 80)
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        text.chars().take(max - 1).chain(['…']).collect()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn gateway_messages_are_parsed() {
        let data = json!({"channel_id": "10", "content": "hi",
            "author": {"id": "5", "username": "oleg", "global_name": "Олег"},
            "attachments": [{"filename": "a.png", "url": "https://cdn/a.png"}]});
        let message = parse(&data, Some("1")).expect("сообщение");
        assert_eq!(message.name, "Олег");
        assert_eq!(message.text, "hi\n📎 a.png: https://cdn/a.png");
        assert!(
            parse(&data, Some("5")).is_none(),
            "свои сообщения пропускаются"
        );
        let hook =
            json!({"webhook_id": "9", "channel_id": "10", "content": "x", "author": {"id": "9"}});
        assert!(
            parse(&hook, None).is_none(),
            "сообщения вебхуков пропускаются"
        );
    }

    #[test]
    fn webhook_names_follow_discord_rules() {
        assert_eq!(webhook_username("Мария"), "Мария");
        assert!(
            !webhook_username("Discord fan")
                .to_lowercase()
                .contains("discord")
        );
        assert_eq!(webhook_username(&"x".repeat(100)).chars().count(), 80);
    }
}
