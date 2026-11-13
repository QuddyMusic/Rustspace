//! Соединения WebSocket: кто на связи, кому что отправить.
//!
//! События собираются в [`Outbox`] внутри транзакции и уходят только после
//! её успешного завершения — отменённое изменение никому не объявляется.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use serde_json::Value;
use tokio::sync::{Notify, mpsc};

/// Сколько сообщений может ждать отправки одному соединению. Клиент, который
/// не читает, отключается, а не копит память сервера.
pub const SEND_QUEUE: usize = 2048;

/// Сообщение для задачи, пишущей в сокет.
#[derive(Debug, Clone)]
pub enum Outgoing {
    /// Текстовый кадр.
    Text(Arc<str>),
    /// Закрыть соединение.
    Close,
}

/// Отправляющая сторона одного соединения.
pub type Sender = mpsc::Sender<Outgoing>;

#[derive(Debug)]
struct Connection {
    id: u64,
    sender: Sender,
    closer: Arc<Notify>,
}

/// Ведро токенов на пользователя (общее для всех его вкладок): 30 событий
/// сразу и ещё 2 в секунду.
#[derive(Debug)]
struct Bucket {
    tokens: f64,
    refilled: Instant,
}

const BUCKET_SIZE: f64 = 30.0;
const BUCKET_RATE: f64 = 2.0;

#[derive(Debug, Default)]
struct Inner {
    connections: HashMap<String, Vec<Connection>>,
    spaces: HashMap<String, HashSet<String>>,
    buckets: HashMap<String, Bucket>,
    /// Поколение отложенного ухода в офлайн: новое соединение его отменяет.
    offline_generation: HashMap<String, u64>,
    statuses: HashMap<String, String>,
    show_activity: HashMap<String, bool>,
    activities: HashMap<String, Vec<Value>>,
    /// Счётчики ограничителей с фиксированным окном: начало окна и число.
    windows: HashMap<String, (Instant, u32)>,
    /// Кого надо отключить от медиасервера: комната и участник.
    evictions: Vec<(String, String)>,
    /// Работа со звонками для других инстансов, ждущая своего исполнителя.
    calls: Vec<crate::voice::CallTask>,
}

/// Концентратор соединений.
#[derive(Debug, Clone, Default)]
pub struct Hub {
    /// Голосовые комнаты и звонки.
    pub voice: crate::voice::Voice,
    inner: Arc<Mutex<Inner>>,
    next_id: Arc<AtomicU64>,
    evicted: Arc<tokio::sync::Notify>,
    calls_ready: Arc<tokio::sync::Notify>,
}

/// Кому адресовано событие.
#[derive(Debug, Clone)]
enum Target {
    User(String),
    Space {
        space: String,
        except: Option<String>,
    },
    Connection(u64),
}

#[derive(Debug, Clone)]
enum Item {
    Send(Target, Arc<str>),
    JoinSpace { user: String, space: String },
    LeaveSpace { user: String, space: String },
    DropSpace(String),
    CloseUser(String),
    Evict { room: String, user: String },
    Call(Box<crate::voice::CallTask>),
}

/// События и изменения подписок, ждущие завершения транзакции. Выполняются
/// по порядку.
#[derive(Debug, Default)]
pub struct Outbox {
    items: Vec<Item>,
}

