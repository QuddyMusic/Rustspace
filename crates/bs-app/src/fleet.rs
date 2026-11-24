//! Несколько инстансов одновременно в одном интерфейсе.
//!
//! Пользователь входит в **домашний** инстанс (слот 0). К нему можно
//! подключить другие инстансы: у каждого своя полноценная [`Session`] —
//! свой токен, WebSocket, кэш и свой идентификатор пользователя. Все они
//! работают одновременно, а интерфейс показывает пространства и личные
//! беседы всех сразу (как `spaceStore` в оригинале).
//!
//! Каждое действие уходит на инстанс-владелец канала или пространства
//! (аналог `channelOriginMap`): экран перед отрисовкой вызывает
//! [`Bridge::focus_on`] с владельцем выбранного, и методы [`Session`],
//! доступные через `Deref`, работают именно с ним.
//! Домашний инстанс дополнительно просят установить S2S-пиринг с
//! подключённым инстансом (`peer/ensure`), чтобы работали личные
//! сообщения между серверами.
//!
//! Вход на чужой инстанс устроен как в оригинале (`connectToRemote`):
//! домашний инстанс проверяет пароль и выдаёт секрет именно для этого
//! origin; пароль на чужой инстанс не отправляется никогда.

use std::ops::{Deref, DerefMut};

use bs_api::{IdentityDeleteMode, RegistryEntry, RegistryStatus, ReplicatedEntry};
use bs_types::TransportPolicy;
use tokio::runtime::Handle;

use crate::bridge::{BridgeRequest, FailureKind, IssuedCredential, Secret, Session};
use crate::i18n::tr;
use crate::registry::{self, Registry, now_ms};

/// Сколько чужих инстансов можно подключить: домашний сервер сам присылает
/// список связанных инстансов, и бесконечно ходить по нему клиент не должен.
pub(crate) const MAX_REMOTES: usize = 8;

/// Слот: один инстанс.
#[derive(Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "независимые признаки этапов подключения"
)]
struct Slot {
    session: Session,
    /// Адрес инстанса (`https://host`), как его вводил пользователь.
    base: Option<String>,
    /// Имя пользователя на этом инстансе (`alice@home.example`).
    username: Option<String>,
    /// Ждём секрет от домашнего инстанса.
    awaiting_credential: bool,
    /// Выданный секрет уже записан на удалённый аккаунт.
    provisioned: bool,
    /// Был ли слот живым в прошлом кадре (для перехода «стал живым»).
    was_live: bool,
    /// Ответ сервера о пиринге.
    peering: Option<String>,
    /// Можно ли создать аккаунт на инстансе, если его там нет. Автоподключение
    /// только входит: аккаунты заводит лишь явное действие пользователя.
    register: bool,
    /// Что об этом слоте уже записано в реестр.
    recorded: Recorded,
    /// Вход собственным паролем аккаунта на инстансе (запасной путь): логин,
    /// пароль и секрет, на который затем сменим пароль.
    manual: Option<Manual>,
    /// Ждём ответа на смену пароля удалённого аккаунта на выданный секрет.
    adopting: Option<IssuedCredential>,
    /// Пароль сменён: после закрытия временной сессии входим выданным секретом.
    restart: Option<IssuedCredential>,
}

/// Что слот уже сообщил реестру.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recorded {
    Nothing,
    Connected,
    Failed,
}

/// Собственные данные аккаунта на удалённом инстансе.
#[derive(Debug)]
struct Manual {
    username: String,
    password: Secret,
}

impl Slot {
    fn new(runtime: &Handle) -> Self {
        Self {
            session: Session::new(runtime.clone()),
            base: None,
            username: None,
            awaiting_credential: false,
            provisioned: true,
            was_live: false,
            peering: None,
            register: false,
            recorded: Recorded::Nothing,
            manual: None,
            adopting: None,
            restart: None,
        }
    }
}

/// Состояние подключения в списке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowState {
    /// Сессия работает.
    Connected,
    /// Идёт подключение.
    Connecting,
    /// Отключён пользователем.
    Disconnected,
    /// Инстанс не отвечал.
    Unreachable,
    /// Нужен вход заново.
    AuthExpired,
}

/// Строка списка подключений.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectionRow {
    /// Адрес инстанса.
    pub(crate) origin: String,
    /// Название для показа.
    pub(crate) label: String,
    /// Имя пользователя на инстансе.
    pub(crate) username: String,
    /// Идентификатор пользователя на инстансе.
    pub(crate) remote_user_id: String,
    /// Состояние.
    pub(crate) state: RowState,
    /// Код причины из реестра.
    pub(crate) reason: Option<String>,
    /// Текст последней ошибки этого запуска (для подробностей).
    pub(crate) detail: Option<String>,
    /// Когда добавлен, мс.
    pub(crate) added_at: i64,
    /// Последнее подключение, мс.
    pub(crate) last_connected_at: Option<i64>,
    /// Когда отключён вручную, мс.
    pub(crate) disconnected_at: Option<i64>,
    /// Число каналов и бесед с непрочитанным.
    pub(crate) unread: usize,
    /// Статус пиринга, как его назвал сервер.
    pub(crate) peering: Option<String>,
    /// Инстанс отказал выданному секрету: можно войти своим паролем там.
    pub(crate) offer_own_login: bool,
}

/// Домашний инстанс в шапке списка.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HomeRow {
    /// Адрес.
    pub(crate) base: String,
    /// Имя пользователя.
    pub(crate) username: String,
    /// Работает ли сессия.
    pub(crate) live: bool,
}

/// Что сделать на инстансе, когда он подключится.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingJoin {
    /// Вступить по коду приглашения.
    Code(String),
    /// Вступить в пространство (или подать заявку).
    Space {
        /// Id пространства.
        id: String,
        /// Заявкой, а не сразу.
        by_request: bool,
    },
}

