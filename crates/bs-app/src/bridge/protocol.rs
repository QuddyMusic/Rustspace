//! Запросы, ответы и события моста.

#![allow(
    clippy::wildcard_imports,
    reason = "части одного модуля, разнесённые по файлам"
)]

use super::*;

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
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Space(items) => items.len(),
            Self::Dm(items) => items.len(),
        }
    }
}

/// Одно найденное сообщение (общий вид для каналов и личных каналов).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchHit {
    pub(crate) id: String,
    /// Отображаемое имя автора.
    pub(crate) author: String,
    pub(crate) created_at: i64,
    pub(crate) content: String,
    /// Число вложений.
    pub(crate) attachments: usize,
}

/// Состояние поиска по сообщениям текущего канала.
#[derive(Debug, Default)]
pub(crate) struct SearchState {
    /// Номер поиска: ответы от прежних поисков отбрасываются.
    pub(super) seq: u64,
    /// Запрос ушёл, ответа нет.
    pub(crate) loading: bool,
    pub(crate) hits: Vec<SearchHit>,
    /// Сколько всего нашёл сервер.
    pub(crate) total: i64,
    pub(crate) error: Option<String>,
}

impl SearchState {
    /// Сколько результатов ещё не показано.
    pub(crate) fn remaining(&self) -> usize {
        usize::try_from(self.total)
            .unwrap_or(0)
            .saturating_sub(self.hits.len())
    }
}

/// Ход скачивания файла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DownloadState {
    /// Принято байт и общий размер (если сервер его сообщил).
    Progress { got: u64, total: Option<u64> },
    /// Файл целиком записан на диск.
    Done,
    /// Не удалось (текст для пользователя).
    Failed(String),
}

/// Через сколько принятых байт сообщаем о ходе скачивания.
pub(super) const DOWNLOAD_STEP: u64 = 128 * 1024;

/// Текст ошибки скачивания для пользователя.
pub(super) fn download_message(err: &bs_api::ApiError) -> String {
    match err {
        bs_api::ApiError::Io(io) => crate::tf!("не удалось записать файл ({io})", io = io),
        other if other.status().is_some_and(|s| s.as_u16() == 404) => {
            tr("файл больше не доступен на сервере").to_owned()
        }
        other => other.to_string(),
    }
}

/// Размер страницы результатов поиска.
pub(super) const SEARCH_PAGE: u16 = 25;
/// Сколько сообщений просим вокруг найденного.
pub(super) const AROUND_PAGE: u16 = 50;

pub(super) fn display_name(user: &bs_types::User) -> String {
    user.display_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| user.username.clone())
}

pub(super) fn hits_of_space(list: Vec<MessageWithUser>) -> Vec<SearchHit> {
    list.into_iter()
        .map(|m| SearchHit {
            author: display_name(&m.user),
            id: m.message.id.clone(),
            created_at: m.message.created_at,
            content: m.message.content.unwrap_or_default(),
            attachments: m.attachments.len(),
        })
        .collect()
}