impl Outbox {
    /// Пустой набор.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn push(&mut self, target: Target, event: &Value) {
        self.items
            .push(Item::Send(target, event.to_string().into()));
    }

    /// Всем соединениям пользователя.
    pub fn to_user(&mut self, user_id: &str, event: &Value) {
        self.push(Target::User(user_id.to_owned()), event);
    }

    /// Всем участникам пространства, которые на связи.
    pub fn to_space(&mut self, space_id: &str, event: &Value) {
        self.push(
            Target::Space {
                space: space_id.to_owned(),
                except: None,
            },
            event,
        );
    }

    /// Участникам пространства, кроме одного.
    pub fn to_space_except(&mut self, space_id: &str, except: &str, event: &Value) {
        self.push(
            Target::Space {
                space: space_id.to_owned(),
                except: Some(except.to_owned()),
            },
            event,
        );
    }

    /// Одному соединению.
    pub fn to_connection(&mut self, connection: u64, event: &Value) {
        self.push(Target::Connection(connection), event);
    }

    /// Пользователь вступил в пространство: дальше он получает его события.
    pub fn join_space(&mut self, user_id: &str, space_id: &str) {
        self.items.push(Item::JoinSpace {
            user: user_id.to_owned(),
            space: space_id.to_owned(),
        });
    }

    /// Пользователь больше не в пространстве: его события ему не идут.
    pub fn leave_space(&mut self, user_id: &str, space_id: &str) {
        self.items.push(Item::LeaveSpace {
            user: user_id.to_owned(),
            space: space_id.to_owned(),
        });
    }

    /// Пространство удалено.
    pub fn drop_space(&mut self, space_id: &str) {
        self.items.push(Item::DropSpace(space_id.to_owned()));
    }

    /// Закрыть все соединения пользователя.
    pub fn close_user(&mut self, user_id: &str) {
        self.items.push(Item::CloseUser(user_id.to_owned()));
    }

    /// Отключает участника от комнаты на медиасервере: модератор выгнал
    /// его или он потерял право быть в канале.
    pub fn evict_media(&mut self, room: &str, user_id: &str) {
        self.items.push(Item::Evict {
            room: room.to_owned(),
            user: user_id.to_owned(),
        });
    }

    /// Работа со звонком для других инстансов: выполняется после фиксации
    /// исполнителем звонков федерации.
    pub fn call(&mut self, task: crate::voice::CallTask) {
        self.items.push(Item::Call(Box::new(task)));
    }

    /// Переносит всё из другого набора в конец этого.
    pub fn append(&mut self, mut other: Self) {
        self.items.append(&mut other.items);
    }

    /// Нечего отправлять.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl Hub {
    /// Ждёт и забирает накопившуюся работу со звонками для других инстансов.
    pub async fn next_call_tasks(&self) -> Vec<crate::voice::CallTask> {
        loop {
            let pending = std::mem::take(&mut self.lock().calls);
            if !pending.is_empty() {
                return pending;
            }
            self.calls_ready.notified().await;
        }
    }

    /// Ждёт и забирает накопившиеся отключения от медиасервера.
    pub async fn next_evictions(&self) -> Vec<(String, String)> {
        loop {
            let pending = std::mem::take(&mut self.lock().evictions);
            if !pending.is_empty() {
                return pending;
            }
            self.evicted.notified().await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Выдаёт номер для будущего соединения.
    #[must_use]
    pub fn next_connection_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Регистрирует соединение под номером `id`. `first` кладётся в его
    /// очередь до того, как соединение станет видно рассылкам, так что
    /// `ready` всегда приходит раньше событий. `closer` будит задачу
    /// соединения, когда его надо оборвать.
    pub fn add_connection(
        &self,
        user_id: &str,
        id: u64,
        sender: Sender,
        closer: Arc<Notify>,
        first: Option<Arc<str>>,
    ) {
        let mut inner = self.lock();
        if let Some(first) = first {
            let _ = sender.try_send(Outgoing::Text(first));
        }
        inner.offline_generation.remove(user_id);
        inner
            .connections
            .entry(user_id.to_owned())
            .or_default()
            .push(Connection { id, sender, closer });
    }

    /// Убирает соединение по номеру, не зная пользователя (вход не удался).
    pub fn forget_connection(&self, connection: u64) {
        let mut inner = self.lock();
        inner.connections.retain(|_, list| {
            list.retain(|conn| conn.id != connection);
            !list.is_empty()
        });
    }

    /// Убирает соединение. Если оно было последним, возвращает поколение
    /// отложенного ухода в офлайн: уход состоится, только если к его сроку
    /// поколение не сменилось ([`Self::take_offline`]).
    pub fn remove_connection(&self, user_id: &str, connection: u64) -> Option<u64> {
        let mut inner = self.lock();
        let list = inner.connections.get_mut(user_id)?;
        list.retain(|conn| conn.id != connection);
        if !list.is_empty() {
            return None;
        }
        inner.connections.remove(user_id);
        let generation = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        inner
            .offline_generation
            .insert(user_id.to_owned(), generation);
        Some(generation)
    }

    /// Срок отложенного ухода вышел: `true`, если пользователь так и не
    /// вернулся и его состояние сеанса забыто.
    pub fn take_offline(&self, user_id: &str, generation: u64) -> bool {
        let mut inner = self.lock();
        if inner.offline_generation.get(user_id) != Some(&generation)
            || inner.connections.contains_key(user_id)
        {
            return false;
        }
        inner.offline_generation.remove(user_id);
        inner.spaces.remove(user_id);
        inner.buckets.remove(user_id);
        inner.statuses.remove(user_id);
        inner.show_activity.remove(user_id);
        inner.activities.remove(user_id);
        true
    }

    /// Есть ли у пользователя открытое соединение.
    #[must_use]
    pub fn is_online(&self, user_id: &str) -> bool {
        self.lock().connections.contains_key(user_id)
    }

    /// Пользователи с открытыми соединениями.
    #[must_use]
    pub fn online_users(&self) -> Vec<String> {
        self.lock().connections.keys().cloned().collect()
    }

    /// Пользователи на связи, состоящие в пространстве.
    #[must_use]
    pub fn online_in_space(&self, space_id: &str) -> Vec<String> {
        let inner = self.lock();
        inner
            .connections
            .keys()
            .filter(|user| {
                inner
                    .spaces
                    .get(*user)
                    .is_some_and(|set| set.contains(space_id))
            })
            .cloned()
            .collect()
    }

    /// Запоминает пространства пользователя (для рассылок).
    pub fn set_user_spaces(&self, user_id: &str, spaces: impl IntoIterator<Item = String>) {
        self.lock()
            .spaces
            .insert(user_id.to_owned(), spaces.into_iter().collect());
    }

    /// Пользователь вступил в пространство.
    pub fn add_user_space(&self, user_id: &str, space_id: &str) {
        self.lock()
            .spaces
            .entry(user_id.to_owned())
            .or_default()
            .insert(space_id.to_owned());
    }

    /// Пользователь покинул пространство.
    pub fn remove_user_space(&self, user_id: &str, space_id: &str) {
        if let Some(set) = self.lock().spaces.get_mut(user_id) {
            set.remove(space_id);
        }
    }

    /// Пространства нет: забыть его у всех.
    pub fn drop_space(&self, space_id: &str) {
        for set in self.lock().spaces.values_mut() {
            set.remove(space_id);
        }
    }

    /// Статус, с которым сеансы пользователя публикуют активность.
    pub fn set_status(&self, user_id: &str, status: &str) {
        self.lock()
            .statuses
            .insert(user_id.to_owned(), status.to_owned());
    }

    /// Запомненный статус.
    #[must_use]
    pub fn status(&self, user_id: &str) -> Option<String> {
        self.lock().statuses.get(user_id).cloned()
    }

    /// Показывает ли пользователь активность.
    pub fn set_show_activity(&self, user_id: &str, show: bool) {
        self.lock().show_activity.insert(user_id.to_owned(), show);
    }

    /// Показывает ли пользователь активность (по умолчанию — да).
    #[must_use]
    pub fn show_activity(&self, user_id: &str) -> bool {
        self.lock()
            .show_activity
            .get(user_id)
            .copied()
            .unwrap_or(true)
    }

    /// Текущая активность пользователя.
    #[must_use]
    pub fn activities(&self, user_id: &str) -> Vec<Value> {
        self.lock()
            .activities
            .get(user_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Заменяет активность пользователя.
    pub fn set_activities(&self, user_id: &str, activities: Vec<Value>) {
        let mut inner = self.lock();
        if activities.is_empty() {
            inner.activities.remove(user_id);
        } else {
            inner.activities.insert(user_id.to_owned(), activities);
        }
    }

    /// Списывает токен за событие клиента; `false` — лимит исчерпан.
    pub fn consume(&self, user_id: &str) -> bool {
        let now = Instant::now();
        let mut inner = self.lock();
        let bucket = inner.buckets.entry(user_id.to_owned()).or_insert(Bucket {
            tokens: BUCKET_SIZE,
            refilled: now,
        });
        let elapsed = now.duration_since(bucket.refilled).as_secs_f64();
        bucket.tokens = elapsed.mul_add(BUCKET_RATE, bucket.tokens).min(BUCKET_SIZE);
        bucket.refilled = now;
        if bucket.tokens < 1.0 {
            return false;
        }
        bucket.tokens -= 1.0;
        true
    }

    /// Ограничитель с фиксированным окном: не больше `max` раз за `window`
    /// на ключ. `false` — предел исчерпан.
    pub fn allow(&self, key: &str, max: u32, window: std::time::Duration) -> bool {
        let now = Instant::now();
        let mut inner = self.lock();
        if inner.windows.len() > 100_000 {
            inner
                .windows
                .retain(|_, (start, _)| now.duration_since(*start) < window);
        }
        let entry = inner.windows.entry(key.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }
        if entry.1 >= max {
            return false;
        }
        entry.1 += 1;
        true
    }

    /// Исчерпан ли предел [`Hub::allow`] для ключа; сам счётчик не трогает.
    #[must_use]
    pub fn exhausted(&self, key: &str, max: u32, window: std::time::Duration) -> bool {
        self.lock()
            .windows
            .get(key)
            .is_some_and(|(start, count)| start.elapsed() < window && *count >= max)
    }

    /// Отправляет накопленные события. Соединение с переполненной очередью
    /// закрывается.
    pub fn flush(&self, outbox: Outbox) {
        if outbox.is_empty() {
            return;
        }
        let mut inner = self.lock();
        for item in outbox.items {
            let (target, text) = match item {
                Item::Send(target, text) => (target, text),
                Item::JoinSpace { user, space } => {
                    inner.spaces.entry(user).or_default().insert(space);
                    continue;
                }
                Item::LeaveSpace { user, space } => {
                    if let Some(set) = inner.spaces.get_mut(&user) {
                        set.remove(&space);
                    }
                    continue;
                }
                Item::DropSpace(space) => {
                    for set in inner.spaces.values_mut() {
                        set.remove(&space);
                    }
                    continue;
                }
                Item::Evict { room, user } => {
                    inner.evictions.push((room, user));
                    self.evicted.notify_one();
                    continue;
                }
                Item::Call(task) => {
                    inner.calls.push(*task);
                    self.calls_ready.notify_one();
                    continue;
                }
                Item::CloseUser(user) => {
                    for conn in inner.connections.get(&user).into_iter().flatten() {
                        if conn.sender.try_send(Outgoing::Close).is_err() {
                            conn.closer.notify_one();
                        }
                    }
                    continue;
                }
            };
            match target {
                Target::User(user) => {
                    if let Some(list) = inner.connections.get(&user) {
                        deliver(list, &text);
                    }
                }
                Target::Space { space, except } => {
                    for (user, list) in &inner.connections {
                        if except.as_deref() == Some(user.as_str()) {
                            continue;
                        }
                        if inner
                            .spaces
                            .get(user)
                            .is_some_and(|set| set.contains(&space))
                        {
                            deliver(list, &text);
                        }
                    }
                }
                Target::Connection(id) => {
                    for list in inner.connections.values() {
                        if let Some(conn) = list.iter().find(|conn| conn.id == id) {
                            deliver(std::slice::from_ref(conn), &text);
                        }
                    }
                }
            }
        }
    }

    /// Отправляет одно событие пользователю сразу.
    pub fn send_to_user(&self, user_id: &str, event: &Value) {
        let mut outbox = Outbox::new();
        outbox.to_user(user_id, event);
        self.flush(outbox);
    }

    /// Закрывает все соединения пользователя (удаление аккаунта, смена пароля).
    pub fn close_user(&self, user_id: &str) {
        if let Some(list) = self.lock().connections.get(user_id) {
            for conn in list {
                if conn.sender.try_send(Outgoing::Close).is_err() {
                    conn.closer.notify_one();
                }
            }
        }
    }
}

fn deliver(list: &[Connection], text: &Arc<str>) {
    for conn in list {
        if let Err(mpsc::error::TrySendError::Full(_)) =
            conn.sender.try_send(Outgoing::Text(Arc::clone(text)))
        {
            tracing::warn!(
                connection = conn.id,
                "очередь отправки переполнена, закрываю"
            );
            conn.closer.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn events_reach_only_their_audience() {
        let hub = Hub::default();
        let (a_tx, mut a_rx) = mpsc::channel(8);
        let (b_tx, mut b_rx) = mpsc::channel(8);
        hub.add_connection("a", 1, a_tx, Arc::default(), None);
        hub.add_connection("b", 2, b_tx, Arc::default(), None);
        hub.set_user_spaces("a", ["s".to_owned()]);
        let mut outbox = Outbox::new();
        outbox.to_space("s", &json!({"type": "x"}));
        outbox.to_user("b", &json!({"type": "y"}));
        hub.flush(outbox);
        assert!(matches!(a_rx.try_recv(), Ok(Outgoing::Text(t)) if t.contains("\"x\"")));
        assert!(a_rx.try_recv().is_err());
        assert!(matches!(b_rx.try_recv(), Ok(Outgoing::Text(t)) if t.contains("\"y\"")));
        assert!(b_rx.try_recv().is_err());
    }

    #[test]
    fn reconnect_cancels_pending_offline() {
        let hub = Hub::default();
        let (tx, _rx) = mpsc::channel(1);
        let first = 1;
        hub.add_connection("a", first, tx.clone(), Arc::default(), None);
        let generation = hub.remove_connection("a", first);
        assert!(generation.is_some());
        hub.add_connection("a", 2, tx, Arc::default(), None);
        assert!(!hub.take_offline("a", generation.unwrap_or_default()));
        assert!(hub.is_online("a"));
    }

    #[test]
    fn bucket_limits_bursts() {
        let hub = Hub::default();
        let allowed = (0..40).filter(|_| hub.consume("a")).count();
        assert_eq!(allowed, 30);
    }
}