/// Мост GUI ↔ tokio для всех инстансов.
///
/// Через `Deref` все методы [`Session`] относятся к **активному** инстансу,
/// так что экраны работают с ним, не зная про остальные.
#[derive(Debug)]
pub(crate) struct Bridge {
    runtime: Handle,
    slots: Vec<Slot>,
    /// Инстанс, которому адресованы действия текущего кадра.
    focus: usize,
    transport: TransportPolicy,
    /// Домашняя сессия потеряна: нужен новый вход.
    lost: bool,
    /// Идёт выход: ждём завершения всех сессий.
    leaving: bool,
    /// Короткое сообщение о подключении инстансов.
    message: Option<String>,
    /// Реестр подключений (источник списка «Подключения»).
    registry: Registry,
    /// Запрос реестра отправлен, ответа ещё нет.
    registry_requested: bool,
    /// Инстансы, на которых просили удалить учётную запись.
    deleting: Vec<String>,
    /// Вступления, ждущие подключения инстанса: (адрес, что сделать).
    pending_joins: Vec<(String, PendingJoin)>,
}

impl Deref for Bridge {
    type Target = Session;

    fn deref(&self) -> &Session {
        &self.slots[self.focus].session
    }
}

impl DerefMut for Bridge {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.slots[self.focus].session
    }
}

/// Приводит адрес к виду `scheme://host[:port]` без пути и хвостовых слэшей.
#[must_use]
pub(crate) fn canonical_base(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let url = url::Url::parse(&with_scheme).ok()?;
    let host = url.host_str()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let mut base = format!("{}://{host}", url.scheme());
    if let Some(port) = url.port() {
        base = format!("{base}:{port}");
    }
    Some(base)
}

/// Один и тот же инстанс, как бы ни был записан адрес.
#[must_use]
pub(crate) fn same_instance(a: &str, b: &str) -> bool {
    match (canonical_base(a), canonical_base(b)) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(&b),
        _ => a.trim() == b.trim(),
    }
}

/// Короткое название инстанса из адреса.
fn host_label(base: &str) -> String {
    bs_identity::identity::host_of(base)
}

impl Bridge {
    /// Мост с одним пустым слотом: форма входа.
    pub(crate) fn new(runtime: Handle) -> Self {
        Self {
            slots: vec![Slot::new(&runtime)],
            runtime,
            focus: 0,
            transport: TransportPolicy::Strict,
            lost: false,
            leaving: false,
            message: None,
            registry: Registry::default(),
            registry_requested: false,
            deleting: Vec::new(),
            pending_joins: Vec::new(),
        }
    }

    /// Код двухфакторной защиты для следующего входа в домашний инстанс.
    pub(crate) fn set_second_factor(&mut self, code: &str) {
        self.slots[0].session.set_second_factor(code);
    }

    /// Вход в домашний инстанс.
    /// Вход по токену (ключ доступа).
    pub(crate) fn start_token_login(
        &mut self,
        base: &str,
        token: crate::bridge::Secret,
        transport: TransportPolicy,
    ) {
        self.transport = transport;
        self.leaving = false;
        self.lost = false;
        let home = &mut self.slots[0];
        home.session.start_token_login(base, token, transport);
        home.base = canonical_base(base);
        self.focus = 0;
    }

    pub(crate) fn start_login(
        &mut self,
        base: &str,
        username: &str,
        password: &str,
        transport: TransportPolicy,
    ) {
        self.transport = transport;
        self.leaving = false;
        self.lost = false;
        let home = &mut self.slots[0];
        home.session
            .start_login(base, username, password, transport);
        home.base = canonical_base(base);
        self.focus = 0;
    }

    /// Регистрирует аккаунт на домашнем инстансе и входит в него.
    pub(crate) fn start_register(
        &mut self,
        base: &str,
        request: bs_api::RegisterRequest,
        transport: TransportPolicy,
    ) {
        self.transport = transport;
        self.leaving = false;
        self.lost = false;
        let home = &mut self.slots[0];
        home.session.start_register(base, request, transport);
        home.base = canonical_base(base);
        self.focus = 0;
    }

    /// Включает сохранение сессии в файл и входит по ней, если она есть.
    ///
    /// Возвращает адрес инстанса, в который пошёл автоматический вход.
    pub(crate) fn restore(&mut self, path: Option<std::path::PathBuf>) -> Option<String> {
        self.slots[0].session.set_persist(path);
        let (base, transport) = self.slots[0].session.resume_saved()?;
        self.transport = transport;
        self.slots[0].base = canonical_base(&base);
        self.focus = 0;
        Some(base)
    }

    /// Адрес домашнего инстанса и разрешён ли для него `http://`.
    pub(crate) fn home(&self) -> Option<(String, bool)> {
        let base = self.slots.first()?.base.clone()?;
        Some((base, self.transport == TransportPolicy::AllowInsecure))
    }

    /// Выход: закрывает все сессии и забывает сохранённый вход.
    pub(crate) fn logout(&mut self) {
        self.slots[0].session.forget_saved();
        self.leaving = true;
        for slot in &mut self.slots {
            slot.session.logout();
        }
    }

    /// Домашний инстанс потерян или пользователь вышел.
    pub(crate) fn take_session_lost(&mut self) -> bool {
        std::mem::take(&mut self.lost)
    }

    /// Сообщение о подключении инстансов (однократно).
    pub(crate) fn take_message(&mut self) -> Option<String> {
        self.message.take()
    }

    /// Забирает события всех сессий и двигает подключения.
    pub(crate) fn poll(&mut self) {
        for slot in &mut self.slots {
            slot.session.poll();
        }
        self.deliver_registry();
        self.deliver_credentials();
        self.deliver_peering();
        self.drive_adoption();
        self.track_liveness();
        self.record_outcomes();
        self.deliver_deletions();
        self.flush_registry();
        self.collect_losses();
        self.drive_pending_joins();
    }

    /// Вступает в пространство на инстансе `origin` по коду приглашения;
    /// если инстанс ещё не подключён — подключает его федеративным входом
    /// через домашний (как оригинал при переходе по чужому приглашению)
    /// и вступает, когда подключение поднимется.
    pub(crate) fn connect_and_join(&mut self, origin: &str, code: &str) {
        self.connect_then(origin, PendingJoin::Code(code.to_owned()));
    }

    /// Вступает в пространство из общего каталога (сразу или заявкой),
    /// подключив его инстанс, если нужно.
    pub(crate) fn connect_and_join_space(
        &mut self,
        origin: &str,
        space_id: &str,
        by_request: bool,
    ) {
        self.connect_then(
            origin,
            PendingJoin::Space {
                id: space_id.to_owned(),
                by_request,
            },
        );
    }

