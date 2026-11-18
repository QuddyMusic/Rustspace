//! Боты для Rustspace на Rust.
//!
//! Бот — пользователь инстанса со своим токеном (`rsb_…`). Его создают в
//! клиенте: «Настройки → Боты», там же добавляют в пространство. Бот
//! говорит с сервером тем же HTTP API и получает те же события по
//! WebSocket, что и обычный клиент, поэтому ботов можно писать на чём
//! угодно; этот крейт — родной для Rust.
//!
//! ```no_run
//! use bs_bot::{Bot, Context, ServerEvent};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), bs_bot::BotError> {
//!     let bot = Bot::new("https://chat.example.com", "rsb_…")?;
//!     bot.run(|ctx: Context, event: ServerEvent| async move {
//!         if let ServerEvent::MessageCreated { message } = event {
//!             if !ctx.is_me(&message.user) && message.message.content.as_deref() == Some("!ping") {
//!                 let _ = ctx.reply(&message, "pong").await;
//!             }
//!         }
//!     })
//!     .await
//! }
//! ```
//!
//! Обработчик получает каждое событие, начиная с `ready`, — в том числе о
//! собственных сообщениях бота: их отсеивает [`Context::is_me`]. События
//! обрабатываются параллельно, каждое в своей задаче tokio. При обрыве бот
//! переподключается сам; [`Bot::run`] возвращает ошибку, только если сервер
//! не принял токен.

#![forbid(unsafe_code)]

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

pub use bs_api::{self as api, ApiClient, ApiError, SendMessageBody};
pub use bs_types::{
    self as types, ClientEvent, DmMessageWithUser, MessageWithUser, ServerEvent, TransportPolicy,
    User,
};
use bs_ws::{WsConnection, WsError, WsSender};

/// Первая пауза перед переподключением; дальше она растёт вдвое.
const RECONNECT_MIN: Duration = Duration::from_secs(1);
/// Самая длинная пауза перед переподключением.
const RECONNECT_MAX: Duration = Duration::from_secs(60);

/// Ошибка бота.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BotError {
    /// Запрос к HTTP API не удался.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// Соединение с сервером событий не удалось или оборвалось.
    #[error(transparent)]
    Gateway(#[from] WsError),
    /// Сервер не принял токен: он неверен, заменён или бот удалён.
    #[error("сервер не принял токен бота: {0}")]
    Rejected(String),
}

/// Обработчик событий.
///
/// Готовая реализация есть у любой функции или замыкания
/// `Fn(Context, ServerEvent) -> impl Future<Output = ()>`; свой тип нужен,
/// когда у бота есть состояние.
pub trait Handler: Send + Sync + 'static {
    /// Вызывается на каждое событие сервера.
    fn handle(&self, ctx: Context, event: ServerEvent) -> impl Future<Output = ()> + Send;
}

impl<F, Fut> Handler for F
where
    F: Fn(Context, ServerEvent) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send,
{
    fn handle(&self, ctx: Context, event: ServerEvent) -> impl Future<Output = ()> + Send {
        self(ctx, event)
    }
}

/// Бот: адрес инстанса и токен.
#[derive(Debug)]
pub struct Bot {
    api: Arc<ApiClient>,
    token: String,
    policy: TransportPolicy,
}

impl Bot {
    /// Бот для инстанса `base` (например, `https://chat.example.com`).
    /// Открытый `http` годится только для `localhost`.
    ///
    /// # Errors
    /// Неверный адрес.
    pub fn new(base: &str, token: &str) -> Result<Self, BotError> {
        Self::with_policy(base, token, TransportPolicy::Strict)
    }

    /// Как [`Bot::new`], но с явной политикой транспорта:
    /// [`TransportPolicy::AllowInsecure`] разрешает открытый `http` к
    /// любому хосту — токен тогда идёт открытым текстом.
    ///
    /// # Errors
    /// Неверный адрес.
    pub fn with_policy(base: &str, token: &str, policy: TransportPolicy) -> Result<Self, BotError> {
        let mut api = ApiClient::with_policy(bs_types::Origin::Home, base, policy)?;
        api.set_token(Some(token.to_owned()));
        Ok(Self {
            api: Arc::new(api),
            token: token.to_owned(),
            policy,
        })
    }

    /// HTTP API от имени бота: всё, что умеет [`ApiClient`].
    pub fn api(&self) -> &ApiClient {
        &self.api
    }

