//! Мост GUI ↔ tokio.
//!
//! Разделение обязанностей:
//! - фоновая tokio-задача держит [`InstanceHandle`] и читает события;
//! - GUI-поток владеет [`Cache`] и применяет к нему [`apply`] прямо в кадре.
//!
//! Так GUI никогда не блокируется на сети, а `Cache` (не `Clone`) не
//! приходится копировать между потоками.
//!
//! # Жизненный цикл одной сессии
//! Вход в инстанс — это переход `Idle → Connecting → Active`. Новый вход
//! возможен только из `Idle`, а выход (`Active → Closing`) прерывает
//! фоновую задачу и ждёт её фактического завершения (событие `Ended`), и
//! лишь потом сессия возвращается в `Idle`: повторное нажатие «Войти» или
//! вход в другой инстанс не создаёт второго соединения в той же сессии.
//!
//! [`Session`] — это один инстанс. Несколько инстансов одновременно держит
//! [`crate::fleet::Bridge`]: у каждого своя `Session`, но активна (то есть
//! показана в интерфейсе) только одна.

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;

use bs_api::{
    ApiClient, ChangePasswordRequest, FederationCredentialRequest, IdentityDeleteMode,
    IdentityDeleteResult, RegisterRequest, RegistryEntry, RegistryResponse, ReplicatedEntry,
};
use bs_core::{Cache, apply};
use bs_identity::CredentialStore;
use bs_instances::{ConnectionState, InstanceEvent, InstanceHandle, ReconnectPolicy};
use bs_types::{
    ClientEvent, ClientKind, DmMessageWithUser, MessageWithUser, Origin, ReadyPayload, ServerEvent,
    TransportPolicy,
};
use tokio::runtime::Handle;
use tokio::sync::{broadcast, mpsc as tokio_mpsc};
use zeroize::Zeroizing;

use crate::logic::describe_server_error;

/// Сколько сообщений просим у сервера за один запрос истории.
pub(crate) const HISTORY_PAGE: u16 = 50;

/// Вид канала: у каналов пространств и личных каналов разные события,
/// разные REST-маршруты и разные хранилища в кэше.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ChannelKind {
    /// Текстовый канал пространства.
    Space,
    /// Личный канал (1-на-1 или группа).
    Dm,
}

/// Состояние загрузки истории одного канала.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HistoryState {
    /// Страница запрошена, ответа ещё нет.
    pub(crate) loading: bool,
    /// Сервер вернул неполную страницу: старее ничего нет.
    pub(crate) exhausted: bool,
    /// Ошибка последнего запроса.
    pub(crate) error: Option<String>,
}

impl HistoryState {
    /// Можно ли запрашивать следующую страницу автоматически (при прокрутке).
    ///
    /// После ошибки автоповтора нет: иначе при сбое (например, `429`) клиент
    /// каждый кадр шлёт новый запрос и сам себя блокирует. Повтор — только по
    /// кнопке «Повторить».
    pub(crate) const fn can_auto_load(&self) -> bool {
        !self.loading && !self.exhausted && self.error.is_none()
    }
}

/// Страница истории из REST-ответа.
#[derive(Debug)]
pub(crate) enum HistoryItems {
    /// Сообщения канала пространства.
    Space(Vec<MessageWithUser>),
    /// Сообщения личного канала.
    Dm(Vec<DmMessageWithUser>),
}

impl HistoryItems {
    fn len(&self) -> usize {
        match self {
            Self::Space(items) => items.len(),
            Self::Dm(items) => items.len(),
        }
    }
}

/// Секрет, который нельзя случайно напечатать: `Debug` скрывает значение,
/// память затирается при уничтожении.
#[derive(Clone)]
pub(crate) struct Secret(Zeroizing<String>);

impl Secret {
    /// Оборачивает строку.
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(Zeroizing::new(value.into()))
    }

    /// Значение для передачи в запрос.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// То, что домашний инстанс выдал для входа на другой инстанс.
#[derive(Debug, Clone)]
pub(crate) struct IssuedCredential {
    /// Имя вида `alice@home.example`.
    pub(crate) username: String,
    /// Секрет для входа и регистрации.
    pub(crate) secret: Secret,
    /// Хост домашнего инстанса.
    pub(crate) home_host: String,
    /// Идентификатор пользователя на домашнем инстансе.
    pub(crate) home_user_id: String,
    /// Отображаемое имя для нового аккаунта.
    pub(crate) display_name: Option<String>,
    /// Секрет уже записан в удалённый аккаунт.
    pub(crate) provisioned: bool,
}

/// REST-запрос, который GUI отправляет фоновой сессии.
#[derive(Debug, Clone)]
pub(crate) enum BridgeRequest {
    /// Список друзей.
    Friends,
    /// Список заявок в друзья.
    FriendRequests,
    /// Поиск пользователей по подстроке.
    SearchUsers(String),
    /// Список личных каналов.
    DmChannels,
    /// Открыть (или создать) личный канал с пользователем.
    OpenDm(String),
    /// Отправить заявку в друзья по имени пользователя.
    SendFriendRequest(String),
    /// Принять или отклонить входящую заявку.
    RespondFriendRequest(String, bool),
    /// Отозвать свою заявку.
    CancelFriendRequest(String),
    /// Удалить друга.
    RemoveFriend(String),
    /// Создать пространство.
    CreateSpace(bs_api::CreateSpaceRequest),
    /// Каталог пространств по подстроке.
    Explore(String),
    /// Вступить в пространство: сразу (публичное) или заявкой.
    JoinSpace {
        /// Идентификатор пространства.
        id: String,
        /// `true` — подать заявку, `false` — вступить сразу.
        by_request: bool,
    },
    /// Изменить свой профиль.
    UpdateProfile(bs_api::UpdateProfileRequest),
    /// Скачать и декодировать картинку инстанса.
    Image(bs_api::AssetPath),
    /// Загрузить файл на инстанс (tus); идёт параллельно с остальным.
    Upload {
        /// Локальный номер загрузки (задаёт композер).
        id: u64,
        /// Имя файла.
        name: String,
        /// Тип содержимого.
        mime: String,
        /// Содержимое.
        bytes: std::sync::Arc<Vec<u8>>,
    },
    /// Задать или снять аватар/баннер: файл грузится tus, затем профиль
    /// получает его имя. Пустые `bytes` снимают картинку.
    SetImage {
        /// Что менять.
        kind: ImageKind,
        /// Имя файла.
        name: String,
        /// Содержимое; пусто — снять.
        bytes: std::sync::Arc<Vec<u8>>,
    },
    /// Действие администратора или управление аккаунтом.
    Admin(AdminRequest),
    /// Включён ли поиск GIF.
    GifEnabled,
    /// Популярные GIF или поиск.
    Gifs {
        /// Запрос; пустой — популярное.
        query: String,
        /// Курсор страницы; пустой — первая.
        pos: String,
    },
    /// Личные стикеры.
    Stickers,
    /// Профиль другого пользователя и общие связи.
    Profile(String),
    /// Скачать файл как есть (копирование картинки, сохранение стикера).
    Raw {
        /// Номер запроса.
        tag: u64,
        /// Что скачать.
        path: bs_api::AssetPath,
    },
    /// Загрузить личный стикер.
    UploadSticker {
        /// Имя.
        name: String,
        /// Файл.
        bytes: std::sync::Arc<Vec<u8>>,
    },
    /// Удалить личный стикер.
    DeleteSticker(String),
    /// Отправить сообщение в канал пространства с вложениями.
    SendRest {
        /// Канал.
        channel_id: String,
        /// Тело.
        body: bs_api::SendMessageBody,
    },
    /// Только для домашней сессии: проверить пароль (если он есть) и
    /// получить секрет для входа на `origin`.
    IssueCredential {
        /// Куда собираемся входить.
        origin: String,
        /// Пароль от домашнего аккаунта; `None` при автоподключении.
        password: Option<Secret>,
    },
    /// Только для домашней сессии: попросить домашний инстанс о пиринге.
    EnsurePeered(String),
    /// Только для домашней сессии: отметить, что секрет принят на `origin`.
    MarkProvisioned(String),
    /// Сообщить инстансу список связанных инстансов.
    SyncInstances(Vec<ReplicatedEntry>),
    /// Только для домашней сессии: прочитать серверный реестр подключений.
    LoadRegistry,
    /// Только для домашней сессии: записать реестр (последний пишет).
    PutRegistry {
        /// Записи.
        entries: Vec<RegistryEntry>,
        /// Метка времени записи, мс.
        updated_at: i64,
    },
    /// Только для домашней сессии: удалить учётку на удалённых инстансах.
    DeleteIdentity {
        /// Инстансы.
        origins: Vec<String>,
        /// Что делать с данными.
        mode: IdentityDeleteMode,
    },
    /// Только для сессии удалённого инстанса: сменить пароль аккаунта на
    /// секрет, выданный домом (так аккаунт переводят на выданные данные).
    AdoptSecret(Secret),
    /// Страница истории: самая новая или старее `before`.
    History {
        /// Канал или личный канал.
        channel_id: String,
        /// Какой это вид канала.
        kind: ChannelKind,
        /// Курсор «старее этого id»; `None` — самые новые.
        before: Option<String>,
    },
}

/// Какую картинку профиля меняем.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageKind {
    /// Аватар.
    Avatar,
    /// Баннер.
    Banner,
}

/// Запросы настроек: администрирование и аккаунт.
#[derive(Debug, Clone)]
pub(crate) enum AdminRequest {
    /// Прочитать настройки инстанса.
    LoadSettings,
    /// Сохранить настройки инстанса.
    SaveSettings(bs_api::InstanceSettingsPatch),
    /// Список пользователей.
    LoadUsers {
        /// Подстрока поиска.
        query: String,
        /// Страница с 1.
        page: u32,
        /// Показывать удалённых.
        show_deleted: bool,
    },
    /// Назначить или снять администратора.
    SetRole {
        /// Пользователь.
        id: String,
        /// Новое значение.
        admin: bool,
    },
    /// Сбросить пароль пользователя.
    ResetPassword(String),
    /// Удалить пользователя.
    DeleteUser(String),
    /// Сводка по хранилищу.
    LoadStorage,
    /// Очистить осиротевшие файлы.
    Cleanup {
        /// Только посчитать.
        dry_run: bool,
    },
    /// Сменить свой пароль.
    ChangePassword {
        /// Текущий (для домашнего аккаунта обязателен).
        current: Option<Secret>,
        /// Новый.
        new: Secret,
    },
    /// Ограничения стриминга.
    LoadStreaming,
    /// Сохранить ограничения стриминга.
    SaveStreaming(bs_api::StreamingPatch),
    /// Состояние телеметрии.
    LoadTelemetry,
    /// Включить или выключить телеметрию.
    SetTelemetry(bool),
    /// Что отправила бы телеметрия.
    LoadTelemetryPreview,
    /// Состояние обновлений; `true` — проверить заново.
    LoadUpdates(bool),
    /// Удалить свой аккаунт.
    DeleteAccount {
        /// Пароль.
        password: Secret,
        /// Логин для подтверждения.
        username: String,
    },
}

/// Ответы на [`AdminRequest`] и загрузку картинки профиля.
#[derive(Debug)]
pub(crate) enum AdminEvent {
    /// Настройки инстанса.
    Settings(Result<bs_api::InstanceSettings, String>),
    /// Итог сохранения настроек.
    SettingsSaved(Result<bs_api::InstanceSettings, String>),
    /// Страница пользователей.
    Users(Result<bs_api::AdminUserPage, String>),
    /// Сводка по хранилищу.
    Storage(Result<bs_api::StorageStats, String>),
    /// Ограничения стриминга (прочитаны или сохранены).
    Streaming(Result<bs_api::StreamingLimits, String>),
    /// Состояние телеметрии.
    Telemetry(Result<bs_api::TelemetryStatus, String>),
    /// Предпросмотр телеметрии (JSON текстом).
    TelemetryPreview(Result<String, String>),
    /// Состояние обновлений.
    Updates(Result<bs_api::UpdateStatus, String>),
    /// Итог очистки.
    Cleaned(Result<bs_api::CleanupResult, String>),
    /// Итог простого действия: текст успеха или ошибки.
    Done {
        /// Что делали.
        action: AdminDone,
        /// Сообщение об успехе или текст ошибки.
        result: Result<String, String>,
    },
}

