//! Реестр подключённых инстансов: что клиент помнит между запусками.
//!
//! Источник правды — домашний инстанс (`/users/@me/federation-registry`),
//! как в оригинале: на нём записаны статус каждого подключения, имя на
//! инстансе, время добавления и последнего подключения. Запись реестра
//! живёт отдельно от сессии: отключённый или отказавший инстанс остаётся
//! в списке и ждёт действий пользователя.
//!
//! Модуль не знает про сеть и интерфейс; он только хранит записи и
//! двигает статусы по тем же правилам, что `writeConnectionState` в
//! `instanceStore.ts`.

use bs_api::{RegistryEntry, RegistryResponse, RegistryStatus};

/// Причины, которые клиент записывает в `errorMessage` (коды, не фразы:
/// слова подбирает интерфейс).
pub(crate) mod reason {
    /// Инстанс не ответил.
    pub(crate) const UNREACHABLE: &str = "unreachable";
    /// Сохранённая сессия отвергнута.
    pub(crate) const SESSION_EXPIRED: &str = "session_expired";
    /// Известное подключение без сессии: нужен вход.
    pub(crate) const REAUTHENTICATE: &str = "reauthenticate";
    /// То же для домашнего инстанса федеративного аккаунта.
    pub(crate) const AUTHENTICATE_HOME: &str = "authenticate_home";
}

/// Текущее время, мс с начала эпохи.
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Реестр с отметкой последней записи.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Registry {
    entries: Vec<RegistryEntry>,
    updated_at: i64,
    /// Серверный реестр прочитан: только тогда его можно записывать, иначе
    /// сбой чтения затёр бы чужие записи.
    authoritative: bool,
    /// Есть изменения, которых сервер ещё не видел.
    dirty: bool,
    /// Реестр собран (с сервера или из запасных источников).
    settled: bool,
}

impl Registry {
    /// Записи реестра.
    pub(crate) fn entries(&self) -> &[RegistryEntry] {
        &self.entries
    }

    /// Запись по адресу.
    pub(crate) fn get(&self, origin: &str) -> Option<&RegistryEntry> {
        self.entries.iter().find(|e| e.origin == origin)
    }

    /// Собран ли реестр.
    pub(crate) const fn is_settled(&self) -> bool {
        self.settled
    }

    /// Очищает всё при выходе.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Собирает реестр: серверные записи плюс то, что известно из профиля
    /// (`replicatedInstances`) и не попало в реестр (старые подключения или
    /// сбой чтения). Такие записи показываем как требующие входа.
    ///
    /// `server` — `None`, если чтение не удалось: тогда реестр только для
    /// показа и на сервер не пишется.
    pub(crate) fn load(
        &mut self,
        server: Option<RegistryResponse>,
        known: &[(String, String)],
        now: i64,
    ) {
        self.authoritative = server.is_some();
        self.updated_at = server.as_ref().map_or(0, |s| s.updated_at);
        self.entries = server.map(|s| s.registry).unwrap_or_default();
        for (origin, username) in known {
            if self.get(origin).is_some() {
                continue;
            }
            self.entries.push(RegistryEntry {
                origin: origin.clone(),
                label: host_of(origin),
                username: username.clone(),
                remote_user_id: String::new(),
                status: RegistryStatus::AuthExpired,
                added_at: now,
                last_connected_at: None,
                disconnected_at: None,
                error_message: Some(reason::REAUTHENTICATE.to_owned()),
            });
        }
        self.settled = true;
        self.dirty = false;
    }

    /// Куда подключаться при запуске: все, кроме отключённых вручную.
    pub(crate) fn auto_targets(&self) -> Vec<(String, String)> {
        self.entries
            .iter()
            .filter(|e| e.status != RegistryStatus::Disconnected)
            .map(|e| (e.origin.clone(), e.username.clone()))
            .collect()
    }

    /// Все записи как список для `replicatedInstances`.
    pub(crate) fn replicated(&self) -> Vec<bs_api::ReplicatedEntry> {
        self.entries
            .iter()
            .map(|e| bs_api::ReplicatedEntry {
                origin: e.origin.clone(),
                username: e.username.clone(),
            })
            .collect()
    }

    /// Сессия установлена.
    pub(crate) fn mark_connected(
        &mut self,
        origin: &str,
        label: &str,
        username: &str,
        remote_user_id: &str,
        now: i64,
    ) {
        let entry = self.entry_mut(origin, now);
        if !label.is_empty() {
            label.clone_into(&mut entry.label);
        }
        if !username.is_empty() {
            username.clone_into(&mut entry.username);
        }
        if !remote_user_id.is_empty() {
            remote_user_id.clone_into(&mut entry.remote_user_id);
        }
        entry.status = RegistryStatus::Connected;
        entry.last_connected_at = Some(now);
        entry.disconnected_at = None;
        entry.error_message = None;
        self.touch(now);
    }

    /// Пользователь отключил инстанс.
    pub(crate) fn mark_disconnected(&mut self, origin: &str, now: i64) {
        let entry = self.entry_mut(origin, now);
        entry.status = RegistryStatus::Disconnected;
        entry.disconnected_at = Some(now);
        entry.error_message = None;
        self.touch(now);
    }

