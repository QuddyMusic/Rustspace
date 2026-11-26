//! Личный мост в Telegram: вход в аккаунт пользователя по номеру и коду
//! через MTProto (крейт grammers, MIT/Apache-2.0, Lonami и участники).
//!
//! Нужны API ID и API hash приложения с my.telegram.org: их задаёт
//! администратор сервера (`TELEGRAM_API_ID`, `TELEGRAM_API_HASH`) или сам
//! пользователь при входе. Ключи сессии хранятся в базе зашифрованными.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use grammers_client::client::{LoginToken, PasswordToken};
use grammers_client::peer::Peer;
use grammers_client::sender::{
    ConnectionParams, SenderPool, SenderPoolFatHandle, UpdatesConfiguration,
};
use grammers_client::update::Update;
use grammers_client::{Client, SignInError};
use grammers_session::types::{
    ChannelState, DcOption, PeerId, PeerInfo, UpdateState, UpdatesState,
};
use grammers_session::{BoxFuture, Session, SessionData};
use serde_json::{Value, json};

use grammers_client::media::Media;
use grammers_client::message::{InputMessage, InputReactions, Message};
use grammers_client::tl;

use super::{
    Outgoing, Remote, RemoteChat, RemoteFile, RemoteProfile, deliver, save_secret, set_error,
};
use crate::bridges::backoff;
use crate::state::AppState;

/// Незаконченный вход живёт не дольше.
const LOGIN_TTL: Duration = Duration::from_mins(10);
/// Сколько ждать ответа Telegram на запрос кода: дольше — значит, сервер до
/// него не достаёт (блокировка у хостера), и клиент получит понятный ответ,
/// а не обрыв по сроку.
const LOGIN_REACH: Duration = Duration::from_secs(25);

/// Telegram не ответил серверу.
fn unreachable_message(state: &AppState) -> String {
    if state.config.bridges.telegram_proxy.is_some() {
        "Telegram не отвечает серверу и через TELEGRAM_PROXY: проверьте, что прокси работает \
         и сам выходит в Telegram"
            .to_owned()
    } else {
        "Telegram не отвечает серверу — похоже, он заблокирован у хостера. Администратор может \
         задать прокси в rustspace.env: TELEGRAM_PROXY=tg://proxy?… (MTProto, например \
         tg-ws-proxy на роутере) или socks5://хост:порт"
            .to_owned()
    }
}

/// Файлы больше не скачиваются из Telegram.
const MAX_FILE: usize = 50 * 1024 * 1024;

/// Сколько диалогов показывать при выборе.
const MAX_DIALOGS: usize = 1000;
/// Сколько своих отправленных сообщений помнить, чтобы не вернуть их эхом.
const SENT_MEMORY: usize = 200;

// ─── Сессия в базе ────────────────────────────────────────────────────────────

/// Состояние сессии, которое переживает перезапуск.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Snapshot {
    home_dc: i32,
    dc_options: Vec<DcOption>,
    #[serde(default)]
    peers: Vec<PeerInfo>,
    #[serde(default)]
    updates: UpdatesState,
}

struct Data {
    home_dc: i32,
    dc_options: HashMap<i32, DcOption>,
    peers: HashMap<PeerId, PeerInfo>,
    updates: UpdatesState,
}

/// Сессия grammers в памяти; изменения сохраняются в базу задачей моста.
pub struct DbSession {
    data: Mutex<Data>,
    dirty: AtomicBool,
}

impl std::fmt::Debug for DbSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbSession").finish_non_exhaustive()
    }
}

impl DbSession {
    fn new(snapshot: Option<Snapshot>) -> Self {
        let defaults = SessionData::default();
        let data = match snapshot {
            Some(snapshot) => Data {
                home_dc: snapshot.home_dc,
                dc_options: defaults
                    .dc_options
                    .into_iter()
                    .chain(snapshot.dc_options.into_iter().map(|dc| (dc.id, dc)))
                    .collect(),
                peers: snapshot
                    .peers
                    .into_iter()
                    .map(|peer| (peer.id(), peer))
                    .collect(),
                updates: snapshot.updates,
            },
            None => Data {
                home_dc: defaults.home_dc,
                dc_options: defaults.dc_options,
                peers: HashMap::new(),
                updates: UpdatesState::default(),
            },
        };
        super::mtproxy::remember_defaults(&data.dc_options.values().cloned().collect::<Vec<_>>());
        Self {
            data: Mutex::new(data),
            dirty: AtomicBool::new(false),
        }
    }

