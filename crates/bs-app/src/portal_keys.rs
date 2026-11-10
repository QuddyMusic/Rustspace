//! Глобальные клавиши под Wayland через портал `GlobalShortcuts`
//! (`globalShortcutsPortal.ts` десктопа оригинала).
//!
//! Под Wayland приложение не может само перехватывать клавиши: оно
//! объявляет действия порталу, а сочетания пользователь назначает в
//! системном диалоге или настройках (GNOME, KDE, Hyprland…). Портал
//! присылает `Activated`/`Deactivated` — нажатие и отпускание, поэтому
//! работает и «говорить, пока зажата».
//!
//! Всё общение с D-Bus — в отдельном потоке на блокирующем API `zbus`;
//! окно получает команды через общую очередь и читает состояние из
//! [`status`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use crate::prefs::Action;

const SERVICE: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.portal.GlobalShortcuts";
/// Сколько ждать ответа пользователя в системном диалоге.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);

/// Состояние портала для настроек.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum PortalState {
    /// Портал не используется (не Wayland или сборка без клавиш).
    #[default]
    Off,
    /// Ждём ответа портала или пользователя.
    Pending,
    /// Действия объявлены; сочетания — по действиям (пусто — не назначено).
    Ready(BTreeMap<Action, String>),
    /// Портал недоступен или отказал.
    Failed(String),
}

static STATUS: Mutex<PortalState> = Mutex::new(PortalState::Off);

fn status_lock() -> MutexGuard<'static, PortalState> {
    STATUS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Текущее состояние портала.
pub(crate) fn status() -> PortalState {
    status_lock().clone()
}

fn set_status(state: PortalState) {
    *status_lock() = state;
}

/// Идёт ли сеанс Wayland, где перехват клавиш возможен только через портал.
pub(crate) fn wanted() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind.eq_ignore_ascii_case("wayland"))
}

/// Идентификатор действия у портала (как у оригинала).
const fn shortcut_id(action: Action) -> &'static str {
    match action {
        Action::ToggleMute => "toggleMute",
        Action::ToggleDeafen => "toggleDeafen",
        Action::PushToTalk => "pushToTalk",
        Action::ToggleCamera => "toggleCamera",
        Action::ToggleScreenShare => "toggleScreenShare",
        Action::Disconnect => "disconnect",
    }
}

fn action_of(id: &str) -> Option<Action> {
    Action::ALL
        .into_iter()
        .find(|action| shortcut_id(*action) == id)
}

/// Описание действия в системном диалоге (английское, как у оригинала:
/// портал показывает его вне приложения).
const fn description(action: Action) -> &'static str {
    match action {
        Action::ToggleMute => "Toggle microphone",
        Action::ToggleDeafen => "Toggle deafen",
        Action::PushToTalk => "Push to talk",
        Action::ToggleCamera => "Toggle camera",
        Action::ToggleScreenShare => "Toggle screen sharing",
        Action::Disconnect => "Disconnect from voice",
    }
}

type Shortcuts = Vec<(String, HashMap<String, OwnedValue>)>;

/// Сочетания из ответа портала: действие → описание сочетания.
fn triggers(shortcuts: &Shortcuts) -> BTreeMap<Action, String> {
    shortcuts
        .iter()
        .filter_map(|(id, props)| {
            let action = action_of(id)?;
            let trigger = props
                .get("trigger_description")
                .and_then(|value| String::try_from(value.clone()).ok())
                .unwrap_or_default();
            Some((action, trigger))
        })
        .collect()
}

fn token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("backspace_{}_{nanos}", std::process::id())
}

type Command = Box<dyn Fn(Action, bool) + Send>;

/// Запускает работу с порталом в отдельном потоке; `on_key` вызывается
/// при нажатии и отпускании.
pub(crate) fn start(on_key: Command) {
    set_status(PortalState::Pending);
    let spawned = std::thread::Builder::new()
        .name("bs-portal-keys".into())
        .spawn(move || {
            if let Err(err) = run(&on_key) {
                tracing::info!(%err, "портал глобальных клавиш недоступен");
                set_status(PortalState::Failed(err));
            }
        });
    if let Err(err) = spawned {
        set_status(PortalState::Failed(err.to_string()));
    }
}

struct Portal {
    connection: Connection,
    signals: MessageIterator,
    request_prefix: String,
}

impl Portal {
    fn connect() -> Result<Self, String> {
        let connection = Connection::session().map_err(|err| err.to_string())?;
        let unique = connection
            .unique_name()
            .ok_or("нет имени на шине")?
            .trim_start_matches(':')
            .replace('.', "_");
        let rule = zbus::MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(SERVICE)
            .map_err(|err| err.to_string())?
            .build();
        let signals = MessageIterator::for_match_rule(rule, &connection, Some(64))
            .map_err(|err| err.to_string())?;
        Ok(Self {
            connection,
            signals,
            request_prefix: format!("{PATH}/request/{unique}/"),
        })
    }