pub(super) fn hits_of_dm(list: Vec<DmMessageWithUser>) -> Vec<SearchHit> {
    list.into_iter()
        .map(|m| SearchHit {
            author: display_name(&m.user),
            id: m.message.id.clone(),
            created_at: m.message.created_at,
            content: m.message.content.unwrap_or_default(),
            attachments: m.attachments.len(),
        })
        .collect()
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
    /// Настройки уведомлений этого инстанса.
    NotificationSettings,
    /// Опубликовать свой открытый ключ шифрования в справочнике сервера.
    E2eePublish(String),
    /// Узнать в справочнике ключ собеседника по переписке.
    E2eePeerKey {
        /// Переписка.
        dm_channel_id: String,
        /// Собеседник.
        user_id: String,
    },
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
    /// Общий каталог пространств других инстансов (через свой инстанс).
    Directory(String),
    /// Сведения об инстансе (`GET /api/instance/info`): версия, ссылки.
    InstanceInfo,
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
    /// Токен LiveKit для входа в голосовой канал пространства.
    VoiceToken(String),
    /// Понизить битрейт голоса звонка (`Some`) или вернуть битрейт инстанса.
    SetVoiceBitrate {
        /// Голосовой канал или переписка.
        target: String,
        /// Кбит/с.
        kbps: Option<u32>,
    },
    /// Токен LiveKit для DM-звонка.
    DmVoiceToken(String),
    /// Код приглашения в пространство.
    SpaceInvite(String),
    /// Отправить другу приглашение в пространство в ЛС.
    InviteFriend {
        /// Друг (для сопоставления ответа).
        friend_id: String,
        /// Тело запроса.
        request: bs_api::SpaceInviteRequest,
    },
    /// Вступить по коду (с адресом инстанса, если он указан в ссылке).
    JoinByCode(String),
    /// Выйти из пространства.
    LeaveSpace {
        /// Пространство.
        space_id: String,
        /// Свой id.
        user_id: String,
    },
    /// Действие над групповой беседой.
    GroupAdmin {
        /// Беседа (для создания — пустая строка).
        dm_id: String,
        /// Что сделать.
        op: GroupOp,
    },
    /// Действие администратора над пространством.
    SpaceAdmin {
        /// Пространство.
        space_id: String,
        /// Что сделать.
        op: SpaceOp,
    },
    /// Создать канал.
    CreateChannel {
        /// Пространство.
        space_id: String,
        /// Параметры канала.
        channel: bs_api::NewChannel,
    },
    /// Создать категорию.
    CreateCategory {
        /// Пространство.
        space_id: String,
        /// Название.
        name: String,
    },
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
        /// Сразу положить в этот свой пак.
        pack: Option<String>,
    },
    /// Стикерпаки Rustspace.
    StickerPacks(PackOp),
    /// Удалить личный стикер.
    DeleteSticker(String),
    /// «Избранное», закрепы, закладки.
    Extras(super::extras::ExtrasOp),
    /// Отправить сообщение с карточкой в канал пространства.
    SendCards {
        /// Канал.
        channel_id: String,
        /// Текст над карточкой.
        content: Option<String>,
        /// Карточка.
        card: bs_api::CardDraft,
    },
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
    /// Поиск по сообщениям канала.
    Search {
        channel_id: String,
        kind: ChannelKind,
        query: bs_api::MessageSearchQuery,
        /// Номер поиска (см. [`SearchState`]).
        seq: u64,
    },
    /// Скачать файл инстанса на диск (потоком, с прогрессом).
    Download {
        id: u64,
        path: bs_api::AssetPath,
        dest: std::path::PathBuf,
    },
    /// Окно сообщений вокруг найденного (переход к результату).
    Around {
        channel_id: String,
        kind: ChannelKind,
        message_id: String,
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
    /// Двухфакторная защита своего аккаунта.
    TwoFactor(TwoFactorOp),
    /// Сменить свой пароль.
    ChangePassword {
        /// Текущий (для домашнего аккаунта обязателен).
        current: Option<Secret>,
        /// Новый.
        new: Secret,
    },
    /// Инвайт-ссылки: активные или архивные.
    LoadInvites {
        /// Показать архив.
        archived: bool,
    },
    /// Действие с инвайтом.
    Invite(InviteOp),
    /// Пиры и запросы на одобрение.
    LoadPeers,
    /// Действие с пиром.
    Peer(PeerOp),
    /// Ограничения стриминга.
    LoadStreaming,
    /// Сохранить ограничения стриминга.
    SaveStreaming(bs_api::StreamingPatch),
    /// Битрейт голоса инстанса (Rustspace).
    LoadVoiceBitrate,
    /// Задать битрейт голоса инстанса, кбит/с.
    SaveVoiceBitrate(u32),
    /// Состояние телеметрии.
    LoadTelemetry,
    /// Включить или выключить телеметрию.
    SetTelemetry(bool),
    /// Что отправила бы телеметрия.
    LoadTelemetryPreview,
    /// Состояние обновлений; `true` — проверить заново.
    LoadUpdates(bool),
    /// Обновить сервер (последний выпуск).
    UpdateServer,
    /// Удалить свой аккаунт.
    DeleteAccount {
        /// Пароль.
        password: Secret,
        /// Логин для подтверждения.
        username: String,
    },
}