    fn data(&self) -> MutexGuard<'_, Data> {
        self.data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn touch(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Снимок для базы, если что-то менялось.
    fn take_snapshot(&self, force: bool) -> Option<Value> {
        if !self.dirty.swap(false, Ordering::Relaxed) && !force {
            return None;
        }
        let data = self.data();
        let snapshot = Snapshot {
            home_dc: data.home_dc,
            dc_options: data.dc_options.values().cloned().collect(),
            peers: data.peers.values().cloned().collect(),
            updates: data.updates.clone(),
        };
        serde_json::to_value(snapshot).ok()
    }
}

impl Session for DbSession {
    type Error = Infallible;

    fn home_dc_id(&self) -> Result<i32, Infallible> {
        Ok(self.data().home_dc)
    }

    fn set_home_dc_id(&self, dc_id: i32) -> BoxFuture<'_, Result<(), Infallible>> {
        Box::pin(async move {
            self.data().home_dc = dc_id;
            self.touch();
            Ok(())
        })
    }

    fn dc_option(&self, dc_id: i32) -> Result<Option<DcOption>, Infallible> {
        Ok(self.data().dc_options.get(&dc_id).cloned())
    }

    fn set_dc_option(&self, dc_option: &DcOption) -> BoxFuture<'_, Result<(), Infallible>> {
        let dc_option = dc_option.clone();
        Box::pin(async move {
            super::mtproxy::remember_defaults(std::slice::from_ref(&dc_option));
            self.data().dc_options.insert(dc_option.id, dc_option);
            self.touch();
            Ok(())
        })
    }

    fn peer(&self, peer: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>, Infallible>> {
        Box::pin(async move { Ok(self.data().peers.get(&peer).cloned()) })
    }

    fn cache_peer(&self, peer: PeerInfo) -> BoxFuture<'_, Result<(), Infallible>> {
        Box::pin(async move {
            let changed = {
                let mut data = self.data();
                match data.peers.get_mut(&peer.id()) {
                    Some(known) => known.extend_info(&peer),
                    None => {
                        data.peers.insert(peer.id(), peer);
                        true
                    }
                }
            };
            if changed {
                self.touch();
            }
            Ok(())
        })
    }

    fn updates_state(&self) -> BoxFuture<'_, Result<UpdatesState, Infallible>> {
        Box::pin(async move { Ok(self.data().updates.clone()) })
    }

    fn set_update_state(&self, update: UpdateState) -> BoxFuture<'_, Result<(), Infallible>> {
        Box::pin(async move {
            {
                let mut data = self.data();
                match update {
                    UpdateState::All(state) => data.updates = state,
                    UpdateState::Primary { pts, date, seq } => {
                        data.updates.pts = pts;
                        data.updates.date = date;
                        data.updates.seq = seq;
                    }
                    UpdateState::Secondary { qts } => data.updates.qts = qts,
                    UpdateState::Channel { id, pts } => {
                        data.updates.channels.retain(|c| c.id != id);
                        data.updates.channels.push(ChannelState { id, pts });
                    }
                }
            }
            self.touch();
            Ok(())
        })
    }
}

// ─── Подключение ──────────────────────────────────────────────────────────────

/// Клиент Telegram с его сессией.
struct Connection {
    client: Client,
    handle: SenderPoolFatHandle,
    session: Arc<DbSession>,
    updates: Option<tokio::sync::mpsc::Receiver<grammers_session::updates::UpdatesLike>>,
    runner: tokio::task::AbortHandle,
}

async fn connect(state: &AppState, api_id: i32, snapshot: Option<Snapshot>) -> Connection {
    let session = Arc::new(DbSession::new(snapshot));
    let raw = state.config.bridges.telegram_proxy.as_deref();
    let mut params = ConnectionParams::default();
    if let Some(proxy) = super::mtproxy::configured(raw) {
        // MTProto-прокси: grammers идёт через свой SOCKS5 на 127.0.0.1,
        // транспортом Intermediate, который прокси и ждёт.
        match super::mtproxy::local_socks(proxy).await {
            Ok(url) => {
                params.proxy_url = Some(url);
                params.transport =
                    Box::new(|| Box::new(grammers_mtproto::transport::Intermediate::new()));
            }
            Err(err) => tracing::warn!(%err, "SOCKS5 для MTProto-прокси не поднялся"),
        }
    } else if let Some(url) = raw {
        // SOCKS5: тоже Intermediate — SOCKS-вход tg-ws-proxy, например,
        // транспорт Full не принимает.
        params.proxy_url = Some(url.to_owned());
        params.transport = Box::new(|| Box::new(grammers_mtproto::transport::Intermediate::new()));
    }
    let SenderPool {
        runner,
        handle,
        updates,
    } = SenderPool::with_configuration(Arc::clone(&session), api_id, params);
    let client = Client::new(handle.clone());
    let runner = tokio::spawn(runner.run()).abort_handle();
    Connection {
        client,
        handle,
        session,
        updates: Some(updates),
        runner,
    }
}

