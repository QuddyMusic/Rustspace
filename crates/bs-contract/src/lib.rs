//! Контрактные проверки клиента против **настоящего** сервера Backspace.
//!
//! Тесты в `tests/` запускаются, только если задан `BS_CONTRACT_URL`
//! (например, `http://127.0.0.1:3105`); без него они ничего не делают, чтобы
//! обычный `cargo test` не требовал сервера. Поднять оригинальный сервер и
//! прогнать тесты одной командой: `cargo xtask contract --server <путь>`.
//!
//! Каждое событие WebSocket и каждый ответ проверяются двумя способами:
//! 1. строгий разбор в типы `bs-types` — ошибка разбора означает, что клиент
//!    несовместим с сервером;
//! 2. сравнение ключей: поля, которые сервер прислал, а клиент при разборе
//!    потерял, попадают в отчёт о недостающих полях.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bs_types::{ClientEvent, ServerEvent};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Переменная окружения с адресом сервера.
pub const URL_ENV: &str = "BS_CONTRACT_URL";

/// Адрес сервера для контрактных тестов, если он задан.
#[must_use]
pub fn server_url() -> Option<String> {
    std::env::var(URL_ENV)
        .ok()
        .map(|url| url.trim_end_matches('/').to_owned())
        .filter(|url| !url.is_empty())
}

/// Короткий уникальный суффикс для имён пользователей и пространств.
#[must_use]
pub fn unique(prefix: &str) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &id[..8])
}

/// Найденные несовпадения протокола.
#[derive(Debug, Default, Clone)]
pub struct Findings {
    /// Кадры, которые не разобрались в `ServerEvent` (тип и ошибка).
    pub parse_errors: Vec<String>,
    /// Типы событий, неизвестные клиенту (`ServerEvent::Unknown`).
    pub unknown_events: BTreeSet<String>,
    /// Пути полей, которые прислал сервер, но не сохранил клиент.
    pub dropped_fields: BTreeSet<String>,
}

impl Findings {
    /// Добавляет находки другого источника.
    pub fn merge(&mut self, other: &Self) {
        self.parse_errors.extend(other.parse_errors.iter().cloned());
        self.unknown_events
            .extend(other.unknown_events.iter().cloned());
        self.dropped_fields
            .extend(other.dropped_fields.iter().cloned());
    }
}

/// Пути ключей, которые есть в `raw`, но пропали в `typed`.
///
/// `null` в исходнике не считается потерей: клиент вправе не хранить пустое
/// значение. Индексы массивов сворачиваются в `[]`, чтобы отчёт был коротким.
pub fn dropped_keys(prefix: &str, raw: &Value, typed: &Value, out: &mut BTreeSet<String>) {
    match (raw, typed) {
        (Value::Object(raw), Value::Object(typed)) => {
            for (key, raw_value) in raw {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                match typed.get(key) {
                    Some(typed_value) => dropped_keys(&path, raw_value, typed_value, out),
                    None if raw_value.is_null() => {}
                    None => {
                        out.insert(path);
                    }
                }
            }
        }
        (Value::Array(raw), Value::Array(typed)) => {
            let path = format!("{prefix}[]");
            for (raw_item, typed_item) in raw.iter().zip(typed) {
                dropped_keys(&path, raw_item, typed_item, out);
            }
        }
        _ => {}
    }
}

/// Строго разбирает JSON в `T` и записывает потерянные поля.
///
/// # Errors
/// Текст ошибки разбора.
pub fn check_typed<T>(label: &str, raw: &Value, findings: &mut Findings) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let typed: T = serde_json::from_value(raw.clone()).map_err(|err| format!("{label}: {err}"))?;
    let back = serde_json::to_value(&typed).map_err(|err| format!("{label}: {err}"))?;
    dropped_keys(label, raw, &back, &mut findings.dropped_fields);
    Ok(typed)
}

/// Соединение WebSocket, которое разбирает каждый кадр строго.
#[derive(Debug)]
pub struct StrictSocket {
    out: mpsc::Sender<String>,
    events: mpsc::Receiver<ServerEvent>,
    findings: Arc<Mutex<Findings>>,
}

