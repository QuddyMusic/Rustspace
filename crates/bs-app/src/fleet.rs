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

use bs_api::ReplicatedEntry;
use bs_types::TransportPolicy;
use tokio::runtime::Handle;

use crate::bridge::{BridgeRequest, Secret, Session};

/// Сколько чужих инстансов можно подключить: домашний сервер сам присылает
/// список связанных инстансов, и бесконечно ходить по нему клиент не должен.
pub(crate) const MAX_REMOTES: usize = 8;

/// Слот: один инстанс.
#[derive(Debug)]
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
        }
    }
}

/// Состояние инстанса для списка в интерфейсе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstanceState {
    /// Работает.
    Live,
    /// Подключается.
    Connecting,
    /// Не подключён (с причиной, если известна).
    Failed(Option<String>),
}

/// Строка списка инстансов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstanceView {
    /// Номер слота.
    pub(crate) index: usize,
    /// Название для показа.
    pub(crate) name: String,
    /// Адрес.
    pub(crate) base: String,
    /// Имя пользователя на инстансе.
    pub(crate) username: Option<String>,
    /// Домашний ли это инстанс.
    pub(crate) home: bool,
    /// Состояние соединения.
    pub(crate) state: InstanceState,
    /// Число каналов и бесед с непрочитанным.
    pub(crate) unread: usize,
    /// Статус пиринга, как его назвал сервер.
    pub(crate) peering: Option<String>,
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
        }
    }

    /// Вход в домашний инстанс.
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

    /// Выход: закрывает все сессии.
    pub(crate) fn logout(&mut self) {
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
        self.deliver_credentials();
        self.deliver_peering();
        self.track_liveness();
        self.collect_losses();
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
                    slot.session
                        .start_remote(&credential, &base, name, transport);
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
                        "rejected" => Some(format!("Пиринг с {name} отклонён")),
                        "pending" | "awaiting_approval" => {
                            Some(format!("Пиринг с {name} ожидает подтверждения"))
                        }
                        "admin_required" => Some(format!(
                            "Администратор вашего инстанса должен одобрить связь с {name}"
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

    /// Домашний инстанс заработал: поднимаем связанные с ним инстансы.
    fn on_home_live(&mut self) {
        let home_base = self.slots[0].base.clone();
        let entries: Vec<(String, String)> = self.slots[0]
            .session
            .cache()
            .map(|cache| {
                cache
                    .me()
                    .replicated_instances
                    .iter()
                    .map(|r| (r.origin.clone(), r.username.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for (origin, username) in entries {
            let Some(base) = canonical_base(&origin) else {
                continue;
            };
            if home_base.as_deref() == Some(base.as_str()) {
                continue;
            }
            self.add_remote(&base, None, Some(username));
        }
    }

    /// Чужой инстанс заработал: просим дом о пиринге и рассылаем список.
    fn on_remote_live(&mut self, index: usize) {
        let Some(base) = self.slots[index].base.clone() else {
            return;
        };
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
    /// Не подключившиеся инстансы остаются в списке: иначе временный сбой
    /// стёр бы их с сервера.
    fn sync_topology(&self) {
        if self.slots.iter().skip(1).any(|s| s.awaiting_credential) {
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
        let remotes: Vec<ReplicatedEntry> = self
            .slots
            .iter()
            .skip(1)
            .filter_map(|s| {
                Some(ReplicatedEntry {
                    origin: s.base.clone()?,
                    username: s.username.clone()?,
                })
            })
            .collect();
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
        }
    }

    fn abort_remotes(&mut self) {
        for slot in self.slots.iter_mut().skip(1) {
            slot.session.logout();
        }
        self.slots.truncate(1);
    }

    /// Подключает чужой инстанс. `password` — пароль домашнего аккаунта:
    /// проверяется домом, на чужой инстанс не уходит. Без пароля (автоподключение)
    /// достаточно живой сессии дома.
    pub(crate) fn connect_remote(&mut self, raw_base: &str, password: Option<&str>) {
        let Some(base) = canonical_base(raw_base) else {
            self.message = Some("Некорректный адрес инстанса".to_owned());
            return;
        };
        self.add_remote(&base, password, None);
    }

    fn add_remote(&mut self, base: &str, password: Option<&str>, username: Option<String>) {
        if !self.slots[0].session.is_live() {
            self.message = Some("Сначала войдите в домашний инстанс".to_owned());
            return;
        }
        if let Ok(url) = url::Url::parse(base)
            && let Err(err) = self.transport.check(&url)
        {
            self.message = Some(err.to_string());
            return;
        }
        if self.slots[0].base.as_deref() == Some(base) {
            self.message = Some("Это ваш домашний инстанс: он уже подключён".to_owned());
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
                self.message = Some("Этот инстанс уже подключён".to_owned());
                return;
            }
            self.slots.remove(existing + 1);
            self.focus = 0;
        }
        if self.slots.len() > MAX_REMOTES {
            self.message = Some(format!(
                "Можно подключить не больше {MAX_REMOTES} инстансов"
            ));
            return;
        }
        let mut slot = Slot::new(&self.runtime);
        slot.base = Some(base.to_owned());
        slot.username = username;
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

    /// Повторяет подключение к инстансу, который не поднялся.
    pub(crate) fn retry(&mut self, index: usize) {
        if index == 0 || index >= self.slots.len() || !self.slots[index].session.is_idle() {
            return;
        }
        let Some(base) = self.slots[index].base.clone() else {
            return;
        };
        let username = self.slots[index].username.clone();
        self.slots.remove(index);
        self.focus = 0;
        self.add_remote(&base, None, username);
    }

    /// Отключает чужой инстанс и забывает его.
    pub(crate) fn disconnect(&mut self, index: usize) {
        if index == 0 || index >= self.slots.len() {
            return;
        }
        self.slots[index].session.logout();
        self.slots.remove(index);
        self.focus = 0;
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

    /// Кэш инстанса по номеру.
    pub(crate) fn cache_at(&self, index: usize) -> Option<&bs_core::Cache> {
        self.slots.get(index)?.session.cache()
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
                    .map(|(path, result)| (base.clone(), path, result)),
            );
        }
        all
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

    /// Описание всех инстансов для интерфейса.
    pub(crate) fn instances(&self) -> Vec<InstanceView> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let base = slot
                    .base
                    .clone()
                    .or_else(|| slot.session.base().map(str::to_owned))?;
                let state = if slot.session.is_live() {
                    InstanceState::Live
                } else if slot.awaiting_credential || !slot.session.is_idle() {
                    InstanceState::Connecting
                } else {
                    InstanceState::Failed(slot.session.last_error().map(str::to_owned))
                };
                Some(InstanceView {
                    index,
                    name: host_label(&base),
                    base,
                    username: slot
                        .username
                        .clone()
                        .or_else(|| slot.session.cache().map(|c| c.me().username.clone())),
                    home: index == 0,
                    state,
                    unread: slot.session.unread_total(),
                    peering: slot.peering.clone(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Bridge, canonical_base};

    #[test]
    fn connecting_needs_a_live_home_session() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut bridge = Bridge::new(runtime.handle().clone());
        bridge.connect_remote("orbit.example", Some("pw"));
        assert!(bridge.take_message().unwrap().contains("домашний"));
        assert_eq!(bridge.instances().len(), 0);
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