impl Connection {
    fn close(&self) {
        self.handle.quit();
        self.runner.abort();
    }
}

/// Подключённый аккаунт Telegram.
pub struct Account {
    client: Client,
    handle: SenderPoolFatHandle,
    session: Arc<DbSession>,
    /// Недавно отправленное мостом: (чат, текст) — чтобы не вернуть эхом.
    sent: Arc<Mutex<VecDeque<(String, String)>>>,
    tasks: Vec<tokio::task::AbortHandle>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for PendingLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingLogin")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

impl Account {
    /// Чат по его id в формате Bot API.
    async fn peer(&self, chat: &str) -> Result<grammers_session::types::PeerRef, String> {
        let id = chat
            .parse::<i64>()
            .ok()
            .and_then(PeerId::from_bot_api_dialog_id)
            .ok_or_else(|| "неверный чат".to_owned())?;
        self.session
            .peer_ref(id)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| "чат неизвестен сессии: откройте список чатов ещё раз".to_owned())
    }

    /// Отправляет текст и файлы (первый — с подписью и ответом). Номер
    /// первого отправленного сообщения.
    pub(super) async fn send_full(
        &self,
        chat: &str,
        message: &Outgoing,
    ) -> Result<Option<String>, String> {
        let peer = self.peer(chat).await?;
        let reply = message
            .reply_to
            .as_deref()
            .and_then(|id| id.parse::<i32>().ok());
        if message.files.is_empty() {
            remember(&self.sent, chat, &message.text);
            let input = InputMessage::new()
                .text(message.text.as_str())
                .reply_to(reply);
            let sent = Box::pin(self.client.send_message(peer, input))
                .await
                .map_err(|err| err.to_string())?;
            return Ok(Some(sent.id().to_string()));
        }
        let mut first = None;
        for (index, (name, path, mime)) in message.files.iter().enumerate() {
            let caption = if index == 0 {
                message.text.as_str()
            } else {
                ""
            };
            let mut file = tokio::fs::File::open(path)
                .await
                .map_err(|err| err.to_string())?;
            let size = file.metadata().await.map_err(|err| err.to_string())?.len();
            let uploaded = self
                .client
                .upload_stream(
                    &mut file,
                    usize::try_from(size).unwrap_or(usize::MAX),
                    name.clone(),
                )
                .await
                .map_err(|err| err.to_string())?;
            let mut input = InputMessage::new().text(caption);
            if index == 0 {
                input = input.reply_to(reply);
            }
            input = if matches!(mime.as_str(), "image/jpeg" | "image/png") {
                input.photo(uploaded)
            } else {
                input.document(uploaded).mime_type(mime)
            };
            remember(&self.sent, chat, caption);
            let sent = Box::pin(self.client.send_message(peer, input))
                .await
                .map_err(|err| err.to_string())?;
            first.get_or_insert_with(|| sent.id().to_string());
        }
        Ok(first)
    }

    /// Своя реакция (`None` — снять).
    pub(super) async fn react(
        &self,
        chat: &str,
        remote_id: &str,
        emoji: Option<&str>,
    ) -> Result<(), String> {
        let peer = self.peer(chat).await?;
        let id = remote_id.parse::<i32>().map_err(|err| err.to_string())?;
        let reactions = emoji.map_or_else(InputReactions::remove, InputReactions::emoticon);
        self.client
            .send_reactions(peer, id, reactions)
            .await
            .map_err(|err| err.to_string())
    }

    pub(super) async fn edit(&self, chat: &str, remote_id: &str, text: &str) -> Result<(), String> {
        let peer = self.peer(chat).await?;
        let id = remote_id.parse::<i32>().map_err(|err| err.to_string())?;
        Box::pin(
            self.client
                .edit_message(peer, id, InputMessage::new().text(text)),
        )
        .await
        .map_err(|err| err.to_string())
    }

    pub(super) async fn delete(&self, chat: &str, remote_id: &str) -> Result<(), String> {
        let peer = self.peer(chat).await?;
        let id = remote_id.parse::<i32>().map_err(|err| err.to_string())?;
        self.client
            .delete_messages(peer, &[id])
            .await
            .map(drop)
            .map_err(|err| err.to_string())
    }