/// Действие администратора с инвайтом.
#[derive(Debug, Clone)]
pub(crate) enum InviteOp {
    /// Создать.
    Create(bs_api::NewInvite),
    /// Изменить.
    Update(String, bs_api::InvitePatch),
    /// Отозвать.
    Revoke(String),
    /// Восстановить.
    Reinstate(String, bs_api::InvitePatch),
    /// Удалить.
    Delete(String),
    /// Кто воспользовался.
    Redemptions(String),
}

/// Действие администратора с пиром.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeerOp {
    /// Перепроверить связь.
    Recheck(String),
    /// Сменить секрет.
    Rotate(String),
    /// Отозвать.
    Revoke(String),
    /// Сбросить.
    Reset(String),
    /// Удалить навсегда.
    Delete(String),
    /// Интервал автоматической ротации, дней.
    Interval(String, u32),
    /// Начать пиринг с адресом.
    Initiate(String),
    /// Одобрить или отклонить запрос.
    Answer(String, bool),
}

/// Действие с двухфакторной защитой.
#[derive(Debug, Clone)]
pub(crate) enum TwoFactorOp {
    /// Узнать состояние.
    Status,
    /// Начать настройку (нужен пароль).
    Setup(Secret),
    /// Подтвердить настройку кодом из приложения.
    Enable(String),
    /// Снять защиту: пароль и код (из приложения или запасной).
    Disable(Secret, String),
    /// Выдать новые запасные коды: пароль и код из приложения.
    NewCodes(Secret, String),
}

/// Что известно о двухфакторной защите после запроса.
#[derive(Debug, Default)]
pub(crate) struct TwoFactorView {
    /// Состояние; `None` — сервер защиту не поддерживает.
    pub(crate) status: Option<bs_api::TwoFactorStatus>,
    /// Секрет, если настройка только что начата.
    pub(crate) setup: Option<bs_api::TwoFactorSetup>,
    /// Запасные коды, если они только что выданы.
    pub(crate) codes: Option<Vec<String>>,
}

/// Ответы на [`AdminRequest`] и загрузку картинки профиля.
#[derive(Debug)]
pub(crate) enum AdminEvent {
    /// Двухфакторная защита: состояние или текст ошибки.
    TwoFactor(Result<TwoFactorView, String>),
    /// Настройки инстанса.
    Settings(Result<bs_api::InstanceSettings, String>),
    /// Итог сохранения настроек.
    SettingsSaved(Result<bs_api::InstanceSettings, String>),
    /// Страница пользователей.
    Users(Result<bs_api::AdminUserPage, String>),
    /// Сводка по хранилищу.
    Storage(Result<bs_api::StorageStats, String>),
    /// Список инвайтов: архив ли и сами ссылки.
    Invites(Result<(bool, Vec<bs_api::Invite>), String>),
    /// Итог действия с инвайтом.
    InviteDone(Result<String, String>),
    /// Кто воспользовался инвайтом.
    Redemptions {
        /// Инвайт.
        id: String,
        /// Список или ошибка.
        result: Result<Vec<bs_api::Redemption>, String>,
    },
    /// Пиры и запросы на одобрение.
    Peers(Result<(Vec<bs_api::Peer>, Vec<bs_api::ApprovalRequest>), String>),
    /// Итог действия с пиром: сообщение или текст ошибки.
    PeerDone(Result<String, String>),
    /// Ограничения стриминга (прочитаны или сохранены).
    Streaming(Result<bs_api::StreamingLimits, String>),
    /// Битрейт голоса инстанса (прочитан или сохранён).
    VoiceBitrate(Result<bs_api::InstanceVoiceBitrate, String>),
    /// Состояние телеметрии.
    Telemetry(Result<bs_api::TelemetryStatus, String>),
    /// Предпросмотр телеметрии (JSON текстом).
    TelemetryPreview(Result<String, String>),
    /// Состояние обновлений.
    Updates(Result<Box<bs_api::UpdateStatus>, String>),
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

/// Событие DM-звонка от сервера.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CallEvent {
    /// Нам звонят.
    Incoming {
        dm_channel_id: Option<String>,
        federated_call_id: Option<String>,
        caller_id: String,
        caller_name: String,
        livekit_url: Option<String>,
        livekit_token: Option<String>,
    },
    /// Собеседник принял звонок.
    Accepted {
        dm_channel_id: Option<String>,
        federated_call_id: Option<String>,
    },
    /// Собеседник отклонил звонок.
    Rejected {
        dm_channel_id: Option<String>,
        federated_call_id: Option<String>,
    },
    /// Звонок завершён.
    Ended {
        dm_channel_id: Option<String>,
        federated_call_id: Option<String>,
    },
    /// Звонок не удалось доставить.
    Undeliverable {
        dm_channel_id: Option<String>,
        federated_call_id: String,
        terminal: bool,
    },
}