/// Какое действие завершилось.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminDone {
    /// Роль пользователя.
    Role,
    /// Сброс пароля пользователя.
    Reset,
    /// Удаление пользователя.
    Delete,
    /// Смена своего пароля.
    Password,
    /// Удаление своего аккаунта.
    Account,
    /// Картинка профиля.
    Image,
}

/// Ответ на запрос профиля.
#[derive(Debug)]
pub(crate) struct ProfileEvent {
    /// Чей профиль.
    pub(crate) user_id: String,
    /// Профиль или текст ошибки.
    pub(crate) user: Result<Box<bs_types::User>, String>,
    /// Общие друзья и пространства, если получены.
    pub(crate) mutuals: Option<bs_api::Mutuals>,
}

/// Что пришло для композера; GUI забирает через [`Session::take_compose_events`].
#[derive(Debug)]
pub(crate) enum ComposeEvent {
    /// Ход загрузки файла.
    Progress {
        /// Номер загрузки.
        id: u64,
        /// Отправлено байт.
        sent: u64,
        /// Всего байт.
        total: u64,
    },
    /// Загрузка закончилась.
    Uploaded {
        /// Номер загрузки.
        id: u64,
        /// Вложение или текст ошибки.
        result: Result<Box<bs_types::Attachment>, String>,
    },
    /// Доступность GIF.
    GifEnabled(Result<bool, String>),
    /// Страница GIF.
    Gifs {
        /// Запрос, для которого пришло.
        query: String,
        /// Курсор, с которого запрашивали.
        pos: String,
        /// Страница или текст ошибки.
        result: Result<bs_api::GifPage, String>,
    },
    /// Личные стикеры.
    Stickers(Result<Vec<bs_api::PersonalSticker>, String>),
    /// Итог загрузки или удаления стикера.
    StickerChanged(Result<(), String>),
    /// Не удалось отправить сообщение.
    SendFailed(String),
}

/// Ответ фоновой сессии на [`BridgeRequest`].
#[derive(Debug)]
pub(crate) enum BridgeResponse {
    /// Пришли друзья.
    Friends(Vec<bs_types::Friend>),
    /// Пришли заявки в друзья.
    FriendRequests(Vec<bs_types::FriendRequest>),
    /// Пришли результаты поиска пользователей.
    Users(Vec<bs_types::User>),
    /// Пришли личные каналы.
    DmChannels(Vec<bs_types::DmChannel>),
    /// Открыт личный канал.
    DmChannel(Box<bs_types::DmChannel>),
    /// Пришёл каталог пространств.
    Explore(bs_api::ExplorePage),
    /// Пространство создано или принято вступление.
    Space(Box<bs_types::SpaceWithChannelsAndMembers>),
    /// Профиль сохранён; пришла его новая версия.
    Profile(Box<bs_types::User>),
    /// Заявка на вступление отправлена.
    JoinRequested,
    /// Картинка скачана и декодирована (или не удалась).
    Image {
        /// Путь, который запрашивали.
        path: bs_api::AssetPath,
        /// Кадры или текст ошибки.
        result: Result<bs_media::Decoded, String>,
    },
    /// События композера (загрузки, GIF, стикеры).
    Compose(ComposeEvent),
    /// Ответ настроек.
    Admin(AdminEvent),
    /// Профиль другого пользователя.
    UserProfile(ProfileEvent),
    /// Скачанный файл.
    Raw {
        /// Номер запроса.
        tag: u64,
        /// Байты или текст ошибки.
        result: Result<Vec<u8>, String>,
    },
    /// Домашний инстанс выдал (или отказал выдать) секрет для входа.
    Credential {
        /// Куда собирались входить.
        origin: String,
        /// Секрет или текст ошибки.
        result: Result<IssuedCredential, String>,
    },
    /// Ответ о пиринге.
    Peering {
        /// С кем пиринг.
        origin: String,
        /// Статус сервера или текст ошибки.
        result: Result<String, String>,
    },
    /// Пришла страница истории.
    History {
        /// Канал, чью историю запрашивали.
        channel_id: String,
        /// Сами сообщения.
        items: HistoryItems,
    },
    /// История не загрузилась.
    HistoryFailed {
        /// Канал, чью историю запрашивали.
        channel_id: String,
        /// Текст ошибки.
        message: String,
    },
    /// Запись прошла успешно.
    Done,
    /// Список связанных инстансов отправлен (или секрет отмечен).
    Synced,
    /// Реестр подключений с сервера.
    Registry(Result<RegistryResponse, String>),
    /// Реестр записан.
    RegistrySaved(Result<(), String>),
    /// Итоги удаления учётки по инстансам.
    IdentityDeleted(Result<HashMap<String, IdentityDeleteResult>, String>),
    /// Пароль удалённого аккаунта заменён выданным секретом.
    Adopted(Result<(), String>),
    /// Запрос не удался.
    Failed(String),
}

/// Почему не удался вход в инстанс.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureKind {
    /// Инстанс не ответил: сеть, DNS, TLS, таймаут.
    Network,
    /// Аккаунт на инстансе есть, но он не принял выданный домом секрет.
    CredentialRefused,
    /// Инстанс не принимает новые аккаунты с других инстансов и отказал во входе.
    RegistrationClosed,
    /// Остальные отказы: токен или пароль не приняты, ошибка сервера.
    Auth,
}

/// Событие из фоновой задачи в GUI-поток.
#[derive(Debug)]
enum BridgeEvent {
    /// Вход не удался; `kind` решает, что показать в списке подключений.
    LoginFailed {
        /// Готовый текст для показа.
        message: String,
        /// Причина.
        kind: FailureKind,
    },
    /// Первый снимок состояния инстанса.
    Ready {
        /// Сам снимок.
        payload: Box<ReadyPayload>,
    },
    /// Дельта состояния инстанса.
    Server {
        /// Само событие.
        event: Box<ServerEvent>,
    },
    /// Ошибка логина или соединения.
    Error {
        /// Текст ошибки.
        message: String,
    },
    /// Сменилось состояние соединения.
    State {
        /// Новое состояние.
        state: ConnectionState,
    },
    /// Поток событий инстанса закрыт.
    Disconnected,
    /// Фоновая задача сессии завершилась: штатно, по ошибке, после `abort`
    /// или паники. Это последнее событие сессии.
    Ended,
}

/// Присылает [`BridgeEvent::Ended`] при уничтожении, чем бы ни закончилась
/// задача сессии: возвратом, `abort()` или паникой.
struct EndGuard(std_mpsc::Sender<BridgeEvent>);

impl Drop for EndGuard {
    fn drop(&mut self) {
        let _ = self.0.send(BridgeEvent::Ended);
    }
}

/// Фаза жизненного цикла сессии.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Сессии нет: можно входить.
    Idle,
    /// Идёт логин и первое подключение.
    Connecting,
    /// Сессия работает.
    Active,
    /// Выход: задача прервана, ждём её завершения.
    Closing,
}

/// Мост GUI ↔ tokio.
#[derive(Debug)]
#[allow(
    clippy::struct_field_names,
    clippy::struct_excessive_bools,
    reason = "флаги состояния одной сессии; `session` — её фоновая задача"
)]
pub(crate) struct Session {
    runtime: Handle,
    phase: Phase,
    events: Option<std_mpsc::Receiver<BridgeEvent>>,
    /// Канал команд (WebSocket-события) живой сессии.
    commands: Option<tokio_mpsc::UnboundedSender<ClientEvent>>,
    /// Канал REST-запросов живой сессии.
    requests: Option<tokio_mpsc::UnboundedSender<BridgeRequest>>,
    responses: Option<std_mpsc::Receiver<BridgeResponse>>,
    /// Фоновая задача сессии: её `abort()` и есть выход.
    session: Option<tokio::task::JoinHandle<()>>,
    friends: Vec<bs_types::Friend>,
    friend_requests: Vec<bs_types::FriendRequest>,
    dm_channels: Vec<bs_types::DmChannel>,
    search_results: Vec<bs_types::User>,
    social_error: Option<String>,
    /// Канал, только что открытый по запросу `OpenDm` (нужно переключить чат).
    opened_dm: Option<String>,
    /// Адрес инстанса текущей сессии.
    base: Option<String>,
    /// Секреты для входа, выданные домашним инстансом (только у домашней сессии).
    issued: Vec<(String, Result<IssuedCredential, String>)>,
    /// Ответы о пиринге (только у домашней сессии).
    peering: Vec<(String, Result<String, String>)>,
    /// Удалось ли отправить список инстансов.
    synced: bool,
    /// Причина последнего неудачного входа.
    failure: Option<FailureKind>,
    /// Ответ на `LoadRegistry` (только у домашней сессии).
    registry_in: Option<Result<RegistryResponse, String>>,
    /// Итоги `PutRegistry` (только у домашней сессии).
    saved_in: Vec<Result<(), String>>,
    /// Итоги `DeleteIdentity` (только у домашней сессии).
    deleted_in: Vec<Result<HashMap<String, IdentityDeleteResult>, String>>,
    /// Итог смены пароля удалённого аккаунта на выданный секрет.
    adopted_in: Option<Result<(), String>>,
    /// События композера, которые GUI ещё не забрал.
    compose_in: Vec<ComposeEvent>,
    profile_in: Vec<ProfileEvent>,
    raw_ready: Vec<(u64, Result<Vec<u8>, String>)>,
    /// Ответы настроек, которые GUI ещё не забрал.
    admin_in: Vec<AdminEvent>,
    /// Готовые картинки, которые GUI ещё не превратил в текстуры.
    images_ready: Vec<(bs_api::AssetPath, Result<bs_media::Decoded, String>)>,
    /// Последняя страница каталога пространств.
    explore: Option<bs_api::ExplorePage>,
    /// Идёт ли запрос каталога.
    explore_loading: bool,
    /// Пространство, только что созданное или в которое вступили.
    opened_space: Option<String>,
    /// Короткое сообщение об успехе («Профиль сохранён»).
    info: Option<String>,
    /// Загрузка истории по id канала.
    history: HashMap<String, HistoryState>,
    /// Кэш инстанса; `None` до первого `ready`.
    cache: Option<Cache>,
    ready: bool,
    /// Потеряна ли живая сессия: GUI должен вернуться на логин.
    session_lost: bool,
    /// Последнее известное состояние соединения.
    connection: ConnectionState,
    last_error: Option<String>,
    /// Отказ сервера на действие пользователя (например, `rate_limited`).
    notice: Option<String>,
}

/// Приводит адрес инстанса к абсолютному URL.
///
/// Пользователь обычно вводит только хост (`backspace.example`), поэтому
/// схема `https://` подставляется автоматически. Явно указанная схема
/// (`http://`, `ws://` и т.п.) сохраняется как есть, а допустимость
/// открытого текста решает [`TransportPolicy`].
fn normalize_base(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    Some(with_scheme)
}

impl Session {
    /// Создаёт мост над данным runtime.
    pub(crate) fn new(runtime: Handle) -> Self {
        Self {
            runtime,
            phase: Phase::Idle,
            events: None,
            commands: None,
            requests: None,
            responses: None,
            session: None,
            friends: Vec::new(),
            friend_requests: Vec::new(),
            dm_channels: Vec::new(),
            search_results: Vec::new(),
            social_error: None,
            opened_dm: None,
            base: None,
            issued: Vec::new(),
            peering: Vec::new(),
            failure: None,
            registry_in: None,
            compose_in: Vec::new(),
            profile_in: Vec::new(),
            raw_ready: Vec::new(),
            admin_in: Vec::new(),
            saved_in: Vec::new(),
            deleted_in: Vec::new(),
            adopted_in: None,
            synced: false,
            images_ready: Vec::new(),
            explore: None,
            explore_loading: false,
            opened_space: None,
            info: None,
            history: HashMap::new(),
            cache: None,
            ready: false,
            session_lost: false,
            connection: ConnectionState::Connecting,
            last_error: None,
            notice: None,
        }
    }

    /// Текущая фаза сессии (нужна только тестам).
    #[cfg(test)]
    pub(crate) const fn phase(&self) -> Phase {
        self.phase
    }

    /// Забирает накопленные события из канала (без блокировки).
    pub(crate) fn poll(&mut self) {
        loop {
            let next = self.events.as_ref().map(std_mpsc::Receiver::try_recv);
            match next {
                Some(Ok(event)) => self.handle_event(event),
                Some(Err(std_mpsc::TryRecvError::Empty | std_mpsc::TryRecvError::Disconnected))
                | None => break,
            }
        }
        loop {
            let next = self.responses.as_ref().map(std_mpsc::Receiver::try_recv);
            match next {
                Some(Ok(response)) => self.handle_response(response),
                Some(Err(std_mpsc::TryRecvError::Empty | std_mpsc::TryRecvError::Disconnected))
                | None => break,
            }
        }
    }

