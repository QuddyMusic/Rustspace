//! Голосовые комнаты: кто где сидит, звонки в переписках, ограничения.
//!
//! Состояние живёт в памяти. Сервер только ведёт учёт и раздаёт токены
//! LiveKit; звук идёт мимо него.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bs_permissions::Permissions;
use serde_json::{Map, Value, json};

use crate::db::{DbResult, Tx};
use crate::hub::{Hub, Outbox};
use crate::state::{AppState, now_ms};
use crate::{broadcast, dm, federation, params, perms};

/// Сколько звонит вызов, на который никто не ответил.
pub const RING_TIMEOUT: Duration = Duration::from_mins(1);
/// Сколько держится место в комнате после обрыва соединения, владевшего голосом.
pub const RECONNECT_GRACE: Duration = Duration::from_mins(1);

/// Состояние микрофона, звука, камеры и показа экрана участника.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Status {
    muted: bool,
    deafened: bool,
    camera: bool,
    screen: bool,
}

impl Status {
    fn json(self) -> Value {
        json!({
            "isMuted": self.muted,
            "isDeafened": self.deafened,
            "isCameraOn": self.camera,
            "isScreenSharing": self.screen,
        })
    }
}

#[derive(Debug, Clone)]
enum Kind {
    Space {
        space_id: String,
    },
    Dm {
        caller: String,
        ringing: bool,
        group: bool,
        declined: HashSet<String>,
        /// Участники с других инстансов, усаженные по их принятию: пользователь
        /// → инстанс, который о нём сообщил (`remoteParticipants`).
        remote: HashMap<String, String>,
    },
}

/// Звонок, который ведёт другой инстанс, с участниками отсюда
/// (`FederatedCallEntry`).
#[derive(Debug, Clone)]
pub struct FedCall {
    /// Переписка здесь, если она уже есть.
    pub dm_channel: Option<String>,
    /// Ключ звонка (ключ переписки).
    pub federated_id: String,
    /// Копия звонящего.
    pub caller: String,
    /// Инстанс, ведущий звонок.
    pub host: String,
    /// Адрес медиасервера ведущего.
    pub livekit_url: String,
    /// Токены участников отсюда.
    pub tokens: HashMap<String, String>,
    /// Кому звонили отсюда.
    pub ringed: Vec<String>,
    /// Кто вошёл отсюда.
    pub joined: Vec<String>,
    /// Групповой звонок с правилами для каждого участника.
    pub group: bool,
    /// Ответили.
    pub active: bool,
    /// Начало.
    pub started_at: i64,
}