/// Что сделать с групповой беседой.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GroupOp {
    /// Создать группу из этих людей.
    Create(Vec<bs_api::GroupMember>),
    /// Новое название (`None` — сбросить).
    Rename(Option<String>),
    /// Добавить друга.
    Add(String),
    /// Убрать участника (только владелец).
    Remove(String),
    /// Покинуть группу.
    Leave,
    /// Передать права владельца.
    Transfer(String),
    /// Новая иконка (байты картинки) или `None` — убрать.
    Icon(Option<Vec<u8>>),
}

/// Итог действия над группой.
#[derive(Debug, Clone)]
pub(crate) struct GroupEvent {
    /// Беседа (у созданной — её новый id).
    pub(crate) dm_id: String,
    /// Что делали.
    pub(crate) op: GroupOp,
    /// Успех либо текст ошибки.
    pub(crate) result: Result<(), String>,
}

/// Административное действие над пространством.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SpaceOp {
    /// Изменить поля.
    Update(bs_api::SpaceUpdate),
    /// Удалить пространство.
    Delete,
    /// Исключить участника.
    Kick(String),
    /// Заблокировать участника с причиной.
    Ban(String, Option<String>),
    /// Снять блокировку.
    Unban(String),
    /// Передать владение.
    Transfer(String),
    /// Загрузить список блокировок.
    ListBans,
    /// Создать роль.
    CreateRole(bs_api::NewRole),
    /// Изменить или переместить роль.
    UpdateRole(String, bs_api::RoleUpdate),
    /// Удалить роль.
    DeleteRole(String),
    /// Изменить настройку уведомлений пространства (`None`) или канала.
    Notify(Option<String>, bs_types::UpdateNotificationSettingRequest),
    /// Имя и тема канала.
    UpdateChannel(String, bs_api::ChannelUpdate),
    /// Удалить канал.
    DeleteChannel(String),
    /// Переименовать категорию.
    RenameCategory(String, String),
    /// Удалить категорию.
    DeleteCategory(String),
    /// Загрузить переопределения прав.
    ListOverrides(bs_api::OverrideScope),
    /// Канал «18+» (сервер Rustspace).
    SetNsfw(String, bool),
    /// Загрузить мосты канала.
    ListBridges(String),
    /// Связать канал с внешним чатом: канал, сеть, чат.
    AddBridge(String, String, String),
    /// Развязать: связь.
    RemoveBridge(String),
    /// Загрузить вебхуки канала.
    ListWebhooks(String),
    /// Создать вебхук: канал, имя.
    CreateWebhook(String, String),
    /// Удалить вебхук.
    DeleteWebhook(String),
    /// Создать или заменить переопределение прав.
    PutOverride(bs_api::OverrideScope, bs_api::PermissionOverride),
    /// Убрать переопределение прав.
    DeleteOverride(bs_api::OverrideScope, bs_types::OverrideTargetType, String),
    /// Загрузить значок (`false`) или баннер (`true`): имя файла и содержимое.
    UploadImage(bool, String, Vec<u8>),
    /// Загрузить ожидающие заявки на вступление.
    ListJoinRequests,
    /// Решить заявку: (идентификатор, принять).
    DecideRequest(String, bool),
    /// Выдать роль участнику: (пользователь, роль).
    AddMemberRole(String, String),
    /// Снять роль с участника: (пользователь, роль).
    RemoveMemberRole(String, String),
}