    fn handle_event(&mut self, event: BridgeEvent) {
        match event {
            BridgeEvent::Ready { payload } => {
                self.cache = Some(Cache::from_ready(*payload));
                self.history.clear();
                self.phase = Phase::Active;
                self.connection = ConnectionState::Connected;
                self.ready = true;
                self.session_lost = false;
                self.last_error = None;
            }
            BridgeEvent::Server { event } => self.apply_server_event(*event),
            BridgeEvent::State { state } => self.connection = state,
            BridgeEvent::Error { message } => self.last_error = Some(message),
            BridgeEvent::LoginFailed { message, kind } => {
                self.last_error = Some(message);
                self.failure = Some(kind);
            }
            BridgeEvent::Disconnected => {
                self.connection = ConnectionState::Stopped;
                self.last_error = Some("соединение закрыто".to_owned());
            }
            BridgeEvent::Ended => self.finish_session(),
        }
    }

    /// Применяет событие сервера к кэшу.
    fn apply_server_event(&mut self, event: ServerEvent) {
        match &event {
            // Отказ на действие пользователя: без этого сообщение, которое
            // сервер не принял, просто молча пропадало.
            ServerEvent::Error { message, code, .. } => {
                self.notice = Some(describe_server_error(code.as_deref(), message));
                return;
            }
            // `ready` после реконнекта пересобирает кэш без сообщений:
            // историю надо загрузить заново.
            ServerEvent::Ready(_) => self.history.clear(),
            _ => {}
        }
        if let Some(cache) = self.cache.as_mut() {
            let _ = apply(cache, event);
        }
    }

    /// Фоновая задача завершилась: сессия окончательно закрыта.
    fn finish_session(&mut self) {
        let was = self.phase;
        self.session = None;
        self.commands = None;
        self.requests = None;
        self.events = None;
        self.responses = None;
        self.cache = None;
        self.history.clear();
        self.friends.clear();
        self.friend_requests.clear();
        self.dm_channels.clear();
        self.search_results.clear();
        self.social_error = None;
        self.opened_dm = None;
        self.base = None;
        self.issued.clear();
        self.peering.clear();
        self.registry_in = None;
        self.saved_in.clear();
        self.deleted_in.clear();
        self.adopted_in = None;
        self.images_ready.clear();
        self.compose_in.clear();
        self.profile_in.clear();
        self.admin_in.clear();
        self.explore = None;
        self.explore_loading = false;
        self.opened_space = None;
        self.info = None;
        self.notice = None;
        self.ready = false;
        self.connection = ConnectionState::Connecting;
        self.phase = Phase::Idle;
        // Отказ логина оставляет пользователя на форме; потеря живой сессии
        // и выход возвращают на неё.
        self.session_lost = matches!(was, Phase::Active | Phase::Closing);
        if was == Phase::Active && self.last_error.is_none() {
            self.last_error = Some("соединение закрыто".to_owned());
        }
    }

    /// `true` ровно один раз — когда пришёл первый `ready`.
    pub(crate) fn take_ready(&mut self) -> bool {
        std::mem::take(&mut self.ready)
    }

    /// Идёт ли логин или выход: форму и кнопки надо заблокировать.
    pub(crate) const fn is_busy(&self) -> bool {
        matches!(self.phase, Phase::Connecting | Phase::Closing)
    }

    /// Сессия работает и получила первый снимок состояния.
    pub(crate) fn is_live(&self) -> bool {
        self.phase == Phase::Active && self.cache.is_some()
    }

    /// Сессия не запущена (нет ни входа, ни выхода в процессе).
    pub(crate) const fn is_idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    /// Показывает, что вход начат, ещё до запуска фоновой задачи
    /// (пока дом выдаёт секрет).
    pub(crate) fn mark_connecting(&mut self, base: &str) {
        self.phase = Phase::Connecting;
        self.base = Some(base.to_owned());
        self.last_error = None;
        self.connection = ConnectionState::Connecting;
    }

    /// Запоминает ошибку и возвращает сессию в покой.
    pub(crate) fn fail(&mut self, message: impl Into<String>) {
        self.last_error = Some(message.into());
        self.phase = Phase::Idle;
        self.connection = ConnectionState::Connecting;
    }

    /// Показывает короткое сообщение в интерфейсе (однократно).
    pub(crate) fn set_info(&mut self, message: String) {
        self.info = Some(message);
    }

    /// Сколько каналов и бесед с непрочитанным.
    pub(crate) fn unread_total(&self) -> usize {
        self.cache.as_ref().map_or(0, |cache| {
            let channels = cache
                .all_channels()
                .filter(|c| cache.unread_count(&c.id) > 0)
                .count();
            let dms = cache
                .dm_channels()
                .filter(|d| cache.unread_count(&d.id) > 0)
                .count();
            channels + dms
        })
    }

    /// Последняя ошибка, если была.
    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Отказ сервера на последнее действие (показывается над лентой).
    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Скрывает уведомление.
    pub(crate) fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    /// Кэш инстанса; `None` до первого `ready`.
    pub(crate) const fn cache(&self) -> Option<&Cache> {
        self.cache.as_ref()
    }

    /// Текущее состояние соединения (для строки статуса).
    pub(crate) const fn connection(&self) -> &ConnectionState {
        &self.connection
    }

    /// `true` ровно один раз — если сессия закрыта и надо вернуться на логин.
    pub(crate) fn take_session_lost(&mut self) -> bool {
        std::mem::take(&mut self.session_lost)
    }

    /// Запускает логин в фоне.
    ///
    /// Работает только из фазы [`Phase::Idle`]: пока прежняя сессия не
    /// закрыта до конца, второй вход (в тот же или другой инстанс) отклоняется.
    pub(crate) fn start_login(
        &mut self,
        base: &str,
        username: &str,
        password: &str,
        transport: TransportPolicy,
    ) {
        if self.phase != Phase::Idle {
            self.last_error = Some("Сначала выйдите из текущего инстанса".to_owned());
            return;
        }
        let Some(base) = normalize_base(base) else {
            self.last_error = Some("заполните адрес, логин и пароль".to_owned());
            return;
        };
        if username.trim().is_empty() || password.is_empty() {
            self.last_error = Some("заполните адрес, логин и пароль".to_owned());
            return;
        }
        // Проверяем адрес до сети: пользователь сразу видит причину отказа,
        // а пароль не уходит ни в какое соединение.
        match url::Url::parse(&base) {
            Ok(parsed) => {
                if let Err(err) = transport.check(&parsed) {
                    self.last_error = Some(err.to_string());
                    return;
                }
            }
            Err(err) => {
                self.last_error = Some(format!("некорректный адрес: {err}"));
                return;
            }
        }

        self.launch(SessionParams {
            origin: Origin::Home,
            base,
            username: username.trim().to_owned(),
            password: Zeroizing::new(password.to_owned()),
            register: None,
            transport,
        });
    }

    /// Запускает вход на другой инстанс по секрету, выданному домом.
    ///
    /// Если аккаунта там ещё нет, он создаётся (`register`); отказ
    /// регистрации не ошибка: аккаунт мог уже существовать.
    pub(crate) fn start_remote(
        &mut self,
        credential: &IssuedCredential,
        base: &str,
        display_name: Option<String>,
        transport: TransportPolicy,
        register: bool,
    ) {
        // Сессия могла быть помечена «подключается» (ждали секрет), но
        // фоновая задача ещё не запущена: только в этом случае входим.
        if self.session.is_some() {
            return;
        }
        let Ok(url) = url::Url::parse(base) else {
            self.fail("некорректный адрес инстанса");
            return;
        };
        if let Err(err) = transport.check(&url) {
            self.fail(err.to_string());
            return;
        }
        self.launch(SessionParams {
            origin: Origin::Remote(url),
            base: base.to_owned(),
            username: credential.username.clone(),
            password: Zeroizing::new(credential.secret.expose().to_owned()),
            register: register.then(|| RegisterRequest {
                username: credential.username.clone(),
                password: credential.secret.expose().to_owned(),
                display_name,
                home_instance: Some(credential.home_host.clone()),
                home_user_id: Some(credential.home_user_id.clone()),
            }),
            transport,
        });
    }

    /// Входит на чужой инстанс **собственным** паролем аккаунта там (без
    /// регистрации): запасной путь, когда инстанс не принял выданный секрет.
    pub(crate) fn start_remote_login(
        &mut self,
        base: &str,
        username: &str,
        password: &Secret,
        transport: TransportPolicy,
    ) {
        if self.session.is_some() {
            return;
        }
        let Ok(url) = url::Url::parse(base) else {
            self.fail("некорректный адрес инстанса");
            return;
        };
        if let Err(err) = transport.check(&url) {
            self.fail(err.to_string());
            return;
        }
        self.launch(SessionParams {
            origin: Origin::Remote(url),
            base: base.to_owned(),
            username: username.to_owned(),
            password: Zeroizing::new(password.expose().to_owned()),
            register: None,
            transport,
        });
    }

    /// Общий запуск фоновой сессии.
    fn launch(&mut self, params: SessionParams) {
        self.phase = Phase::Connecting;
        self.base = Some(params.base.clone());
        self.last_error = None;
        self.failure = None;
        self.session_lost = false;
        self.connection = ConnectionState::Connecting;

        let (events_tx, events_rx) = std_mpsc::channel();
        let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, responses_rx) = std_mpsc::channel();
        self.events = Some(events_rx);
        self.commands = Some(commands_tx);
        self.requests = Some(requests_tx);
        self.responses = Some(responses_rx);

