//! Установка соединения, рукопожатие `auth` и heartbeat.

use std::time::Duration;

use bs_types::{ClientEvent, ClientKind, ServerEvent, TransportPolicy};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async_with_config};
use url::Url;

use crate::error::WsError;

/// Интервал клиентского `ping` (сервер ждёт активности не реже ~65с).
pub const CLIENT_PING_INTERVAL: Duration = Duration::from_secs(15);

/// Таймаут на получение `ready` после `auth`.
pub const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Максимальный размер одного входящего сообщения WebSocket.
///
/// Умолчание tungstenite — 64 МиБ; события Backspace (даже `ready` со
/// списками участников) на порядки меньше, а сервер — внешний.
pub const MAX_INCOMING_MESSAGE: usize = 8 * 1024 * 1024;

/// Максимальный размер одного входящего фрейма.
pub const MAX_INCOMING_FRAME: usize = 2 * 1024 * 1024;

/// Тип потока tokio-tungstenite.
pub type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Активное WebSocket-соединение с инстансом.
#[derive(Debug)]
pub struct WsConnection {
    /// Исходящий поток событий клиента.
    sender: mpsc::Sender<ClientEvent>,
    /// Входящий поток событий сервера.
    receiver: mpsc::Receiver<ServerEvent>,
    /// Задача, обслуживающая сокет.
    task: tokio::task::JoinHandle<()>,
}

impl WsConnection {
    /// Подключается к `/ws` инстанса и проходит `auth`.
    ///
    /// `base` — абсолютный URL инстанса (например, `https://chat.example.com`).
    /// Возвращает соединение и payload `ready` (первое событие после auth).
    pub async fn connect(
        base: &str,
        token: &str,
        client: Option<ClientKind>,
    ) -> Result<(Self, ServerEvent), WsError> {
        Self::connect_with_policy(base, token, client, TransportPolicy::Strict).await
    }