/// Итог действия над пространством.
#[derive(Debug)]
pub(crate) enum SpaceEvent {
    /// Итог административного действия; после успеха сервер присылает
    /// свежий снимок пространства (кроме удаления и списка блокировок).
    Admin {
        /// Пространство.
        space_id: String,
        /// Что делали.
        op: SpaceOp,
        /// Итог.
        result: Result<(), String>,
        /// Обновлённое пространство.
        space: Option<Box<bs_types::SpaceWithChannelsAndMembers>>,
    },
    /// Картинка загружена: имя файла на сервере либо текст ошибки.
    Uploaded {
        /// Пространство.
        space_id: String,
        /// Баннер, а не значок.
        banner: bool,
        /// Имя файла или ошибка.
        result: Result<String, String>,
    },
    /// Переопределения прав канала или категории.
    Overrides {
        /// Пространство.
        space_id: String,
        /// Канал или категория.
        scope: bs_api::OverrideScope,
        /// Список или текст ошибки.
        result: Result<Vec<bs_api::PermissionOverride>, String>,
    },
    /// Вебхуки канала.
    Webhooks {
        /// Пространство.
        space_id: String,
        /// Канал.
        channel_id: String,
        /// Вебхуки или текст ошибки.
        result: Result<Vec<bs_api::Webhook>, String>,
    },
    /// Мосты канала.
    Bridges {
        /// Пространство.
        space_id: String,
        /// Канал.
        channel_id: String,
        /// Мосты или текст ошибки.
        result: Result<bs_api::ChannelBridges, String>,
    },
    /// Итог изменения настроек уведомлений.
    Notified {
        /// Пространство.
        space_id: String,
        /// Новая настройка или текст ошибки.
        result: Result<bs_types::NotificationSetting, String>,
    },
    /// Ожидающие заявки.
    Requests {
        /// Пространство.
        space_id: String,
        /// Список или текст ошибки.
        result: Result<Vec<bs_api::JoinRequest>, String>,
    },
    /// Список блокировок.
    Bans {
        /// Пространство.
        space_id: String,
        /// Список или текст ошибки.
        result: Result<Vec<bs_api::Ban>, String>,
    },
    /// Код приглашения (или ошибка).
    Invite {
        /// Код или текст ошибки.
        result: Result<String, String>,
    },
    /// Приглашение другу в ЛС отправлено (или нет).
    InviteSent {
        /// Друг.
        friend_id: String,
        /// Итог.
        result: Result<(), String>,
    },
    /// Канал или категория созданы (или нет).
    Created(Result<(), String>),
    /// Вступление не удалось (успех приходит как `Space`).
    JoinFailed(String),
    /// Выход из пространства.
    Left {
        /// Пространство.
        space_id: String,
        /// Итог.
        result: Result<(), String>,
    },
}

/// Сообщение, которое только что пришло по сети.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewMessage {
    /// Личное сообщение.
    pub(crate) dm: bool,
    /// Канал (или личная беседа).
    pub(crate) channel_id: String,
    /// Автор.
    pub(crate) author_id: String,
    /// Текст.
    pub(crate) content: String,
    /// Инстанс пометил `@everyone`/`@here`.
    pub(crate) mention_everyone: bool,
    /// Инстанс пометил упомянутые роли.
    pub(crate) mention_roles: Vec<String>,
}