/// Что делать, если ведущий не принял событие участника.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnFail {
    /// Только записать.
    Log,
    /// Сообщить участнику (`dm_call_undeliverable`, не окончательное).
    Report(&'static str),
    /// Забыть звонок и сообщить участнику окончательно.
    Drop(&'static str),
}

/// Работа со звонком для других инстансов; выполняется после фиксации.
#[derive(Debug, Clone)]
pub enum CallTask {
    /// Вызов начат здесь (`sendFederatedCallStart`).
    Start {
        /// Переписка.
        dm_channel: String,
        /// Звонящий.
        caller: String,
        /// Его имя для вызова.
        caller_name: String,
    },
    /// Принятие или завершение звонка, который ведёт этот инстанс, всем
    /// инстансам участников (`fanOutCallEvent`).
    FanOut {
        /// Переписка.
        dm_channel: String,
        /// `dm_call_accept` или `dm_call_end`.
        kind: &'static str,
        /// От чьего имени, по порядку предпочтения.
        actors: Vec<String>,
        /// Кроме этого инстанса.
        exclude: Option<String>,
        /// Кто ответил (для принятия).
        answered_by: Option<Value>,
        /// Кому сообщить о неудачах и на каком шаге.
        report: Option<(String, &'static str)>,
    },
    /// Действие участника отсюда в звонке, который ведёт другой инстанс
    /// (`relayToCallHost`).
    ToHost {
        /// Звонок, каким он был в момент действия.
        call: Box<FedCall>,
        /// Участник.
        user: String,
        /// `dm_call_accept`, `dm_call_reject` или `dm_call_end`.
        kind: &'static str,
        /// Кто ответил (для принятия).
        answered_by: Option<Value>,
        /// Что делать при неудаче.
        on_fail: OnFail,
    },
    /// «Печатает» для других инстансов (`sendTypingRelay`).
    Typing {
        /// Переписка.
        dm_channel: String,
        /// Перестал печатать.
        stop: bool,
        /// Кто печатает.
        user: String,
    },
}

#[derive(Debug, Clone)]
struct Room {
    kind: Kind,
    /// В порядке входа.
    participants: Vec<String>,
    started_at: i64,
}

impl Room {
    fn elapsed_seconds(&self) -> i64 {
        ((now_ms() - self.started_at) / 1000).max(0)
    }
}

#[derive(Debug, Default)]
struct Inner {
    rooms: HashMap<String, Room>,
    user_room: HashMap<String, String>,
    status: HashMap<String, Status>,
    /// Соединение, которое владеет голосом пользователя.
    voice_connection: HashMap<String, u64>,
    space_muted: HashSet<(String, String)>,
    space_deafened: HashSet<(String, String)>,
    permission_muted: HashSet<(String, String)>,
    /// Поколения таймеров вызова по комнатам.
    ring: HashMap<String, u64>,
    /// Поколения отсрочек после обрыва: пользователь → (поколение, комната,
    /// звонок другого инстанса).
    reconnect: HashMap<String, (u64, Option<String>, Option<String>)>,
    /// Звонки, которые ведут другие инстансы.
    federated: HashMap<String, FedCall>,
    /// Поколения таймеров таких звонков.
    federated_timer: HashMap<String, u64>,
    next: u64,
}

/// Голосовое состояние сервера.
#[derive(Debug, Clone, Default)]
pub struct Voice {
    inner: Arc<Mutex<Inner>>,
}

/// Комната, из которой участник вышел.
#[derive(Debug)]
struct Left {
    room_id: String,
    room: Room,
}

impl Inner {
    fn leave_room(&mut self, room_id: &str, user: &str) -> Option<Room> {
        let room = self.rooms.get_mut(room_id)?;
        let index = room.participants.iter().position(|id| id == user)?;
        room.participants.remove(index);
        self.user_room.remove(user);
        let snapshot = room.clone();
        if let Kind::Space { space_id } = &snapshot.kind {
            let key = (space_id.clone(), user.to_owned());
            self.space_muted.remove(&key);
            self.space_deafened.remove(&key);
            self.permission_muted.remove(&key);
            if snapshot.participants.is_empty() {
                self.rooms.remove(room_id);
            }
        }
        Some(snapshot)
    }

    fn leave_current(&mut self, user: &str) -> Option<Left> {
        let room_id = self.user_room.get(user)?.clone();
        let room = self.leave_room(&room_id, user)?;
        Some(Left { room_id, room })
    }

    fn join(&mut self, room_id: &str, user: &str) {
        if let Some(current) = self.user_room.get(user).cloned()
            && current != room_id
            && let Some(old) = self.rooms.get_mut(&current)
        {
            old.participants.retain(|id| id != user);
            if old.participants.is_empty() && matches!(old.kind, Kind::Space { .. }) {
                self.rooms.remove(&current);
            }
        }
        if let Some(room) = self.rooms.get_mut(room_id) {
            if !room.participants.iter().any(|id| id == user) {
                room.participants.push(user.to_owned());
            }
            self.user_room.insert(user.to_owned(), room_id.to_owned());
        }
    }

    fn set_voice_connection(&mut self, user: &str, connection: u64) {
        self.reconnect.remove(user);
        self.voice_connection.insert(user.to_owned(), connection);
    }

    fn clear_voice_connection(&mut self, user: &str) {
        self.reconnect.remove(user);
        self.voice_connection.remove(user);
    }

    fn destroy(&mut self, room_id: &str) -> Vec<String> {
        let Some(room) = self.rooms.remove(room_id) else {
            return Vec::new();
        };
        for user in &room.participants {
            self.user_room.remove(user);
            self.reconnect.remove(user);
        }
        if let Kind::Dm { caller, .. } = &room.kind {
            self.reconnect.remove(caller);
        }
        self.ring.remove(room_id);
        room.participants
    }
}

impl Inner {
    /// Звонок другого инстанса, в который пользователь вошёл отсюда
    /// (`getJoinedFederatedCall`).
    fn joined_federated(&self, user: &str) -> Option<String> {
        self.federated
            .values()
            .find(|call| call.joined.iter().any(|id| id == user))
            .map(|call| call.federated_id.clone())
    }

    /// Звонок другого инстанса в переписке (`getFederatedCallByDmChannel`).
    fn federated_by_dm(&self, dm_channel: &str) -> Option<String> {
        self.federated
            .values()
            .find(|call| call.dm_channel.as_deref() == Some(dm_channel))
            .map(|call| call.federated_id.clone())
    }

    /// Забывает звонок другого инстанса (`clearFederatedCall`).
    fn clear_federated(&mut self, federated: &str) {
        let joined = self
            .federated
            .get(federated)
            .map(|call| call.joined.clone())
            .unwrap_or_default();
        for user in joined {
            if !self.user_room.contains_key(&user) {
                self.clear_voice_connection(&user);
            }
        }
        self.federated.remove(federated);
        self.federated_timer.remove(federated);
    }

    /// Забывает отвеченный звонок, в котором никого отсюда не осталось
    /// (`dropFederatedCallIfIdle`).
    fn drop_federated_if_idle(&mut self, federated: &str) {
        let idle = self
            .federated
            .get(federated)
            .is_some_and(|call| call.active && call.joined.is_empty());
        if idle && !self.federated_timer.contains_key(federated) {
            self.clear_federated(federated);
        }
    }

    /// Участник выходит из звонка другого инстанса (`leaveFederatedCallEntry`).
    fn leave_federated_entry(&mut self, federated: &str, user: &str) {
        let Some(call) = self.federated.get_mut(federated) else {
            return;
        };
        call.joined.retain(|id| id != user);
        self.drop_federated_if_idle(federated);
    }
}

impl Voice {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Комната, в которой сидит пользователь, и пространство, если это канал.
    #[must_use]
    pub fn user_space_room(&self, user: &str) -> Option<(String, String)> {
        let inner = self.lock();
        let room_id = inner.user_room.get(user)?;
        match &inner.rooms.get(room_id)?.kind {
            Kind::Space { space_id } => Some((room_id.clone(), space_id.clone())),
            Kind::Dm { .. } => None,
        }
    }
}

/// Ограничения, наложенные на участника модераторами пространства.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Restrictions {
    /// Микрофон отключён модератором.
    pub muted: bool,
    /// Звук отключён модератором.
    pub deafened: bool,
}

/// Читает ограничения участника в пространстве (`voice_restrictions`).
///
/// # Errors
/// Ошибка базы.
pub fn restrictions(tx: &mut dyn Tx, space_id: &str, user_id: &str) -> DbResult<Restrictions> {
    let mut found = Restrictions::default();
    for row in tx.query(
        "SELECT restriction_type FROM voice_restrictions WHERE space_id = ? AND user_id = ?",
        params![space_id, user_id],
    )? {
        match row.text("restriction_type")?.as_str() {
            "mute" => found.muted = true,
            "deafen" => found.deafened = true,
            _ => {}
        }
    }
    Ok(found)
}

/// Событие комнаты канала — тем участникам пространства, кто видит канал.
/// Оригинал рассылает его всему пространству, и по нему видно, кто и когда
/// сидит в скрытом канале.
fn to_channel_viewers(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space_id: &str,
    channel_id: &str,
    event: &Value,
) -> DbResult<()> {
    for user in hub.online_in_space(space_id) {
        if perms::can_view(perms::compute(tx, &user, space_id, Some(channel_id))?) {
            out.to_user(&user, event);
        }
    }
    Ok(())
}

fn state_update(channel: &str, user: &str, join: bool) -> Value {
    json!({
        "type": "voice_state_update",
        "channelId": channel,
        "userId": user,
        "action": if join { "join" } else { "leave" },
    })
}

/// Завершает звонок в переписке (`endDmRoom`).
fn end_dm_room(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    dm_channel: &str,
    rejected: bool,
) -> DbResult<bool> {
    let participants = {
        let mut inner = hub.voice.lock();
        let Some(room) = inner.rooms.get(dm_channel) else {
            return Ok(false);
        };
        let Kind::Dm { caller, .. } = &room.kind else {
            return Ok(false);
        };
        let caller = caller.clone();
        let participants = room.participants.clone();
        let caller_room = inner.user_room.get(&caller).cloned();
        if caller_room.as_deref().is_none_or(|room| room == dm_channel) {
            inner.clear_voice_connection(&caller);
        }
        for user in &participants {
            inner.status.remove(user);
            inner.clear_voice_connection(user);
        }
        inner.destroy(dm_channel);
        participants
    };
    for user in &participants {
        broadcast::to_dm_members(
            tx,
            out,
            dm_channel,
            &state_update(dm_channel, user, false),
            None,
        )?;
    }
    let kind = if rejected {
        "dm_call_rejected"
    } else {
        "dm_call_ended"
    };
    broadcast::to_dm_members(
        tx,
        out,
        dm_channel,
        &json!({"type": kind, "dmChannelId": dm_channel}),
        None,
    )?;
    Ok(true)
}

/// После выхода участника из звонка: объявить и, если никого не осталось,
/// завершить (`afterDmCallLeave`).
fn after_dm_leave(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    dm_channel: &str,
    user: &str,
) -> DbResult<bool> {
    if let Some(Kind::Dm { remote, .. }) = hub
        .voice
        .lock()
        .rooms
        .get_mut(dm_channel)
        .map(|room| &mut room.kind)
    {
        remote.remove(user);
    }
    broadcast::to_dm_members(
        tx,
        out,
        dm_channel,
        &state_update(dm_channel, user, false),
        None,
    )?;
    let empty_active = hub.voice.lock().rooms.get(dm_channel).is_some_and(|room| {
        room.participants.is_empty() && matches!(room.kind, Kind::Dm { ringing: false, .. })
    });
    if empty_active {
        end_dm_room(tx, hub, out, dm_channel, false)?;
    }
    Ok(empty_active)
}

/// Конец звонка, который вёл этот инстанс, — инстансам участников
/// (`fanOutCallEnd`, `sendFederatedCallEnd`).
fn fan_out_end(out: &mut Outbox, dm_channel: &str, actors: Vec<String>) {
    out.call(CallTask::FanOut {
        dm_channel: dm_channel.to_owned(),
        kind: "dm_call_end",
        actors,
        exclude: None,
        answered_by: None,
        report: None,
    });
}

/// Объявляет выход из комнаты (`broadcastRoomLeave`).
///
/// Звонок, который выход опустошил, завершается и у других инстансов; конец
/// называет участника (`broadcastRoomLeave`) или звонящего
/// (`leaveCurrentRoomAnnounced`).
fn announce_leave(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    left: &Left,
    user: &str,
    name_user: bool,
) -> DbResult<()> {
    match &left.room.kind {
        Kind::Space { space_id } => to_channel_viewers(
            tx,
            hub,
            out,
            space_id,
            &left.room_id,
            &state_update(&left.room_id, user, false),
        ),
        Kind::Dm { caller, .. } => {
            if after_dm_leave(tx, hub, out, &left.room_id, user)? {
                let mut actors = vec![caller.clone()];
                if name_user {
                    actors.insert(0, user.to_owned());
                }
                fan_out_end(out, &left.room_id, actors);
            }
            Ok(())
        }
    }
}

/// Участник покидает свою комнату, и это объявляется.
fn leave_announced(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    user: &str,
    name_user: bool,
) -> DbResult<Option<String>> {
    let left = hub.voice.lock().leave_current(user);
    match left {
        Some(left) => {
            announce_leave(tx, hub, out, &left, user, name_user)?;
            Ok(Some(left.room_id))
        }
        None => Ok(None),
    }
}

/// Завершает вызовы, которые пользователь начал и на которые ещё не ответили.
fn end_ringing_placed_by(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    user: &str,
    except: Option<&str>,
) -> DbResult<()> {
    let rooms: Vec<String> = hub
        .voice
        .lock()
        .rooms
        .iter()
        .filter(|(id, room)| {
            Some(id.as_str()) != except
                && matches!(&room.kind, Kind::Dm { caller, ringing: true, .. } if caller == user)
        })
        .map(|(id, _)| id.clone())
        .collect();
    for room in rooms {
        end_dm_room(tx, hub, out, &room, false)?;
        fan_out_end(out, &room, vec![user.to_owned()]);
    }
    Ok(())
}

fn error(out: &mut Outbox, user: &str, message: &str) {
    out.to_user(user, &json!({"type": "error", "message": message}));
}

fn status_update(channel: &str, user: &str, status: Status) -> Value {
    let mut event = status.json();
    event["type"] = json!("voice_status_update");
    event["userId"] = json!(user);
    event["channelId"] = json!(channel);
    event
}

/// Применяет к вошедшему ограничения модераторов и запрет говорить.
/// `everyone` — объявлять всем, кто видит канал (первый вход), иначе только
/// самому участнику (повторный вход с другого устройства).
fn apply_restrictions(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space: &str,
    channel: &str,
    user: &str,
    everyone: bool,
) -> DbResult<()> {
    let found = restrictions(tx, space, user)?;
    let key = (space.to_owned(), user.to_owned());
    let mut events = Vec::new();
    if found.muted {
        hub.voice.lock().space_muted.insert(key.clone());
        events.push(json!({
            "type": "voice_space_muted", "userId": user, "channelId": channel, "spaceId": space, "muted": true,
        }));
    }
    if found.deafened {
        hub.voice.lock().space_deafened.insert(key.clone());
        events.push(json!({
            "type": "voice_space_deafened", "userId": user, "channelId": channel, "spaceId": space,
            "deafened": true,
        }));
    }
    let can_speak = perms::has(
        perms::compute(tx, user, space, Some(channel))?,
        Permissions::SPEAK,
    );
    if can_speak {
        hub.voice.lock().permission_muted.remove(&key);
    } else {
        hub.voice.lock().permission_muted.insert(key);
        events.push(json!({"type": "voice_permission_muted", "userId": user, "spaceId": space, "muted": true}));
    }
    for event in events {
        if everyone {
            to_channel_viewers(tx, hub, out, space, channel, &event)?;
        } else {
            out.to_user(user, &event);
        }
    }
    Ok(())
}

fn reject_join(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    user: &str,
    channel: &str,
) -> DbResult<()> {
    let in_it = hub
        .voice
        .lock()
        .user_room
        .get(user)
        .is_some_and(|room| room == channel);
    if in_it {
        leave(tx, hub, out, user)?;
    }
    out.to_user(
        user,
        &json!({"type": "voice_disconnected", "userId": user, "channelId": channel, "reason": "rejected"}),
    );
    Ok(())
}

/// `voice_join`.
///
/// # Errors
/// Ошибка базы.
pub fn join(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    user: &str,
    connection: u64,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let Some(channel) = event
        .get("channelId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        error(out, user, "channelId is required");
        return Ok(());
    };
    let Some(space) = crate::messages::channel_space(tx, channel)? else {
        error(out, user, "Channel not found");
        return reject_join(tx, hub, out, user, channel);
    };
    let held = perms::compute(tx, user, &space, Some(channel))?;
    // Как и с токеном: в канал, которого участник не видит, войти нельзя.
    if !perms::has(held, Permissions::CONNECT) || !perms::can_view(held) {
        error(out, user, "Missing CONNECT permission");
        return reject_join(tx, hub, out, user, channel);
    }
    let (displaced_from, already_here, status) = {
        let mut inner = hub.voice.lock();
        let old = inner
            .voice_connection
            .get(user)
            .copied()
            .filter(|old| *old != connection);
        let current = inner.user_room.get(user).cloned();
        inner.set_voice_connection(user, connection);
        (
            old.map(|old| (old, current.clone())),
            current.as_deref() == Some(channel),
            inner.status.get(user).copied(),
        )
    };
    if let Some((old, room)) = displaced_from {
        out.to_connection(
            old,
            &json!({
                "type": "voice_disconnected",
                "userId": user,
                "channelId": room.as_deref().unwrap_or(channel),
                "reason": "displaced",
            }),
        );
    }
    if already_here {
        if let Some(status) = status {
            to_channel_viewers(
                tx,
                hub,
                out,
                &space,
                channel,
                &status_update(channel, user, status),
            )?;
        }
        return apply_restrictions(tx, hub, out, &space, channel, user, false);
    }
    leave_joined_federated(hub, out, user);
    if let Some(left) = leave_announced(tx, hub, out, user, true)? {
        out.to_user(
            user,
            &json!({"type": "voice_disconnected", "userId": user, "channelId": left, "reason": "displaced"}),
        );
    }
    let elapsed = {
        let mut inner = hub.voice.lock();
        inner
            .rooms
            .entry(channel.to_owned())
            .or_insert_with(|| Room {
                kind: Kind::Space {
                    space_id: space.clone(),
                },
                participants: Vec::new(),
                started_at: now_ms(),
            });
        inner.join(channel, user);
        inner.rooms.get(channel).map(Room::elapsed_seconds)
    };
    end_ringing_placed_by(tx, hub, out, user, None)?;
    let mut joined = state_update(channel, user, true);
    if let Some(elapsed) = elapsed {
        joined["channelElapsedSeconds"] = json!(elapsed);
    }
    to_channel_viewers(tx, hub, out, &space, channel, &joined)?;
    if let Some(status) = status {
        to_channel_viewers(
            tx,
            hub,
            out,
            &space,
            channel,
            &status_update(channel, user, status),
        )?;
    }
    apply_restrictions(tx, hub, out, &space, channel, user, true)
}

/// `voice_leave`.
///
/// # Errors
/// Ошибка базы.
pub fn leave(tx: &mut dyn Tx, hub: &Hub, out: &mut Outbox, user: &str) -> DbResult<()> {
    leave_joined_federated(hub, out, user);
    hub.voice.lock().clear_voice_connection(user);
    leave_announced(tx, hub, out, user, true)?;
    hub.voice.lock().status.remove(user);
    Ok(())
}

/// `voice_status`.
///
/// # Errors
/// Ошибка базы.
pub fn status(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    user: &str,
    connection: u64,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let flag = |name: &str| event.get(name) == Some(&Value::Bool(true));
    let (room_id, kind, effective) = {
        let mut inner = hub.voice.lock();
        let Some(room_id) = inner.user_room.get(user).cloned() else {
            // Звонок другого инстанса комнаты здесь не имеет, но состояние —
            // знак, что сеанс вернулся после обрыва.
            if inner.joined_federated(user).is_some() {
                inner.set_voice_connection(user, connection);
            }
            return Ok(());
        };
        let Some(room) = inner.rooms.get(&room_id) else {
            return Ok(());
        };
        let kind = room.kind.clone();
        let mut status = Status {
            muted: flag("isMuted"),
            deafened: flag("isDeafened"),
            camera: flag("isCameraOn"),
            screen: flag("isScreenSharing"),
        };
        match &kind {
            Kind::Space { space_id } => {
                let key = (space_id.clone(), user.to_owned());
                if inner.space_muted.contains(&key) || inner.permission_muted.contains(&key) {
                    status.muted = true;
                }
                if inner.space_deafened.contains(&key) {
                    status.deafened = true;
                }
            }
            Kind::Dm { .. } => inner.set_voice_connection(user, connection),
        }
        inner.status.insert(user.to_owned(), status);
        (room_id, kind, status)
    };
    let update = status_update(&room_id, user, effective);
    match kind {
        Kind::Space { space_id } => to_channel_viewers(tx, hub, out, &space_id, &room_id, &update),
        Kind::Dm { .. } => broadcast::to_dm_members(tx, out, &room_id, &update, None),
    }
}

/// Комната канала, где сидит цель, для действий модератора.
fn target_room(hub: &Hub, out: &mut Outbox, actor: &str, target: &str) -> Option<(String, String)> {
    let found = hub.voice.user_space_room(target);
    if found.is_none() {
        error(out, actor, "Target user is not in a voice channel");
    }
    found
}

fn hierarchy_refused(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    space: &str,
    actor: &str,
    target: &str,
) -> DbResult<bool> {
    if actor == target || perms::can_act_on(tx, space, actor, target)? {
        return Ok(false);
    }
    out.to_user(
        actor,
        &json!({
            "type": "error",
            "message": crate::error::error_text("role_hierarchy", None),
            "code": "role_hierarchy",
        }),
    );
    Ok(true)
}

/// `voice_space_mute` и `voice_space_deafen`.
///
/// # Errors
/// Ошибка базы.
pub fn moderate(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    actor: &str,
    event: &Map<String, Value>,
    deafen: bool,
) -> DbResult<()> {
    let Some(target) = event
        .get("userId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        error(out, actor, "userId is required");
        return Ok(());
    };
    let flag_name = if deafen { "deafened" } else { "muted" };
    let on = event.get(flag_name) == Some(&Value::Bool(true));
    let Some((channel, space)) = target_room(hub, out, actor, target) else {
        return Ok(());
    };
    let (needed, missing, own) = if deafen {
        (
            Permissions::DEAFEN_MEMBERS,
            "Missing DEAFEN_MEMBERS permission",
            "Cannot space-deafen yourself",
        )
    } else {
        (
            Permissions::MUTE_MEMBERS,
            "Missing MUTE_MEMBERS permission",
            "Cannot space-mute yourself",
        )
    };
    if !perms::has_channel_permission(tx, actor, &space, &channel, needed)? {
        error(out, actor, missing);
        return Ok(());
    }
    if target == actor {
        error(out, actor, own);
        return Ok(());
    }
    if hierarchy_refused(tx, out, &space, actor, target)? {
        return Ok(());
    }
    let kind = if deafen { "deafen" } else { "mute" };
    {
        let mut inner = hub.voice.lock();
        let key = (space.clone(), target.to_owned());
        let set = if deafen {
            &mut inner.space_deafened
        } else {
            &mut inner.space_muted
        };
        if on {
            set.insert(key);
        } else {
            set.remove(&key);
        }
    }
    if on {
        tx.execute(
            "INSERT INTO voice_restrictions (space_id, user_id, restriction_type, moderator_id, created_at) \
             VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            params![&space, target, kind, actor, now_ms()],
        )?;
    } else {
        tx.execute(
            "DELETE FROM voice_restrictions WHERE space_id = ? AND user_id = ? AND restriction_type = ?",
            params![&space, target, kind],
        )?;
    }
    let mut event = json!({
        "type": if deafen { "voice_space_deafened" } else { "voice_space_muted" },
        "userId": target,
        "channelId": channel,
        "spaceId": space,
    });
    event[flag_name] = json!(on);
    to_channel_viewers(tx, hub, out, &space, &channel, &event)
}

/// `voice_move` — модератор переводит участника в другой канал.
///
/// # Errors
/// Ошибка базы.
pub fn move_member(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    actor: &str,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let text = |name: &str| {
        event
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let Some(target) = text("userId") else {
        error(out, actor, "userId is required");
        return Ok(());
    };
    let Some(to) = text("targetChannelId") else {
        error(out, actor, "targetChannelId is required");
        return Ok(());
    };
    let Some((from, space)) = target_room(hub, out, actor, target) else {
        return Ok(());
    };
    if !perms::has_channel_permission(tx, actor, &space, &from, Permissions::MOVE_MEMBERS)? {
        error(out, actor, "Missing MOVE_MEMBERS permission");
        return Ok(());
    }
    if hierarchy_refused(tx, out, &space, actor, target)? {
        return Ok(());
    }
    let destination = tx.query_one(
        "SELECT space_id, type FROM channels WHERE id = ?",
        params![to],
    )?;
    let Some(destination) =
        destination.filter(|row| row.text("space_id").ok().as_deref() == Some(space.as_str()))
    else {
        error(out, actor, "Target channel not found in this space");
        return Ok(());
    };
    if destination.text("type")? != "voice" {
        error(out, actor, "Target channel is not a voice channel");
        return Ok(());
    }
    if to == from {
        error(out, actor, "User is already in that channel");
        return Ok(());
    }
    let (elapsed, status) = {
        let mut inner = hub.voice.lock();
        // Ограничения привязаны к пространству и переезжают вместе с участником.
        let key = (space.clone(), target.to_owned());
        let kept = (
            inner.space_muted.contains(&key),
            inner.space_deafened.contains(&key),
            inner.permission_muted.contains(&key),
        );
        inner.leave_room(&from, target);
        inner.rooms.entry(to.to_owned()).or_insert_with(|| Room {
            kind: Kind::Space {
                space_id: space.clone(),
            },
            participants: Vec::new(),
            started_at: now_ms(),
        });
        inner.join(to, target);
        if kept.0 {
            inner.space_muted.insert(key.clone());
        }
        if kept.1 {
            inner.space_deafened.insert(key.clone());
        }
        if kept.2 {
            inner.permission_muted.insert(key);
        }
        (
            inner.rooms.get(to).map(Room::elapsed_seconds),
            inner.status.get(target).copied(),
        )
    };
    to_channel_viewers(
        tx,
        hub,
        out,
        &space,
        &from,
        &state_update(&from, target, false),
    )?;
    let mut joined = state_update(to, target, true);
    if let Some(elapsed) = elapsed {
        joined["channelElapsedSeconds"] = json!(elapsed);
    }
    to_channel_viewers(tx, hub, out, &space, to, &joined)?;
    out.to_user(
        target,
        &json!({"type": "voice_moved", "userId": target, "oldChannelId": from, "newChannelId": to}),
    );
    if let Some(status) = status {
        to_channel_viewers(tx, hub, out, &space, to, &status_update(to, target, status))?;
    }
    Ok(())
}

/// `voice_disconnect` — модератор отключает участника от канала.
///
/// # Errors
/// Ошибка базы.
pub fn disconnect_member(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    actor: &str,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let Some(target) = event
        .get("userId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        error(out, actor, "userId is required");
        return Ok(());
    };
    if target == actor {
        error(out, actor, "Cannot disconnect yourself");
        return Ok(());
    }
    let Some((channel, space)) = target_room(hub, out, actor, target) else {
        return Ok(());
    };
    if !perms::has_channel_permission(tx, actor, &space, &channel, Permissions::DISCONNECT_MEMBERS)?
    {
        error(out, actor, "Missing DISCONNECT_MEMBERS permission");
        return Ok(());
    }
    if hierarchy_refused(tx, out, &space, actor, target)? {
        return Ok(());
    }
    kick_from_room(tx, hub, out, &space, &channel, target)
}

fn kick_from_room(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space: &str,
    channel: &str,
    target: &str,
) -> DbResult<()> {
    {
        let mut inner = hub.voice.lock();
        inner.leave_room(channel, target);
        inner.clear_voice_connection(target);
        inner.status.remove(target);
    }
    to_channel_viewers(
        tx,
        hub,
        out,
        space,
        channel,
        &state_update(channel, target, false),
    )?;
    out.to_user(
        target,
        &json!({"type": "voice_disconnected", "userId": target, "channelId": channel}),
    );
    // Клиент мог бы не послушаться события: участника убирает сам медиасервер.
    out.evict_media(channel, target);
    Ok(())
}

fn refuse_call(out: &mut Outbox, connection: u64, code: &'static str, dm_channel: Option<&str>) {
    let mut event = json!({
        "type": "error",
        "message": crate::error::error_text(code, None),
        "code": code,
    });
    if let Some(dm_channel) = dm_channel {
        event["dmChannelId"] = json!(dm_channel);
    }
    out.to_connection(connection, &event);
}

/// Переписка по её идентификатору или по ключу звонка.
fn call_channel(
    tx: &mut dyn Tx,
    event: &Map<String, Value>,
) -> DbResult<(Option<String>, Option<String>)> {
    let text = |name: &str| {
        event
            .get(name)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let federated = text("federatedCallId");
    let mut channel = text("dmChannelId");
    if channel.is_none()
        && let Some(federated) = &federated
        && let Some(row) = tx.query_one(
            "SELECT id FROM dm_channels WHERE federated_id = ?",
            params![federated],
        )?
    {
        channel = Some(row.text("id")?);
    }
    Ok((channel, federated))
}

/// Участник входит в звонок, который ведёт этот инстанс (`joinHostedDmCall`).
fn join_hosted_call(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    dm_channel: &str,
    user: &str,
    connection: u64,
) -> DbResult<()> {
    let hub = &app.hub;
    let activated_caller = {
        let mut inner = hub.voice.lock();
        match inner.rooms.get_mut(dm_channel).map(|room| &mut room.kind) {
            Some(Kind::Dm {
                caller, ringing, ..
            }) if *ringing => {
                *ringing = false;
                let caller = caller.clone();
                inner.ring.remove(dm_channel);
                Some(caller)
            }
            Some(Kind::Dm { .. }) => None,
            _ => return Ok(()),
        }
    };
    if let Some(caller) = activated_caller {
        let left = {
            let mut inner = hub.voice.lock();
            let left = inner.leave_current(&caller);
            inner.join(dm_channel, &caller);
            left
        };
        if let Some(left) = left {
            announce_leave(tx, hub, out, &left, &caller, true)?;
        }
        broadcast::to_dm_members(
            tx,
            out,
            dm_channel,
            &state_update(dm_channel, &caller, true),
            None,
        )?;
    }
    let in_it = hub
        .voice
        .lock()
        .user_room
        .get(user)
        .is_some_and(|room| room == dm_channel);
    if !in_it {
        let left = hub.voice.lock().leave_current(user);
        if let Some(left) = left {
            announce_leave(tx, hub, out, &left, user, true)?;
        }
    }
    leave_joined_federated(hub, out, user);
    {
        let mut inner = hub.voice.lock();
        if let Some(Kind::Dm { declined, .. }) =
            inner.rooms.get_mut(dm_channel).map(|room| &mut room.kind)
        {
            declined.remove(user);
        }
        inner.join(dm_channel, user);
        inner.set_voice_connection(user, connection);
    }
    let answered_by = relay_identity(tx, app, user)?;
    let mut accepted = json!({
        "type": "dm_call_accepted",
        "dmChannelId": dm_channel,
    });
    if let Some(federated) = federation::dm_federated_id(tx, dm_channel)? {
        accepted["federatedCallId"] = json!(federated);
    }
    if let Some(answered_by) = &answered_by {
        accepted["answeredBy"] = answered_by.clone();
    }
    let caller = match hub
        .voice
        .lock()
        .rooms
        .get(dm_channel)
        .map(|room| &room.kind)
    {
        Some(Kind::Dm { caller, .. }) => caller.clone(),
        _ => String::new(),
    };
    broadcast::to_dm_members(tx, out, dm_channel, &accepted, None)?;
    broadcast::to_dm_members(
        tx,
        out,
        dm_channel,
        &state_update(dm_channel, user, true),
        None,
    )?;
    out.call(CallTask::FanOut {
        dm_channel: dm_channel.to_owned(),
        kind: "dm_call_accept",
        actors: vec![user.to_owned(), caller],
        exclude: None,
        answered_by,
        report: Some((user.to_owned(), "accept")),
    });
    Ok(())
}

/// Идентичность пользователя для событий звонка (`userRelayIdentity`).
fn relay_identity(tx: &mut dyn Tx, app: &AppState, user: &str) -> DbResult<Option<Value>> {
    let Some(row) = tx.query_one("SELECT * FROM users WHERE id = ?", params![user])? else {
        return Ok(None);
    };
    Ok(federation::identity::actor_of_user(&app.config, &row)?.map(
        |actor| json!({"homeUserId": actor.home_user_id, "homeInstance": actor.home_instance}),
    ))
}

/// Запускает таймер вызова: если за минуту никто не ответил, звонок завершается.
fn start_ring_timer(app: &AppState, dm_channel: &str) {
    let generation = {
        let mut inner = app.hub.voice.lock();
        inner.next += 1;
        let generation = inner.next;
        inner.ring.insert(dm_channel.to_owned(), generation);
        generation
    };
    let app = app.clone();
    let dm_channel = dm_channel.to_owned();
    tokio::spawn(async move {
        tokio::time::sleep(RING_TIMEOUT).await;
        let state = app.clone();
        let result = app
            .run(move |tx, out| {
                let still = {
                    let inner = state.hub.voice.lock();
                    if inner.ring.get(&dm_channel) == Some(&generation) {
                        match inner.rooms.get(&dm_channel).map(|room| &room.kind) {
                            Some(Kind::Dm {
                                caller,
                                ringing: true,
                                ..
                            }) => Some(caller.clone()),
                            _ => None,
                        }
                    } else {
                        None
                    }
                };
                if let Some(caller) = still {
                    end_dm_room(tx, &state.hub, out, &dm_channel, false)?;
                    fan_out_end(out, &dm_channel, vec![caller]);
                }
                Ok(())
            })
            .await;
        if let Err(error) = result {
            tracing::error!(?error, "не удалось завершить неотвеченный вызов");
        }
    });
}

/// `dm_call_start`.
///
/// # Errors
/// Ошибка базы.
pub fn call_start(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    user: &str,
    username: &str,
    connection: u64,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let hub = &app.hub;
    let Some(dm_channel) = event
        .get("dmChannelId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    else {
        refuse_call(out, connection, "validation_failed", None);
        return Ok(());
    };
    if !dm::is_member(tx, dm_channel, user)? {
        refuse_call(out, connection, "not_dm_member", Some(dm_channel));
        return Ok(());
    }
    let existing = hub.voice.lock().rooms.get(dm_channel).cloned();
    if let Some(Room {
        kind: Kind::Dm {
            caller, ringing, ..
        },
        participants,
        ..
    }) = existing
    {
        if participants.iter().any(|id| id == user) || (ringing && caller == user) {
            refuse_call(out, connection, "dm_call_in_progress", Some(dm_channel));
            return Ok(());
        }
        return join_hosted_call(app, tx, out, dm_channel, user, connection);
    }
    // Звонок в этой переписке ведёт другой инстанс: пока он идёт, второй
    // звонок здесь разделил бы переписку на две комнаты.
    let federated = hub.voice.lock().federated_by_dm(dm_channel);
    if let Some(federated) = federated {
        if federated_call_still_live(hub, out, &federated, user, connection) {
            refuse_call(out, connection, "dm_call_in_progress", Some(dm_channel));
            return Ok(());
        }
        hub.voice.lock().clear_federated(&federated);
    }
    let left = hub.voice.lock().leave_current(user);
    if let Some(left) = left {
        announce_leave(tx, hub, out, &left, user, true)?;
    }
    leave_joined_federated(hub, out, user);
    hub.voice.lock().status.remove(user);
    end_ringing_placed_by(tx, hub, out, user, Some(dm_channel))?;
    let group = match dm::channel_row(tx, dm_channel)? {
        Some(row) => !dm::may_be_one_on_one(
            row.opt_text("owner_id")?.as_deref(),
            row.opt_text("federated_id")?.as_deref(),
        ),
        None => false,
    };
    {
        let mut inner = hub.voice.lock();
        inner.rooms.insert(
            dm_channel.to_owned(),
            Room {
                kind: Kind::Dm {
                    caller: user.to_owned(),
                    ringing: true,
                    group,
                    declined: HashSet::new(),
                    remote: HashMap::new(),
                },
                participants: Vec::new(),
                started_at: now_ms(),
            },
        );
        inner.set_voice_connection(user, connection);
    }
    start_ring_timer(app, dm_channel);
    dm::reopen_closed(tx, out, dm_channel)?;
    broadcast::to_dm_members(
        tx,
        out,
        dm_channel,
        &json!({
            "type": "dm_call_incoming",
            "dmChannelId": dm_channel,
            "callerId": user,
            "callerName": username,
        }),
        Some(user),
    )?;
    out.call(CallTask::Start {
        dm_channel: dm_channel.to_owned(),
        caller: user.to_owned(),
        caller_name: username.to_owned(),
    });
    Ok(())
}

/// `dm_call_accept`.
///
/// # Errors
/// Ошибка базы.
pub fn call_accept(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    user: &str,
    connection: u64,
    event: &Map<String, Value>,
) -> DbResult<()> {
    let hub = &app.hub;
    let (channel, federated) = call_channel(tx, event)?;
    let named = event
        .get("dmChannelId")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    let label = named.or_else(|| federated.clone());
    if channel.is_none() && federated.is_none() {
        refuse_call(out, connection, "validation_failed", None);
        return Ok(());
    }
    if let Some(channel) = &channel {
        if !dm::is_member(tx, channel, user)? {
            refuse_call(out, connection, "not_dm_member", label.as_deref());
            return Ok(());
        }
        let hosted = hub
            .voice
            .lock()
            .rooms
            .get(channel)
            .is_some_and(|room| matches!(room.kind, Kind::Dm { .. }));
        if hosted {
            return join_hosted_call(app, tx, out, channel, user, connection);
        }
    }
    let call = find_federated(hub, federated.as_deref(), channel.as_deref());
    let Some(call) = call else {
        refuse_call(out, connection, "dm_call_not_found", label.as_deref());
        return Ok(());
    };
    // Комната здесь, где бы она ни была, покидается и объявляется; звонок
    // другого инстанса, в котором участник уже был, — тоже.
    leave_announced(tx, hub, out, user, false)?;
    let other = hub.voice.lock().joined_federated(user);
    if other.is_some_and(|other| other != call.federated_id) {
        leave_joined_federated(hub, out, user);
    }
    let call = {
        let mut inner = hub.voice.lock();
        if let Some(entry) = inner.federated.get_mut(&call.federated_id) {
            entry.active = true;
            if !entry.joined.iter().any(|id| id == user) {
                entry.joined.push(user.to_owned());
            }
        }
        inner.set_voice_connection(user, connection);
        inner
            .federated
            .get(&call.federated_id)
            .cloned()
            .unwrap_or(call)
    };
    let answered_by = relay_identity(tx, app, user)?;
    let mut accepted = json!({
        "type": "dm_call_accepted",
        "dmChannelId": call.dm_channel,
        "federatedCallId": call.federated_id,
    });
    if let Some(answered_by) = &answered_by {
        accepted["answeredBy"] = answered_by.clone();
    }
    to_federated_users(out, &call, &accepted, None);
    out.call(CallTask::ToHost {
        call: Box::new(call),
        user: user.to_owned(),
        kind: "dm_call_accept",
        answered_by,
        on_fail: OnFail::Drop("accept"),
    });
    Ok(())
}

/// Исход выхода из группового звонка или отказа от него.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupOutcome {
    /// Ничего не изменилось.
    Ignored,
    /// Участник вышел (отказался), звонок идёт.
    Left,
    /// Звонок закончился.
    Ended,
}

/// Участник выходит из группового звонка (`leaveGroupDmCall`).
fn leave_group_call(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    channel: &str,
    user: &str,
) -> DbResult<GroupOutcome> {
    let Some(Room {
        kind: Kind::Dm {
            caller, ringing, ..
        },
        participants,
        ..
    }) = hub.voice.lock().rooms.get(channel).cloned()
    else {
        return Ok(GroupOutcome::Ignored);
    };
    if ringing && caller == user {
        end_dm_room(tx, hub, out, channel, false)?;
        return Ok(GroupOutcome::Ended);
    }
    if !participants.iter().any(|id| id == user) {
        return Ok(GroupOutcome::Ignored);
    }
    {
        let mut inner = hub.voice.lock();
        inner.leave_room(channel, user);
        inner.status.remove(user);
        inner.clear_voice_connection(user);
    }
    Ok(if after_dm_leave(tx, hub, out, channel, user)? {
        GroupOutcome::Ended
    } else {
        GroupOutcome::Left
    })
}

/// Участник отказывается от группового звонка (`declineGroupDmCall`).
fn decline_group_call(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    channel: &str,
    user: &str,
) -> DbResult<GroupOutcome> {
    let (caller, ringing, declined) = {
        let mut inner = hub.voice.lock();
        let Some(room) = inner.rooms.get_mut(channel) else {
            return Ok(GroupOutcome::Ignored);
        };
        let seated = room.participants.iter().any(|id| id == user);
        let Kind::Dm {
            caller,
            ringing,
            declined,
            ..
        } = &mut room.kind
        else {
            return Ok(GroupOutcome::Ignored);
        };
        if caller == user || seated {
            return Ok(GroupOutcome::Ignored);
        }
        declined.insert(user.to_owned());
        (caller.clone(), *ringing, declined.clone())
    };
    if !ringing {
        return Ok(GroupOutcome::Left);
    }
    for row in tx.query(
        "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
        params![channel],
    )? {
        let member = row.text("user_id")?;
        if member != caller && !declined.contains(&member) {
            return Ok(GroupOutcome::Left);
        }
    }
    end_dm_room(tx, hub, out, channel, true)?;
    Ok(GroupOutcome::Ended)
}

/// `dm_call_reject` и `dm_call_end`.
///
/// # Errors
/// Ошибка базы.
pub fn call_stop(
    app: &AppState,
    tx: &mut dyn Tx,
    out: &mut Outbox,
    user: &str,
    event: &Map<String, Value>,
    reject: bool,
) -> DbResult<()> {
    let hub = &app.hub;
    let (channel, federated) = call_channel(tx, event)?;
    if channel.is_none() && federated.is_none() {
        return Ok(());
    }
    let phase = if reject { "reject" } else { "end" };
    if let Some(channel) = &channel {
        if !dm::is_member(tx, channel, user)? {
            return Ok(());
        }
        let hosted = match hub.voice.lock().rooms.get(channel).map(|room| &room.kind) {
            Some(Kind::Dm { caller, group, .. }) => Some((caller.clone(), *group)),
            _ => None,
        };
        if let Some((caller, group)) = hosted {
            if group {
                let outcome = if reject {
                    let outcome = decline_group_call(tx, hub, out, channel, user)?;
                    if outcome == GroupOutcome::Left {
                        out.to_user(
                            user,
                            &json!({"type": "dm_call_rejected", "dmChannelId": channel}),
                        );
                    }
                    outcome
                } else {
                    leave_group_call(tx, hub, out, channel, user)?
                };
                if outcome != GroupOutcome::Ended {
                    return Ok(());
                }
            } else {
                end_dm_room(tx, hub, out, channel, reject)?;
            }
            out.call(CallTask::FanOut {
                dm_channel: channel.clone(),
                kind: "dm_call_end",
                actors: vec![user.to_owned(), caller],
                exclude: None,
                answered_by: None,
                report: Some((user.to_owned(), phase)),
            });
            return Ok(());
        }
    }
    let Some(call) = find_federated(hub, federated.as_deref(), channel.as_deref()) else {
        return Ok(());
    };
    let kind = if reject {
        "dm_call_rejected"
    } else {
        "dm_call_ended"
    };
    let wire = json!({
        "type": kind,
        "dmChannelId": call.dm_channel,
        "federatedCallId": call.federated_id,
    });
    if call.group {
        let joined = call.joined.iter().any(|id| id == user);
        if reject == joined {
            // Отказ — только пока не вошёл; выход — только вошедшему.
            return Ok(());
        }
        let mut inner = hub.voice.lock();
        if let Some(entry) = inner.federated.get_mut(&call.federated_id) {
            entry.ringed.retain(|id| id != user);
        }
        if reject {
            out.to_user(user, &wire);
        } else {
            if !inner.user_room.contains_key(user) {
                inner.clear_voice_connection(user);
            }
            inner.leave_federated_entry(&call.federated_id, user);
        }
    } else {
        to_federated_users(out, &call, &wire, Some(user));
        hub.voice.lock().clear_federated(&call.federated_id);
    }
    out.call(CallTask::ToHost {
        call: Box::new(call),
        user: user.to_owned(),
        kind: if reject {
            "dm_call_reject"
        } else {
            "dm_call_end"
        },
        answered_by: None,
        on_fail: OnFail::Report(phase),
    });
    Ok(())
}

/// Голосовое состояние пространства, как его видит пользователь
/// (`buildSpaceVoiceState`): `(voiceStates, elapsed, userStates, spaceStates)`.
///
/// # Errors
/// Ошибка базы.
pub fn space_snapshot(
    tx: &mut dyn Tx,
    hub: &Hub,
    space: &str,
    user: &str,
) -> DbResult<(
    Map<String, Value>,
    Map<String, Value>,
    Map<String, Value>,
    Map<String, Value>,
)> {
    let (mut states, mut elapsed, mut users, mut flags) =
        (Map::new(), Map::new(), Map::new(), Map::new());
    let channels = tx.query(
        "SELECT id FROM channels WHERE space_id = ? AND type = 'voice'",
        params![space],
    )?;
    for row in channels {
        let channel = row.text("id")?;
        if !perms::can_view(perms::compute(tx, user, space, Some(&channel))?) {
            continue;
        }
        let inner = hub.voice.lock();
        let Some(room) = inner
            .rooms
            .get(&channel)
            .filter(|room| !room.participants.is_empty())
        else {
            continue;
        };
        states.insert(channel.clone(), json!(room.participants));
        elapsed.insert(channel, json!(room.elapsed_seconds()));
        for participant in &room.participants {
            if let Some(status) = inner.status.get(participant) {
                users.insert(participant.clone(), status.json());
            }
        }
    }
    let entry = |flags: &mut Map<String, Value>, who: &str, name: &str| {
        let slot = flags.entry(format!("{space}:{who}")).or_insert_with(
            || json!({"spaceMuted": false, "spaceDeafened": false, "permissionMuted": false}),
        );
        slot[name] = json!(true);
    };
    for row in tx.query(
        "SELECT user_id, restriction_type FROM voice_restrictions WHERE space_id = ?",
        params![space],
    )? {
        match row.text("restriction_type")?.as_str() {
            "mute" => entry(&mut flags, &row.text("user_id")?, "spaceMuted"),
            "deafen" => entry(&mut flags, &row.text("user_id")?, "spaceDeafened"),
            _ => {}
        }
    }
    let muted: Vec<String> = hub
        .voice
        .lock()
        .permission_muted
        .iter()
        .filter(|(muted_space, _)| muted_space == space)
        .map(|(_, who)| who.clone())
        .collect();
    for who in muted {
        entry(&mut flags, &who, "permissionMuted");
    }
    Ok((states, elapsed, users, flags))
}

/// Отправляет пользователю голосовое состояние пространства, которое он
/// сейчас видит (`pushSpaceVoiceState`).
///
/// # Errors
/// Ошибка базы.
pub fn push_space_state(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    user_id: &str,
    space_id: &str,
) -> DbResult<()> {
    if !hub.is_online(user_id) {
        return Ok(());
    }
    let (states, elapsed, users, flags) = space_snapshot(tx, hub, space_id, user_id)?;
    if states.is_empty() && flags.is_empty() {
        return Ok(());
    }
    out.to_user(
        user_id,
        &json!({
            "type": "space_voice_state",
            "spaceId": space_id,
            "voiceStates": states,
            "voiceChannelElapsedSeconds": elapsed,
            "voiceUserStates": users,
            "spaceVoiceStates": flags,
        }),
    );
    Ok(())
}

/// Звонки в переписках пользователя для `ready`: список `activeCalls` и
/// участники по комнатам.
#[must_use]
pub fn dm_calls(
    hub: &Hub,
    user: &str,
    dm_channels: &[String],
) -> (Vec<Value>, Map<String, Value>, Map<String, Value>) {
    let inner = hub.voice.lock();
    let mut calls = Vec::new();
    let mut states = Map::new();
    let mut users = Map::new();
    for channel in dm_channels {
        let Some(room) = inner.rooms.get(channel) else {
            continue;
        };
        let Kind::Dm {
            caller,
            ringing,
            declined,
            ..
        } = &room.kind
        else {
            continue;
        };
        if *ringing && declined.contains(user) {
            continue;
        }
        calls.push(json!({
            "dmChannelId": channel,
            "callerId": caller,
            "participants": room.participants,
            "startedAt": room.started_at,
            "state": if *ringing { "ringing" } else { "active" },
        }));
        if !room.participants.is_empty() {
            states.insert(channel.clone(), json!(room.participants));
            for participant in &room.participants {
                if let Some(status) = inner.status.get(participant) {
                    users.insert(participant.clone(), status.json());
                }
            }
        }
    }
    // Звонки, которые ведут другие инстансы.
    let mut federated: Vec<&FedCall> = inner.federated.values().collect();
    federated.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then(a.federated_id.cmp(&b.federated_id))
    });
    for call in federated {
        let ringed = call.ringed.iter().any(|id| id == user);
        let member = call
            .dm_channel
            .as_ref()
            .is_some_and(|channel| dm_channels.contains(channel));
        // Отказавшийся от группового вызова выпал из `ringed`: пока звонят,
        // ему не звонят снова.
        let declined = call.group && !call.active && !ringed;
        if !(ringed || member) || declined {
            continue;
        }
        let mut entry = json!({
            "dmChannelId": call.dm_channel,
            "federatedCallId": call.federated_id,
            "callerId": call.caller,
            "participants": [],
            "startedAt": call.started_at,
            "state": if call.active { "active" } else { "ringing" },
            "federatedCallHost": call.host,
            "livekitUrl": call.livekit_url,
        });
        if let Some(token) = call.tokens.get(user) {
            entry["livekitToken"] = json!(token);
        }
        calls.push(entry);
    }
    (calls, states, users)
}