    /// Приложение вне песочницы называет себя порталу (иначе GNOME не
    /// знает, чьи это сочетания). Старые порталы метода не знают — не страшно.
    fn register(&self) {
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        if let Err(err) = self.connection.call_method(
            Some(SERVICE),
            PATH,
            Some("org.freedesktop.host.portal.Registry"),
            "Register",
            &("backspace-rs", options),
        ) {
            tracing::debug!(%err, "Registry.Register не сработал");
        }
    }

    /// Вызывает метод с `handle_token` и ждёт сигнал `Response`.
    fn request<B>(
        &mut self,
        method: &str,
        body: impl FnOnce(&str) -> B,
    ) -> Result<HashMap<String, OwnedValue>, String>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
    {
        let token = token();
        let expected = format!("{}{token}", self.request_prefix);
        self.connection
            .call_method(Some(SERVICE), PATH, Some(INTERFACE), method, &body(&token))
            .map_err(|err| err.to_string())?;
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        while Instant::now() < deadline {
            let Some(message) = self.signals.next() else {
                return Err("соединение с шиной закрыто".into());
            };
            let Ok(message) = message else { continue };
            let header = message.header();
            let is_response = header.member().is_some_and(|m| m.as_str() == "Response")
                && header.path().is_some_and(|p| p.as_str() == expected);
            if !is_response {
                continue;
            }
            let (code, results): (u32, HashMap<String, OwnedValue>) = message
                .body()
                .deserialize()
                .map_err(|err| err.to_string())?;
            return if code == 0 {
                Ok(results)
            } else {
                Err("пользователь отклонил или отменил запрос".into())
            };
        }
        Err("портал не ответил".into())
    }
}