        self.session = Some(self.runtime.spawn(run_session(
            params,
            events_tx,
            commands_rx,
            requests_rx,
            responses_tx,
        )));
    }

    /// Адрес инстанса этой сессии.
    pub(crate) fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    /// Забирает выданные секреты (у домашней сессии).
    pub(crate) fn take_issued(&mut self) -> Vec<(String, Result<IssuedCredential, String>)> {
        std::mem::take(&mut self.issued)
    }

    /// Причина последнего неудачного входа.
    pub(crate) const fn failure(&self) -> Option<FailureKind> {
        self.failure
    }

    /// Забирает реестр подключений (у домашней сессии).
    pub(crate) fn take_registry(&mut self) -> Option<Result<RegistryResponse, String>> {
        self.registry_in.take()
    }

    /// Забирает итоги записи реестра (у домашней сессии).
    pub(crate) fn take_saved(&mut self) -> Vec<Result<(), String>> {
        std::mem::take(&mut self.saved_in)
    }

    /// Забирает итоги удаления учёток (у домашней сессии).
    pub(crate) fn take_deleted(
        &mut self,
    ) -> Vec<Result<HashMap<String, IdentityDeleteResult>, String>> {
        std::mem::take(&mut self.deleted_in)
    }

    /// Забирает итог смены пароля на выданный секрет.
    pub(crate) fn take_adopted(&mut self) -> Option<Result<(), String>> {
        self.adopted_in.take()
    }

    /// Забирает ответы о пиринге (у домашней сессии).
    pub(crate) fn take_peering(&mut self) -> Vec<(String, Result<String, String>)> {
        std::mem::take(&mut self.peering)
    }

    /// Выходит из инстанса: прерывает фоновую сессию.
    ///
    /// Мост остаётся занятым ([`Phase::Closing`]) до события `Ended`, так что
    /// новый вход невозможен, пока старое соединение не закрыто полностью.
    pub(crate) fn logout(&mut self) {
        if !matches!(self.phase, Phase::Connecting | Phase::Active) {
            return;
        }
        self.phase = Phase::Closing;
        self.commands = None;
        self.requests = None;
        if let Some(session) = self.session.as_ref() {
            session.abort();
        }
    }

    /// Отправляет сообщение в канал или личный канал (в фоне).
    ///
    /// Для ЛС сервер ждёт отдельное событие `dm_message_create`: обычный
    /// `message_create` с id личного канала он молча отбрасывает.
    pub(crate) fn send_message(
        &self,
        kind: ChannelKind,
        channel_id: String,
        content: String,
        reply_to_id: Option<String>,
    ) {
        self.send(match kind {
            ChannelKind::Space => ClientEvent::MessageCreate {
                channel_id,
                content,
                reply_to_id,
            },
            ChannelKind::Dm => ClientEvent::DmMessageCreate {
                dm_channel_id: channel_id,
                content: Some(content),
                attachments: None,
                reply_to_id,
            },
        });
    }

    /// Отправляет команду серверу, если сессия жива.
    pub(crate) fn send(&self, event: ClientEvent) {
        if let Some(commands) = self.commands.as_ref() {
            let _ = commands.send(event);
        }
    }

    /// Правит своё сообщение.
    pub(crate) fn edit_message(&self, kind: ChannelKind, message_id: String, content: String) {
        self.send(match kind {
            ChannelKind::Space => ClientEvent::MessageEdit {
                message_id,
                content,
            },
            ChannelKind::Dm => ClientEvent::DmMessageEdit {
                message_id,
                content,
            },
        });
    }

    /// Удаляет своё сообщение.
    pub(crate) fn delete_message(&self, kind: ChannelKind, message_id: String) {
        self.send(match kind {
            ChannelKind::Space => ClientEvent::MessageDelete { message_id },
            ChannelKind::Dm => ClientEvent::DmMessageDelete { message_id },
        });
    }

    /// Ставит реакцию.
    pub(crate) fn add_reaction(&self, message_id: String, emoji: String) {
        self.send(ClientEvent::ReactionAdd { message_id, emoji });
    }

    /// Убирает реакцию.
    pub(crate) fn remove_reaction(&self, message_id: String, emoji: String) {
        self.send(ClientEvent::ReactionRemove { message_id, emoji });
    }

    /// Подтверждает прочтение канала до сообщения.
    pub(crate) fn ack_channel(&self, channel_id: String, message_id: String) {
        self.send(ClientEvent::ChannelAck {
            channel_id,
            message_id,
        });
    }

    /// Отправляет REST-запрос; `false`, если сессии нет.
    pub(crate) fn request(&self, request: BridgeRequest) -> bool {
        self.requests
            .as_ref()
            .is_some_and(|requests| requests.send(request).is_ok())
    }

    /// Забирает накопленные ответы настроек.
    pub(crate) fn take_admin_events(&mut self) -> Vec<AdminEvent> {
        std::mem::take(&mut self.admin_in)
    }

    /// Забирает скачанные файлы.
    pub(crate) fn take_raw(&mut self) -> Vec<(u64, Result<Vec<u8>, String>)> {
        std::mem::take(&mut self.raw_ready)
    }

    /// Забирает накопленные ответы профилей.
    pub(crate) fn take_profile_events(&mut self) -> Vec<ProfileEvent> {
        std::mem::take(&mut self.profile_in)
    }

    /// Забирает накопленные события композера.
    pub(crate) fn take_compose_events(&mut self) -> Vec<ComposeEvent> {
        std::mem::take(&mut self.compose_in)
    }

    /// Отправляет сообщение в канал пространства (REST, нужен для вложений).
    pub(crate) fn send_rest(&self, channel_id: String, body: bs_api::SendMessageBody) {
        self.request(BridgeRequest::SendRest { channel_id, body });
    }

    /// Отправляет личное сообщение с вложениями и ответом.
    pub(crate) fn send_dm_with(
        &self,
        channel_id: String,
        content: Option<String>,
        attachments: Vec<String>,
        reply_to_id: Option<String>,
    ) {
        self.send(ClientEvent::DmMessageCreate {
            dm_channel_id: channel_id,
            content,
            attachments: if attachments.is_empty() {
                None
            } else {
                Some(attachments)
            },
            reply_to_id,
        });
    }

    /// Последние полученные друзья.
    pub(crate) fn friends(&self) -> &[bs_types::Friend] {
        &self.friends
    }

    /// Последние полученные заявки.
    pub(crate) fn friend_requests(&self) -> &[bs_types::FriendRequest] {
        &self.friend_requests
    }

    /// Последние результаты поиска пользователей.
    pub(crate) fn search_results(&self) -> &[bs_types::User] {
        &self.search_results
    }

    /// `true` ровно один раз — если только что открыли личный канал.
    pub(crate) fn take_opened_dm(&mut self) -> Option<String> {
        self.opened_dm.take()
    }

    /// Адрес инстанса и канал запросов живой сессии — для кэша картинок.
    pub(crate) fn session_link(
        &self,
    ) -> Option<(String, tokio_mpsc::UnboundedSender<BridgeRequest>)> {
        Some((self.base.clone()?, self.requests.clone()?))
    }

    /// Забирает картинки, пришедшие с прошлого кадра.
    pub(crate) fn take_images(
        &mut self,
    ) -> Vec<(bs_api::AssetPath, Result<bs_media::Decoded, String>)> {
        std::mem::take(&mut self.images_ready)
    }

    /// Последняя страница каталога пространств.
    pub(crate) fn explore(&self) -> Option<&bs_api::ExplorePage> {
        self.explore.as_ref()
    }

    /// Идёт ли запрос каталога.
    pub(crate) const fn explore_loading(&self) -> bool {
        self.explore_loading
    }

    /// Возвращает пространство, которое только что создали или в которое вступили.
    pub(crate) fn take_opened_space(&mut self) -> Option<String> {
        self.opened_space.take()
    }

    /// Короткое сообщение об успехе; читается один раз.
    pub(crate) fn take_info(&mut self) -> Option<String> {
        self.info.take()
    }

    /// Запрашивает каталог и помечает загрузку.
    pub(crate) fn load_explore(&mut self, query: &str) {
        if self.request(BridgeRequest::Explore(query.to_owned())) {
            self.explore_loading = true;
        }
    }

    /// Ошибка последнего REST-запроса.
    pub(crate) fn social_error(&self) -> Option<&str> {
        self.social_error.as_deref()
    }

    /// Состояние загрузки истории канала.
    pub(crate) fn history_state(&self, channel_id: &str) -> Option<&HistoryState> {
        self.history.get(channel_id)
    }

    /// Просит самые новые сообщения канала, если ещё не просили.
    ///
    /// Безопасно вызывать каждый кадр: повторный запрос не уходит, пока
    /// состояние канала известно.
    pub(crate) fn ensure_history(&mut self, kind: ChannelKind, channel_id: &str) {
        if self.history.contains_key(channel_id) {
            return;
        }
        let request = BridgeRequest::History {
            channel_id: channel_id.to_owned(),
            kind,
            before: None,
        };
        if self.request(request) {
            self.history.insert(
                channel_id.to_owned(),
                HistoryState {
                    loading: true,
                    ..HistoryState::default()
                },
            );
        }
    }

    /// Подгружает страницу старее уже загруженного (или повторяет первую
    /// страницу после ошибки).
    pub(crate) fn load_older(&mut self, kind: ChannelKind, channel_id: &str) {
        let Some(state) = self.history.get(channel_id) else {
            return;
        };
        if state.loading || state.exhausted {
            return;
        }
        let oldest = self.cache.as_ref().and_then(|cache| match kind {
            ChannelKind::Space => cache
                .messages(channel_id)
                .first()
                .map(|m| m.message.id.clone()),
            ChannelKind::Dm => cache
                .dm_messages(channel_id)
                .first()
                .map(|m| m.message.id.clone()),
        });
        if oldest.is_none() && state.error.is_none() {
            return;
        }
        let request = BridgeRequest::History {
            channel_id: channel_id.to_owned(),
            kind,
            before: oldest,
        };
        if self.request(request)
            && let Some(state) = self.history.get_mut(channel_id)
        {
            state.loading = true;
            state.error = None;
        }
    }

    /// Вливает страницу истории в кэш и обновляет состояние канала.
    fn apply_history(&mut self, channel_id: &str, items: HistoryItems) {
        let Some(state) = self.history.get_mut(channel_id) else {
            // Пришла после `ready`, который уже сбросил кэш и состояние.
            return;
        };
        state.loading = false;
        state.error = None;
        state.exhausted = items.len() < usize::from(HISTORY_PAGE);
        let Some(cache) = self.cache.as_mut() else {
            return;
        };
        match items {
            HistoryItems::Space(page) => {
                cache.merge_history(channel_id, page);
            }
            HistoryItems::Dm(page) => {
                cache.merge_dm_history(channel_id, page);
            }
        }
    }

    /// Применяет ответ REST-запроса.
    fn handle_response(&mut self, response: BridgeResponse) {
        match response {
            BridgeResponse::History { channel_id, items } => {
                self.apply_history(&channel_id, items);
            }
            BridgeResponse::HistoryFailed {
                channel_id,
                message,
            } => {
                if let Some(state) = self.history.get_mut(&channel_id) {
                    state.loading = false;
                    state.error = Some(message);
                }
            }
            BridgeResponse::Friends(friends) => {
                self.friends = friends;
                self.social_error = None;
            }
            BridgeResponse::FriendRequests(requests) => {
                self.friend_requests = requests;
                self.social_error = None;
            }
            BridgeResponse::Users(users) => {
                self.search_results = users;
                self.social_error = None;
            }
            BridgeResponse::DmChannels(channels) => {
                self.dm_channels = channels;
                self.social_error = None;
            }
            BridgeResponse::DmChannel(channel) => {
                let id = channel.id.clone();
                self.opened_dm = Some(id.clone());
                // Сервер не обязан присылать создателю `dm_channel_created`,
                // а сайдбар и лента читают личные каналы из кэша.
                if let Some(cache) = self.cache.as_mut() {
                    cache.upsert_dm_channel((*channel).clone());
                }
                if !self.dm_channels.iter().any(|known| known.id == id) {
                    self.dm_channels.push(*channel);
                }
                self.social_error = None;
            }
            BridgeResponse::Explore(page) => {
                self.explore = Some(page);
                self.explore_loading = false;
                self.social_error = None;
            }
            BridgeResponse::Space(space) => {
                let id = space.space.id.clone();
                if let Some(cache) = self.cache.as_mut() {
                    cache.upsert_space(*space);
                }
                self.opened_space = Some(id);
                self.social_error = None;
                // Каталог помечает вступившие пространства: обновим его.
                self.explore = None;
            }
            BridgeResponse::Profile(user) => {
                if let Some(cache) = self.cache.as_mut() {
                    let _ = bs_core::apply(cache, ServerEvent::UserUpdated { user: *user });
                }
                self.info = Some("Профиль сохранён".to_owned());
                self.social_error = None;
            }
            BridgeResponse::Image { path, result } => {
                self.images_ready.push((path, result));
            }
            BridgeResponse::Compose(event) => self.compose_in.push(event),
            BridgeResponse::Admin(event) => self.admin_in.push(event),
            BridgeResponse::UserProfile(event) => self.profile_in.push(event),
            BridgeResponse::Raw { tag, result } => self.raw_ready.push((tag, result)),
            BridgeResponse::JoinRequested => {
                self.info = Some("Заявка отправлена".to_owned());
                self.social_error = None;
            }
            BridgeResponse::Credential { origin, result } => {
                self.issued.push((origin, result));
            }
            BridgeResponse::Peering { origin, result } => {
                self.peering.push((origin, result));
            }
            BridgeResponse::Synced => {
                self.synced = true;
            }
            BridgeResponse::Registry(result) => self.registry_in = Some(result),
            BridgeResponse::RegistrySaved(result) => self.saved_in.push(result),
            BridgeResponse::IdentityDeleted(result) => self.deleted_in.push(result),
            BridgeResponse::Adopted(result) => self.adopted_in = Some(result),
            BridgeResponse::Done => {
                self.social_error = None;
                self.request(BridgeRequest::Friends);
                self.request(BridgeRequest::FriendRequests);
                self.request(BridgeRequest::DmChannels);
            }
            BridgeResponse::Failed(message) => {
                self.explore_loading = false;
                self.social_error = Some(message);
            }
        }
    }
}

/// Параметры входа, переданные фоновой сессии.
struct SessionParams {
    origin: Origin,
    base: String,
    username: String,
    /// Пароль затирается в памяти, как только логин выполнен.
    password: Zeroizing<String>,
    /// Регистрация перед входом (федеративный аккаунт на чужом инстансе).
    register: Option<RegisterRequest>,
    transport: TransportPolicy,
}