/// После изменения ролей или прав в пространстве: затронутым участникам
/// уходит голосовое состояние, которое они теперь видят, а права говорить
/// перепроверяются.
///
/// # Errors
/// Ошибка базы.
pub fn access_changed(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space_id: &str,
    affected: &[String],
) -> DbResult<()> {
    let online = hub.online_in_space(space_id);
    for user in affected {
        if online.contains(user) {
            push_space_state(tx, hub, out, user, space_id)?;
        }
    }
    check_permissions(tx, hub, out, space_id)
}

/// Перепроверяет право говорить у сидящих в голосовых каналах пространства
/// (`checkVoicePermissions`). Тот, кто больше не видит канал или потерял
/// право подключаться, отключается: оригинал оставляет его в комнате.
///
/// # Errors
/// Ошибка базы.
pub fn check_permissions(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space_id: &str,
) -> DbResult<()> {
    let seated: Vec<(String, Vec<String>)> = hub
        .voice
        .lock()
        .rooms
        .iter()
        .filter(
            |(_, room)| matches!(&room.kind, Kind::Space { space_id: space } if space == space_id),
        )
        .map(|(id, room)| (id.clone(), room.participants.clone()))
        .collect();
    for (channel, participants) in seated {
        for user in participants {
            let held = perms::compute(tx, &user, space_id, Some(&channel))?;
            if !perms::can_view(held) || !perms::has(held, Permissions::CONNECT) {
                kick_from_room(tx, hub, out, space_id, &channel, &user)?;
                continue;
            }
            let should_mute = !perms::has(held, Permissions::SPEAK);
            let key = (space_id.to_owned(), user.clone());
            let changed = {
                let mut inner = hub.voice.lock();
                let was = inner.permission_muted.contains(&key);
                if should_mute {
                    inner.permission_muted.insert(key);
                } else {
                    inner.permission_muted.remove(&key);
                }
                was != should_mute
            };
            if changed {
                to_channel_viewers(
                    tx,
                    hub,
                    out,
                    space_id,
                    &channel,
                    &json!({
                        "type": "voice_permission_muted",
                        "userId": user,
                        "spaceId": space_id,
                        "muted": should_mute,
                    }),
                )?;
            }
        }
    }
    Ok(())
}