    /// Инстанс не ответил.
    pub(crate) fn mark_unreachable(&mut self, origin: &str, now: i64) {
        let entry = self.entry_mut(origin, now);
        entry.status = RegistryStatus::Unreachable;
        entry.error_message = Some(reason::UNREACHABLE.to_owned());
        self.touch(now);
    }

    /// Сессию не приняли: войти можно только заново.
    pub(crate) fn mark_auth_expired(&mut self, origin: &str, code: &str, now: i64) {
        let entry = self.entry_mut(origin, now);
        entry.status = RegistryStatus::AuthExpired;
        entry.error_message = Some(code.to_owned());
        self.touch(now);
    }

    /// Убирает запись.
    pub(crate) fn remove(&mut self, origin: &str, now: i64) {
        let before = self.entries.len();
        self.entries.retain(|e| e.origin != origin);
        if self.entries.len() != before {
            self.touch(now);
        }
    }

    /// Что и с какой меткой записать на сервер; `None`, если писать нечего
    /// или сервер ещё не прочитан.
    pub(crate) fn take_pending_write(&mut self) -> Option<(Vec<RegistryEntry>, i64)> {
        if !self.authoritative || !self.dirty {
            return None;
        }
        self.dirty = false;
        Some((self.entries.clone(), self.updated_at))
    }

    fn entry_mut(&mut self, origin: &str, now: i64) -> &mut RegistryEntry {
        if let Some(index) = self.entries.iter().position(|e| e.origin == origin) {
            return &mut self.entries[index];
        }
        self.entries.push(RegistryEntry {
            origin: origin.to_owned(),
            label: host_of(origin),
            username: String::new(),
            remote_user_id: String::new(),
            status: RegistryStatus::AuthExpired,
            added_at: now,
            last_connected_at: None,
            disconnected_at: None,
            error_message: None,
        });
        let last = self.entries.len() - 1;
        &mut self.entries[last]
    }

    /// Метка записи строго растёт: сервер отвергает не новее прежней.
    fn touch(&mut self, now: i64) {
        self.updated_at = now.max(self.updated_at.saturating_add(1));
        self.dirty = true;
    }
}

/// Хост адреса для подписи.
fn host_of(origin: &str) -> String {
    url::Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_else(|| origin.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(entries: Vec<RegistryEntry>, updated_at: i64) -> RegistryResponse {
        RegistryResponse {
            registry: entries,
            updated_at,
        }
    }

    fn known(origin: &str) -> Vec<(String, String)> {
        vec![(origin.to_owned(), "q@home".to_owned())]
    }

    #[test]
    fn profile_instances_missing_from_the_server_registry_need_a_login() {
        let mut registry = Registry::default();
        registry.load(Some(server(vec![], 5)), &known("https://a.example"), 100);
        let entry = registry.get("https://a.example").expect("entry");
        assert_eq!(entry.status, RegistryStatus::AuthExpired);
        assert_eq!(entry.error_message.as_deref(), Some(reason::REAUTHENTICATE));
        assert_eq!(registry.auto_targets().len(), 1);
    }

    #[test]
    fn a_manual_disconnect_stops_auto_connect() {
        let mut registry = Registry::default();
        registry.load(Some(server(vec![], 1)), &known("https://a.example"), 10);
        registry.mark_disconnected("https://a.example", 20);
        assert!(registry.auto_targets().is_empty());
        assert!(registry.get("https://a.example").is_some());
    }

    #[test]
    fn nothing_is_written_until_the_server_registry_was_read() {
        let mut registry = Registry::default();
        registry.load(None, &known("https://a.example"), 10);
        registry.mark_connected("https://a.example", "A", "q@home", "7", 20);
        assert!(registry.take_pending_write().is_none());
    }

    #[test]
    fn write_stamp_always_grows() {
        let mut registry = Registry::default();
        registry.load(Some(server(vec![], 500)), &[], 10);
        registry.mark_connected("https://a.example", "A", "q@home", "7", 20);
        let (_, stamp) = registry.take_pending_write().expect("write");
        assert!(stamp > 500);
        assert!(registry.take_pending_write().is_none());
    }

    #[test]
    fn connect_records_identity_and_clears_the_reason() {
        let mut registry = Registry::default();
        registry.load(Some(server(vec![], 1)), &known("https://a.example"), 10);
        registry.mark_connected("https://a.example", "Орбита", "q@home", "42", 30);
        let entry = registry.get("https://a.example").expect("entry");
        assert_eq!(entry.status, RegistryStatus::Connected);
        assert_eq!(entry.remote_user_id, "42");
        assert_eq!(entry.last_connected_at, Some(30));
        assert_eq!(entry.error_message, None);
    }

    #[test]
    fn remove_forgets_the_entry() {
        let mut registry = Registry::default();
        registry.load(Some(server(vec![], 1)), &known("https://a.example"), 10);
        registry.remove("https://a.example", 20);
        assert!(registry.get("https://a.example").is_none());
        assert!(registry.take_pending_write().is_some());
    }
}