fn run(on_key: &Command) -> Result<(), String> {
    let mut portal = Portal::connect()?;
    portal.register();
    let created = portal.request("CreateSession", |token| {
        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        options.insert("handle_token", Value::from(token.to_owned()));
        options.insert("session_handle_token", Value::from(token.to_owned()));
        (options,)
    })?;
    let session: OwnedObjectPath = created
        .get("session_handle")
        .and_then(|value| {
            String::try_from(value.clone())
                .ok()
                .and_then(|s| OwnedObjectPath::try_from(s).ok())
                .or_else(|| OwnedObjectPath::try_from(value.clone()).ok())
        })
        .ok_or("портал не вернул сессию")?;
    let shortcuts: Vec<(&str, HashMap<&str, Value<'_>>)> = Action::ALL
        .into_iter()
        .map(|action| {
            let mut props: HashMap<&str, Value<'_>> = HashMap::new();
            props.insert("description", Value::from(description(action)));
            (shortcut_id(action), props)
        })
        .collect();
    let session_path = ObjectPath::from(&session);
    let bound = portal.request("BindShortcuts", |token| {
        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        options.insert("handle_token", Value::from(token.to_owned()));
        (session_path.clone(), shortcuts, "", options)
    })?;
    let list: Shortcuts = bound
        .get("shortcuts")
        .and_then(|value| Shortcuts::try_from(value.clone()).ok())
        .unwrap_or_default();
    set_status(PortalState::Ready(triggers(&list)));
    tracing::info!("глобальные клавиши через портал готовы");
    listen(&mut portal, &session, on_key)
}

/// Сигналы сессии: нажатия, отпускания и смена сочетаний.
fn listen(portal: &mut Portal, session: &OwnedObjectPath, on_key: &Command) -> Result<(), String> {
    for message in portal.signals.by_ref() {
        let Ok(message) = message else { continue };
        let header = message.header();
        if header.interface().is_none_or(|i| i.as_str() != INTERFACE) {
            continue;
        }
        let Some(member) = header.member().map(|m| m.as_str().to_owned()) else {
            continue;
        };
        match member.as_str() {
            "Activated" | "Deactivated" => {
                let Ok((from, id, _, _)): Result<
                    (OwnedObjectPath, String, u64, HashMap<String, OwnedValue>),
                    _,
                > = message.body().deserialize() else {
                    continue;
                };
                if from == *session
                    && let Some(action) = action_of(&id)
                {
                    on_key(action, member == "Activated");
                }
            }
            "ShortcutsChanged" => {
                if let Ok((from, list)) =
                    message.body().deserialize::<(OwnedObjectPath, Shortcuts)>()
                    && from == *session
                {
                    set_status(PortalState::Ready(triggers(&list)));
                }
            }
            _ => {}
        }
    }
    Err("соединение с порталом закрыто".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for action in Action::ALL {
            assert_eq!(action_of(shortcut_id(action)), Some(action));
        }
        assert_eq!(action_of("nope"), None);
    }

    #[test]
    fn triggers_are_read_from_portal_reply() {
        let mut props = HashMap::new();
        props.insert(
            "trigger_description".to_owned(),
            OwnedValue::try_from(Value::from("Ctrl+M")).expect("строка"),
        );
        let list: Shortcuts = vec![
            ("toggleMute".to_owned(), props),
            ("pushToTalk".to_owned(), HashMap::new()),
            ("unknown".to_owned(), HashMap::new()),
        ];
        let map = triggers(&list);
        assert_eq!(
            map.get(&Action::ToggleMute).map(String::as_str),
            Some("Ctrl+M")
        );
        assert_eq!(map.get(&Action::PushToTalk).map(String::as_str), Some(""));
        assert_eq!(map.len(), 2);
    }

    /// Поддельный портал: отвечает на запросы сигналами `Response`.
    struct Mock;

    fn request_path(
        header: &zbus::message::Header<'_>,
        options: &HashMap<String, OwnedValue>,
    ) -> String {
        let sender = header
            .sender()
            .map(|s| s.as_str().trim_start_matches(':').replace('.', "_"))
            .unwrap_or_default();
        let token = options
            .get("handle_token")
            .and_then(|v| String::try_from(v.clone()).ok())
            .unwrap_or_default();
        format!("{PATH}/request/{sender}/{token}")
    }

    fn respond(connection: zbus::Connection, path: String, results: HashMap<String, OwnedValue>) {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            let connection = zbus::blocking::Connection::from(connection);
            connection
                .emit_signal(
                    None::<()>,
                    path.as_str(),
                    "org.freedesktop.portal.Request",
                    "Response",
                    &(0_u32, results),
                )
                .expect("ответ");
        });
    }

    fn string_value(text: &str) -> OwnedValue {
        OwnedValue::try_from(Value::from(text)).expect("строка")
    }

    // Сигнатуры методов задаёт макрос zbus: аргументы по значению, `&self`.
    #[allow(clippy::unused_self, clippy::needless_pass_by_value)]
    #[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts")]
    impl Mock {
        fn create_session(
            &self,
            options: HashMap<String, OwnedValue>,
            #[zbus(header)] header: zbus::message::Header<'_>,
            #[zbus(connection)] connection: &zbus::Connection,
        ) -> OwnedObjectPath {
            let path = request_path(&header, &options);
            let mut results = HashMap::new();
            results.insert(
                "session_handle".to_owned(),
                string_value(&format!("{PATH}/session/test/s1")),
            );
            respond(connection.clone(), path.clone(), results);
            OwnedObjectPath::try_from(path).expect("путь")
        }

        fn bind_shortcuts(
            &self,
            session: OwnedObjectPath,
            shortcuts: Vec<(String, HashMap<String, OwnedValue>)>,
            parent: String,
            options: HashMap<String, OwnedValue>,
            #[zbus(header)] header: zbus::message::Header<'_>,
            #[zbus(connection)] connection: &zbus::Connection,
        ) -> OwnedObjectPath {
            assert_eq!(shortcuts.len(), Action::ALL.len());
            assert!(session.as_str().starts_with(PATH));
            assert!(parent.is_empty());
            let path = request_path(&header, &options);
            let mut mute = HashMap::new();
            mute.insert("trigger_description".to_owned(), string_value("Ctrl+M"));
            let list: Shortcuts = vec![("toggleMute".to_owned(), mute)];
            let mut results = HashMap::new();
            results.insert(
                "shortcuts".to_owned(),
                OwnedValue::try_from(Value::from(list)).expect("список"),
            );
            respond(connection.clone(), path.clone(), results);
            OwnedObjectPath::try_from(path).expect("путь")
        }
    }

    /// Весь обмен с порталом: сессия, объявление действий, сочетания и
    /// нажатия. Запуск: `dbus-run-session -- cargo test -p bs-app
    /// --all-features portal_keys -- --ignored`.
    #[test]
    #[ignore = "нужна своя сессионная шина D-Bus"]
    fn talks_to_portal() {
        let server = zbus::blocking::connection::Builder::session()
            .and_then(|b| b.name(SERVICE))
            .and_then(|b| b.serve_at(PATH, Mock))
            .and_then(zbus::blocking::connection::Builder::build)
            .expect("поддельный портал");
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        start(Box::new(move |action, pressed| {
            let _ = tx.lock().map(|tx| tx.send((action, pressed)));
        }));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !matches!(status(), PortalState::Ready(_)) {
            assert!(Instant::now() < deadline, "портал не готов: {:?}", status());
            std::thread::sleep(Duration::from_millis(20));
        }
        let PortalState::Ready(map) = status() else {
            unreachable!()
        };
        assert_eq!(
            map.get(&Action::ToggleMute).map(String::as_str),
            Some("Ctrl+M")
        );
        let session = OwnedObjectPath::try_from(format!("{PATH}/session/test/s1")).expect("путь");
        for member in ["Activated", "Deactivated"] {
            server
                .emit_signal(
                    None::<()>,
                    PATH,
                    INTERFACE,
                    member,
                    &(
                        &session,
                        "pushToTalk",
                        0_u64,
                        HashMap::<String, OwnedValue>::new(),
                    ),
                )
                .expect("сигнал");
        }
        let timeout = Duration::from_secs(5);
        assert_eq!(rx.recv_timeout(timeout), Ok((Action::PushToTalk, true)));
        assert_eq!(rx.recv_timeout(timeout), Ok((Action::PushToTalk, false)));
    }
}