/// Канал удаляют: сидящие в нём отключаются.
///
/// # Errors
/// Ошибка базы.
pub fn channel_deleted(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space_id: &str,
    channel_id: &str,
) -> DbResult<()> {
    let participants = hub
        .voice
        .lock()
        .rooms
        .get(channel_id)
        .map(|room| room.participants.clone())
        .unwrap_or_default();
    for user in participants {
        kick_from_room(tx, hub, out, space_id, channel_id, &user)?;
    }
    hub.voice.lock().destroy(channel_id);
    Ok(())
}

/// Участника добавили в группу, где идёт звонок: ему приходит вызов.
///
/// # Errors
/// Ошибка базы.
pub fn dm_member_added(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    dm_channel_id: &str,
    user_id: &str,
) -> DbResult<()> {
    let caller = match hub
        .voice
        .lock()
        .rooms
        .get(dm_channel_id)
        .map(|room| &room.kind)
    {
        Some(Kind::Dm { caller, .. }) => caller.clone(),
        _ => return Ok(()),
    };
    let name = tx
        .query_one(
            "SELECT display_name, username FROM users WHERE id = ?",
            params![&caller],
        )?
        .map(|row| -> DbResult<String> {
            Ok(match row.opt_text("display_name")? {
                Some(name) => name,
                None => row.text("username")?,
            })
        })
        .transpose()?
        .unwrap_or_else(|| caller.clone());
    out.to_user(
        user_id,
        &json!({
            "type": "dm_call_incoming",
            "dmChannelId": dm_channel_id,
            "callerId": caller,
            "callerName": name,
        }),
    );
    Ok(())
}

