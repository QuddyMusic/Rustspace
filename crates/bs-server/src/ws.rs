//! Шлюз WebSocket (`/ws`): вход по токену, `ready`, пульс, присутствие.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

use crate::db::{DbResult, Tx};
use crate::error::ApiError;
use crate::hub::{Outbox, Outgoing, SEND_QUEUE};
use crate::params;
use crate::state::AppState;
use crate::{auth, events, ready};

/// Сколько клиент может молчать до входа.
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// Период проверки связи.
const HEARTBEAT: Duration = Duration::from_secs(30);
/// Отсрочка ухода в офлайн после последнего соединения.
const OFFLINE_GRACE: Duration = Duration::from_secs(5);
/// Наибольший кадр от клиента. Самое большое событие — сообщение с текстом
/// в 4000 знаков, так что мегабайта хватает с запасом.
const MAX_FRAME: usize = 1_048_576;

/// `GET /ws`.
pub async fn upgrade(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let ip = crate::ratelimit::client_ip(&headers, peer, state.config.trusted_proxy_hops);
    upgrade
        .max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| serve(state, socket, ip))
}

fn error_frame(message: &str) -> Message {
    Message::Text(
        json!({"type": "error", "message": message})
            .to_string()
            .into(),
    )
}

/// Событие `presence_update` о пользователе.
///
/// # Errors
/// Ошибка базы.
pub fn presence_update(
    tx: &mut dyn Tx,
    user_id: &str,
    status: &str,
    activities: Option<&[Value]>,
) -> DbResult<Value> {
    let row = tx.query_one(
        "SELECT home_user_id, home_instance FROM users WHERE id = ?",
        params![user_id],
    )?;
    let (home_user, home_instance) = match row {
        Some(row) => (
            row.opt_text("home_user_id")?,
            row.opt_text("home_instance")?,
        ),
        None => (None, None),
    };
    let mut event = ready::presence_identity(home_user, home_instance);
    event["type"] = json!("presence_update");
    event["userId"] = json!(user_id);
    event["status"] = json!(status);
    if let Some(activities) = activities {
        event["activities"] = json!(activities);
    }
    Ok(event)
}

/// Кому видны изменения профиля и присутствия пользователя: участники общих
/// пространств и переписок, друзья (`collectProfileBroadcastTargetIds`).
///
/// # Errors
/// Ошибка базы.
pub fn profile_targets(tx: &mut dyn Tx, user_id: &str) -> DbResult<Vec<String>> {
    tx.query(
        "SELECT o.user_id AS id FROM space_members m JOIN space_members o ON o.space_id = m.space_id \
         WHERE m.user_id = ? AND o.user_id <> ? \
         UNION SELECT o.user_id FROM dm_members m JOIN dm_members o ON o.dm_channel_id = m.dm_channel_id \
         WHERE m.user_id = ? AND o.user_id <> ? \
         UNION SELECT friend_id FROM friends WHERE user_id = ? AND friend_id <> ? \
         UNION SELECT user_id FROM friends WHERE friend_id = ? AND user_id <> ?",
        params![user_id, user_id, user_id, user_id, user_id, user_id, user_id, user_id],
    )?
    .iter()
    .map(|row| row.text("id"))
    .collect()
}

/// Ставит в очередь `event` всем, кому виден профиль пользователя.
///
/// # Errors
/// Ошибка базы.
pub fn to_profile_audience(
    tx: &mut dyn Tx,
    outbox: &mut Outbox,
    user_id: &str,
    event: &Value,
) -> DbResult<()> {
    for target in profile_targets(tx, user_id)? {
        outbox.to_user(&target, event);
    }
    Ok(())
}

