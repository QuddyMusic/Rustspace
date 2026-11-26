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

use super::{Remote, RemoteChat, deliver, save_secret, set_error};
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

/// Сколько диалогов показывать при выборе.
const MAX_DIALOGS: usize = 100;
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
    pub(super) async fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        let id = chat
            .parse::<i64>()
            .ok()
            .and_then(PeerId::from_bot_api_dialog_id)
            .ok_or_else(|| "неверный чат".to_owned())?;
        let peer = self
            .session
            .peer_ref(id)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| "чат неизвестен сессии: откройте список чатов ещё раз".to_owned())?;
        remember(&self.sent, chat, text);
        Box::pin(self.client.send_message(peer, text))
            .await
            .map(drop)
            .map_err(|err| err.to_string())
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
            out.push(RemoteChat {
                id: id.to_string(),
                title: peer_name(peer),
                group: !matches!(peer, Peer::User(_)),
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
        .stream_updates(updates, UpdatesConfiguration { catch_up: false })
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
                let Some(chat) = message.peer_id().bot_api_dialog_id() else {
                    continue;
                };
                let chat = chat.to_string();
                let text = message.text().trim().to_owned();
                if text.is_empty() {
                    continue;
                }
                let mine = message.outgoing();
                if mine && was_sent(&sent, &chat, &text) {
                    continue;
                }
                let sender = message
                    .sender()
                    .map(peer_name)
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "Telegram".to_owned());
                deliver(
                    &state,
                    Remote {
                        account: account.clone(),
                        chat,
                        sender,
                        text,
                        mine,
                    },
                )
                .await;
            }
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
}