/// Ответ на запрос токена LiveKit.
#[derive(Debug)]
pub(crate) struct VoiceTokenEvent {
    /// Канал, для которого запрошен токен.
    pub(crate) channel_id: String,
    /// Токен и адрес сервера или текст ошибки.
    pub(crate) result: Result<bs_types::LiveKitTokenResponse, String>,
    /// Ограничения стриминга этого инстанса (его SFU несёт трансляцию).
    pub(crate) limits: Option<bs_api::StreamingLimits>,
    /// Битрейт микрофона для этого звонка (сервер Rustspace).
    pub(crate) bitrate: Option<bs_api::VoiceBitrate>,
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

/// Действие со стикерпаками; после любого, кроме [`PackOp::With`], приходит
/// свежий список паков.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PackOp {
    /// Только список.
    List,
    /// Новый пак.
    Create(String),
    /// Переименовать.
    Rename { pack: String, name: String },
    /// Открыть или закрыть для других.
    Share { pack: String, shared: bool },
    /// Удалить свой пак.
    Delete(String),
    /// Положить стикер в пак.
    Put { pack: String, sticker: String },
    /// Убрать стикер из пака.
    Remove { pack: String, sticker: String },
    /// Новый порядок стикеров.
    Reorder { pack: String, order: Vec<String> },
    /// Добавить чужой пак к себе или убрать.
    Added { pack: String, added: bool },
    /// Найти открытые паки с этим стикером.
    With(String),
}

/// Ошибка стикерпаков.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PackError {
    /// Сервер не знает паков (не Rustspace или старая сборка).
    Unsupported,
    /// Действие не удалось.
    Failed(String),
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
    /// Свои и добавленные стикерпаки после действия (или ошибка действия).
    StickerPacks(Result<bs_api::StickerPacks, PackError>),
    /// Открытые паки со стикером из сообщения.
    PacksWith(Result<Vec<bs_api::StickerPack>, String>),
    /// Не удалось отправить сообщение.
    SendFailed(String),
}

/// Ответ фоновой сессии на [`BridgeRequest`].
#[derive(Debug)]
pub(crate) enum BridgeResponse {
    /// Пришли друзья.
    Friends(Vec<bs_types::Friend>),
    /// Пришли настройки уведомлений.
    NotificationSettings(Vec<bs_types::NotificationSetting>),
    /// Ключ собеседника из справочника сервера.
    E2eePeerKey {
        /// Переписка.
        dm_channel_id: String,
        /// Открытый ключ.
        public_key: String,
    },
    /// Запрос выполнен, показывать нечего.
    Quiet,
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
    /// Пришёл общий каталог (или почему его нет).
    Directory(Result<Vec<bs_api::DirectoryEntry>, String>),
    /// Сведения об инстансе.
    InstanceInfo(Result<Box<bs_api::InstanceInfo>, String>),
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
    /// Токен голосового канала.
    VoiceToken(VoiceTokenEvent),
    /// «Избранное», закрепы, закладки.
    Extras(super::extras::ExtrasEvent),
    /// Битрейт голоса после изменения: звонок и итог.
    VoiceBitrate(String, Result<bs_api::VoiceBitrate, String>),
    /// Итог действия над пространством.
    SpaceAction(SpaceEvent),
    /// Итог действия над группой и созданный канал.
    GroupAction(GroupEvent, Option<Box<bs_types::DmChannel>>),
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
    /// Ход скачивания файла на диск.
    Download { id: u64, state: DownloadState },
    /// Страница результатов поиска.
    SearchDone {
        seq: u64,
        result: Result<(Vec<SearchHit>, i64), String>,
    },
    /// Окно вокруг сообщения загружено (или нет).
    AroundDone {
        channel_id: String,
        message_id: String,
        result: Result<HistoryItems, String>,
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
pub(super) enum BridgeEvent {
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
pub(super) struct EndGuard(pub(super) std_mpsc::Sender<BridgeEvent>);

impl Drop for EndGuard {
    fn drop(&mut self) {
        let _ = self.0.send(BridgeEvent::Ended);
    }
}