    /// Как [`WsConnection::connect`], но с явной политикой транспорта.
    ///
    /// Токен уходит первым фреймом `auth`; по открытому `ws://` он был бы
    /// виден любому на пути, поэтому `Strict` пускает только `wss` и loopback.
    pub async fn connect_with_policy(
        base: &str,
        token: &str,
        client: Option<ClientKind>,
        policy: TransportPolicy,
    ) -> Result<(Self, ServerEvent), WsError> {
        let url = ws_url(base)?;
        policy.check(&url)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_INCOMING_MESSAGE))
            .max_frame_size(Some(MAX_INCOMING_FRAME));
        let (stream, _response) =
            connect_async_with_config(url.as_str(), Some(config), false).await?;
        let (mut sink, mut source) = stream.split();

        let auth = ClientEvent::Auth {
            token: token.to_owned(),
            client,
        };
        let payload = serde_json::to_string(&auth).map_err(tungstenite_json_error)?;
        sink.send(WsMessage::Text(payload.into())).await?;

        let ready = tokio::time::timeout(READY_TIMEOUT, read_ready(&mut source)).await;
        let ready = match ready {
            Ok(Ok(event)) => event,
            Ok(Err(err)) => return Err(err),
            Err(_) => return Err(WsError::ClosedBeforeReady),
        };

        let (out_tx, mut out_rx) = mpsc::channel::<ClientEvent>(64);
        let (in_tx, in_rx) = mpsc::channel::<ServerEvent>(256);

        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(CLIENT_PING_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    incoming = source.next() => {
                        match incoming {
                            Some(Ok(WsMessage::Text(text))) => {
                                match serde_json::from_str::<ServerEvent>(&text) {
                                    Ok(event) => {
                                        if in_tx.send(event).await.is_err() {
                                            break;
                                        }
                                    }
                                    // Содержимое не логируем: это чужие сообщения.
                                    Err(err) => {
                                        tracing::debug!(%err, bytes = text.len(), "событие сервера не разобрано");
                                    }
                                }
                            }
                            Some(Ok(WsMessage::Close(_)) | Err(_)) | None => break,
                            Some(Ok(_)) => {}
                        }
                    }
                    outgoing = out_rx.recv() => {
                        let Some(event) = outgoing else { break };
                        let Ok(text) = serde_json::to_string(&event) else { continue };
                        if sink.send(WsMessage::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    _ = ticker.tick() => {
                        if sink.send(WsMessage::Text("{\"type\":\"ping\"}".into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let connection = Self {
            sender: out_tx,
            receiver: in_rx,
            task,
        };
        Ok((connection, ready))
    }

    /// Отправляет событие серверу.
    pub async fn send(&self, event: ClientEvent) -> Result<(), WsError> {
        self.sender
            .send(event)
            .await
            .map_err(|_| WsError::Disconnected)
    }

    /// Ждёт следующее событие сервера.
    pub async fn recv(&mut self) -> Option<ServerEvent> {
        self.receiver.recv().await
    }

    /// Закрывает соединение, дожидаясь завершения задачи.
    pub async fn close(self) {
        drop(self.sender);
        let _ = self.task.await;
    }
}

/// Исходящая половина соединения (можно клонировать и слать из разных задач).
#[derive(Debug, Clone)]
pub struct WsSender {
    sender: mpsc::Sender<ClientEvent>,
}

impl WsSender {
    /// Отправляет событие серверу.
    pub async fn send(&self, event: ClientEvent) -> Result<(), WsError> {
        self.sender
            .send(event)
            .await
            .map_err(|_| WsError::Disconnected)
    }

    /// `true`, если фоновая задача сокета уже завершилась.
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

/// Разобранное соединение: независимые половинки + задача сокета.
#[derive(Debug)]
pub struct WsParts {
    /// Исходящий поток событий клиента.
    pub sender: WsSender,
    /// Входящий поток событий сервера.
    pub receiver: mpsc::Receiver<ServerEvent>,
    /// Задача, обслуживающая сокет.
    pub task: tokio::task::JoinHandle<()>,
}

impl WsConnection {
    /// Разбирает соединение на независимые половины.
    ///
    /// Позволяет слать и принимать одновременно из разных задач —
    /// например, в диспетчере переподключений вышестоящего крейта.
    pub fn into_parts(self) -> WsParts {
        WsParts {
            sender: WsSender {
                sender: self.sender,
            },
            receiver: self.receiver,
            task: self.task,
        }
    }
}

/// Переводит базу `http(s)://host` в `ws(s)://host/ws`.
fn ws_url(base: &str) -> Result<Url, WsError> {
    let mut url = Url::parse(base)?;
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        other => other,
    }
    .to_owned();
    url.set_scheme(&scheme)
        .map_err(|()| WsError::Url(url::ParseError::RelativeUrlWithoutBase))?;
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&format!("{path}/ws"));
    Ok(url)
}

/// Ждёт `ready`, пропуская прочие события; `error` превращает в отказ.
async fn read_ready<S>(source: &mut S) -> Result<ServerEvent, WsError>
where
    S: futures_util::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    while let Some(message) = source.next().await {
        let Ok(WsMessage::Text(text)) = message else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<ServerEvent>(&text) else {
            continue;
        };
        match &event {
            ServerEvent::Ready { .. } => return Ok(event),
            ServerEvent::Error { message, .. } => {
                // Сервер Backspace не шлёт машинного кода, поэтому
                // причина восстанавливается по тексту сообщения.
                return Err(WsError::AuthFailed {
                    failure: crate::auth::AuthFailure::classify(message),
                    message: message.clone(),
                });
            }
            _ => {}
        }
    }
    Err(WsError::ClosedBeforeReady)
}

/// Обёртка для ошибки сериализации `auth`.
fn tungstenite_json_error(err: serde_json::Error) -> WsError {
    WsError::Transport(tokio_tungstenite::tungstenite::Error::Io(
        std::io::Error::other(err),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::ws_url;

    #[test]
    fn https_becomes_wss() {
        let url = ws_url("https://chat.example.com").unwrap();
        assert_eq!(url.as_str(), "wss://chat.example.com/ws");
    }

    #[test]
    fn http_becomes_ws() {
        let url = ws_url("http://localhost:3000").unwrap();
        assert_eq!(url.as_str(), "ws://localhost:3000/ws");
    }

    #[test]
    fn trailing_slash_is_trimmed() {
        let url = ws_url("https://chat.example.com/").unwrap();
        assert_eq!(url.as_str(), "wss://chat.example.com/ws");
    }

    #[test]
    fn keeps_base_path_prefix() {
        let url = ws_url("https://example.com/backspace").unwrap();
        assert_eq!(url.as_str(), "wss://example.com/backspace/ws");
    }

    #[test]
    fn existing_ws_scheme_is_preserved() {
        let url = ws_url("ws://localhost:8080").unwrap();
        assert_eq!(url.as_str(), "ws://localhost:8080/ws");
    }

    #[test]
    fn rejects_relative_url() {
        assert!(ws_url("not a url").is_err());
    }

    #[tokio::test]
    async fn plaintext_ws_to_remote_host_is_refused_before_connecting() {
        // 192.0.2.0/24 (TEST-NET-1) недостижим: если бы политика не сработала,
        // тест завис бы на соединении, а не получил мгновенный отказ.
        let result = super::WsConnection::connect("http://192.0.2.1:3005", "jwt", None).await;
        assert!(matches!(result, Err(crate::error::WsError::Insecure(_))));
    }
}