/// Участник покидает группу: если он в её звонке, его отключают
/// (`evictUserFromDmVoiceRoom`).
///
/// # Errors
/// Ошибка базы.
pub fn dm_member_left(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    dm_channel_id: &str,
    user_id: &str,
) -> DbResult<()> {
    let in_it = hub
        .voice
        .lock()
        .user_room
        .get(user_id)
        .is_some_and(|room| room == dm_channel_id);
    if !in_it {
        return Ok(());
    }
    let left = hub.voice.lock().leave_current(user_id);
    if left.is_some() {
        broadcast::to_dm_members(
            tx,
            out,
            dm_channel_id,
            &state_update(dm_channel_id, user_id, false),
            None,
        )?;
        let empty = hub
            .voice
            .lock()
            .rooms
            .get(dm_channel_id)
            .is_some_and(|room| room.participants.is_empty());
        if empty {
            hub.voice.lock().destroy(dm_channel_id);
            broadcast::to_dm_members(
                tx,
                out,
                dm_channel_id,
                &json!({"type": "dm_call_ended", "dmChannelId": dm_channel_id}),
                None,
            )?;
        }
    }
    hub.voice.lock().status.remove(user_id);
    Ok(())
}

/// Аккаунт удалён или отключён принудительно: он покидает голосовую комнату,
/// а вызовы, которые он начал и на которые ещё не ответили, завершаются.
///
/// # Errors
/// Ошибка базы.
pub fn user_removed(tx: &mut dyn Tx, hub: &Hub, out: &mut Outbox, user_id: &str) -> DbResult<()> {
    {
        let mut inner = hub.voice.lock();
        inner.status.remove(user_id);
        inner.clear_voice_connection(user_id);
    }
    leave_announced(tx, hub, out, user_id, false)?;
    leave_joined_federated(hub, out, user_id);
    end_ringing_placed_by(tx, hub, out, user_id, None)
}