    /// Подключается к событиям и передаёт их обработчику, пока сервер
    /// принимает токен. Обрывы связи переживает сам.
    ///
    /// # Errors
    /// [`BotError::Rejected`] — токен не принят; другие ошибки сюда не
    /// доходят.
    pub async fn run(self, handler: impl Handler) -> Result<(), BotError> {
        let handler = Arc::new(handler);
        let mut delay = RECONNECT_MIN;
        loop {
            match WsConnection::connect_with_policy(self.api.base(), &self.token, None, self.policy)
                .await
            {
                Ok((connection, ready)) => {
                    delay = RECONNECT_MIN;
                    let me = match &ready {
                        ServerEvent::Ready(payload) => payload.user.clone(),
                        _ => self.api.me().await?,
                    };
                    let mut parts = connection.into_parts();
                    let ctx = Context {
                        api: Arc::clone(&self.api),
                        gateway: parts.sender,
                        me: Arc::new(me),
                    };
                    tracing::info!(bot = %ctx.me.username, "бот на связи");
                    dispatch(&handler, &ctx, ready);
                    while let Some(event) = parts.receiver.recv().await {
                        dispatch(&handler, &ctx, event);
                    }
                    tracing::warn!("связь с сервером оборвалась, переподключаюсь");
                }
                Err(WsError::AuthFailed { message, .. }) => {
                    return Err(BotError::Rejected(message));
                }
                Err(err) => tracing::warn!(%err, "не удалось подключиться, повтор через {delay:?}"),
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(RECONNECT_MAX);
        }
    }
}

fn dispatch<H: Handler>(handler: &Arc<H>, ctx: &Context, event: ServerEvent) {
    let handler = Arc::clone(handler);
    let ctx = ctx.clone();
    tokio::spawn(async move { handler.handle(ctx, event).await });
}

/// Что доступно обработчику: API, соединение событий и сам бот.
#[derive(Debug, Clone)]
pub struct Context {
    api: Arc<ApiClient>,
    gateway: WsSender,
    me: Arc<User>,
}

impl Context {
    /// Пользователь бота.
    pub fn me(&self) -> &User {
        &self.me
    }

    /// `true`, если это сам бот — чтобы не отвечать себе.
    pub fn is_me(&self, user: &User) -> bool {
        user.id == self.me.id
    }

    /// HTTP API от имени бота.
    pub fn api(&self) -> &ApiClient {
        &self.api
    }

    /// Отправить серверу событие как есть.
    ///
    /// # Errors
    /// Соединение уже закрыто.
    pub async fn emit(&self, event: ClientEvent) -> Result<(), BotError> {
        Ok(self.gateway.send(event).await?)
    }

    /// Сообщение в канал.
    ///
    /// # Errors
    /// Нет права писать в канал.
    pub async fn send(&self, channel_id: &str, text: &str) -> Result<MessageWithUser, BotError> {
        let body = SendMessageBody {
            content: text.to_owned(),
            ..SendMessageBody::default()
        };
        Ok(self.api.send_channel_message(channel_id, &body).await?)
    }

    /// Ответ на сообщение в том же канале.
    ///
    /// # Errors
    /// Нет права писать в канал.
    pub async fn reply(
        &self,
        to: &MessageWithUser,
        text: &str,
    ) -> Result<MessageWithUser, BotError> {
        let body = SendMessageBody {
            content: text.to_owned(),
            reply_to_id: Some(to.message.id.clone()),
            ..SendMessageBody::default()
        };
        Ok(self
            .api
            .send_channel_message(&to.message.channel_id, &body)
            .await?)
    }

    /// Сообщение в личку или группу.
    ///
    /// # Errors
    /// Нет доступа к переписке.
    pub async fn send_dm(
        &self,
        dm_channel_id: &str,
        text: &str,
    ) -> Result<DmMessageWithUser, BotError> {
        let body = SendMessageBody {
            content: text.to_owned(),
            ..SendMessageBody::default()
        };
        Ok(self.api.send_dm_message(dm_channel_id, &body).await?)
    }

    /// Ответ на сообщение в личке.
    ///
    /// # Errors
    /// Нет доступа к переписке.
    pub async fn reply_dm(
        &self,
        to: &DmMessageWithUser,
        text: &str,
    ) -> Result<DmMessageWithUser, BotError> {
        let body = SendMessageBody {
            content: text.to_owned(),
            reply_to_id: Some(to.message.id.clone()),
            ..SendMessageBody::default()
        };
        Ok(self
            .api
            .send_dm_message(&to.message.dm_channel_id, &body)
            .await?)
    }

    /// Изменить своё сообщение в канале.
    ///
    /// # Errors
    /// Сообщение не своё или его нет.
    pub async fn edit(&self, message_id: &str, text: &str) -> Result<MessageWithUser, BotError> {
        Ok(self.api.edit_channel_message(message_id, text).await?)
    }

    /// Удалить сообщение в канале (чужое — с правом управлять сообщениями).
    ///
    /// # Errors
    /// Нет права или сообщения нет.
    pub async fn delete(&self, message_id: &str) -> Result<(), BotError> {
        Ok(self.api.delete_channel_message(message_id).await?)
    }

    /// Поставить реакцию: эмодзи или `:имя:` своего эмодзи пространства.
    ///
    /// # Errors
    /// Соединение уже закрыто.
    pub async fn react(&self, message_id: &str, emoji: &str) -> Result<(), BotError> {
        self.emit(ClientEvent::ReactionAdd {
            message_id: message_id.to_owned(),
            emoji: emoji.to_owned(),
        })
        .await
    }

    /// Убрать свою реакцию.
    ///
    /// # Errors
    /// Соединение уже закрыто.
    pub async fn unreact(&self, message_id: &str, emoji: &str) -> Result<(), BotError> {
        self.emit(ClientEvent::ReactionRemove {
            message_id: message_id.to_owned(),
            emoji: emoji.to_owned(),
        })
        .await
    }

    /// Показать «печатает…» в канале.
    ///
    /// # Errors
    /// Соединение уже закрыто.
    pub async fn typing(&self, channel_id: &str) -> Result<(), BotError> {
        self.emit(ClientEvent::TypingStart {
            channel_id: channel_id.to_owned(),
        })
        .await
    }
}