    fn connect_then(&mut self, origin: &str, join: PendingJoin) {
        let Some(base) = canonical_base(origin) else {
            self.message = Some(tr("Некорректный адрес инстанса").to_owned());
            return;
        };
        if let Some(index) = self.slot_index(&base)
            && self.slots[index].session.is_live()
        {
            self.join_on(index, join);
            return;
        }
        let connecting = self
            .slot_index(&base)
            .is_some_and(|index| !self.slots[index].session.is_idle());
        if !connecting {
            let before = self.slots.len();
            self.add_remote(&base, None, None, true);
            if self.slots.len() == before {
                return;
            }
        }
        self.pending_joins.retain(|(b, _)| *b != base);
        self.pending_joins.push((base, join));
        self.message = Some(tr("Подключаем инстанс, затем вступим в пространство…").to_owned());
    }

    fn slot_index(&self, base: &str) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.base.as_deref() == Some(base))
    }

    fn join_on(&mut self, index: usize, join: PendingJoin) {
        let previous = self.focus;
        self.focus = index;
        self.request(match join {
            PendingJoin::Code(code) => BridgeRequest::JoinByCode(code),
            PendingJoin::Space { id, by_request } => BridgeRequest::JoinSpace { id, by_request },
        });
        self.focus = previous;
    }

    /// Вступления, чей инстанс поднялся (или не поднялся).
    fn drive_pending_joins(&mut self) {
        let pending = std::mem::take(&mut self.pending_joins);
        for (base, join) in pending {
            match self.slot_index(&base) {
                Some(index) if self.slots[index].session.is_live() => self.join_on(index, join),
                Some(index)
                    if !self.slots[index].session.is_idle()
                        || self.slots[index].awaiting_credential =>
                {
                    self.pending_joins.push((base, join));
                }
                _ => {
                    self.message = Some(crate::tf!(
                        "Не удалось подключиться к {base}: вступление отменено",
                        base = base
                    ));
                }
            }
        }
    }

    /// Серверный реестр пришёл: собираем список и поднимаем подключения.
    fn deliver_registry(&mut self) {
        let Some(answer) = self.slots[0].session.take_registry() else {
            return;
        };
        self.registry_requested = false;
        let known: Vec<(String, String)> = self.slots[0]
            .session
            .cache()
            .map(|cache| {
                cache
                    .me()
                    .replicated_instances
                    .iter()
                    .filter_map(|r| Some((canonical_base(&r.origin)?, r.username.clone())))
                    .collect()
            })
            .unwrap_or_default();
        let home_base = self.slots[0].base.clone();
        let known: Vec<(String, String)> = known
            .into_iter()
            .filter(|(origin, _)| home_base.as_deref() != Some(origin.as_str()))
            .collect();
        match answer {
            Ok(server) => self.registry.load(Some(server), &known, now_ms()),
            Err(message) => {
                tracing::warn!(%message, "реестр подключений не прочитан");
                self.message =
                    Some(tr("Не удалось прочитать список подключений с сервера").to_owned());
                self.registry.load(None, &known, now_ms());
            }
        }
        for (origin, username) in self.registry.auto_targets() {
            let Some(base) = canonical_base(&origin) else {
                continue;
            };
            if home_base.as_deref() == Some(base.as_str()) {
                continue;
            }
            self.add_remote(&base, None, Some(username), false);
        }
    }

    fn deliver_credentials(&mut self) {
        let issued = self.slots[0].session.take_issued();
        for (origin, result) in issued {
            let transport = self.transport;
            let Some(slot) = self.slots.iter_mut().skip(1).find(|s| {
                s.awaiting_credential && s.base.as_deref() == canonical_base(&origin).as_deref()
            }) else {
                continue;
            };
            slot.awaiting_credential = false;
            match result {
                Ok(credential) => {
                    let base = slot.base.clone().unwrap_or(origin);
                    slot.username = Some(credential.username.clone());
                    slot.provisioned = credential.provisioned;
                    let name = credential.display_name.clone();
                    if let Some(manual) = slot.manual.as_ref() {
                        // Запасной путь: входим своим паролем на инстансе и
                        // потом переводим аккаунт на выданный секрет.
                        slot.session.start_remote_login(
                            &base,
                            &manual.username,
                            &manual.password,
                            transport,
                        );
                        slot.adopting = Some(credential);
                    } else {
                        slot.session.start_remote(
                            &credential,
                            &base,
                            name,
                            transport,
                            slot.register,
                        );
                    }
                }
                Err(message) => slot.session.fail(message),
            }
        }
    }

    fn deliver_peering(&mut self) {
        for (origin, result) in self.slots[0].session.take_peering() {
            let Some(slot) = self
                .slots
                .iter_mut()
                .skip(1)
                .find(|s| s.base.as_deref() == canonical_base(&origin).as_deref())
            else {
                continue;
            };
            let name = host_label(&origin);
            match result {
                Ok(status) => {
                    self.message = match status.as_str() {
                        "rejected" => Some(crate::tf!("Пиринг с {name} отклонён", name = name)),
                        "pending" | "awaiting_approval" => Some(crate::tf!(
                            "Пиринг с {name} ожидает подтверждения",
                            name = name
                        )),
                        "admin_required" => Some(crate::tf!(
                            "Администратор вашего инстанса должен одобрить связь с {name}",
                            name = name
                        )),
                        _ => self.message.take(),
                    };
                    slot.peering = Some(status);
                }
                Err(message) => {
                    tracing::warn!(%origin, %message, "пиринг не запрошен");
                    slot.peering = None;
                }
            }
        }
    }

    /// Вход своим паролем удался: просим инстанс принять выданный секрет и
    /// затем входим уже им. Так аккаунт переводится на выданные данные.
    fn drive_adoption(&mut self) {
        let transport = self.transport;
        for slot in self.slots.iter_mut().skip(1) {
            if slot.session.is_live()
                && let Some(credential) = slot.adopting.take()
            {
                slot.session
                    .request(BridgeRequest::AdoptSecret(credential.secret.clone()));
                slot.restart = Some(credential);
            }
            if slot.restart.is_some()
                && let Some(answer) = slot.session.take_adopted()
            {
                match answer {
                    Ok(()) => slot.session.logout(),
                    Err(message) => {
                        tracing::warn!(%message, "пароль на инстансе не заменён выданным");
                        slot.restart = None;
                        slot.manual = None;
                        self.message = Some(
                            tr("Вход выполнен, но перевести аккаунт на выданный пароль не удалось")
                                .to_owned(),
                        );
                    }
                }
            }
            if slot.restart.is_some()
                && slot.session.is_idle()
                && let Some(credential) = slot.restart.take()
                && let Some(base) = slot.base.clone()
            {
                slot.manual = None;
                slot.recorded = Recorded::Nothing;
                slot.provisioned = false;
                let name = credential.display_name.clone();
                slot.session
                    .start_remote(&credential, &base, name, transport, false);
            }
        }
    }

    /// Реагирует на переход слота в рабочее состояние.
    fn track_liveness(&mut self) {
        let mut newly_live = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let live = slot.session.is_live();
            if live && !slot.was_live {
                newly_live.push(index);
            }
            slot.was_live = live;
        }
        for index in newly_live {
            if index == 0 {
                self.on_home_live();
            } else {
                self.on_remote_live(index);
            }
        }
    }

    /// Записывает в реестр итог каждой попытки: подключились или нет и почему.
    fn record_outcomes(&mut self) {
        if !self.registry.is_settled() {
            return;
        }
        let now = now_ms();
        for slot in self.slots.iter_mut().skip(1) {
            let Some(base) = slot.base.clone() else {
                continue;
            };
            if slot.session.is_live() {
                if slot.recorded != Recorded::Connected
                    && slot.adopting.is_none()
                    && slot.restart.is_none()
                    && let Some(cache) = slot.session.cache()
                {
                    let me = cache.me();
                    self.registry
                        .mark_connected(&base, "", &me.username, &me.id, now);
                    slot.recorded = Recorded::Connected;
                }
            } else if slot.session.is_idle()
                && !slot.awaiting_credential
                && slot.restart.is_none()
                && slot.recorded != Recorded::Failed
            {
                match slot.session.failure() {
                    Some(FailureKind::Network) => self.registry.mark_unreachable(&base, now),
                    Some(FailureKind::CredentialRefused | FailureKind::RegistrationClosed) => {
                        self.registry.mark_auth_expired(
                            &base,
                            registry::reason::REAUTHENTICATE,
                            now,
                        );
                    }
                    Some(FailureKind::Auth) => {
                        self.registry.mark_auth_expired(
                            &base,
                            registry::reason::SESSION_EXPIRED,
                            now,
                        );
                    }
                    None => {}
                }
                slot.recorded = Recorded::Failed;
            }
        }
    }

    /// Итоги удаления учётных записей на инстансах.
    fn deliver_deletions(&mut self) {
        for answer in self.slots[0].session.take_deleted() {
            let origins = std::mem::take(&mut self.deleting);
            match answer {
                Ok(results) => {
                    let mut removed = 0_usize;
                    let mut problems = Vec::new();
                    for origin in origins {
                        match results.get(&origin) {
                            Some(result) if result.success => {
                                self.forget(&origin);
                                removed += 1;
                            }
                            Some(result) => problems.push(format!(
                                "{}: {}",
                                host_label(&origin),
                                delete_error(result.error.as_deref())
                            )),
                            None => problems.push(crate::tf!(
                                "{a0}: сервер не ответил по этому инстансу",
                                a0 = host_label(&origin)
                            )),
                        }
                    }
                    self.message = Some(if problems.is_empty() {
                        match removed {
                            0 | 1 => tr("Учётная запись удалена").to_owned(),
                            n => crate::tf!("Учётная запись удалена на {n} инстансах", n = n),
                        }
                    } else {
                        problems.join("\n")
                    });
                }
                Err(message) => {
                    self.message = Some(crate::tf!(
                        "Не удалось удалить учётную запись: {message}",
                        message = message
                    ));
                }
            }
        }
    }

    /// Пишет изменённый реестр на домашний сервер.
    fn flush_registry(&mut self) {
        for answer in self.slots[0].session.take_saved() {
            if let Err(message) = answer {
                tracing::warn!(%message, "реестр подключений не записан");
            }
        }
        if !self.slots[0].session.is_live() {
            return;
        }
        if let Some((entries, updated_at)) = self.registry.take_pending_write() {
            self.slots[0].session.request(BridgeRequest::PutRegistry {
                entries,
                updated_at,
            });
        }
    }

    /// Домашний инстанс заработал: читаем реестр подключений. Остальное
    /// (подключение инстансов) продолжается, когда реестр придёт.
    fn on_home_live(&mut self) {
        if !self.registry_requested {
            self.registry_requested = true;
            self.slots[0].session.request(BridgeRequest::LoadRegistry);
        }
    }

    /// Чужой инстанс заработал: просим дом о пиринге и рассылаем список.
    fn on_remote_live(&mut self, index: usize) {
        let Some(base) = self.slots[index].base.clone() else {
            return;
        };
        if self.slots[index].adopting.is_some() || self.slots[index].restart.is_some() {
            // Временная сессия своим паролем: настоящая будет после смены.
            return;
        }
        self.slots[0]
            .session
            .request(BridgeRequest::EnsurePeered(base.clone()));
        if !self.slots[index].provisioned {
            self.slots[0]
                .session
                .request(BridgeRequest::MarkProvisioned(base));
            self.slots[index].provisioned = true;
        }
        self.sync_topology();
    }

    /// Рассылает каждому инстансу список остальных (как `syncInstanceList`).
    ///
    /// В списке остаются и отключённые, и не подключившиеся записи: иначе
    /// временный сбой стёр бы их с сервера.
    fn sync_topology(&self) {
        if !self.registry.is_settled() || !self.slots[0].session.is_live() {
            return;
        }
        let Some(home_base) = self.slots[0].base.clone() else {
            return;
        };
        let home_username = self.slots[0]
            .session
            .cache()
            .map(|c| {
                c.me()
                    .username
                    .split('@')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .unwrap_or_default();
        let remotes = self.registry.replicated();
        self.slots[0]
            .session
            .request(BridgeRequest::SyncInstances(remotes.clone()));
        for slot in self.slots.iter().skip(1).filter(|s| s.session.is_live()) {
            let mut list = vec![ReplicatedEntry {
                origin: home_base.clone(),
                username: home_username.clone(),
            }];
            list.extend(
                remotes
                    .iter()
                    .filter(|r| Some(&r.origin) != slot.base.as_ref())
                    .cloned(),
            );
            slot.session.request(BridgeRequest::SyncInstances(list));
        }
    }

    /// Разбирает потери: упавший чужой инстанс — это ошибка в его строке,
    /// потеря домашнего или активного без запасного — возврат к логину.
    fn collect_losses(&mut self) {
        for index in 0..self.slots.len() {
            if self.slots[index].session.take_session_lost() && index == 0 {
                self.lost = true;
            }
        }
        if self.lost {
            self.abort_remotes();
            self.focus = 0;
            return;
        }
        if !self.slots[self.focus].session.is_live() {
            self.focus = 0;
        }
        if self.leaving && self.slots.iter().all(|s| s.session.is_idle()) {
            self.leaving = false;
            self.slots.truncate(1);
            self.focus = 0;
            self.registry.clear();
            self.registry_requested = false;
        }
    }

    fn abort_remotes(&mut self) {
        for slot in self.slots.iter_mut().skip(1) {
            slot.session.logout();
        }
        self.slots.truncate(1);
        self.registry.clear();
        self.registry_requested = false;
    }

    /// Подключает новый инстанс. `password` — пароль домашнего аккаунта:
    /// проверяется домом, на чужой инстанс не уходит.
    pub(crate) fn connect_remote(&mut self, raw_base: &str, password: Option<&str>) {
        let Some(base) = canonical_base(raw_base) else {
            self.message = Some(tr("Некорректный адрес инстанса").to_owned());
            return;
        };
        self.add_remote(&base, password, None, true);
    }

    fn add_remote(
        &mut self,
        base: &str,
        password: Option<&str>,
        username: Option<String>,
        register: bool,
    ) {
        if !self.slots[0].session.is_live() {
            self.message = Some(tr("Сначала войдите в домашний инстанс").to_owned());
            return;
        }
        if let Ok(url) = url::Url::parse(base)
            && let Err(err) = self.transport.check(&url)
        {
            self.message = Some(err.to_string());
            return;
        }
        if self.slots[0].base.as_deref() == Some(base) {
            self.message = Some(tr("Это ваш домашний инстанс: он уже подключён").to_owned());
            return;
        }
        if let Some(existing) = self
            .slots
            .iter()
            .skip(1)
            .position(|s| s.base.as_deref() == Some(base))
        {
            let slot = &self.slots[existing + 1];
            if !slot.session.is_idle() || slot.awaiting_credential {
                self.message = Some(tr("Этот инстанс уже подключён").to_owned());
                return;
            }
            self.slots.remove(existing + 1);
            self.focus = 0;
        }
        if self.slots.len() > MAX_REMOTES {
            self.message = Some(crate::tf!(
                "Можно подключить не больше {MAX_REMOTES} инстансов",
                MAX_REMOTES = MAX_REMOTES
            ));
            return;
        }
        let mut slot = Slot::new(&self.runtime);
        slot.base = Some(base.to_owned());
        slot.username = username;
        slot.register = register;
        slot.awaiting_credential = true;
        slot.session.mark_connecting(base);
        self.slots.push(slot);
        self.slots[0]
            .session
            .request(BridgeRequest::IssueCredential {
                origin: base.to_owned(),
                password: password.map(Secret::new),
            });
    }

    /// Подключает отключённый или недоступный инстанс заново (без пароля:
    /// секрет выдаёт домашний инстанс по живой сессии).
    pub(crate) fn reconnect(&mut self, origin: &str) {
        let Some(base) = canonical_base(origin) else {
            return;
        };
        let username = self.registry.get(&base).map(|e| e.username.clone());
        self.add_remote(&base, None, username, false);
    }

    /// Входит заново с паролем домашнего аккаунта (проверяет дом).
    pub(crate) fn reauthenticate(&mut self, origin: &str, home_password: &str) {
        let Some(base) = canonical_base(origin) else {
            return;
        };
        let username = self.registry.get(&base).map(|e| e.username.clone());
        self.add_remote(&base, Some(home_password), username, true);
    }

    /// Входит собственным паролем аккаунта на инстансе, который не принял
    /// выданный секрет; после входа аккаунт переводится на выданный секрет.
    pub(crate) fn own_login(&mut self, origin: &str, username: &str, password: &str) {
        let Some(base) = canonical_base(origin) else {
            return;
        };
        let username = username.trim();
        if username.is_empty() || password.is_empty() {
            self.message = Some(tr("Введите имя и пароль на этом инстансе").to_owned());
            return;
        }
        self.add_remote(&base, None, Some(username.to_owned()), false);
        if let Some(slot) = self
            .slots
            .iter_mut()
            .skip(1)
            .find(|s| s.base.as_deref() == Some(base.as_str()))
        {
            slot.manual = Some(Manual {
                username: username.to_owned(),
                password: Secret::new(password),
            });
        }
    }

    /// Отключает инстанс по просьбе пользователя: запись остаётся в списке и
    /// больше не подключается сама.
    pub(crate) fn disconnect(&mut self, origin: &str) {
        let Some(base) = canonical_base(origin) else {
            return;
        };
        self.drop_slot(&base);
        self.registry.mark_disconnected(&base, now_ms());
        self.sync_topology();
    }

    /// Убирает запись о недоступном инстансе, не уведомляя его.
    pub(crate) fn force_remove(&mut self, origin: &str) {
        let Some(base) = canonical_base(origin) else {
            return;
        };
        self.forget(&base);
    }

    /// Просит домашний инстанс удалить учётную запись на этих инстансах.
    pub(crate) fn delete_identity(&mut self, origins: &[String], mode: IdentityDeleteMode) {
        if origins.is_empty() || !self.slots[0].session.is_live() {
            return;
        }
        self.deleting = origins.to_vec();
        self.slots[0]
            .session
            .request(BridgeRequest::DeleteIdentity {
                origins: origins.to_vec(),
                mode,
            });
    }

    /// Закрывает сессию инстанса и убирает слот.
    fn drop_slot(&mut self, base: &str) {
        if let Some(position) = self
            .slots
            .iter()
            .skip(1)
            .position(|s| s.base.as_deref() == Some(base))
        {
            self.slots[position + 1].session.logout();
            self.slots.remove(position + 1);
            self.focus = 0;
        }
    }

    /// Полностью забывает инстанс: сессия, слот, запись реестра.
    fn forget(&mut self, base: &str) {
        self.drop_slot(base);
        self.registry.remove(base, now_ms());
        self.sync_topology();
    }

    /// Адресует действия текущего кадра инстансу `index`. Если такого нет или
    /// он не работает, действия идут на домашний.
    pub(crate) fn focus_on(&mut self, index: usize) {
        self.focus = if self.slots.get(index).is_some_and(|s| s.session.is_live()) {
            index
        } else {
            0
        };
    }

    /// Номер инстанса, которому адресованы действия сейчас.
    pub(crate) const fn focus_index(&self) -> usize {
        self.focus
    }

    /// Номера работающих инстансов: домашний первым.
    pub(crate) fn live_origins(&self) -> Vec<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.session.is_live())
            .map(|(i, _)| i)
            .collect()
    }

    /// Сессия инстанса по номеру.
    pub(crate) fn session_at(&self, index: usize) -> Option<&crate::bridge::Session> {
        self.slots.get(index).map(|slot| &slot.session)
    }

    /// Запрос к домашнему инстансу, какой бы чат ни был открыт: личное
    /// (стикеры, паки) принадлежит аккаунту, а не пространству.
    pub(crate) fn request_home(&self, request: crate::bridge::BridgeRequest) -> bool {
        self.session_at(0)
            .is_some_and(|session| session.request(request))
    }

    /// Личные события композера домашнего инстанса (стикеры и паки), когда
    /// открыт чужой: остальное (загрузки файлов) остаётся своей сессии.
    pub(crate) fn take_home_sticker_events(&mut self) -> Vec<crate::bridge::ComposeEvent> {
        if self.focus_index() == 0 {
            return Vec::new();
        }
        self.session_at_mut(0)
            .map(crate::bridge::Session::take_sticker_events)
            .unwrap_or_default()
    }

    /// Сессия инстанса по номеру (для запросов к нему).
    pub(crate) fn session_at_mut(&mut self, index: usize) -> Option<&mut crate::bridge::Session> {
        self.slots.get_mut(index).map(|slot| &mut slot.session)
    }

    /// Кэш инстанса по номеру.
    pub(crate) fn cache_at(&self, index: usize) -> Option<&bs_core::Cache> {
        self.slots.get(index)?.session.cache()
    }

    /// Запрашивает сведения о домашнем инстансе, если их ещё нет.
    pub(crate) fn ensure_home_info(&mut self) {
        if let Some(home) = self.slots.first_mut() {
            home.session.ensure_instance_info();
        }
    }

    /// Сведения о домашнем инстансе, если пришли.
    pub(crate) fn home_info(&self) -> Option<&Result<Box<bs_api::InstanceInfo>, String>> {
        self.slots.first()?.session.instance_info()
    }

    /// Адрес инстанса по номеру.
    pub(crate) fn base_at(&self, index: usize) -> Option<&str> {
        self.slots.get(index)?.base.as_deref()
    }

    /// Адрес и канал запросов каждого работающего инстанса (для картинок).
    pub(crate) fn image_links(
        &self,
    ) -> Vec<(String, tokio::sync::mpsc::UnboundedSender<BridgeRequest>)> {
        self.slots
            .iter()
            .filter_map(|slot| {
                let (base, sender) = slot.session.session_link()?;
                slot.session.is_live().then_some((base, sender))
            })
            .collect()
    }

    /// Готовые картинки со всех инстансов, с адресом владельца.
    pub(crate) fn take_images(
        &mut self,
    ) -> Vec<(String, bs_api::AssetPath, Result<bs_media::Decoded, String>)> {
        let mut all = Vec::new();
        for slot in &mut self.slots {
            let base = slot.base.clone().unwrap_or_default();
            all.extend(
                slot.session
                    .take_images()
                    .into_iter()
                    .map(|(path, result)| {
                        // Внешний файл (GIF) не принадлежит инстансу: у него
                        // общий ключ, под которым его и ищет ImageHub.
                        let owner = if path.is_external() {
                            crate::images::EXTERNAL.to_owned()
                        } else {
                            base.clone()
                        };
                        (owner, path, result)
                    }),
            );
        }
        all
    }

    /// События DM-звонков со всех инстансов: номер инстанса и событие.
    pub(crate) fn take_call_events(&mut self) -> Vec<(usize, crate::bridge::CallEvent)> {
        self.slots
            .iter_mut()
            .enumerate()
            .flat_map(|(i, slot)| {
                slot.session
                    .take_call_events()
                    .into_iter()
                    .map(move |event| (i, event))
            })
            .collect()
    }

    /// Итоги действий над группами со всех инстансов.
    pub(crate) fn take_group_events(&mut self) -> Vec<(usize, crate::bridge::GroupEvent)> {
        self.slots
            .iter_mut()
            .enumerate()
            .flat_map(|(i, slot)| {
                slot.session
                    .take_group_events()
                    .into_iter()
                    .map(move |event| (i, event))
            })
            .collect()
    }

    /// Итоги действий над пространствами со всех инстансов.
    pub(crate) fn take_space_events(&mut self) -> Vec<(usize, crate::bridge::SpaceEvent)> {
        self.slots
            .iter_mut()
            .enumerate()
            .flat_map(|(i, slot)| {
                slot.session
                    .take_space_events()
                    .into_iter()
                    .map(move |event| (i, event))
            })
            .collect()
    }

    /// Новые сообщения со всех инстансов: номер инстанса и сообщение.
    pub(crate) fn take_new_messages(&mut self) -> Vec<(usize, crate::bridge::NewMessage)> {
        self.slots
            .iter_mut()
            .enumerate()
            .flat_map(|(i, slot)| {
                slot.session
                    .take_new_messages()
                    .into_iter()
                    .map(move |message| (i, message))
            })
            .collect()
    }

    /// Изменения битрейта голоса со всех инстансов.
    pub(crate) fn take_voice_bitrates(
        &mut self,
    ) -> Vec<(usize, String, Result<bs_api::VoiceBitrate, String>)> {
        self.slots
            .iter_mut()
            .enumerate()
            .flat_map(|(i, slot)| {
                slot.session
                    .take_voice_bitrates()
                    .into_iter()
                    .map(move |(target, result)| (i, target, result))
            })
            .collect()
    }

    /// Токены голоса со всех инстансов: номер инстанса и ответ.
    pub(crate) fn take_voice_tokens(&mut self) -> Vec<(usize, crate::bridge::VoiceTokenEvent)> {
        self.slots
            .iter_mut()
            .enumerate()
            .flat_map(|(i, slot)| {
                slot.session
                    .take_voice_tokens()
                    .into_iter()
                    .map(move |event| (i, event))
            })
            .collect()
    }

    /// Скачанные файлы со всех инстансов.
    /// Ход скачиваний на диск со всех инстансов.
    pub(crate) fn take_downloads(&mut self) -> Vec<(u64, crate::bridge::DownloadState)> {
        self.slots
            .iter_mut()
            .flat_map(|slot| slot.session.take_downloads())
            .collect()
    }

    pub(crate) fn take_raw(&mut self) -> Vec<(u64, Result<Vec<u8>, String>)> {
        self.slots
            .iter_mut()
            .flat_map(|slot| slot.session.take_raw())
            .collect()
    }

    /// Личный канал, только что открытый по запросу: инстанс и id.
    pub(crate) fn take_opened_dm(&mut self) -> Option<(usize, String)> {
        self.slots
            .iter_mut()
            .enumerate()
            .find_map(|(i, s)| Some((i, s.session.take_opened_dm()?)))
    }

    /// Пространство, только что созданное или вступившее: инстанс и id.
    pub(crate) fn take_opened_space(&mut self) -> Option<(usize, String)> {
        self.slots
            .iter_mut()
            .enumerate()
            .find_map(|(i, s)| Some((i, s.session.take_opened_space()?)))
    }

    /// Короткое сообщение об успехе с любого инстанса.
    pub(crate) fn take_info(&mut self) -> Option<String> {
        self.slots.iter_mut().find_map(|s| s.session.take_info())
    }

    /// Состояние шифрования переписки на инстансе `origin`.
    pub(crate) fn e2ee_status(
        &self,
        origin: usize,
        dm_id: &str,
    ) -> crate::bridge::e2ee::ChatStatus {
        self.slots
            .get(origin)
            .map(|slot| slot.session.e2ee_status(dm_id))
            .unwrap_or_default()
    }

    /// Открыта переписка: спросить ключ собеседника для автошифрования.
    pub(crate) fn e2ee_probe(&self, origin: usize, dm_id: &str) {
        if let Some(slot) = self.slots.get(origin) {
            slot.session.e2ee_probe(dm_id);
        }
    }

    /// Включает или выключает шифрование переписки.
    pub(crate) fn set_e2ee(&mut self, origin: usize, dm_id: &str, enabled: bool) {
        if let Some(slot) = self.slots.get_mut(origin) {
            slot.session.set_e2ee(dm_id, enabled);
        }
    }

    /// Ключ шифрования аккаунта из системного хранилища недоступен.
    pub(crate) fn e2ee_locked(&self, origin: usize) -> bool {
        self.slots
            .get(origin)
            .is_some_and(|slot| slot.session.e2ee_locked())
    }

    /// Запись ключа шифрования для другого устройства.
    ///
    /// # Errors
    /// Текст для пользователя.
    pub(crate) fn export_e2ee_key(
        &self,
        origin: usize,
        passphrase: &str,
    ) -> Result<String, String> {
        self.slots.get(origin).map_or_else(
            || Err(String::new()),
            |slot| slot.session.export_e2ee_key(passphrase),
        )
    }

    /// Ставит ключ шифрования с другого устройства.
    ///
    /// # Errors
    /// Текст для пользователя.
    pub(crate) fn import_e2ee_key(
        &mut self,
        origin: usize,
        text: &str,
        passphrase: &str,
    ) -> Result<(), String> {
        self.slots.get_mut(origin).map_or_else(
            || Err(String::new()),
            |slot| slot.session.import_e2ee_key(text, passphrase),
        )
    }

    /// Принимает новый ключ собеседника.
    pub(crate) fn accept_e2ee_key(&mut self, origin: usize, dm_id: &str) {
        if let Some(slot) = self.slots.get_mut(origin) {
            slot.session.accept_e2ee_key(dm_id);
        }
    }

    /// Отказ сервера на действие: первый, какой есть.
    pub(crate) fn notice(&self) -> Option<&str> {
        self.slots.iter().find_map(|s| s.session.notice())
    }

    /// Скрывает все отказы.
    pub(crate) fn dismiss_notice(&mut self) {
        for slot in &mut self.slots {
            slot.session.dismiss_notice();
        }
    }

    /// Состояние соединения домашнего инстанса.
    pub(crate) fn connection(&self) -> &bs_instances::ConnectionState {
        self.slots[0].session.connection()
    }

    /// Первый `ready` домашнего инстанса.
    pub(crate) fn take_ready(&mut self) -> bool {
        self.slots[0].session.take_ready()
    }

    /// Идёт ли вход или выход домашнего инстанса.
    pub(crate) fn is_busy(&self) -> bool {
        self.slots[0].session.is_busy()
    }

    /// Последняя ошибка входа.
    pub(crate) fn last_error(&self) -> Option<&str> {
        self.slots[0].session.last_error()
    }

    /// Домашний инстанс для шапки списка подключений.
    pub(crate) fn home_row(&self) -> Option<HomeRow> {
        let slot = &self.slots[0];
        let base = slot
            .base
            .clone()
            .or_else(|| slot.session.base().map(str::to_owned))?;
        Some(HomeRow {
            base,
            username: slot
                .session
                .cache()
                .map(|c| c.me().username.clone())
                .unwrap_or_default(),
            live: slot.session.is_live(),
        })
    }

    /// Строки списка подключений: записи реестра с живым состоянием слотов.
    pub(crate) fn rows(&self) -> Vec<ConnectionRow> {
        // Домашний инстанс показывается отдельной карточкой, а не в списке
        // пиринговых: запись реестра, указывающую на него, скрываем.
        let home = self.slots[0]
            .base
            .clone()
            .or_else(|| self.slots[0].session.base().map(str::to_owned));
        let mut rows: Vec<ConnectionRow> = self
            .registry
            .entries()
            .iter()
            .filter(|entry| {
                !home
                    .as_deref()
                    .is_some_and(|home| same_instance(home, &entry.origin))
            })
            .map(|entry| self.row_for(entry))
            .collect();
        // Слоты, которых в реестре ещё нет (идёт первое подключение).
        for slot in self.slots.iter().skip(1) {
            let Some(base) = slot.base.as_deref() else {
                continue;
            };
            if self.registry.get(base).is_some() {
                continue;
            }
            rows.push(ConnectionRow {
                origin: base.to_owned(),
                label: host_label(base),
                username: slot.username.clone().unwrap_or_default(),
                remote_user_id: String::new(),
                state: Self::slot_state(slot).unwrap_or(RowState::AuthExpired),
                reason: None,
                detail: slot.session.last_error().map(str::to_owned),
                added_at: now_ms(),
                last_connected_at: None,
                disconnected_at: None,
                unread: slot.session.unread_total(),
                peering: slot.peering.clone(),
                offer_own_login: Self::offers_own_login(slot),
            });
        }
        rows
    }

    fn row_for(&self, entry: &RegistryEntry) -> ConnectionRow {
        let slot = self
            .slots
            .iter()
            .skip(1)
            .find(|s| s.base.as_deref() == Some(entry.origin.as_str()));
        let state = slot
            .and_then(Self::slot_state)
            .unwrap_or(match entry.status {
                RegistryStatus::Connected | RegistryStatus::Unreachable => RowState::Unreachable,
                RegistryStatus::Disconnected => RowState::Disconnected,
                RegistryStatus::AuthExpired => RowState::AuthExpired,
            });
        let detail = slot
            .filter(|s| s.session.is_idle())
            .and_then(|s| s.session.last_error().map(str::to_owned));
        ConnectionRow {
            origin: entry.origin.clone(),
            label: if entry.label.is_empty() {
                host_label(&entry.origin)
            } else {
                entry.label.clone()
            },
            username: entry.username.clone(),
            remote_user_id: entry.remote_user_id.clone(),
            state,
            reason: entry.error_message.clone(),
            detail,
            added_at: entry.added_at,
            last_connected_at: entry.last_connected_at,
            disconnected_at: entry.disconnected_at,
            unread: slot.map_or(0, |s| s.session.unread_total()),
            peering: slot.and_then(|s| s.peering.clone()),
            offer_own_login: slot.is_some_and(Self::offers_own_login),
        }
    }

    /// Живое состояние слота; `None` — слот ничего не говорит (нет сессии
    /// или она закончилась), решает реестр.
    fn slot_state(slot: &Slot) -> Option<RowState> {
        if slot.session.is_live() {
            Some(RowState::Connected)
        } else if slot.awaiting_credential || !slot.session.is_idle() || slot.restart.is_some() {
            Some(RowState::Connecting)
        } else {
            None
        }
    }

    fn offers_own_login(slot: &Slot) -> bool {
        slot.session.is_idle()
            && matches!(
                slot.session.failure(),
                Some(FailureKind::CredentialRefused | FailureKind::RegistrationClosed)
            )
    }
}

