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
    let (requests_tx, mut requests_rx) = tokio_mpsc::unbounded_channel();
    bridge.commands = Some(commands_tx);
    bridge.requests = Some(requests_tx);
    bridge.phase = Phase::Connecting;
    bridge.handle_event(BridgeEvent::Ready {
        payload: Box::new(ready_payload()),
    });
    // Первым после `ready` уходит загрузка настроек уведомлений.
    assert!(matches!(
        requests_rx.try_recv(),
        Ok(BridgeRequest::NotificationSettings)
    ));
    // Следом клиент публикует свой открытый ключ шифрования переписок.
    assert!(matches!(
        requests_rx.try_recv(),
        Ok(BridgeRequest::E2eePublish(_))
    ));
    (bridge, commands_rx, requests_rx)
}

fn params() -> SessionParams {
    SessionParams {
        origin: bs_types::Origin::Home,
        base: "http://127.0.0.1:1".to_owned(),
        username: "alice".to_owned(),
        password: zeroize::Zeroizing::new("secret".to_owned()),
        second_factor: None,
        register: None,
        resume: None,
        persist: None,
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

/// Скачивание вложения на диск: прогресс, затем «готово»; ошибка — текстом.
#[tokio::test]
#[allow(clippy::expect_used, reason = "в тестах паника допустима")]
async fn download_request_writes_the_file_and_reports_failures() {
    let ready = ServerEvent::Ready(Box::new(ready_payload()));
    let (base, _server_events) = crate::fake_instance::spawn(ready).await;
    let dir = std::env::temp_dir().join(format!("bs-bridge-dl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("папка");

    let (events_tx, events_rx) = std_mpsc::channel();
    let (_commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
    let (requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
    let (responses_tx, responses_rx) = std_mpsc::channel();
    let mut p = params();
    p.base = base;
    let session = tokio::spawn(run_session(
        p,
        events_tx,
        commands_rx,
        requests_rx,
        responses_tx,
    ));
    let ready = tokio::task::spawn_blocking(move || {
        let event = events_rx.recv_timeout(Duration::from_secs(10));
        (events_rx, event)
    })
    .await
    .expect("join");
    assert!(
        matches!(&ready.1, Ok(BridgeEvent::Ready { .. })),
        "{:?}",
        ready.1
    );

    let ok_path = dir.join("a.bin");
    let bad_path = dir.join("b.bin");
    for (id, name, dest) in [
        (1, "a.bin", ok_path.clone()),
        (2, "gone.bin", bad_path.clone()),
    ] {
        requests_tx
            .send(BridgeRequest::Download {
                id,
                path: bs_api::AssetPath::parse(&format!("/api/uploads/{name}")).expect("путь"),
                dest,
            })
            .expect("запрос");
    }
    let states = tokio::task::spawn_blocking(move || {
        let mut done = std::collections::HashMap::new();
        while done.len() < 2 {
            match responses_rx.recv_timeout(Duration::from_secs(10)) {
                Ok(BridgeResponse::Download { id, state })
                    if !matches!(state, super::DownloadState::Progress { .. }) =>
                {
                    done.insert(id, state);
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        done
    })
    .await
    .expect("join");
    assert_eq!(states.get(&1), Some(&super::DownloadState::Done));
    assert!(
        matches!(states.get(&2), Some(super::DownloadState::Failed(m)) if m.contains("больше не доступен")),
        "{states:?}"
    );
    assert_eq!(std::fs::read(&ok_path).expect("файл").len(), 3000);
    assert!(!bad_path.exists());
    session.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Явный выход стирает сохранённую сессию, а без файла входа по токену нет.
#[test]
#[allow(clippy::expect_used, reason = "в тестах паника допустима")]
fn forget_saved_removes_the_file_and_blocks_resume() {
    let path = std::env::temp_dir()
        .join(format!("bs-forget-{}", std::process::id()))
        .join("session.json");
    crate::saved_session::save(&path, "https://x.example", "tok", TransportPolicy::Strict)
        .expect("запись");
    let (mut bridge, _commands, _requests) = active_bridge();
    bridge.set_persist(Some(path.clone()));
    bridge.forget_saved();
    assert!(crate::saved_session::load(&path).is_none());
    assert!(bridge.resume_saved().is_none());
}

/// Вход по сохранённому токену: пароль не нужен, токен пишется обратно.
#[tokio::test]
#[allow(clippy::expect_used, reason = "в тестах паника допустима")]
async fn resume_with_saved_token_reaches_ready_and_persists() {
    let ready = ServerEvent::Ready(Box::new(ready_payload()));
    let (base, _server_events) = crate::fake_instance::spawn(ready).await;
    let path = std::env::temp_dir()
        .join(format!("bs-resume-ok-{}", std::process::id()))
        .join("session.json");

    let (events_tx, events_rx) = std_mpsc::channel();
    let (_commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
    let (_requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
    let (responses_tx, _responses_rx) = std_mpsc::channel();

    let mut p = params();
    p.base = base.clone();
    p.password = zeroize::Zeroizing::new(String::new());
    p.resume = Some(Secret::new("jwt-abc"));
    p.persist = Some(path.clone());
    let session = tokio::spawn(run_session(
        p,
        events_tx,
        commands_rx,
        requests_rx,
        responses_tx,
    ));
    let event =
        tokio::task::spawn_blocking(move || events_rx.recv_timeout(Duration::from_secs(10)))
            .await
            .expect("join");
    assert!(matches!(&event, Ok(BridgeEvent::Ready { .. })), "{event:?}");
    let saved = crate::saved_session::load(&path).expect("сессия сохранена");
    assert_eq!(saved.base, base);
    assert_eq!(saved.token.expose(), "jwt-abc");
    session.abort();
    crate::saved_session::clear(&path);
}

/// Сервер не принял токен: файл стирается, пользователь видит причину.
#[tokio::test]
#[allow(clippy::expect_used, reason = "в тестах паника допустима")]
async fn resume_with_rejected_token_clears_the_file() {
    let ready = ServerEvent::Ready(Box::new(ready_payload()));
    let (base, _server_events) = crate::fake_instance::spawn(ready).await;
    let path = std::env::temp_dir()
        .join(format!("bs-resume-bad-{}", std::process::id()))
        .join("session.json");
    crate::saved_session::save(&path, &base, "old-token", TransportPolicy::Strict).expect("запись");

    let (events_tx, events_rx) = std_mpsc::channel();
    let (_commands_tx, commands_rx) = tokio_mpsc::unbounded_channel();
    let (_requests_tx, requests_rx) = tokio_mpsc::unbounded_channel();
    let (responses_tx, _responses_rx) = std_mpsc::channel();

    let mut p = params();
    p.base = base;
    p.resume = Some(Secret::new("old-token"));
    p.persist = Some(path.clone());
    run_session(p, events_tx, commands_rx, requests_rx, responses_tx).await;

    let event = events_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("событие об отказе");
    let BridgeEvent::LoginFailed { message, kind } = event else {
        unreachable!("ожидался отказ входа, пришло {event:?}");
    };
    assert_eq!(kind, super::FailureKind::Auth);
    assert!(message.contains("войдите заново"), "{message}");
    assert!(crate::saved_session::load(&path).is_none(), "файл стёрт");
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
    let space: bs_types::SpaceWithChannelsAndMembers = serde_json::from_value(serde_json::json!({
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