    /// Последние сообщения чата, от старых к новым.
    pub(super) async fn history(&self, account: &str, chat: &str, limit: usize) -> Vec<Remote> {
        let Ok(peer) = self.peer(chat).await else {
            return Vec::new();
        };
        let mut messages = self.client.iter_messages(peer).limit(limit);
        let mut out = Vec::new();
        loop {
            match messages.next().await {
                Ok(Some(message)) => {
                    if let Some(remote) = to_remote(&self.client, account, chat, &message).await {
                        out.push(remote);
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    tracing::warn!(account, %err, "личный мост Telegram: история не загрузилась");
                    break;
                }
            }
        }
        out.reverse();
        out
    }

    /// Фото, «о себе» и «в сети» собеседника; фото с id `photo_known` не
    /// скачивается заново.
    pub(super) async fn profile(
        &self,
        chat: &str,
        photo_known: Option<&str>,
    ) -> Option<RemoteProfile> {
        let peer_ref = self.peer(chat).await.ok()?;
        let peer = self.client.resolve_peer(peer_ref).await.ok()?;
        let mut profile = RemoteProfile::default();
        if let Peer::User(user) = &peer {
            profile.online = Some(matches!(user.status(), tl::enums::UserStatus::Online(_)));
            let full = self
                .client
                .invoke(&tl::functions::users::GetFullUser {
                    id: (&peer_ref).into(),
                })
                .await;
            if let Ok(tl::enums::users::UserFull::Full(full)) = full {
                let tl::enums::UserFull::Full(about) = full.full_user;
                profile.bio = about.about.filter(|text| !text.trim().is_empty());
            }
        }
        let photo_id = match &peer {
            Peer::User(user) => user.photo().map(|photo| photo.photo_id),
            Peer::Group(group) => group.photo().map(|photo| photo.photo_id),
            Peer::Channel(channel) => channel.photo().map(|photo| photo.photo_id),
            Peer::Community(community) => community.photo().map(|photo| photo.photo_id),
        };
        if let Some(photo_id) = photo_id.map(|id| id.to_string())
            && photo_known != Some(photo_id.as_str())
            && let Ok(Some(photo)) = peer.photo(false).await
        {
            let bytes = download(&self.client, &photo, MAX_FILE).await;
            if !bytes.is_empty() {
                profile.photo = Some((photo_id, bytes));
            }
        }
        Some(profile)
    }

    pub(super) async fn chats(&self) -> Result<Vec<RemoteChat>, String> {
        let mut dialogs = self.client.iter_dialogs();
        let mut out = Vec::new();
        while out.len() < MAX_DIALOGS {
            let Some(dialog) = dialogs.next().await.map_err(|err| err.to_string())? else {
                break;
            };
            let peer = dialog.peer();
            let Some(id) = peer.id().bot_api_dialog_id() else {
                continue;
            };
            let kind = match peer {
                Peer::User(user) if user.is_bot() => "bot",
                Peer::User(_) => "person",
                Peer::Group(_) | Peer::Community(_) => "group",
                Peer::Channel(_) => "channel",
            };
            out.push(RemoteChat {
                id: id.to_string(),
                title: peer_name(peer),
                group: !matches!(peer, Peer::User(_)),
                kind,
                username: peer.username().map(str::to_owned),
            });
        }
        Ok(out)
    }

    pub(super) async fn sign_out(&self) {
        let _ = self.client.sign_out().await;
    }

    pub(super) fn stop(&self) {
        self.handle.quit();
        for task in &self.tasks {
            task.abort();
        }
    }
}

fn peer_name(peer: &Peer) -> String {
    match peer {
        Peer::User(user) => user.full_name(),
        other => other.name().unwrap_or_default().to_owned(),
    }
}

fn remember(sent: &Mutex<VecDeque<(String, String)>>, chat: &str, text: &str) {
    let mut sent = sent
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    sent.push_back((chat.to_owned(), text.trim().to_owned()));
    while sent.len() > SENT_MEMORY {
        sent.pop_front();
    }
}

/// Было ли это отправлено мостом (и забыть об этом).
fn was_sent(sent: &Mutex<VecDeque<(String, String)>>, chat: &str, text: &str) -> bool {
    let mut sent = sent
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let found = sent.iter().position(|(c, t)| c == chat && t == text.trim());
    found.is_some_and(|index| sent.remove(index).is_some())
}

/// Ключи приложения: от администратора или от пользователя.
fn app_keys(state: &AppState, secret: &Value) -> Result<(i32, String), String> {
    let id = secret["apiId"]
        .as_i64()
        .and_then(|id| i32::try_from(id).ok())
        .or(state.config.bridges.telegram_api_id);
    let hash = secret["apiHash"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| state.config.bridges.telegram_api_hash.clone());
    match (id, hash) {
        (Some(id), Some(hash)) => Ok((id, hash)),
        _ => Err("нет API ID и API hash Telegram (my.telegram.org)".to_owned()),
    }
}

/// Запускает сохранённый аккаунт.
pub async fn resume(state: &AppState, account: &str, secret: &Value) -> Result<Account, String> {
    let (api_id, _) = app_keys(state, secret)?;
    let snapshot: Option<Snapshot> = serde_json::from_value(secret["session"].clone()).ok();
    let mut connection = connect(state, api_id, snapshot).await;
    if !connection
        .client
        .is_authorized()
        .await
        .map_err(|err| err.to_string())?
    {
        connection.close();
        return Err("Telegram завершил сеанс: войдите заново".to_owned());
    }
    let sent: Arc<Mutex<VecDeque<(String, String)>>> = Arc::default();
    let updates = connection
        .updates
        .take()
        .ok_or_else(|| "нет потока обновлений".to_owned())?;
    let listen = tokio::spawn(listen(
        state.clone(),
        account.to_owned(),
        connection.client.clone(),
        updates,
        Arc::clone(&sent),
    ))
    .abort_handle();
    let saver = tokio::spawn(keep_saved(
        state.clone(),
        account.to_owned(),
        secret.clone(),
        Arc::clone(&connection.session),
    ))
    .abort_handle();
    Ok(Account {
        client: connection.client,
        handle: connection.handle,
        session: connection.session,
        sent,
        tasks: vec![listen, saver, connection.runner],
    })
}

/// Раз в минуту сохраняет изменившуюся сессию.
async fn keep_saved(state: AppState, account: String, mut secret: Value, session: Arc<DbSession>) {
    let mut timer = tokio::time::interval(Duration::from_mins(1));
    loop {
        timer.tick().await;
        if let Some(snapshot) = session.take_snapshot(false) {
            secret["session"] = snapshot;
            save_secret(&state, &account, &secret).await;
        }
    }
}

async fn listen(
    state: AppState,
    account: String,
    client: Client,
    updates: tokio::sync::mpsc::Receiver<grammers_session::updates::UpdatesLike>,
    sent: Arc<Mutex<VecDeque<(String, String)>>>,
) {
    // Диалоги — чтобы сессия знала собеседников: без этого обновления с
    // пропусками не восстановить и ответить некому.
    let mut dialogs = client.iter_dialogs();
    let mut seen = 0;
    while seen < 500 {
        match dialogs.next().await {
            Ok(Some(_)) => seen += 1,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(account, %err, "личный мост Telegram: диалоги не загрузились");
                break;
            }
        }
    }
    let mut stream = match client
        .stream_updates(updates, UpdatesConfiguration { catch_up: true })
        .await
    {
        Ok(stream) => stream,
        Err(err) => {
            set_error(&state, &account, &format!("Telegram: {err}")).await;
            return;
        }
    };
    let mut attempt = 0;
    loop {
        match stream.next().await {
            Ok(Update::NewMessage(message)) => {
                attempt = 0;
                let Some(chat) = message
                    .peer_id()
                    .bot_api_dialog_id()
                    .map(|id| id.to_string())
                else {
                    continue;
                };
                if message.outgoing() && was_sent(&sent, &chat, message.text().trim()) {
                    continue;
                }
                if let Some(remote) = to_remote(&client, &account, &chat, &message).await {
                    deliver(&state, remote).await;
                }
            }
            Ok(Update::MessageEdited(message)) => {
                if let Some(chat) = message.peer_id().bot_api_dialog_id() {
                    super::sync::remote_edit(
                        &state,
                        &account,
                        &chat.to_string(),
                        &message.id().to_string(),
                        message.text(),
                    )
                    .await;
                }
            }
            Ok(Update::MessageDeleted(deletion)) => {
                let chat = deletion
                    .channel_id()
                    .and_then(PeerId::channel)
                    .and_then(PeerId::bot_api_dialog_id)
                    .map(|id| id.to_string());
                let ids: Vec<String> = deletion
                    .messages()
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                super::sync::remote_delete(&state, &account, chat.as_deref(), &ids).await;
            }
            Ok(Update::Raw(raw)) => raw_update(&state, &account, raw.raw).await,
            Ok(_) => {}
            Err(err) => {
                attempt += 1;
                tracing::warn!(account, %err, "личный мост Telegram: обновления прервались");
                if attempt > 8 {
                    set_error(&state, &account, &format!("Telegram: {err}")).await;
                    return;
                }
                tokio::time::sleep(backoff(attempt)).await;
            }
        }
    }
}