/// Статус, который аккаунт публикует при подключении.
fn connect_status(tx: &mut dyn Tx, user_id: &str) -> DbResult<String> {
    let row = tx.query_one(
        "SELECT home_instance, federation_home_orphaned, chosen_status FROM users WHERE id = ?",
        params![user_id],
    )?;
    let mut status = "online".to_owned();
    if let Some(row) = row {
        let owns = row
            .opt_text("home_instance")?
            .is_none_or(|home| home.is_empty())
            || row.opt_flag("federation_home_orphaned")? == Some(true);
        if owns
            && let Some(chosen) = row.opt_text("chosen_status")?
            && matches!(chosen.as_str(), "online" | "idle" | "dnd")
        {
            status = chosen;
        }
    }
    tx.execute(
        "UPDATE users SET status = ? WHERE id = ?",
        params![&status, user_id],
    )?;
    Ok(status)
}

struct Session {
    user_id: String,
    username: String,
    federated: bool,
    connection: u64,
}

async fn authenticate(
    state: &AppState,
    token: String,
    sender: mpsc::Sender<Outgoing>,
    closer: Arc<Notify>,
) -> Result<Session, &'static str> {
    let hub = state.hub.clone();
    let state_for_tx = state.clone();
    let connection = hub.next_connection_id();
    let result = state
        .db
        .transaction(move |tx| {
            let user = auth::verify_token(&state_for_tx, tx, &token)?;
            let status = connect_status(tx, &user.id)?;
            let mut payload = ready::build(tx, &hub, &user.id)?;
            payload.insert("type".into(), json!("ready"));
            let mut outbox = Outbox::new();
            let presence = presence_update(tx, &user.id, &status, None)?;
            to_profile_audience(tx, &mut outbox, &user.id, &presence)?;
            let text: Arc<str> = Value::Object(payload).to_string().into();
            hub.add_connection(&user.id, connection, sender, closer, Some(text));
            Ok::<_, ApiError>((user, outbox))
        })
        .await;
    match result {
        Ok((user, outbox)) => {
            state.hub.flush(outbox);
            Ok(Session {
                federated: user
                    .home_instance
                    .as_deref()
                    .is_some_and(|home| !home.is_empty()),
                user_id: user.id,
                username: user.username,
                connection,
            })
        }
        Err(error) => {
            // Сбой при фиксации транзакции мог оставить соединение в списке.
            state.hub.forget_connection(connection);
            Err(match (error.code(), error.custom_text()) {
                ("account_deleted", _) => "This account has been deleted",
                (_, Some(text)) if text.starts_with("Token has been revoked") => {
                    "Token has been revoked"
                }
                _ => "Invalid token",
            })
        }
    }
}