/// Соединение закрылось. Если оно владело голосом, место в комнате держится
/// ещё [`RECONNECT_GRACE`], после чего участник выходит.
pub fn connection_closed(app: &AppState, user_id: &str, connection: u64) {
    let generation = {
        let mut inner = app.hub.voice.lock();
        if inner.voice_connection.get(user_id) != Some(&connection) {
            return;
        }
        inner.voice_connection.remove(user_id);
        let room = inner.user_room.get(user_id).cloned().or_else(|| {
            inner
                .rooms
                .iter()
                .find(|(_, room)| matches!(&room.kind, Kind::Dm { caller, ringing: true, .. } if caller == user_id))
                .map(|(id, _)| id.clone())
        });
        let federated = if room.is_none() {
            inner.joined_federated(user_id)
        } else {
            None
        };
        if room.is_none() && federated.is_none() {
            return;
        }
        inner.next += 1;
        let generation = inner.next;
        inner
            .reconnect
            .insert(user_id.to_owned(), (generation, room, federated));
        generation
    };
    let app = app.clone();
    let user_id = user_id.to_owned();
    tokio::spawn(async move {
        tokio::time::sleep(RECONNECT_GRACE).await;
        let state = app.clone();
        let result = app
            .run(move |tx, out| {
                let (expected, federated) = {
                    let mut inner = state.hub.voice.lock();
                    match inner.reconnect.get(&user_id) {
                        Some((current, ..)) if *current == generation => {}
                        _ => return Ok(()),
                    }
                    let (expected, federated) = inner
                        .reconnect
                        .remove(&user_id)
                        .map_or((None, None), |(_, room, federated)| (room, federated));
                    if inner.voice_connection.contains_key(&user_id) {
                        return Ok(());
                    }
                    inner.status.remove(&user_id);
                    (expected, federated)
                };
                // Сеанс держал звонок другого инстанса: участник выходит из
                // него, как если бы положил трубку.
                if let Some(federated) = federated {
                    let joined = state
                        .hub
                        .voice
                        .lock()
                        .federated
                        .get(&federated)
                        .is_some_and(|call| call.joined.contains(&user_id));
                    if joined {
                        leave_joined_federated(&state.hub, out, &user_id);
                    }
                    return Ok(());
                }
                let current = state.hub.voice.lock().user_room.get(&user_id).cloned();
                if expected.is_none() || current == expected {
                    leave_announced(tx, &state.hub, out, &user_id, false)?;
                }
                let ringing: Vec<String> = state
                    .hub
                    .voice
                    .lock()
                    .rooms
                    .iter()
                    .filter(|(id, room)| {
                        matches!(&room.kind, Kind::Dm { caller, ringing: true, .. } if *caller == user_id)
                            && expected.as_ref().is_none_or(|expected| expected == *id)
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                for room in ringing {
                    end_dm_room(tx, &state.hub, out, &room, false)?;
                    fan_out_end(out, &room, vec![user_id.clone()]);
                }
                Ok(())
            })
            .await;
        if let Err(error) = result {
            tracing::error!(?error, "не удалось завершить голосовой сеанс после обрыва");
        }
    });
}

// ─── Звонки между инстансами ────────────────────────────────────────────────

/// Как долго звонят по вызову с другого инстанса.
const FEDERATED_RING_TIMEOUT: Duration = Duration::from_mins(1);

/// Звонок другого инстанса по ключу, а без ключа — по переписке.
fn find_federated(hub: &Hub, federated: Option<&str>, channel: Option<&str>) -> Option<FedCall> {
    let inner = hub.voice.lock();
    match (federated, channel) {
        (Some(federated), _) => inner.federated.get(federated).cloned(),
        (None, Some(channel)) => inner
            .federated_by_dm(channel)
            .and_then(|id| inner.federated.get(&id).cloned()),
        (None, None) => None,
    }
}

/// Тем, кому звонили отсюда (`sendToFederatedCallUsers`): не участникам
/// переписки, среди которых и копия звонящего.
fn to_federated_users(out: &mut Outbox, call: &FedCall, event: &Value, except: Option<&str>) {
    for user in &call.ringed {
        if except != Some(user.as_str()) {
            out.to_user(user, event);
        }
    }
}

/// Участник выходит из звонка другого инстанса, в который вошёл отсюда, без
/// своего «положить трубку» (`leaveJoinedFederatedCall`). В групповом звонке
/// это и есть его выход, и ведущий о нём узнаёт.
fn leave_joined_federated(hub: &Hub, out: &mut Outbox, user: &str) {
    let call = {
        let mut inner = hub.voice.lock();
        let Some(id) = inner.joined_federated(user) else {
            return;
        };
        let mut snapshot = inner.federated.get(&id).cloned();
        inner.leave_federated_entry(&id, user);
        if let Some(call) = inner.federated.get_mut(&id)
            && call.group
        {
            call.ringed.retain(|id| id != user);
        }
        if let Some(call) = &mut snapshot {
            call.joined.retain(|id| id != user);
            call.ringed.retain(|id| id != user);
        }
        snapshot
    };
    if let Some(call) = call.filter(|call| call.group) {
        out.call(CallTask::ToHost {
            call: Box::new(call),
            user: user.to_owned(),
            kind: "dm_call_end",
            answered_by: None,
            on_fail: OnFail::Log,
        });
    }
}

/// Идёт ли ещё звонок другого инстанса в переписке (`federatedCallStillLive`).
fn federated_call_still_live(
    hub: &Hub,
    out: &mut Outbox,
    federated: &str,
    user: &str,
    connection: u64,
) -> bool {
    let (joined, session) = {
        let inner = hub.voice.lock();
        let joined = inner
            .federated
            .get(federated)
            .is_some_and(|call| call.joined.iter().any(|id| id == user));
        (joined, inner.voice_connection.get(user).copied())
    };
    if joined && session.is_none_or(|session| session == connection) {
        leave_joined_federated(hub, out, user);
    }
    hub.voice
        .lock()
        .federated
        .get(federated)
        .is_some_and(|call| !call.active || !call.joined.is_empty())
}

/// Запоминает вызов с другого инстанса и через минуту без ответа сообщает,
/// что он закончился (`createFederatedCall`).
pub fn create_federated(app: &AppState, call: FedCall) {
    let federated = call.federated_id.clone();
    let generation = {
        let mut inner = app.hub.voice.lock();
        inner.clear_federated(&federated);
        inner.federated.insert(federated.clone(), call);
        inner.next += 1;
        let generation = inner.next;
        inner.federated_timer.insert(federated.clone(), generation);
        generation
    };
    let app = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(FEDERATED_RING_TIMEOUT).await;
        let ended = {
            let mut inner = app.hub.voice.lock();
            if inner.federated_timer.get(&federated) != Some(&generation) {
                return;
            }
            inner.federated_timer.remove(&federated);
            match inner.federated.get(&federated).map(|call| call.active) {
                Some(true) => {
                    inner.drop_federated_if_idle(&federated);
                    None
                }
                Some(false) => inner.federated.remove(&federated),
                None => None,
            }
        };
        if let Some(call) = ended {
            let event = json!({
                "type": "dm_call_ended",
                "dmChannelId": call.dm_channel,
                "federatedCallId": call.federated_id,
            });
            for user in &call.ringed {
                app.hub.send_to_user(user, &event);
            }
        }
    });
}