/// Сообщение Telegram → сообщение моста (с файлами).
async fn to_remote(
    client: &Client,
    account: &str,
    chat: &str,
    message: &Message,
) -> Option<Remote> {
    let text = message.text().trim().to_owned();
    let files = message_files(client, message).await;
    if text.is_empty() && files.is_empty() {
        return None;
    }
    let sender = message
        .sender()
        .map(peer_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Telegram".to_owned());
    let date = match &message.raw {
        tl::enums::Message::Message(raw) => Some(i64::from(raw.date) * 1000),
        _ => None,
    };
    Some(Remote {
        account: account.to_owned(),
        chat: chat.to_owned(),
        sender,
        text,
        mine: message.outgoing(),
        id: Some(message.id().to_string()),
        date,
        files,
        reply_to: message.reply_to_message_id().map(|id| id.to_string()),
    })
}

/// Фото, файл, видео, голосовое или стикер сообщения.
async fn message_files(client: &Client, message: &Message) -> Vec<RemoteFile> {
    let Some(media) = message.media() else {
        return Vec::new();
    };
    let id = message.id();
    let name = match &media {
        Media::Photo(_) => format!("photo_{id}.jpg"),
        Media::Document(document) => match document.name().filter(|name| !name.trim().is_empty()) {
            Some(name) => name.to_owned(),
            None => format!(
                "file_{id}{}",
                extension_for(document.mime_type().unwrap_or_default())
            ),
        },
        Media::Sticker(sticker) => {
            let mime = sticker.document.mime_type().unwrap_or_default();
            if mime != "image/webp" {
                // Анимированные (tgs) и видео-стикеры показать нечем.
                return Vec::new();
            }
            format!("sticker_{id}.webp")
        }
        _ => return Vec::new(),
    };
    let bytes = download(client, &media, MAX_FILE).await;
    if bytes.is_empty() {
        return Vec::new();
    }
    vec![RemoteFile { name, bytes }]
}

fn extension_for(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => ".jpg",
        "image/png" => ".png",
        "image/webp" => ".webp",
        "image/gif" => ".gif",
        "video/mp4" => ".mp4",
        "video/webm" => ".webm",
        "audio/ogg" => ".ogg",
        "audio/mpeg" => ".mp3",
        "audio/mp4" => ".m4a",
        "application/pdf" => ".pdf",
        _ => "",
    }
}