/// Фоновая сессия: логин, затем пересылка событий и команд.
#[allow(
    clippy::too_many_lines,
    reason = "один цикл выбора между источниками событий"
)]
async fn run_session(
    params: SessionParams,
    events: std_mpsc::Sender<BridgeEvent>,
    mut commands: tokio_mpsc::UnboundedReceiver<ClientEvent>,
    mut requests: tokio_mpsc::UnboundedReceiver<BridgeRequest>,
    responses: std_mpsc::Sender<BridgeResponse>,
) {
    // Сработает при любом выходе из функции, включая `abort()` и панику.
    let _end = EndGuard(events.clone());

    let SessionParams {
        origin,
        base,
        username,
        password,
        register,
        transport,
    } = params;
    let mut refusal = None;
    if let Some(request) = register {
        refusal = try_register(&origin, &base, &request, transport, &events).await;
        if refusal == Some(RegisterOutcome::Failed) {
            return;
        }
    }
    let mut store = CredentialStore::new();
    let login = InstanceHandle::login_with_transport(
        &mut store,
        origin,
        &base,
        &username,
        &password,
        Some(ClientKind::Desktop),
        ReconnectPolicy::default(),
        transport,
    )
    .await;
    drop(password);

    let (handle, ready) = match login {
        Ok(pair) => pair,
        Err(err) => {
            let message = if refusal.is_some() {
                remote_login_refused(refusal, &err.to_string())
            } else {
                err.to_string()
            };
            let kind = match refusal {
                Some(RegisterOutcome::Taken) => FailureKind::CredentialRefused,
                Some(RegisterOutcome::Closed) => FailureKind::RegistrationClosed,
                _ if is_network_error(&err) => FailureKind::Network,
                _ => FailureKind::Auth,
            };
            let _ = events.send(BridgeEvent::LoginFailed { message, kind });
            return;
        }
    };

    match ready {
        ServerEvent::Ready(payload) => {
            if events.send(BridgeEvent::Ready { payload }).is_err() {
                return;
            }
        }
        other => {
            let _ = events.send(BridgeEvent::Error {
                message: format!("ожидался ready, пришло {other:?}"),
            });
            return;
        }
    }

    let mut sub = handle.subscribe();
    // Загрузки картинок идут параллельно, но не больше `MAX_IMAGE_JOBS`
    // сразу; при выходе из сессии `JoinSet` обрывает незавершённые.
    let mut image_jobs: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();
    let image_gate = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_IMAGE_JOBS));
    loop {
        tokio::select! {
            Some(_) = image_jobs.join_next(), if !image_jobs.is_empty() => {}
            event = sub.recv() => match event {
                Ok(InstanceEvent::Server(boxed)) => {
                    if events.send(BridgeEvent::Server { event: boxed }).is_err() {
                        return;
                    }
                }
                Ok(InstanceEvent::AuthRejected { failure, message, .. }) => {
                    let _ = events.send(BridgeEvent::Error {
                        message: format!("{message} ({failure:?})"),
                    });
                }
                Ok(InstanceEvent::StateChanged(state)) => {
                    if events.send(BridgeEvent::State { state }).is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "пропущены события инстанса");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    let _ = events.send(BridgeEvent::Disconnected);
                    return;
                }
            },
            request = requests.recv() => match request {
                Some(BridgeRequest::Image(path)) => {
                    let client = handle.client().clone();
                    let gate = std::sync::Arc::clone(&image_gate);
                    let tx = responses.clone();
                    image_jobs.spawn(async move {
                        let Ok(_permit) = gate.acquire_owned().await else {
                            return;
                        };
                        let result = load_image(&client, &path).await;
                        let _ = tx.send(BridgeResponse::Image { path, result });
                    });
                }
                Some(BridgeRequest::Raw { tag, path }) => {
                    let client = handle.client().clone();
                    let tx = responses.clone();
                    image_jobs.spawn(async move {
                        let result = client
                            .fetch_asset(&path, bs_media::Limits::default().max_input_bytes)
                            .await
                            .map_err(|err| err.to_string());
                        let _ = tx.send(BridgeResponse::Raw { tag, result });
                    });
                }
                Some(BridgeRequest::Upload { id, name, mime, bytes }) => {
                    let client = handle.client().clone();
                    let tx = responses.clone();
                    image_jobs.spawn(async move {
                        let progress_tx = tx.clone();
                        let result = client
                            .upload_file(&name, &mime, &bytes, move |sent, total| {
                                let _ = progress_tx.send(BridgeResponse::Compose(
                                    ComposeEvent::Progress { id, sent, total },
                                ));
                            })
                            .await
                            .map(Box::new)
                            .map_err(|err| upload_message(&err));
                        let _ = tx.send(BridgeResponse::Compose(ComposeEvent::Uploaded {
                            id,
                            result,
                        }));
                    });
                }
                Some(BridgeRequest::SetImage { kind, name, bytes }) => {
                    let client = handle.client().clone();
                    let tx = responses.clone();
                    image_jobs.spawn(async move {
                        let response = set_profile_image(&client, kind, &name, &bytes).await;
                        let _ = tx.send(response);
                    });
                }
                Some(request) => {
                    let response = handle_request(handle.client(), request).await;
                    if responses.send(response).is_err() {
                        return;
                    }
                }
                None => return,
            },
            command = commands.recv() => match command {
                Some(event) => {
                    if let Err(err) = handle.send(event).await {
                        let _ = events.send(BridgeEvent::Error { message: err.to_string() });
                    }
                }
                None => return,
            },
        }
    }
}

/// Чем закончилась попытка регистрации на чужом инстансе.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegisterOutcome {
    /// Аккаунт создан.
    Created,
    /// Имя занято: аккаунт, возможно, уже есть.
    Taken,
    /// Инстанс не принимает новых федеративных аккаунтов.
    Closed,
    /// Другая ошибка; текст уже отправлен в GUI.
    Failed,
}

/// Сетевая ли это ошибка (инстанс не ответил), а не отказ сервера.
fn is_network_error(err: &bs_instances::InstanceError) -> bool {
    use bs_api::ApiError;
    use bs_identity::IdentityError;
    use bs_instances::InstanceError;
    matches!(
        err,
        InstanceError::Api(ApiError::Transport(_))
            | InstanceError::Identity(IdentityError::Api(ApiError::Transport(_)))
            | InstanceError::Ws(_)
    )
}

/// Пытается создать аккаунт; отказ «занято/закрыто» не ошибка, дальше идёт вход.
async fn try_register(
    origin: &Origin,
    base: &str,
    request: &RegisterRequest,
    transport: TransportPolicy,
    events: &std_mpsc::Sender<BridgeEvent>,
) -> Option<RegisterOutcome> {
    let client = match ApiClient::with_policy(origin.clone(), base, transport) {
        Ok(client) => client,
        Err(err) => {
            let _ = events.send(BridgeEvent::Error {
                message: err.to_string(),
            });
            return Some(RegisterOutcome::Failed);
        }
    };
    match client.register(request).await {
        Ok(_) => Some(RegisterOutcome::Created),
        Err(bs_api::ApiError::Server { status, .. }) if status.as_u16() == 409 => {
            Some(RegisterOutcome::Taken)
        }
        Err(bs_api::ApiError::Server { status, .. }) if status.as_u16() == 403 => {
            Some(RegisterOutcome::Closed)
        }
        Err(err) => {
            let kind = if matches!(err, bs_api::ApiError::Transport(_)) {
                FailureKind::Network
            } else {
                FailureKind::Auth
            };
            let _ = events.send(BridgeEvent::LoginFailed {
                message: err.to_string(),
                kind,
            });
            Some(RegisterOutcome::Failed)
        }
    }
}

/// Объяснение отказа входа на чужой инстанс: что именно можно утверждать.
///
/// Про существующий аккаунт говорим только после ответа «имя занято»:
/// закрытый инстанс одинаково отвечает на вход и за несуществующий
/// аккаунт, и за аккаунт с другим паролем.
fn remote_login_refused(outcome: Option<RegisterOutcome>, raw: &str) -> String {
    match outcome {
        Some(RegisterOutcome::Taken) => "На этом инстансе уже есть аккаунт с таким именем, \
            но он не принимает пароль, выданный вашим домашним инстансом."
            .to_owned(),
        Some(RegisterOutcome::Closed) => "Инстанс не принимает новые аккаунты с других \
            инстансов. Если аккаунт у вас там уже есть, он не принял выданный пароль."
            .to_owned(),
        _ => raw.to_owned(),
    }
}

/// Сколько картинок качается и декодируется одновременно.
const MAX_IMAGE_JOBS: usize = 6;

/// Скачивает и декодирует картинку; ошибка — готовый текст для показа.
async fn load_image(
    client: &bs_api::ApiClient,
    path: &bs_api::AssetPath,
) -> Result<bs_media::Decoded, String> {
    let limits = bs_media::Limits::default();
    let bytes = client
        .fetch_asset(path, limits.max_input_bytes)
        .await
        .map_err(|err| err.to_string())?;
    // Декодирование грузит процессор — уводим его с рабочих потоков сети.
    tokio::task::spawn_blocking(move || bs_media::decode(&bytes, &limits))
        .await
        .map_err(|err| format!("декодер упал: {err}"))?
        .map_err(|err| err.to_string())
}

/// Выполняет REST-запрос и превращает результат в [`BridgeResponse`].
#[allow(
    clippy::too_many_lines,
    reason = "раскладка egui читается лучше одним блоком"
)]
/// Проверяет пароль (если он есть) и просит у дома секрет для входа на `origin`.
async fn issue_credential(
    client: &ApiClient,
    origin: &str,
    password: Option<&Secret>,
) -> BridgeResponse {
    let result = issue_credential_inner(client, origin, password).await;
    BridgeResponse::Credential {
        origin: origin.to_owned(),
        result,
    }
}

async fn issue_credential_inner(
    client: &ApiClient,
    origin: &str,
    password: Option<&Secret>,
) -> Result<IssuedCredential, String> {
    let me = client.me().await.map_err(|err| err.to_string())?;
    let home_host = me
        .home_instance
        .clone()
        .unwrap_or_else(|| bs_identity::identity::host_of(client.base()));
    if bs_identity::identity::host_of(origin) == home_host {
        return Err("Это ваш домашний инстанс: он уже подключён".to_owned());
    }
    if let Some(password) = password
        && !client
            .verify_password(password.expose())
            .await
            .map_err(|err| err.to_string())?
    {
        return Err("Неверный пароль".to_owned());
    }
    let issued = client
        .federation_credential(&FederationCredentialRequest::new(origin))
        .await
        .map_err(|err| err.to_string())?;
    let bare = me.username.split('@').next().unwrap_or(&me.username);
    Ok(IssuedCredential {
        username: format!("{bare}@{home_host}"),
        secret: Secret::new(issued.secret),
        home_host,
        home_user_id: me.home_user_id.clone().unwrap_or_else(|| me.id.clone()),
        display_name: me.display_name.clone(),
        provisioned: issued.provisioned,
    })
}

/// Загружает картинку профиля и привязывает её к аккаунту.
async fn set_profile_image(
    client: &bs_api::ApiClient,
    kind: ImageKind,
    name: &str,
    bytes: &[u8],
) -> BridgeResponse {
    let fail = |message: String| {
        BridgeResponse::Admin(AdminEvent::Done {
            action: AdminDone::Image,
            result: Err(message),
        })
    };
    let file = if bytes.is_empty() {
        String::new()
    } else {
        let mime = crate::media::mime_for_name(name);
        match client.upload_file(name, mime, bytes, |_, _| {}).await {
            Ok(attachment) => attachment.filename,
            Err(err) => return fail(upload_message(&err)),
        }
    };
    let mut request = bs_api::UpdateProfileRequest::default();
    match kind {
        ImageKind::Avatar => request.avatar = Some(file),
        ImageKind::Banner => request.banner = Some(file),
    }
    match client.update_me(&request).await {
        Ok(user) => BridgeResponse::Profile(Box::new(user)),
        Err(err) => fail(upload_message(&err)),
    }
}