/// Звонок, который ведёт этот инстанс: `(звонящий, звонят ли ещё, групповой)`.
#[must_use]
pub fn hosted_call(hub: &Hub, dm_channel: &str) -> Option<(String, bool, bool)> {
    match hub
        .voice
        .lock()
        .rooms
        .get(dm_channel)
        .map(|room| &room.kind)
    {
        Some(Kind::Dm {
            caller,
            ringing,
            group,
            ..
        }) => Some((caller.clone(), *ringing, *group)),
        _ => None,
    }
}

/// Пользователь покидает свою комнату, если это не `keep`, и это объявляется
/// (`leaveCurrentRoomAnnounced`).
fn leave_announced_except(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    user: &str,
    keep: &str,
) -> DbResult<()> {
    if hub.voice.lock().user_room.get(user).map(String::as_str) == Some(keep) {
        return Ok(());
    }
    leave_announced(tx, hub, out, user, false)?;
    Ok(())
}

/// Принятие с другого инстанса звонка, который ведёт этот
/// (`processDmCallAcceptEvent`, ведущий): звонящий входит, принявший из
/// группового звонка садится, и принятие уходит остальным инстансам.
///
/// `seat` — принявший и инстанс, сообщивший о нём; `actor` — он же здесь
/// (или пусто); `source` — инстанс события.
///
/// # Errors
/// Ошибка базы.
#[allow(clippy::too_many_arguments, reason = "всё, что даёт событие")]
pub fn relayed_accept_hosted(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    dm_channel: &str,
    federated: &str,
    seat: Option<(String, String)>,
    actor: String,
    answered_by: Option<Value>,
    source: &str,
) -> DbResult<()> {
    let Some((caller, ringing, _)) = hosted_call(hub, dm_channel) else {
        return Ok(());
    };
    if ringing {
        {
            let mut inner = hub.voice.lock();
            if let Some(Kind::Dm { ringing, .. }) =
                inner.rooms.get_mut(dm_channel).map(|room| &mut room.kind)
            {
                *ringing = false;
            }
            inner.ring.remove(dm_channel);
        }
        leave_announced_except(tx, hub, out, &caller, dm_channel)?;
        hub.voice.lock().join(dm_channel, &caller);
        broadcast::to_dm_members(
            tx,
            out,
            dm_channel,
            &state_update(dm_channel, &caller, true),
            None,
        )?;
    }
    if let Some((user, origin)) = seat {
        leave_announced_except(tx, hub, out, &user, dm_channel)?;
        {
            let mut inner = hub.voice.lock();
            inner.join(dm_channel, &user);
            if let Some(Kind::Dm {
                remote, declined, ..
            }) = inner.rooms.get_mut(dm_channel).map(|room| &mut room.kind)
            {
                remote.insert(user.clone(), origin);
                declined.remove(&user);
            }
        }
        broadcast::to_dm_members(
            tx,
            out,
            dm_channel,
            &state_update(dm_channel, &user, true),
            None,
        )?;
    }
    let mut accepted = json!({
        "type": "dm_call_accepted",
        "dmChannelId": dm_channel,
        "federatedCallId": federated,
    });
    if let Some(answered_by) = &answered_by {
        accepted["answeredBy"] = answered_by.clone();
    }
    broadcast::to_dm_members(tx, out, dm_channel, &accepted, None)?;
    out.call(CallTask::FanOut {
        dm_channel: dm_channel.to_owned(),
        kind: "dm_call_accept",
        actors: vec![actor, caller.clone()],
        exclude: Some(source.to_owned()),
        answered_by,
        report: Some((caller, "accept")),
    });
    Ok(())
}

/// Отказ или конец с другого инстанса для звонка, который ведёт этот
/// (`processDmCallRejectEvent`, `processDmCallEndEvent`, ведущий).
///
/// `member` — участник здесь, если он есть и состоит в переписке; `actor` —
/// он же для имени события (или пусто).
///
/// # Errors
/// Ошибка базы.
#[allow(clippy::too_many_arguments, reason = "всё, что даёт событие")]
pub fn relayed_leave_hosted(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    dm_channel: &str,
    decline: bool,
    member: Option<String>,
    actor: String,
    per_member: bool,
    source: &str,
) -> DbResult<()> {
    let Some((caller, _, group)) = hosted_call(hub, dm_channel) else {
        return Ok(());
    };
    if group {
        // Отказ или выход убирает только этого участника. Когда отвечать
        // больше некому, звонок кончается у всех, и у отправителя тоже;
        // конец называет звонящего.
        let mut ended = !per_member && leave_peer_participants(tx, hub, out, dm_channel, source)?.1;
        if !ended && let Some(member) = member {
            let outcome = if decline {
                decline_group_call(tx, hub, out, dm_channel, &member)?
            } else {
                leave_group_call(tx, hub, out, dm_channel, &member)?
            };
            ended = outcome == GroupOutcome::Ended;
        }
        if ended {
            fan_out_end(out, dm_channel, vec![caller]);
        }
        return Ok(());
    }
    end_dm_room(tx, hub, out, dm_channel, decline)?;
    out.call(CallTask::FanOut {
        dm_channel: dm_channel.to_owned(),
        kind: "dm_call_end",
        actors: vec![actor, caller.clone()],
        exclude: Some(source.to_owned()),
        answered_by: None,
        report: Some((caller, if decline { "reject" } else { "end" })),
    });
    Ok(())
}