/// Скачивает целиком (пусто — не вышло или больше `limit`).
async fn download<D: grammers_client::media::Downloadable + Sync>(
    client: &Client,
    item: &D,
    limit: usize,
) -> Vec<u8> {
    let mut parts = client.iter_download(item);
    let mut bytes = Vec::new();
    loop {
        match parts.next().await {
            Ok(Some(chunk)) => {
                bytes.extend_from_slice(&chunk);
                if bytes.len() > limit {
                    return Vec::new();
                }
            }
            Ok(None) => return bytes,
            Err(err) => {
                tracing::debug!(%err, "личный мост Telegram: файл не скачался");
                return Vec::new();
            }
        }
    }
}

/// Обновления без своей обёртки в grammers: реакции и «в сети».
async fn raw_update(state: &AppState, account: &str, update: tl::enums::Update) {
    match update {
        tl::enums::Update::MessageReactions(update) => {
            let chat = match update.peer {
                tl::enums::Peer::User(user) => PeerId::user(user.user_id),
                tl::enums::Peer::Chat(chat) => PeerId::chat(chat.chat_id),
                tl::enums::Peer::Channel(channel) => PeerId::channel(channel.channel_id),
            }
            .and_then(PeerId::bot_api_dialog_id);
            let Some(chat) = chat.map(|id| id.to_string()) else {
                return;
            };
            let (theirs, mine) = split_reactions(&update.reactions);
            super::sync::remote_reactions(
                state,
                account,
                &chat,
                &update.msg_id.to_string(),
                theirs,
                mine,
            )
            .await;
        }
        tl::enums::Update::UserStatus(update) => {
            if let Some(chat) = PeerId::user(update.user_id).and_then(PeerId::bot_api_dialog_id) {
                let online = matches!(update.status, tl::enums::UserStatus::Online(_));
                super::sync::remote_presence(state, account, &chat.to_string(), online).await;
            }
        }
        _ => {}
    }
}

/// Реакции сообщения: (чужие, свои). Свои — помеченные `chosen_order`;
/// чужая есть, если счётчик больше своей.
fn split_reactions(reactions: &tl::enums::MessageReactions) -> (Vec<String>, Vec<String>) {
    let tl::enums::MessageReactions::Reactions(reactions) = reactions;
    let mut theirs = Vec::new();
    let mut mine = Vec::new();
    for result in &reactions.results {
        let tl::enums::ReactionCount::Count(count) = result;
        let tl::enums::Reaction::Emoji(emoji) = &count.reaction else {
            continue;
        };
        let chosen = count.chosen_order.is_some();
        if chosen {
            mine.push(emoji.emoticon.clone());
        }
        if count.count > i32::from(chosen) {
            theirs.push(emoji.emoticon.clone());
        }
    }
    (theirs, mine)
}

// ─── Вход ─────────────────────────────────────────────────────────────────────

/// Незаконченный вход: ждёт код или пароль.
pub struct PendingLogin {
    user: String,
    phone: String,
    api_id: i32,
    api_hash: Option<String>,
    connection: Connection,
    token: Option<LoginToken>,
    password: Option<PasswordToken>,
    started: Instant,
}