/// Понятный текст ошибки удаления учётной записи.
fn delete_error(code: Option<&str>) -> String {
    match code {
        Some("owns_spaces" | "user_owns_spaces") => {
            tr("сначала передайте владение пространствами").to_owned()
        }
        Some(other) if !other.is_empty() => other.to_owned(),
        _ => tr("не удалось").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn same_instance_ignores_case_slash_and_path() {
        assert!(super::same_instance(
            "https://Home.example/",
            "https://home.example"
        ));
        assert!(super::same_instance(
            "home.example",
            "https://home.example/app"
        ));
        assert!(!super::same_instance(
            "https://a.example",
            "https://b.example"
        ));
        assert!(!super::same_instance(
            "http://a.example",
            "https://a.example"
        ));
    }

    use super::{Bridge, canonical_base};

    #[test]
    fn connecting_needs_a_live_home_session() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut bridge = Bridge::new(runtime.handle().clone());
        bridge.connect_remote("orbit.example", Some("pw"));
        assert!(bridge.take_message().unwrap().contains("домашний"));
        assert_eq!(bridge.rows().len(), 0);
        bridge.focus_on(5);
        assert_eq!(bridge.focus_index(), 0);
    }

    #[test]
    fn bad_address_is_reported() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut bridge = Bridge::new(runtime.handle().clone());
        bridge.connect_remote("   ", None);
        assert!(bridge.take_message().unwrap().contains("адрес"));
    }

    #[test]
    fn join_without_home_is_not_queued() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut bridge = Bridge::new(runtime.handle().clone());
        bridge.connect_and_join("orbit.example", "abc123");
        assert!(bridge.take_message().unwrap().contains("домашний"));
        assert!(
            bridge.pending_joins.is_empty(),
            "без подключения ждать нечего"
        );
        bridge.connect_and_join_space("   ", "s1", false);
        assert!(bridge.take_message().unwrap().contains("адрес"));
    }

    #[test]
    fn base_is_canonicalised() {
        assert_eq!(
            canonical_base("Orbit.Example").as_deref(),
            Some("https://orbit.example")
        );
        assert_eq!(
            canonical_base("https://orbit.example:8443/path/?q=1").as_deref(),
            Some("https://orbit.example:8443")
        );
        assert_eq!(canonical_base("  "), None);
        assert_eq!(canonical_base("https://user:pw@orbit.example"), None);
    }
}