async fn serve(state: AppState, socket: WebSocket, ip: std::net::IpAddr) {
    let (mut sink, mut stream) = socket.split();
    let (sender, mut queue) = mpsc::channel::<Outgoing>(SEND_QUEUE);
    let closer = Arc::new(Notify::new());
    let alive = Arc::new(AtomicBool::new(true));

    let writer_alive = Arc::clone(&alive);
    let writer_closer = Arc::clone(&closer);
    let writer = tokio::spawn(async move {
        let mut heartbeat =
            tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
        loop {
            tokio::select! {
                message = queue.recv() => match message {
                    Some(Outgoing::Text(text)) => {
                        if sink.send(Message::Text(text.as_ref().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Outgoing::Close) | None => break,
                },
                _ = heartbeat.tick() => {
                    if !writer_alive.swap(false, Ordering::Relaxed)
                        || sink.send(Message::Ping(Vec::new().into())).await.is_err()
                    {
                        break;
                    }
                }
            }
        }
        let _ = sink.close().await;
        writer_closer.notify_one();
    });

    let mut session: Option<Session> = None;
    let auth_deadline = tokio::time::sleep(AUTH_TIMEOUT);
    tokio::pin!(auth_deadline);
    loop {
        let frame = tokio::select! {
            frame = stream.next() => frame,
            () = closer.notified() => break,
            () = &mut auth_deadline, if session.is_none() => {
                let _ = sender.try_send(Outgoing::Text(
                    json!({"type": "error", "message": "Authentication timeout"}).to_string().into(),
                ));
                let _ = sender.try_send(Outgoing::Close);
                break;
            }
        };
        let text = match frame {
            Some(Ok(Message::Text(text))) => text.to_string(),
            Some(Ok(Message::Binary(bytes))) => String::from_utf8_lossy(&bytes).into_owned(),
            Some(Ok(Message::Pong(_) | Message::Ping(_))) => {
                alive.store(true, Ordering::Relaxed);
                continue;
            }
            Some(Ok(Message::Close(_)) | Err(_)) | None => break,
        };
        let send = |message: &str| {
            let Message::Text(text) = error_frame(message) else {
                return;
            };
            let _ = sender.try_send(Outgoing::Text(text.as_str().into()));
        };
        let Ok(Value::Object(event)) = serde_json::from_str::<Value>(&text) else {
            send("Invalid JSON");
            continue;
        };
        alive.store(true, Ordering::Relaxed);
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();

        let Some(current) = &session else {
            let Some(token) = event
                .get("token")
                .and_then(Value::as_str)
                .filter(|_| kind == "auth")
            else {
                send("First message must be auth");
                let _ = sender.try_send(Outgoing::Close);
                break;
            };
            match authenticate(
                &state,
                token.to_owned(),
                sender.clone(),
                Arc::clone(&closer),
            )
            .await
            {
                Ok(new) => session = Some(new),
                Err(message) => {
                    send(message);
                    let _ = sender.try_send(Outgoing::Close);
                    break;
                }
            }
            continue;
        };

        if kind == "ping" {
            let _ = sender.try_send(Outgoing::Text(r#"{"type":"pong"}"#.into()));
            continue;
        }
        if !state.hub.consume(&current.user_id) {
            send("Rate limited");
            continue;
        }
        let context = events::Context {
            user_id: current.user_id.clone(),
            username: current.username.clone(),
            federated: current.federated,
            connection: current.connection,
            ip,
        };
        if let Err(error) = events::handle(&state, context, event).await {
            tracing::error!(?error, kind = %kind, user = %current.user_id, "ошибка обработки события");
            send("Internal server error");
        }
    }

    drop(sender);
    if let Some(session) = session {
        disconnected(&state, &session);
    }
    // Задача записи завершится сама: канал закрыт или сокет оборван.
    let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
}

/// Соединение закрыто: если оно было последним, через отсрочку пользователь
/// уходит в офлайн.
fn disconnected(state: &AppState, session: &Session) {
    events::connection_closed(state, &session.user_id, session.connection);
    let Some(generation) = state
        .hub
        .remove_connection(&session.user_id, session.connection)
    else {
        return;
    };
    let state = state.clone();
    let user_id = session.user_id.clone();
    tokio::spawn(async move {
        tokio::time::sleep(OFFLINE_GRACE).await;
        if !state.hub.take_offline(&user_id, generation) {
            return;
        }
        let hub = state.hub.clone();
        let result = state
            .db
            .transaction(move |tx| {
                let mut outbox = Outbox::new();
                let row = tx.query_one(
                    "SELECT home_instance, federation_home_orphaned, status FROM users WHERE id = ?",
                    params![&user_id],
                )?;
                let Some(row) = row else { return Ok::<_, ApiError>(outbox) };
                let owns = row.opt_text("home_instance")?.is_none_or(|home| home.is_empty())
                    || row.opt_flag("federation_home_orphaned")? == Some(true);
                let changed = row.opt_text("status")?.as_deref() != Some("offline");
                tx.execute("UPDATE users SET status = 'offline' WHERE id = ?", params![&user_id])?;
                // Реплика без известной проекции дома тоже возвращается в
                // офлайн, но молчит, если её статус не изменился.
                if owns || changed {
                    let event = presence_update(tx, &user_id, "offline", Some(&[]))?;
                    to_profile_audience(tx, &mut outbox, &user_id, &event)?;
                }
                Ok(outbox)
            })
            .await;
        match result {
            Ok(outbox) => hub.flush(outbox),
            Err(error) => tracing::error!(?error, "не удалось завершить сеанс"),
        }
    });
}