/// Чем закончился шаг входа.
#[derive(Debug)]
pub enum Step {
    /// Нужен облачный пароль (двухэтапная проверка); подсказка.
    Password(Option<String>),
    /// Вошли: аккаунт заведён.
    Done(Value),
}

fn drop_stale(state: &AppState) {
    state.personal.logins().retain(|_, login| {
        let fresh = login.started.elapsed() < LOGIN_TTL;
        if !fresh {
            login.connection.close();
        }
        fresh
    });
}

/// Шаг 1: отправить код на номер. Возвращает идентификатор входа.
pub async fn start_login(
    state: &AppState,
    user: &str,
    phone: &str,
    api_id: Option<i32>,
    api_hash: Option<String>,
) -> Result<String, String> {
    drop_stale(state);
    let mut keys = json!({});
    if let (Some(id), Some(hash)) = (api_id, &api_hash) {
        keys = json!({"apiId": id, "apiHash": hash});
    }
    let (api_id, hash) = app_keys(state, &keys)?;
    let phone: String = phone
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '+')
        .collect();
    if phone.len() < 6 {
        return Err("неверный номер телефона".to_owned());
    }
    let connection = connect(state, api_id, None).await;
    let asked = tokio::time::timeout(
        LOGIN_REACH,
        connection.client.request_login_code(&phone, &hash),
    )
    .await;
    let token = match asked {
        Ok(Ok(token)) => token,
        Ok(Err(err)) => {
            connection.close();
            return Err(explain(&err.to_string()));
        }
        Err(_) => {
            connection.close();
            return Err(unreachable_message(state));
        }
    };
    let id = uuid::Uuid::new_v4().to_string();
    state.personal.logins().insert(
        id.clone(),
        PendingLogin {
            user: user.to_owned(),
            phone,
            api_id,
            api_hash: keys["apiHash"].as_str().map(str::to_owned),
            connection,
            token: Some(token),
            password: None,
            started: Instant::now(),
        },
    );
    Ok(id)
}

fn take_login(state: &AppState, user: &str, id: &str) -> Result<PendingLogin, String> {
    drop_stale(state);
    let mut logins = state.personal.logins();
    match logins.get(id) {
        Some(login) if login.user == user => {}
        _ => return Err("вход устарел — начните заново".to_owned()),
    }
    logins
        .remove(id)
        .ok_or_else(|| "вход устарел — начните заново".to_owned())
}

/// Шаг 2: код из Telegram.
pub async fn submit_code(
    state: &AppState,
    user: &str,
    id: &str,
    code: &str,
) -> Result<Step, String> {
    let mut login = take_login(state, user, id)?;
    let token = login
        .token
        .take()
        .ok_or_else(|| "код уже введён".to_owned())?;
    let result = login.connection.client.sign_in(&token, code.trim()).await;
    finish(state, id, login, result, Some(token)).await
}

/// Шаг 3: облачный пароль.
pub async fn submit_password(
    state: &AppState,
    user: &str,
    id: &str,
    password: &str,
) -> Result<Step, String> {
    let mut login = take_login(state, user, id)?;
    let token = login
        .password
        .take()
        .ok_or_else(|| "пароль не запрашивался".to_owned())?;
    let result = login
        .connection
        .client
        .check_password(token, password)
        .await;
    finish(state, id, login, result, None).await
}

/// Понятный текст для ошибок входа Telegram (`rpc error 420: FLOOD_WAIT … (value: 3600)`).
fn explain(error: &str) -> String {
    let wait = error
        .rsplit_once("(value: ")
        .and_then(|(_, rest)| rest.trim_end_matches(')').parse::<u64>().ok());
    let has = |name: &str| error.contains(name);
    if has("PHONE_NUMBER_FLOOD") {
        "Telegram временно не присылает коды на этот номер: их запрашивали слишком часто. \
         Подождите сутки и не запрашивайте код в это время — каждая попытка продлевает запрет."
            .to_owned()
    } else if has("FLOOD_WAIT") || has("FLOOD_PREMIUM_WAIT") {
        match wait {
            Some(seconds) if seconds >= 3600 => format!(
                "Telegram просит подождать {} ч перед следующей попыткой",
                seconds.div_ceil(3600)
            ),
            Some(seconds) => format!(
                "Telegram просит подождать {} мин перед следующей попыткой",
                seconds.div_ceil(60).max(1)
            ),
            None => "Telegram просит подождать перед следующей попыткой".to_owned(),
        }
    } else if has("PHONE_NUMBER_INVALID") {
        "неверный номер телефона".to_owned()
    } else if has("PHONE_NUMBER_BANNED") {
        "этот номер заблокирован в Telegram".to_owned()
    } else if has("PHONE_CODE_EXPIRED") {
        "код устарел — начните вход заново".to_owned()
    } else if has("API_ID_INVALID") || has("API_ID_PUBLISHED_FLOOD") {
        "Telegram не принял ключи приложения (API ID и hash) — проверьте их на my.telegram.org"
            .to_owned()
    } else {
        error.to_owned()
    }
}