/// Выполняет запрос настроек.
#[allow(clippy::too_many_lines, reason = "по ветке на действие")]
async fn handle_admin(client: &bs_api::ApiClient, request: AdminRequest) -> AdminEvent {
    let text = |err: &bs_api::ApiError| upload_message(err);
    let done =
        |action: AdminDone, result: Result<String, String>| AdminEvent::Done { action, result };
    match request {
        AdminRequest::LoadStreaming => {
            AdminEvent::Streaming(client.streaming_limits().await.map_err(|e| text(&e)))
        }
        AdminRequest::SaveStreaming(patch) => {
            AdminEvent::Streaming(client.update_streaming(&patch).await.map_err(|e| text(&e)))
        }
        AdminRequest::LoadTelemetry => {
            AdminEvent::Telemetry(client.telemetry().await.map_err(|e| text(&e)))
        }
        AdminRequest::SetTelemetry(on) => {
            AdminEvent::Telemetry(client.set_telemetry(on).await.map_err(|e| text(&e)))
        }
        AdminRequest::LoadTelemetryPreview => AdminEvent::TelemetryPreview(
            client
                .telemetry_preview()
                .await
                .map(|v| serde_json::to_string_pretty(&v).unwrap_or_default())
                .map_err(|e| text(&e)),
        ),
        AdminRequest::LoadUpdates(refresh) => {
            AdminEvent::Updates(client.update_status(refresh).await.map_err(|e| text(&e)))
        }
        AdminRequest::LoadSettings => {
            AdminEvent::Settings(client.instance_settings().await.map_err(|e| text(&e)))
        }
        AdminRequest::SaveSettings(patch) => AdminEvent::SettingsSaved(
            client
                .update_instance_settings(&patch)
                .await
                .map_err(|e| text(&e)),
        ),
        AdminRequest::LoadUsers {
            query,
            page,
            show_deleted,
        } => AdminEvent::Users(
            client
                .admin_users(&query, page, show_deleted)
                .await
                .map_err(|e| text(&e)),
        ),
        AdminRequest::SetRole { id, admin } => done(
            AdminDone::Role,
            client
                .admin_set_role(&id, admin)
                .await
                .map(|_| "Роль изменена".to_owned())
                .map_err(|e| text(&e)),
        ),
        AdminRequest::ResetPassword(id) => done(
            AdminDone::Reset,
            client
                .admin_reset_password(&id)
                .await
                .map(|r| r.temporary_password)
                .map_err(|e| text(&e)),
        ),
        AdminRequest::DeleteUser(id) => done(
            AdminDone::Delete,
            client
                .admin_delete_user(&id)
                .await
                .map(|_| "Пользователь удалён".to_owned())
                .map_err(|e| text(&e)),
        ),
        AdminRequest::LoadStorage => {
            AdminEvent::Storage(client.storage_stats().await.map_err(|e| text(&e)))
        }
        AdminRequest::Cleanup { dry_run } => {
            AdminEvent::Cleaned(client.storage_cleanup(dry_run).await.map_err(|e| text(&e)))
        }
        AdminRequest::ChangePassword { current, new } => {
            let body = current.map_or_else(
                || bs_api::ChangePasswordRequest::new(new.expose()),
                |current| {
                    bs_api::ChangePasswordRequest::with_current(current.expose(), new.expose())
                },
            );
            done(
                AdminDone::Password,
                client
                    .change_password(&body)
                    .await
                    .map(|_| "Пароль изменён".to_owned())
                    .map_err(|e| text(&e)),
            )
        }
        AdminRequest::DeleteAccount { password, username } => done(
            AdminDone::Account,
            client
                .delete_account(password.expose(), &username)
                .await
                .map(|_| "Аккаунт удалён".to_owned())
                .map_err(|e| text(&e)),
        ),
    }
}

/// Понятный текст отказа сервера для композера.
fn upload_message(err: &bs_api::ApiError) -> String {
    use bs_api::ApiError;
    match err {
        ApiError::Unexpected { status, .. } if status.as_u16() == 413 => {
            "Файл больше лимита сервера".to_owned()
        }
        ApiError::Unexpected { status, .. } if status.as_u16() == 429 => {
            "Слишком много загрузок, подождите минуту".to_owned()
        }
        ApiError::Unexpected { status, .. } | ApiError::Server { status, .. }
            if status.as_u16() == 403 =>
        {
            "Нет права прикреплять файлы".to_owned()
        }
        other => other.to_string(),
    }
}