/// Событие ведущего для звонка, который ведёт другой инстанс
/// (`processDmCall*Event`, не ведущий): `kind` — `dm_call_accept`,
/// `dm_call_reject` или `dm_call_end`.
pub fn relayed_to_federated(
    hub: &Hub,
    out: &mut Outbox,
    federated: &str,
    kind: &str,
    answered_by: Option<Value>,
) {
    let Some(call) = hub.voice.lock().federated.get(federated).cloned() else {
        return;
    };
    if kind == "dm_call_accept" {
        // Уже отвеченный (например, принятие отсюда, которое ведущий
        // разослал обратно) не объявляется второй раз.
        if call.active {
            return;
        }
        if let Some(entry) = hub.voice.lock().federated.get_mut(federated) {
            entry.active = true;
        }
        let mut accepted = json!({
            "type": "dm_call_accepted",
            "dmChannelId": call.dm_channel,
            "federatedCallId": federated,
        });
        if let Some(answered_by) = answered_by {
            accepted["answeredBy"] = answered_by;
        }
        to_federated_users(out, &call, &accepted, None);
        return;
    }
    let wire = if kind == "dm_call_reject" {
        "dm_call_rejected"
    } else {
        "dm_call_ended"
    };
    to_federated_users(
        out,
        &call,
        &json!({"type": wire, "dmChannelId": call.dm_channel, "federatedCallId": federated}),
        None,
    );
    hub.voice.lock().clear_federated(federated);
}

/// Участники звонка с инстанса `origin` выходят из него
/// (`leavePeerParticipants`): `(сколько вышло, закончился ли звонок)`.
fn leave_peer_participants(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    dm_channel: &str,
    origin: &str,
) -> DbResult<(usize, bool)> {
    let Some(key) = crate::federation_auth::normalize_origin(Some(origin)) else {
        return Ok((0, false));
    };
    let seated: Vec<String> = match hub
        .voice
        .lock()
        .rooms
        .get(dm_channel)
        .map(|room| &room.kind)
    {
        Some(Kind::Dm { remote, .. }) => {
            let mut seated: Vec<(String, String)> =
                remote.iter().map(|(u, o)| (u.clone(), o.clone())).collect();
            seated.sort();
            seated
                .into_iter()
                .filter(|(_, o)| {
                    crate::federation_auth::normalize_origin(Some(o)).as_deref()
                        == Some(key.as_str())
                })
                .map(|(user, _)| user)
                .collect()
        }
        _ => return Ok((0, false)),
    };
    let mut removed = 0;
    for user in seated {
        hub.voice.lock().leave_room(dm_channel, &user);
        removed += 1;
        if after_dm_leave(tx, hub, out, dm_channel, &user)? {
            return Ok((removed, true));
        }
    }
    Ok((removed, false))
}

/// Пир перестал быть действующим: участники звонков, которые ведёт этот
/// инстанс, с его дома выходят (`dropRemoteCallParticipants`).
///
/// # Errors
/// Ошибка базы.
pub fn drop_remote_participants(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    origin: &str,
) -> DbResult<usize> {
    let rooms: Vec<(String, String)> = hub
        .voice
        .lock()
        .rooms
        .iter()
        .filter_map(|(id, room)| match &room.kind {
            Kind::Dm { caller, .. } => Some((id.clone(), caller.clone())),
            Kind::Space { .. } => None,
        })
        .collect();
    let mut removed = 0;
    for (room, caller) in rooms {
        let (count, ended) = leave_peer_participants(tx, hub, out, &room, origin)?;
        removed += count;
        if ended {
            fan_out_end(out, &room, vec![caller]);
        }
    }
    Ok(removed)
}

/// Ведущий звонков недоступен: звонки, которые он ведёт, забываются, а
/// тем, кому звонили, уходит окончательное `dm_call_undeliverable`
/// (`evictFederatedCallsForHost`).
pub fn evict_for_host(
    hub: &Hub,
    out: &mut Outbox,
    host: &str,
    reason: &str,
    label: Option<&str>,
) -> usize {
    let calls: Vec<FedCall> = hub
        .voice
        .lock()
        .federated
        .values()
        .filter(|call| call.host == host)
        .cloned()
        .collect();
    let mut evicted = 0;
    for call in calls {
        let mut failure = json!({"reason": reason, "peerOrigin": host});
        if let Some(label) = label {
            failure["peerLabel"] = json!(label);
        }
        let event = json!({
            "type": "dm_call_undeliverable",
            "dmChannelId": call.dm_channel,
            "federatedCallId": call.federated_id,
            "terminal": true,
            "phase": "host_unreachable",
            "failures": [failure],
        });
        for user in &call.ringed {
            out.to_user(user, &event);
        }
        hub.voice.lock().clear_federated(&call.federated_id);
        evicted += 1;
    }
    evicted
}

/// Ведущие звонков, в которых участвуют отсюда.
#[must_use]
pub fn federated_hosts(hub: &Hub) -> Vec<String> {
    let mut hosts: Vec<String> = hub
        .voice
        .lock()
        .federated
        .values()
        .map(|call| call.host.clone())
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

/// Переписка появилась здесь после вызова (`lateBindFederatedCall`).
pub fn late_bind(hub: &Hub, federated: &str, dm_channel: &str) {
    if let Some(call) = hub.voice.lock().federated.get_mut(federated)
        && call.dm_channel.is_none()
    {
        call.dm_channel = Some(dm_channel.to_owned());
    }
}

/// Забывает звонок другого инстанса (`clearFederatedCall`).
pub fn forget_federated(hub: &Hub, federated: &str) {
    hub.voice.lock().clear_federated(federated);
}

/// Вызов никуда не дошёл (`emitUndeliverableAndMaybeDestroy`): звонящему
/// сообщается, а если отвечать некому — звонок здесь заканчивается.
///
/// # Errors
/// Ошибка базы.
#[allow(clippy::too_many_arguments, reason = "всё, что нужно сообщению")]
pub fn start_undeliverable(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    caller: &str,
    dm_channel: &str,
    federated: &str,
    terminal: bool,
    failures: Value,
) -> DbResult<()> {
    if !hub.voice.lock().rooms.contains_key(dm_channel) {
        return Ok(());
    }
    if terminal {
        {
            let mut inner = hub.voice.lock();
            inner.clear_voice_connection(caller);
            inner.destroy(dm_channel);
        }
        broadcast::to_dm_members(
            tx,
            out,
            dm_channel,
            &json!({"type": "dm_call_ended", "dmChannelId": dm_channel}),
            Some(caller),
        )?;
    }
    out.to_user(
        caller,
        &json!({
            "type": "dm_call_undeliverable",
            "dmChannelId": dm_channel,
            "federatedCallId": federated,
            "terminal": terminal,
            "phase": "start",
            "failures": failures,
        }),
    );
    Ok(())
}

/// Групповой ли звонок, который ведёт этот инстанс в переписке.
#[must_use]
pub fn hosted_group(hub: &Hub, dm_channel: &str) -> bool {
    hosted_call(hub, dm_channel).is_some_and(|(_, _, group)| group)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn federated(id: &str, active: bool, joined: &[&str]) -> FedCall {
        FedCall {
            dm_channel: Some(format!("dm-{id}")),
            federated_id: id.to_owned(),
            caller: "caller".to_owned(),
            host: "https://y.test".to_owned(),
            livekit_url: "wss://lk".to_owned(),
            tokens: HashMap::new(),
            ringed: joined.iter().map(|u| (*u).to_owned()).collect(),
            joined: joined.iter().map(|u| (*u).to_owned()).collect(),
            group: true,
            active,
            started_at: 0,
        }
    }

    #[test]
    fn federated_call_lookup_and_idle_drop() {
        let mut inner = Inner::default();
        inner
            .federated
            .insert("f".into(), federated("f", true, &["u", "v"]));
        inner.voice_connection.insert("u".into(), 1);
        assert_eq!(inner.joined_federated("u").as_deref(), Some("f"));
        assert_eq!(inner.federated_by_dm("dm-f").as_deref(), Some("f"));
        inner.leave_federated_entry("f", "u");
        assert!(inner.federated.contains_key("f"), "в звонке ещё есть v");
        assert!(inner.joined_federated("u").is_none());
        // Пока идёт таймер вызова, отвеченный звонок без участников держится.
        inner.federated_timer.insert("f".into(), 1);
        inner.leave_federated_entry("f", "v");
        assert!(inner.federated.contains_key("f"));
        inner.federated_timer.clear();
        inner.drop_federated_if_idle("f");
        assert!(!inner.federated.contains_key("f"));
    }

    #[test]
    fn clearing_a_federated_call_frees_its_sessions() {
        let mut inner = Inner::default();
        inner
            .federated
            .insert("f".into(), federated("f", true, &["u", "w"]));
        inner.voice_connection.insert("u".into(), 1);
        inner.voice_connection.insert("w".into(), 2);
        inner.rooms.insert("room".into(), space_room("s"));
        inner.join("room", "w");
        inner.clear_federated("f");
        assert!(!inner.voice_connection.contains_key("u"));
        assert!(
            inner.voice_connection.contains_key("w"),
            "сеанс того, кто сидит в комнате здесь, остаётся"
        );
        // Неотвеченный звонок не считается простаивающим.
        inner
            .federated
            .insert("g".into(), federated("g", false, &[]));
        inner.drop_federated_if_idle("g");
        assert!(inner.federated.contains_key("g"));
    }

    fn space_room(space: &str) -> Room {
        Room {
            kind: Kind::Space {
                space_id: space.to_owned(),
            },
            participants: Vec::new(),
            started_at: now_ms(),
        }
    }

    #[test]
    fn joining_another_room_leaves_the_first() {
        let mut inner = Inner::default();
        inner.rooms.insert("a".into(), space_room("s"));
        inner.rooms.insert("b".into(), space_room("s"));
        inner.join("a", "u");
        inner.join("b", "u");
        assert!(
            !inner.rooms.contains_key("a"),
            "пустая комната канала исчезает"
        );
        assert_eq!(inner.rooms["b"].participants, vec!["u"]);
        assert_eq!(inner.user_room.get("u").map(String::as_str), Some("b"));
    }

    #[test]
    fn leaving_clears_space_flags() {
        let mut inner = Inner::default();
        inner.rooms.insert("a".into(), space_room("s"));
        inner.join("a", "u");
        inner.space_muted.insert(("s".into(), "u".into()));
        assert!(inner.leave_current("u").is_some());
        assert!(inner.space_muted.is_empty());
        assert!(inner.user_room.is_empty());
    }

    #[test]
    fn destroying_a_call_frees_everyone() {
        let mut inner = Inner::default();
        inner.rooms.insert(
            "dm".into(),
            Room {
                kind: Kind::Dm {
                    caller: "a".into(),
                    ringing: false,
                    group: false,
                    declined: HashSet::new(),
                    remote: HashMap::new(),
                },
                participants: Vec::new(),
                started_at: 0,
            },
        );
        inner.join("dm", "a");
        inner.join("dm", "b");
        assert_eq!(inner.destroy("dm"), vec!["a", "b"]);
        assert!(inner.user_room.is_empty());
    }
}
