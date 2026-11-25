//! Сокет: tokio-tungstenite на десктопе, WebSocket браузера в веб-версии.
//!
//! Наружу — текстовые фреймы: протокол Backspace других не использует.

use futures_util::{Sink, SinkExt, Stream, StreamExt};
use url::Url;

use crate::error::WsError;

/// Входящий фрейм.
#[derive(Debug)]
pub(crate) enum Frame {
    /// Текст (событие сервера).
    Text(String),
    /// Сервер закрыл соединение.
    Close,
    /// Прочее (двоичные данные, служебные ping/pong).
    Other,
}

/// Открывает сокет: исходящая половина принимает текст, входящая отдаёт
/// фреймы.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn open(
    url: &Url,
) -> Result<
    (
        impl Sink<String, Error = WsError> + Unpin + Send + 'static,
        impl Stream<Item = Result<Frame, WsError>> + Unpin + Send + 'static,
    ),
    WsError,
> {
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

    let config = WebSocketConfig::default()
        .max_message_size(Some(crate::client::MAX_INCOMING_MESSAGE))
        .max_frame_size(Some(crate::client::MAX_INCOMING_FRAME));
    let (stream, _response) =
        tokio_tungstenite::connect_async_with_config(url.as_str(), Some(config), false).await?;
    let (sink, source) = stream.split();
    let sink = sink
        .sink_map_err(WsError::from)
        .with(|text: String| std::future::ready(Ok::<_, WsError>(Message::Text(text.into()))));
    let source = source.map(|message| {
        message.map_err(WsError::from).map(|message| match message {
            Message::Text(text) => Frame::Text(text.as_str().to_owned()),
            Message::Close(_) => Frame::Close,
            _ => Frame::Other,
        })
    });
    Ok((Box::pin(sink), source))
}

/// Открывает сокет браузера: исходящая половина принимает текст, входящая
/// отдаёт фреймы. Размеры сообщений ограничивает браузер.
#[cfg(target_arch = "wasm32")]
#[allow(
    clippy::unused_async,
    reason = "та же подпись, что на десктопе; сокет браузера открывается при первой отправке"
)]
pub(crate) async fn open(
    url: &Url,
) -> Result<
    (
        impl Sink<String, Error = WsError> + Unpin + 'static,
        impl Stream<Item = Result<Frame, WsError>> + Unpin + 'static,
    ),
    WsError,
> {
    use gloo_net::websocket::Message;
    use gloo_net::websocket::futures::WebSocket;

    let socket =
        WebSocket::open(url.as_str()).map_err(|err| WsError::Transport(err.to_string()))?;
    let (sink, source) = socket.split();
    let sink = sink
        .sink_map_err(|err| WsError::Transport(err.to_string()))
        .with(|text: String| std::future::ready(Ok::<_, WsError>(Message::Text(text))));
    let source = source.map(|message| match message {
        Ok(Message::Text(text)) => Ok(Frame::Text(text)),
        Ok(Message::Bytes(_)) => Ok(Frame::Other),
        Err(gloo_net::websocket::WebSocketError::ConnectionClose(_)) => Ok(Frame::Close),
        Err(err) => Err(WsError::Transport(err.to_string())),
    });
    Ok((Box::pin(sink), source))
}