async fn finish(
    state: &AppState,
    id: &str,
    mut login: PendingLogin,
    result: Result<grammers_client::peer::User, SignInError>,
    token: Option<LoginToken>,
) -> Result<Step, String> {
    let me = match result {
        Ok(me) => me,
        Err(SignInError::PasswordRequired(password) | SignInError::InvalidPassword(password)) => {
            let hint = password.hint().map(str::to_owned);
            login.password = Some(password);
            state.personal.logins().insert(id.to_owned(), login);
            return Ok(Step::Password(hint));
        }
        Err(SignInError::InvalidCode) => {
            // Код можно ввести ещё раз.
            login.token = token;
            state.personal.logins().insert(id.to_owned(), login);
            return Err("неверный код".to_owned());
        }
        Err(SignInError::SignUpRequired) => {
            login.connection.close();
            return Err("на этот номер нет аккаунта Telegram".to_owned());
        }
        Err(err) => {
            login.connection.close();
            return Err(explain(&err.to_string()));
        }
    };
    let snapshot = login
        .connection
        .session
        .take_snapshot(true)
        .unwrap_or(Value::Null);
    login.connection.close();
    let mut secret = json!({"session": snapshot});
    if let Some(hash) = &login.api_hash {
        secret["apiId"] = json!(login.api_id);
        secret["apiHash"] = json!(hash);
    }
    let display = {
        let name = me.full_name();
        if name.trim().is_empty() {
            login.phone.clone()
        } else {
            name
        }
    };
    let remote_id = me
        .id()
        .bot_api_dialog_id()
        .map_or_else(|| login.phone.clone(), |id| id.to_string());
    let account = super::add_account(
        state,
        &login.user,
        super::Kind::Telegram,
        &remote_id,
        &display,
        &secret,
    )
    .await
    .map_err(|err| format!("{err:?}"))?;
    match resume(state, &account, &secret).await {
        Ok(link) => state.personal.insert(&account, super::Link::Telegram(link)),
        Err(err) => set_error(state, &account, &err).await,
    }
    let view = super::account_view(state, &login.user, &account)
        .await
        .map_err(|err| format!("{err:?}"))?;
    Ok(Step::Done(view))
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn session_snapshot_survives_a_round_trip() {
        let session = DbSession::new(None);
        let home = session.home_dc_id().expect("dc");
        let snapshot = session.take_snapshot(true).expect("снимок");
        let restored: Snapshot = serde_json::from_value(snapshot).expect("разбор");
        assert_eq!(restored.home_dc, home);
        assert_eq!(restored.dc_options.len(), 5);
        assert!(session.take_snapshot(false).is_none());
    }

    #[test]
    fn login_errors_are_explained() {
        assert!(
            super::explain("rpc error 420: FLOOD_WAIT caused by auth.sendCode (value: 7200)")
                .contains("2 ч")
        );
        assert!(super::explain("rpc error 400: PHONE_NUMBER_FLOOD").contains("сутки"));
        assert_eq!(super::explain("что-то ещё"), "что-то ещё");
    }

    #[test]
    fn bridge_echo_is_recognised_once() {
        let sent = Mutex::new(VecDeque::new());
        remember(&sent, "42", "привет ");
        assert!(was_sent(&sent, "42", "привет"));
        assert!(!was_sent(&sent, "42", "привет"));
        assert!(!was_sent(&sent, "7", "привет"));
    }

    #[test]
    fn reactions_split_into_theirs_and_mine() {
        use super::tl;
        let count = |emoji: &str, count: i32, chosen: bool| {
            tl::enums::ReactionCount::Count(tl::types::ReactionCount {
                chosen_order: chosen.then_some(0),
                reaction: tl::enums::Reaction::Emoji(tl::types::ReactionEmoji {
                    emoticon: emoji.to_owned(),
                }),
                count,
            })
        };
        let reactions = tl::enums::MessageReactions::Reactions(tl::types::MessageReactions {
            min: false,
            can_see_list: false,
            reactions_as_tags: false,
            results: vec![
                count("👍", 2, true),
                count("❤", 1, true),
                count("🔥", 1, false),
            ],
            recent_reactions: None,
            top_reactors: None,
        });
        let (theirs, mine) = super::split_reactions(&reactions);
        assert_eq!(theirs, vec!["👍".to_owned(), "🔥".to_owned()]);
        assert_eq!(mine, vec!["👍".to_owned(), "❤".to_owned()]);
    }
}