#[allow(clippy::too_many_lines, reason = "по ветке на каждый вид запроса")]
async fn handle_request(client: &bs_api::ApiClient, request: BridgeRequest) -> BridgeResponse {
    match request {
        BridgeRequest::Friends => match client.friends().await {
            Ok(friends) => BridgeResponse::Friends(friends),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::FriendRequests => match client.friend_requests().await {
            Ok(requests) => BridgeResponse::FriendRequests(requests),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::SearchUsers(query) => match client.search_users(&query).await {
            Ok(users) => BridgeResponse::Users(users),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::DmChannels => match client.dm_channels().await {
            Ok(channels) => BridgeResponse::DmChannels(channels),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::OpenDm(user_id) => {
            let request = bs_api::CreateDmRequest::user(user_id);
            match client.open_dm(&request).await {
                Ok(channel) => BridgeResponse::DmChannel(Box::new(channel)),
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::SendFriendRequest(username) => {
            let request = bs_api::SendFriendRequest::username(username);
            match client.send_friend_request(&request).await {
                Ok(_) => BridgeResponse::Done,
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::RespondFriendRequest(id, accept) => {
            let decision = if accept {
                bs_api::FriendRequestDecision::Accept
            } else {
                bs_api::FriendRequestDecision::Decline
            };
            match client.respond_friend_request(&id, decision).await {
                Ok(()) => BridgeResponse::Done,
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::CancelFriendRequest(id) => match client.cancel_friend_request(&id).await {
            Ok(()) => BridgeResponse::Done,
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::RemoveFriend(user_id) => match client.remove_friend(&user_id).await {
            Ok(()) => BridgeResponse::Done,
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        // Картинки обрабатываются отдельно и параллельно (см. `run_session`).
        BridgeRequest::Image(_)
        | BridgeRequest::Raw { .. }
        | BridgeRequest::Upload { .. }
        | BridgeRequest::SetImage { .. } => BridgeResponse::Done,
        BridgeRequest::Admin(request) => BridgeResponse::Admin(handle_admin(client, request).await),
        BridgeRequest::GifEnabled => BridgeResponse::Compose(ComposeEvent::GifEnabled(
            client
                .gif_enabled()
                .await
                .map_err(|err| upload_message(&err)),
        )),
        BridgeRequest::Gifs { query, pos } => {
            let result = client
                .gifs(&query, 30, &pos)
                .await
                .map_err(|err| err.to_string());
            BridgeResponse::Compose(ComposeEvent::Gifs { query, pos, result })
        }
        BridgeRequest::Profile(user_id) => {
            let user = client
                .user(&user_id)
                .await
                .map(Box::new)
                .map_err(|err| upload_message(&err));
            let mutuals = client.mutuals(&user_id).await.ok();
            BridgeResponse::UserProfile(ProfileEvent {
                user_id,
                user,
                mutuals,
            })
        }
        BridgeRequest::Stickers => BridgeResponse::Compose(ComposeEvent::Stickers(
            client.stickers().await.map_err(|err| upload_message(&err)),
        )),
        BridgeRequest::UploadSticker { name, bytes } => {
            BridgeResponse::Compose(ComposeEvent::StickerChanged(
                client
                    .upload_sticker(&name, &bytes)
                    .await
                    .map(|_| ())
                    .map_err(|err| upload_message(&err)),
            ))
        }
        BridgeRequest::DeleteSticker(id) => BridgeResponse::Compose(ComposeEvent::StickerChanged(
            client
                .delete_sticker(&id)
                .await
                .map(|_| ())
                .map_err(|err| upload_message(&err)),
        )),
        BridgeRequest::SendRest { channel_id, body } => {
            match client.send_channel_message(&channel_id, &body).await {
                Ok(_) => BridgeResponse::Done,
                Err(err) => BridgeResponse::Compose(ComposeEvent::SendFailed(upload_message(&err))),
            }
        }
        BridgeRequest::IssueCredential { origin, password } => {
            issue_credential(client, &origin, password.as_ref()).await
        }
        BridgeRequest::EnsurePeered(origin) => BridgeResponse::Peering {
            result: client
                .ensure_peered(&origin)
                .await
                .map(|answer| answer.peering_status)
                .map_err(|err| err.to_string()),
            origin,
        },
        BridgeRequest::MarkProvisioned(origin) => {
            let request = FederationCredentialRequest::marking_provisioned(origin);
            match client.federation_credential(&request).await {
                Ok(_) => BridgeResponse::Synced,
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::LoadRegistry => BridgeResponse::Registry(
            client
                .federation_registry()
                .await
                .map_err(|e| e.to_string()),
        ),
        BridgeRequest::PutRegistry {
            entries,
            updated_at,
        } => BridgeResponse::RegistrySaved(
            client
                .put_federation_registry(&entries, updated_at)
                .await
                .map_err(|e| e.to_string()),
        ),
        BridgeRequest::DeleteIdentity { origins, mode } => BridgeResponse::IdentityDeleted(
            client
                .delete_federation_identity(&origins, mode)
                .await
                .map_err(|e| e.to_string()),
        ),
        BridgeRequest::AdoptSecret(secret) => BridgeResponse::Adopted(
            client
                .change_password(&ChangePasswordRequest::new(secret.expose()))
                .await
                .map(|_| ())
                .map_err(|e| e.to_string()),
        ),
        BridgeRequest::SyncInstances(entries) => {
            match client.set_replicated_instances(&entries).await {
                Ok(_) => BridgeResponse::Synced,
                Err(err) => BridgeResponse::Failed(err.to_string()),
            }
        }
        BridgeRequest::CreateSpace(body) => match client.create_space(&body).await {
            Ok(space) => BridgeResponse::Space(Box::new(space)),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::Explore(query) => match client.explore_spaces(&query, 50, 0).await {
            Ok(page) => BridgeResponse::Explore(page),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::JoinSpace { id, by_request } => {
            if by_request {
                match client.request_join_space(&id, None).await {
                    Ok(()) => BridgeResponse::JoinRequested,
                    Err(err) => BridgeResponse::Failed(err.to_string()),
                }
            } else {
                match client.join_public_space(&id).await {
                    Ok(space) => BridgeResponse::Space(Box::new(space)),
                    Err(err) => BridgeResponse::Failed(err.to_string()),
                }
            }
        }
        BridgeRequest::UpdateProfile(body) => match client.update_me(&body).await {
            Ok(user) => BridgeResponse::Profile(Box::new(user)),
            Err(err) => BridgeResponse::Failed(err.to_string()),
        },
        BridgeRequest::History {
            channel_id,
            kind,
            before,
        } => {
            let cursor = before.map(bs_api::PagingCursor::Before);
            let items = match kind {
                ChannelKind::Space => client
                    .messages(&channel_id, cursor, Some(HISTORY_PAGE))
                    .await
                    .map(|page| HistoryItems::Space(page.messages)),
                ChannelKind::Dm => client
                    .dm_messages(&channel_id, cursor, Some(HISTORY_PAGE))
                    .await
                    .map(|page| HistoryItems::Dm(page.messages)),
            };
            match items {
                Ok(items) => BridgeResponse::History { channel_id, items },
                Err(err) => BridgeResponse::HistoryFailed {
                    channel_id,
                    message: err.to_string(),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bs_instances::ConnectionState;
    use bs_types::{ClientEvent, DmMessageWithUser, ReadyPayload, ServerEvent, TransportPolicy};
    use tokio::sync::mpsc as tokio_mpsc;

    use super::{
        BridgeEvent, BridgeRequest, BridgeResponse, ChannelKind, HISTORY_PAGE, HistoryItems, Phase,
        RegisterOutcome, Secret, Session, SessionParams, normalize_base, remote_login_refused,
        run_session, std_mpsc,
    };

    /// Минимальный `ready`, достаточный для создания кэша.
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    fn ready_payload() -> ReadyPayload {
        serde_json::from_value(serde_json::json!({
            "user": {
                "id": "123456789012345678",
                "username": "alice",
                "displayName": null,
                "avatar": null,
                "banner": null,
                "accentColor": null,
                "avatarColor": "mint",
                "bio": null,
                "status": "online",
                "customStatus": null,
                "isAdmin": false,
                "createdAt": 1_750_000_000_000_i64,
                "homeInstance": null,
                "homeUserId": null,
                "replicatedInstances": []
            }
        }))
        .expect("ready payload должен разбираться")
    }

    /// Пустой runtime нужен только как держатель `Handle`; задачи в этих
    /// тестах не запускаются, сети нет.
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    fn new_bridge() -> Session {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let handle = runtime.handle().clone();
        // Runtime намеренно утекает: `Handle` должен оставаться валидным
        // на всё время теста, а дропать его вручную ради этого не нужно.
        std::mem::forget(runtime);
        Session::new(handle)
    }

    /// Мост с живой сессией: кэш и каналы есть, сети нет.
    fn active_bridge() -> (
        Session,
        tokio_mpsc::UnboundedReceiver<ClientEvent>,
        tokio_mpsc::UnboundedReceiver<BridgeRequest>,
    ) {
        let mut bridge = new_bridge();
        let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        bridge.commands = Some(commands_tx);
        bridge.requests = Some(requests_tx);
        bridge.phase = Phase::Connecting;
        bridge.handle_event(BridgeEvent::Ready {
            payload: Box::new(ready_payload()),
        });
        (bridge, commands_rx, requests_rx)
    }

    fn params() -> SessionParams {
        SessionParams {
            origin: bs_types::Origin::Home,
            base: "http://127.0.0.1:1".to_owned(),
            username: "alice".to_owned(),
            password: zeroize::Zeroizing::new("secret".to_owned()),
            register: None,
            transport: TransportPolicy::Strict,
        }
    }

    #[test]
    fn secrets_never_print() {
        let secret = Secret::new("очень-секретно");
        assert!(!format!("{secret:?}").contains("секретно"));
        let request = BridgeRequest::IssueCredential {
            origin: "https://o.example".into(),
            password: Some(secret),
        };
        assert!(!format!("{request:?}").contains("секретно"));
    }

    #[test]
    fn refusals_say_only_what_is_known() {
        let taken = remote_login_refused(Some(RegisterOutcome::Taken), "x");
        assert!(taken.contains("уже есть аккаунт"));
        // Закрытый инстанс не выдаёт, есть ли там аккаунт: утверждать нельзя.
        let closed = remote_login_refused(Some(RegisterOutcome::Closed), "x");
        assert!(closed.contains("Если аккаунт"));
        assert!(!closed.contains("уже есть аккаунт"));
        assert_eq!(
            remote_login_refused(None, "сеть недоступна"),
            "сеть недоступна"
        );
    }

    #[test]
    fn remote_start_requires_a_waiting_session() {
        let mut session = new_bridge();
        session.mark_connecting("https://orbit.example");
        assert_eq!(session.phase(), Phase::Connecting);
        session.fail("нет");
        assert_eq!(session.phase(), Phase::Idle);
        assert_eq!(session.last_error(), Some("нет"));
    }

    // ─── Жизненный цикл сессии ───────────────────────────────────────────

    #[test]
    fn ready_activates_session_and_clears_errors() {
        let mut bridge = new_bridge();
        bridge.phase = Phase::Connecting;
        bridge.last_error = Some("старое".to_owned());
        bridge.handle_event(BridgeEvent::Ready {
            payload: Box::new(ready_payload()),
        });
        assert_eq!(bridge.phase(), Phase::Active);
        assert_eq!(bridge.last_error(), None);
        assert!(bridge.cache().is_some());
        assert_eq!(bridge.connection(), &ConnectionState::Connected);
        assert!(!bridge.is_busy());
    }

    #[test]
    fn take_ready_is_one_shot() {
        let (mut bridge, _commands, _requests) = active_bridge();
        assert!(bridge.take_ready());
        assert!(!bridge.take_ready());
    }

    #[test]
    fn state_event_updates_connection() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.handle_event(BridgeEvent::State {
            state: ConnectionState::Reconnecting { attempt: 2 },
        });
        assert_eq!(
            bridge.connection(),
            &ConnectionState::Reconnecting { attempt: 2 }
        );
        assert!(!bridge.take_session_lost());
    }

    #[test]
    fn failed_login_returns_to_idle_without_session_loss() {
        let mut bridge = new_bridge();
        bridge.phase = Phase::Connecting;
        bridge.handle_event(BridgeEvent::Error {
            message: "неверный пароль".to_owned(),
        });
        bridge.handle_event(BridgeEvent::Ended);
        assert_eq!(bridge.phase(), Phase::Idle);
        assert!(
            !bridge.take_session_lost(),
            "пользователь остаётся на форме"
        );
        assert_eq!(bridge.last_error(), Some("неверный пароль"));
    }

    #[test]
    fn dropped_active_session_is_reported_as_lost() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.handle_event(BridgeEvent::Disconnected);
        assert_eq!(bridge.connection(), &ConnectionState::Stopped);
        bridge.handle_event(BridgeEvent::Ended);
        assert_eq!(bridge.phase(), Phase::Idle);
        assert!(bridge.take_session_lost());
        assert!(!bridge.take_session_lost(), "флаг одноразовый");
        assert_eq!(bridge.last_error(), Some("соединение закрыто"));
        assert!(bridge.cache().is_none(), "кэш чужой сессии не остаётся");
    }

    #[test]
    fn ended_clears_all_session_state() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        bridge.friends = Vec::new();
        bridge.notice = Some("отказ".to_owned());
        bridge.handle_event(BridgeEvent::Ended);
        assert!(bridge.cache().is_none());
        assert!(bridge.history_state("dm1").is_none());
        assert!(bridge.commands.is_none() && bridge.requests.is_none());
        assert!(bridge.notice().is_none());
        assert_eq!(bridge.connection(), &ConnectionState::Connecting);
    }

    // ─── Одна сессия за раз ──────────────────────────────────────────────

    #[test]
    fn start_login_rejects_empty_fields() {
        let cases = [
            ("", "user", "pass"),
            ("   ", "user", "pass"),
            ("https://example.com", "", "pass"),
            ("https://example.com", "user", ""),
        ];
        for (base, user, pass) in cases {
            let mut bridge = new_bridge();
            bridge.start_login(base, user, pass, TransportPolicy::Strict);
            assert_eq!(bridge.phase(), Phase::Idle, "base={base:?} user={user:?}");
            assert_eq!(bridge.last_error(), Some("заполните адрес, логин и пароль"));
            assert!(bridge.events.is_none() && bridge.commands.is_none());
        }
    }

    #[test]
    fn start_login_accepts_host_without_scheme() {
        let mut bridge = new_bridge();
        bridge.start_login("backspace.example", "user", "pass", TransportPolicy::Strict);
        assert_eq!(bridge.phase(), Phase::Connecting);
        assert!(bridge.is_busy());
        assert_eq!(bridge.last_error(), None);
        assert!(bridge.events.is_some() && bridge.commands.is_some());
        assert!(bridge.session.is_some());
    }

    #[test]
    fn start_login_refuses_plaintext_http_without_consent() {
        for base in ["http://chat.example.com", "http://192.168.1.10:3005"] {
            let mut bridge = new_bridge();
            bridge.start_login(base, "user", "pass", TransportPolicy::Strict);
            assert_eq!(bridge.phase(), Phase::Idle, "{base}");
            assert!(
                bridge
                    .last_error()
                    .is_some_and(|e| e.contains("без шифрования")),
                "{base}: {:?}",
                bridge.last_error()
            );
            assert!(bridge.session.is_none(), "пароль не должен уйти в сеть");
        }
    }

    #[test]
    fn start_login_allows_loopback_and_explicit_consent() {
        let mut bridge = new_bridge();
        bridge.start_login("http://localhost:3005", "u", "p", TransportPolicy::Strict);
        assert_eq!(bridge.phase(), Phase::Connecting);

        let mut bridge = new_bridge();
        bridge.start_login(
            "http://192.168.1.10:3005",
            "u",
            "p",
            TransportPolicy::AllowInsecure,
        );
        assert_eq!(bridge.phase(), Phase::Connecting);
    }

    #[test]
    fn second_login_is_refused_in_every_busy_phase() {
        for phase in [Phase::Connecting, Phase::Active, Phase::Closing] {
            let mut bridge = new_bridge();
            bridge.phase = phase;
            bridge.start_login("https://other.example", "u", "p", TransportPolicy::Strict);
            assert_eq!(bridge.phase(), phase, "{phase:?}");
            assert_eq!(
                bridge.last_error(),
                Some("Сначала выйдите из текущего инстанса")
            );
            assert!(bridge.events.is_none(), "второй сессии не создано");
            assert!(bridge.session.is_none());
        }
    }

    #[test]
    fn login_is_possible_again_only_after_the_old_session_ended() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.logout();
        assert_eq!(bridge.phase(), Phase::Closing);
        assert!(bridge.is_busy());
        bridge.start_login("https://other.example", "u", "p", TransportPolicy::Strict);
        assert_eq!(bridge.phase(), Phase::Closing, "старое ещё закрывается");

        bridge.handle_event(BridgeEvent::Ended);
        assert_eq!(bridge.phase(), Phase::Idle);
        assert!(
            bridge.take_session_lost(),
            "выход возвращает на форму входа"
        );
        bridge.start_login("https://other.example", "u", "p", TransportPolicy::Strict);
        assert_eq!(bridge.phase(), Phase::Connecting);
    }

    #[test]
    fn logout_closes_channels_and_aborts_the_task() {
        let mut bridge = new_bridge();
        bridge.start_login("backspace.example", "user", "pass", TransportPolicy::Strict);
        bridge.logout();
        assert_eq!(bridge.phase(), Phase::Closing);
        assert!(bridge.commands.is_none() && bridge.requests.is_none());
        // Команды после выхода никуда не уходят.
        bridge.send_message(ChannelKind::Space, "c".to_owned(), "x".to_owned(), None);
        assert!(!bridge.request(BridgeRequest::Friends));
        // Повторный выход безвреден.
        bridge.logout();
        assert_eq!(bridge.phase(), Phase::Closing);
    }

    #[test]
    fn logout_without_session_is_noop() {
        let mut bridge = new_bridge();
        bridge.logout();
        assert_eq!(bridge.phase(), Phase::Idle);
        assert!(!bridge.take_session_lost());
    }

    #[test]
    fn normalize_base_adds_https_scheme() {
        assert_eq!(
            normalize_base("example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            normalize_base("  example.com  ").as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn normalize_base_keeps_explicit_scheme() {
        assert_eq!(
            normalize_base("http://localhost:3000").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            normalize_base("https://example.com").as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn normalize_base_rejects_empty() {
        assert_eq!(normalize_base(""), None);
        assert_eq!(normalize_base("   "), None);
    }

    // ─── Фоновая сессия ──────────────────────────────────────────────────

    /// Ошибка HTTP-логина доезжает до GUI, затем приходит `Ended`.
    #[tokio::test]
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    async fn run_session_reports_http_login_error_then_ends() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/auth/login"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": "Invalid credentials",
                "code": "invalid_credentials"
            })))
            .mount(&server)
            .await;

        let (events_tx, events_rx) = std_mpsc::channel();
        let (_commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (_requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, _responses_rx) = std_mpsc::channel();

        let mut p = params();
        p.base = server.uri();
        run_session(p, events_tx, commands_rx, requests_rx, responses_tx).await;

        let event = events_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("run_session должен прислать событие об ошибке");
        let BridgeEvent::LoginFailed { message, kind } = event else {
            unreachable!("ожидалась ошибка входа, пришло {event:?}");
        };
        assert_eq!(
            kind,
            super::FailureKind::Auth,
            "отказ сервера не сетевая ошибка"
        );
        assert!(message.contains("401"), "текст ошибки: {message}");
        assert!(message.contains("Invalid credentials"), "{message}");
        assert!(
            matches!(events_rx.try_recv(), Ok(BridgeEvent::Ended)),
            "последним событием сессии всегда `Ended`"
        );
        assert!(events_rx.try_recv().is_err());
    }

    /// Полный happy-path: логин, `auth`, `ready`, команда доходит до сервера.
    #[tokio::test]
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    async fn run_session_happy_path_relays_commands() {
        let ready = ServerEvent::Ready(Box::new(ready_payload()));
        let (base, mut server_events) = crate::fake_instance::spawn(ready).await;

        let (events_tx, events_rx) = std_mpsc::channel();
        let (commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (_requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, _responses_rx) = std_mpsc::channel();

        let mut p = params();
        p.base = base;
        let session = tokio::spawn(run_session(
            p,
            events_tx,
            commands_rx,
            requests_rx,
            responses_tx,
        ));

        let event = tokio::task::spawn_blocking({
            let events_rx = events_rx;
            move || events_rx.recv_timeout(Duration::from_secs(10))
        })
        .await
        .expect("join");
        assert!(
            matches!(&event, Ok(BridgeEvent::Ready { .. })),
            "ожидался ready, пришло {event:?}"
        );

        commands_tx
            .send(ClientEvent::MessageCreate {
                channel_id: "chan-1".to_owned(),
                content: "привет".to_owned(),
                reply_to_id: None,
            })
            .expect("команда должна уйти в канал");

        // Heartbeat шлёт `ping` в произвольный момент — пропускаем его.
        let received = loop {
            let event = tokio::time::timeout(Duration::from_secs(10), server_events.recv())
                .await
                .expect("сервер должен получить событие")
                .expect("канал событий сервера не должен закрыться");
            if matches!(event, ClientEvent::Ping) {
                continue;
            }
            break event;
        };
        assert!(
            matches!(
                &received,
                ClientEvent::MessageCreate { channel_id, content, .. }
                    if channel_id == "chan-1" && content == "привет"
            ),
            "сервер получил {received:?}"
        );

        drop(commands_tx);
        tokio::time::timeout(Duration::from_secs(10), session)
            .await
            .expect("run_session должен завершиться")
            .expect("задача не должна паниковать");
    }

    /// `abort()` (так работает выход) всё равно сообщает `Ended`:
    /// мост не зависнет в `Closing`, а соединение не останется жить.
    #[tokio::test]
    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    async fn aborted_session_still_reports_ended() {
        let ready = ServerEvent::Ready(Box::new(ready_payload()));
        let (base, _server_events) = crate::fake_instance::spawn(ready).await;

        let (events_tx, events_rx) = std_mpsc::channel();
        let (_commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
        let (_requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
        let (responses_tx, _responses_rx) = std_mpsc::channel();

        let mut p = params();
        p.base = base;
        let session = tokio::spawn(run_session(
            p,
            events_tx,
            commands_rx,
            requests_rx,
            responses_tx,
        ));

        let events_rx = tokio::task::spawn_blocking(move || {
            let first = events_rx.recv_timeout(Duration::from_secs(10));
            assert!(matches!(first, Ok(BridgeEvent::Ready { .. })), "{first:?}");
            events_rx
        })
        .await
        .expect("join");

        session.abort();
        let _ = session.await;

        let events_rx = tokio::task::spawn_blocking(move || {
            // Допускаем промежуточные `State`, но `Ended` обязан прийти.
            loop {
                match events_rx.recv_timeout(Duration::from_secs(10)) {
                    Ok(BridgeEvent::Ended) => return true,
                    Ok(_) => {}
                    Err(_) => return false,
                }
            }
        })
        .await
        .expect("join");
        assert!(events_rx, "после abort должно прийти `Ended`");
    }

    // ─── Сообщения ЛС и каналов ──────────────────────────────────────────

    #[test]
    fn send_without_session_is_noop() {
        let bridge = new_bridge();
        bridge.send_message(
            ChannelKind::Space,
            "chan-1".to_owned(),
            "hello".to_owned(),
            None,
        );
        assert!(bridge.commands.is_none());
    }

    #[test]
    fn dm_send_uses_dm_message_create() {
        let (bridge, mut commands, _requests) = active_bridge();
        bridge.send_message(ChannelKind::Dm, "dm1".to_owned(), "привет".to_owned(), None);
        match commands.try_recv() {
            Ok(ClientEvent::DmMessageCreate {
                dm_channel_id,
                content,
                attachments,
                reply_to_id,
            }) => {
                assert_eq!(dm_channel_id, "dm1");
                assert_eq!(content.as_deref(), Some("привет"));
                assert!(attachments.is_none() && reply_to_id.is_none());
            }
            other => panic!("ожидался dm_message_create, пришло {other:?}"),
        }
    }

    #[test]
    fn dm_edit_and_delete_use_dm_events() {
        let (bridge, mut commands, _requests) = active_bridge();
        bridge.edit_message(ChannelKind::Dm, "m1".to_owned(), "правка".to_owned());
        bridge.delete_message(ChannelKind::Dm, "m1".to_owned());
        assert!(matches!(
            commands.try_recv(),
            Ok(ClientEvent::DmMessageEdit { .. })
        ));
        assert!(matches!(
            commands.try_recv(),
            Ok(ClientEvent::DmMessageDelete { .. })
        ));
    }

    #[test]
    fn space_send_keeps_message_create() {
        let (bridge, mut commands, _requests) = active_bridge();
        bridge.send_message(ChannelKind::Space, "c1".to_owned(), "hi".to_owned(), None);
        assert!(matches!(
            commands.try_recv(),
            Ok(ClientEvent::MessageCreate { .. })
        ));
    }

    #[test]
    fn server_error_becomes_a_visible_notice() {
        let (mut bridge, _commands, _requests) = active_bridge();
        let error: ServerEvent = serde_json::from_value(serde_json::json!({
            "type": "error", "message": "Too many messages", "code": "rate_limited"
        }))
        .unwrap_or_else(|err| panic!("error разбирается: {err}"));
        bridge.handle_event(BridgeEvent::Server {
            event: Box::new(error),
        });
        assert_eq!(
            bridge.notice(),
            Some("Слишком часто: подождите несколько секунд")
        );
        bridge.dismiss_notice();
        assert_eq!(bridge.notice(), None);
    }

    // ─── История ─────────────────────────────────────────────────────────

    #[allow(clippy::expect_used, reason = "в тестах паника допустима")]
    fn dm_msg(id: &str, created_at: i64) -> DmMessageWithUser {
        serde_json::from_value(serde_json::json!({
            "id": id, "dmChannelId": "dm1", "userId": "u2", "content": "привет",
            "createdAt": created_at,
            "user": {
                "id": "u2", "username": "bob", "displayName": null, "avatar": null,
                "banner": null, "accentColor": null, "avatarColor": null, "bio": null,
                "status": "online", "customStatus": null, "isAdmin": false,
                "createdAt": 1, "homeInstance": null, "homeUserId": null,
                "replicatedInstances": []
            },
            "attachments": [], "embeds": [], "reactions": []
        }))
        .expect("dm-сообщение разбирается")
    }

    fn page_of(count: usize) -> HistoryItems {
        // Сообщения 1..=count, id возрастают вместе со временем.
        HistoryItems::Dm(
            (1..=count)
                .map(|n| dm_msg(&format!("{n:03}"), i64::try_from(n).unwrap_or(0)))
                .collect(),
        )
    }

    #[test]
    fn ensure_history_requests_once() {
        let (mut bridge, _commands, mut requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        assert!(matches!(
            requests.try_recv(),
            Ok(BridgeRequest::History {
                before: None,
                kind: ChannelKind::Dm,
                ..
            })
        ));
        assert!(requests.try_recv().is_err(), "повторного запроса нет");
        assert!(
            bridge
                .history_state("dm1")
                .is_some_and(|state| state.loading)
        );
    }

    #[test]
    fn ensure_history_without_session_leaves_no_state() {
        let mut bridge = new_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        assert!(bridge.history_state("dm1").is_none());
    }

    #[test]
    fn full_page_keeps_history_open_and_short_page_closes_it() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        bridge.handle_response(BridgeResponse::History {
            channel_id: "dm1".to_owned(),
            items: page_of(usize::from(HISTORY_PAGE)),
        });
        let state = bridge.history_state("dm1").cloned();
        assert_eq!(
            state.map(|s| (s.loading, s.exhausted)),
            Some((false, false))
        );
        assert_eq!(
            bridge.cache().map(|c| c.dm_messages("dm1").len()),
            Some(usize::from(HISTORY_PAGE))
        );

        bridge.handle_response(BridgeResponse::History {
            channel_id: "dm1".to_owned(),
            items: HistoryItems::Dm(vec![dm_msg("000", 0)]),
        });
        assert!(bridge.history_state("dm1").is_some_and(|s| s.exhausted));
    }

    #[test]
    fn load_older_uses_oldest_message_as_cursor() {
        let (mut bridge, _commands, mut requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        let _ = requests.try_recv();
        bridge.handle_response(BridgeResponse::History {
            channel_id: "dm1".to_owned(),
            items: page_of(usize::from(HISTORY_PAGE)),
        });
        bridge.load_older(ChannelKind::Dm, "dm1");
        match requests.try_recv() {
            Ok(BridgeRequest::History { before, .. }) => {
                assert_eq!(before.as_deref(), Some("001"));
            }
            other => panic!("ожидался запрос истории, пришло {other:?}"),
        }
        // Пока страница в пути, второй запрос не уходит.
        bridge.load_older(ChannelKind::Dm, "dm1");
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn failed_history_is_not_auto_retried() {
        let ok = super::HistoryState::default();
        assert!(ok.can_auto_load());
        for state in [
            super::HistoryState {
                loading: true,
                ..Default::default()
            },
            super::HistoryState {
                exhausted: true,
                ..Default::default()
            },
            super::HistoryState {
                error: Some("429".to_owned()),
                ..Default::default()
            },
        ] {
            assert!(!state.can_auto_load(), "{state:?}");
        }
    }

    #[test]
    fn failed_history_can_be_retried() {
        let (mut bridge, _commands, mut requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        let _ = requests.try_recv();
        bridge.handle_response(BridgeResponse::HistoryFailed {
            channel_id: "dm1".to_owned(),
            message: "нет сети".to_owned(),
        });
        let state = bridge.history_state("dm1").cloned();
        assert_eq!(state.and_then(|s| s.error).as_deref(), Some("нет сети"));
        bridge.load_older(ChannelKind::Dm, "dm1");
        assert!(matches!(
            requests.try_recv(),
            Ok(BridgeRequest::History { before: None, .. })
        ));
    }

    #[test]
    fn ready_after_reconnect_resets_history() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.ensure_history(ChannelKind::Dm, "dm1");
        bridge.handle_response(BridgeResponse::History {
            channel_id: "dm1".to_owned(),
            items: page_of(3),
        });
        // Повторный `ready` приходит как обычное серверное событие.
        bridge.handle_event(BridgeEvent::Server {
            event: Box::new(ServerEvent::Ready(Box::new(ready_payload()))),
        });
        assert!(bridge.history_state("dm1").is_none(), "состояние сброшено");
        assert_eq!(
            bridge.cache().map(|c| c.dm_messages("dm1").len()),
            Some(0),
            "кэш пересобран без сообщений"
        );
    }

    #[test]
    fn late_history_response_after_reset_is_ignored() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.handle_response(BridgeResponse::History {
            channel_id: "dm1".to_owned(),
            items: page_of(3),
        });
        assert_eq!(bridge.cache().map(|c| c.dm_messages("dm1").len()), Some(0));
    }

    #[test]
    fn joined_space_is_added_to_cache_and_reported_once() {
        let (mut bridge, _commands, _requests) = active_bridge();
        let space: bs_types::SpaceWithChannelsAndMembers =
            serde_json::from_value(serde_json::json!({
                "id": "s77", "name": "New", "ownerId": "1", "visibility": "private",
                "directoryListed": false, "createdAt": 1, "channels": [], "members": []
            }))
            .unwrap_or_else(|err| panic!("пространство разбирается: {err}"));
        bridge.handle_response(BridgeResponse::Space(Box::new(space)));
        assert!(bridge.cache().is_some_and(|c| c.space("s77").is_some()));
        assert_eq!(bridge.take_opened_space().as_deref(), Some("s77"));
        assert_eq!(bridge.take_opened_space(), None);
    }

    #[test]
    fn explore_response_ends_loading_and_failure_too() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.load_explore("rust");
        assert!(bridge.explore_loading());
        let page = bs_api::ExplorePage {
            spaces: Vec::new(),
            total: 0,
            discovery_enabled: true,
        };
        bridge.handle_response(BridgeResponse::Explore(page));
        assert!(!bridge.explore_loading());
        assert!(bridge.explore().is_some());
        bridge.load_explore("");
        bridge.handle_response(BridgeResponse::Failed("нет сети".into()));
        assert!(!bridge.explore_loading());
        assert_eq!(bridge.social_error(), Some("нет сети"));
    }

    #[test]
    fn join_request_sets_one_shot_info() {
        let (mut bridge, _commands, _requests) = active_bridge();
        bridge.handle_response(BridgeResponse::JoinRequested);
        assert_eq!(bridge.take_info().as_deref(), Some("Заявка отправлена"));
        assert_eq!(bridge.take_info(), None);
    }

    #[test]
    fn opened_dm_is_added_to_cache() {
        let (mut bridge, _commands, _requests) = active_bridge();
        let channel: bs_types::DmChannel = serde_json::from_value(serde_json::json!({
            "id": "dm9", "federatedId": null, "ownerId": null,
            "ownerHomeUserId": null, "ownerHomeInstance": null, "createdAt": 1,
            "members": [], "metadataUpdatedAt": 2
        }))
        .unwrap_or_else(|err| panic!("dm разбирается: {err}"));
        bridge.handle_response(BridgeResponse::DmChannel(Box::new(channel)));
        assert!(
            bridge
                .cache()
                .is_some_and(|c| c.dm_channel("dm9").is_some())
        );
        assert_eq!(bridge.take_opened_dm().as_deref(), Some("dm9"));
    }
}