impl StrictSocket {
    /// Подключается к `/ws`, отправляет `auth` и ждёт `ready`.
    ///
    /// # Errors
    /// Ошибка соединения или отсутствие `ready`.
    pub async fn connect(base: &str, token: &str) -> Result<(Self, ServerEvent), String> {
        let ws = base
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let (stream, _) = tokio_tungstenite::connect_async(format!("{ws}/ws"))
            .await
            .map_err(|err| format!("ws connect: {err}"))?;
        let (mut sink, mut source) = stream.split();
        let auth = serde_json::to_string(&ClientEvent::Auth {
            token: token.to_owned(),
            client: None,
        })
        .map_err(|err| err.to_string())?;
        sink.send(WsMessage::Text(auth.into()))
            .await
            .map_err(|err| format!("ws auth: {err}"))?;

        let findings = Arc::new(Mutex::new(Findings::default()));
        let (event_tx, mut events) = mpsc::channel::<ServerEvent>(1024);
        let (out, mut out_rx) = mpsc::channel::<String>(64);
        let sink_findings = Arc::clone(&findings);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    frame = source.next() => {
                        let Some(Ok(WsMessage::Text(text))) = frame else {
                            if matches!(frame, Some(Ok(_))) { continue; }
                            break;
                        };
                        let Some(event) = parse_frame(&text, &sink_findings) else { continue };
                        if event_tx.send(event).await.is_err() {
                            break;
                        }
                    }
                    outgoing = out_rx.recv() => {
                        let Some(text) = outgoing else { break };
                        if sink.send(WsMessage::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let ready = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .map_err(|_| "ready не пришёл за 10 с".to_owned())?
            .ok_or_else(|| "сокет закрыт до ready".to_owned())?;
        if !matches!(ready, ServerEvent::Ready(_)) {
            return Err(format!("первым пришло не ready: {ready:?}"));
        }
        Ok((
            Self {
                out,
                events,
                findings,
            },
            ready,
        ))
    }

    /// Отправляет событие клиента.
    ///
    /// # Errors
    /// Соединение закрыто.
    pub async fn send(&self, event: &ClientEvent) -> Result<(), String> {
        let text = serde_json::to_string(event).map_err(|err| err.to_string())?;
        self.out
            .send(text)
            .await
            .map_err(|_| "сокет закрыт".to_owned())
    }

    /// Ждёт событие, для которого `pick` вернёт `Some`; остальные пропускает.
    ///
    /// # Errors
    /// Таймаут или закрытие сокета.
    pub async fn wait_for<T>(
        &mut self,
        what: &str,
        mut pick: impl FnMut(&ServerEvent) -> Option<T>,
    ) -> Result<T, String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let event = tokio::time::timeout_at(deadline, self.events.recv())
                .await
                .map_err(|_| format!("не дождались события: {what}"))?
                .ok_or_else(|| format!("сокет закрыт в ожидании: {what}"))?;
            if let Some(found) = pick(&event) {
                return Ok(found);
            }
        }
    }

    /// Находки этого соединения.
    #[must_use]
    pub fn findings(&self) -> Findings {
        self.findings
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

fn parse_frame(text: &str, findings: &Mutex<Findings>) -> Option<ServerEvent> {
    let Ok(mut findings) = findings.lock() else {
        return None;
    };
    let raw: Value = match serde_json::from_str(text) {
        Ok(raw) => raw,
        Err(err) => {
            findings.parse_errors.push(format!("не JSON: {err}"));
            return None;
        }
    };
    let kind = raw
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("<без type>")
        .to_owned();
    if kind == "pong" {
        return None;
    }
    match check_typed::<ServerEvent>(&kind, &raw, &mut findings) {
        Ok(ServerEvent::Unknown) => {
            findings.unknown_events.insert(kind);
            None
        }
        Ok(event) => Some(event),
        Err(err) => {
            findings.parse_errors.push(err);
            None
        }
    }
}
