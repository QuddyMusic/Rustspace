//! Мост GUI ↔ tokio.
//!
//! Разделение обязанностей:
//! - фоновая tokio-задача держит [`InstanceHandle`] и читает события;
//! - GUI-поток владеет [`Cache`] и применяет к нему [`mod@apply`] прямо в кадре.
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

pub(crate) mod e2ee;
pub(crate) mod extras;
mod local_bookmarks;
mod protocol;
mod responses;
mod session;
mod worker;

#[allow(clippy::wildcard_imports, reason = "реэкспорт частей моста")]
pub(crate) use self::protocol::*;
#[allow(clippy::wildcard_imports, reason = "реэкспорт частей моста")]
use self::worker::*;

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

use crate::i18n::tr;
use crate::logic::describe_server_error;

/// Сколько сообщений просим у сервера за один запрос истории.
pub(crate) const HISTORY_PAGE: u16 = 50;

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
    /// Куда сохранять сессию домашнего инстанса (`None` — не сохранять).
    persist: Option<std::path::PathBuf>,
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
    voice_in: Vec<VoiceTokenEvent>,
    /// Битрейт голоса после изменения.
    bitrate_in: Vec<(String, Result<bs_api::VoiceBitrate, String>)>,
    /// «Избранное», закрепы и закладки.
    extras: extras::Extras,
    space_in: Vec<SpaceEvent>,
    group_in: Vec<GroupEvent>,
    call_in: Vec<CallEvent>,
    /// Только что пришедшие сообщения (для звука); историю сюда не кладём.
    new_messages: Vec<NewMessage>,
    raw_ready: Vec<(u64, Result<Vec<u8>, String>)>,
    /// Ход скачиваний на диск, ещё не переданный хабу картинок.
    downloads_ready: Vec<(u64, DownloadState)>,
    /// Ответы настроек, которые GUI ещё не забрал.
    admin_in: Vec<AdminEvent>,
    /// Готовые картинки, которые GUI ещё не превратил в текстуры.
    images_ready: Vec<(bs_api::AssetPath, Result<bs_media::Decoded, String>)>,
    /// Последняя страница каталога пространств.
    explore: Option<bs_api::ExplorePage>,
    /// Общий каталог других инстансов: пространства или причина, почему его нет.
    directory: Option<Result<Vec<bs_api::DirectoryEntry>, String>>,
    /// Сведения об инстансе (для страницы «Backspace»).
    instance_info: Option<Result<Box<bs_api::InstanceInfo>, String>>,
    /// Запрос сведений об инстансе уже ушёл.
    instance_info_asked: bool,
    /// Идёт ли запрос каталога.
    explore_loading: bool,
    /// Пространство, только что созданное или в которое вступили.
    opened_space: Option<String>,
    /// Пришедшие страницы галереи вложений.
    gallery_pages: Vec<GalleryPage>,
    /// Резервная копия ключа шифрования на сервере (`None` — не спрашивали).
    e2ee_backup: Option<Result<Option<bs_api::E2eeBackup>, String>>,
    /// Короткое сообщение об успехе («Профиль сохранён»).
    info: Option<String>,
    /// Загрузка истории по id канала.
    history: HashMap<String, HistoryState>,
    /// Поиск по сообщениям.
    search: SearchState,
    /// Куда прокрутить после загрузки окна: (канал, сообщение).
    jump: Option<(String, String)>,
    /// Каналы, где вместо свежих сообщений загружено окно вокруг найденного.
    detached: std::collections::HashSet<String>,
    /// Кэш инстанса; `None` до первого `ready`.
    cache: Option<Cache>,
    ready: bool,
    /// Потеряна ли живая сессия: GUI должен вернуться на логин.
    session_lost: bool,
    /// Последнее известное состояние соединения.
    connection: ConnectionState,
    last_error: Option<String>,
    /// Код двухфакторной защиты для ближайшего входа.
    second_factor: Option<zeroize::Zeroizing<String>>,
    /// Отказ сервера на действие пользователя (например, `rate_limited`).
    notice: Option<String>,
    /// Сквозное шифрование переписок этого аккаунта (после входа).
    e2ee: std::cell::RefCell<Option<e2ee::E2ee>>,
    /// Почему исходящее сообщение не ушло: показать при ближайшем опросе.
    e2ee_notice: std::cell::RefCell<Option<String>>,
}

/// Приводит адрес инстанса к абсолютному URL.
///
/// Пользователь обычно вводит только хост (`backspace.example`), поэтому
/// схема `https://` подставляется автоматически. Явно указанная схема
/// (`http://`, `ws://` и т.п.) сохраняется как есть, а допустимость
/// открытого текста решает [`TransportPolicy`].
pub(crate) fn normalize_base(raw: &str) -> Option<String> {
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

#[cfg(test)]
mod tests;
