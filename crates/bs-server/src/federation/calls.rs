//! Звонки и «печатает» между инстансами: прямые запросы к пирам в обход
//! очереди (`sendCallRelay`, `utils/callFanout.ts`, `sendFederatedCallStart`,
//! `sendTypingRelay`).
//!
//! Голосовое состояние меняется в транзакции, а запросы к пирам — работа
//! [`CallTask`], которую после фиксации выполняет исполнитель отсюда.

use std::time::Duration;

use serde_json::{Value, json};

use super::fetch::{self, FetchError, Trust};
use super::handshake::{self, Ensured, Intent};
use super::identity::{self, Actor};
use crate::db::{DbResult, Row, Tx};
use crate::federation_auth::{canonical_home_instance, normalize_origin};
use crate::params;
use crate::state::{AppState, now_ms};
use crate::voice::{self, CallTask, FedCall, OnFail};

/// Сколько ждать рукопожатия перед событием звонка (`CALL_PEERING_TIMEOUT_MS`).
const PEERING_TIMEOUT: Duration = Duration::from_secs(3);
/// Сколько ждать ответа пира.
const RELAY_TIMEOUT: Duration = Duration::from_secs(10);
/// Как часто проверять ведущих звонков (`FEDERATED_CALL_SENTINEL_MS`).
const SENTINEL_EVERY: Duration = Duration::from_secs(30);
/// Срок токена медиасервера для участника с другого инстанса.
const CALL_TOKEN_TTL_SECONDS: i64 = 5 * 60;

/// Почему событие звонка не дошло (`CallRelayFailureReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Пир отказал в пиринге.
    PeerRejected,
    /// Пиринг ждёт одобрения.
    AwaitingApproval,
    /// Пирингу нужно решение администратора.
    AdminRequired,
    /// Временная неудача.
    Transient,
    /// Пир отказал в запросе (4xx).
    PostFailed,
    /// Пир не примет ни одного имени, которым можно назвать участника.
    IdentityNotAccepted,
}

impl Failure {
    /// Причина для клиента (`mapCallReasonToEventReason`).
    #[must_use]
    pub const fn event_reason(self) -> &'static str {
        match self {
            Self::PeerRejected => "peer_rejected",
            Self::AwaitingApproval => "peer_awaiting_approval",
            Self::AdminRequired | Self::Transient | Self::PostFailed => "peer_transient_failure",
            Self::IdentityNotAccepted => "identity_not_accepted",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::PeerRejected => "peer_rejected",
            Self::AwaitingApproval => "peer_awaiting_approval",
            Self::AdminRequired => "peer_admin_required",
            Self::Transient => "peer_transient_failure",
            Self::PostFailed => "post_failed",
            Self::IdentityNotAccepted => "identity_not_accepted",
        }
    }
}

/// Итог прямого запроса: идентификаторы событий, которые пир не смог никому
/// доставить, или причина неудачи.
pub type Relayed = Result<Vec<String>, (Failure, String)>;

async fn peer_row(app: &AppState, origin: &str) -> Option<Row> {
    let origin = origin.to_owned();
    app.db
        .transaction(move |tx| {
            tx.query_one(
                "SELECT * FROM federation_peers WHERE origin = ?",
                params![&origin],
            )
        })
        .await
        .ok()
        .flatten()
}

fn usable(row: Option<Row>) -> Option<Row> {
    row.filter(|row| {
        row.text("status")
            .is_ok_and(|status| status == "active" || status == "unreachable")
    })
}

/// Отправляет события пиру сразу (`sendCallRelay`). Без пиринга ждёт
/// рукопожатия не дольше `peering`; `None` — не ждать вовсе, а начать
/// рукопожатие в фоне.
pub async fn send(
    app: &AppState,
    origin: &str,
    events: Vec<Value>,
    peering: Option<Duration>,
) -> Relayed {
    let mut peer = usable(peer_row(app, origin).await);
    if peer.is_none() {
        let Some(limit) = peering else {
            let (app, origin) = (app.clone(), origin.to_owned());
            tokio::spawn(async move {
                handshake::ensure_peered(&app, &origin, Intent::System).await;
            });
            return Err((Failure::Transient, "peer not active".to_owned()));
        };
        let handshake = {
            let (app, origin) = (app.clone(), origin.to_owned());
            tokio::spawn(
                async move { handshake::ensure_peered(&app, &origin, Intent::System).await },
            )
        };
        match tokio::time::timeout(limit, handshake).await {
            Ok(Ok(Ensured::Active(_))) => {
                peer = peer_row(app, origin).await;
                if peer.is_none() {
                    return Err((
                        Failure::Transient,
                        "peer row missing after handshake".to_owned(),
                    ));
                }
            }
            Ok(Ok(Ensured::Rejected(error))) => return Err((Failure::PeerRejected, error)),
            Ok(Ok(Ensured::Pending(error))) => return Err((Failure::AwaitingApproval, error)),
            Ok(Ok(Ensured::AdminRequired(error))) => return Err((Failure::AdminRequired, error)),
            Ok(Ok(Ensured::Failed(error))) => return Err((Failure::Transient, error)),
            Ok(Err(_)) => return Err((Failure::Transient, "handshake task failed".to_owned())),
            Err(_) => {
                return Err((
                    Failure::Transient,
                    format!(
                        "Peering handshake did not complete within {:.1}s",
                        limit.as_secs_f64()
                    ),
                ));
            }
        }
    }
    let Some(peer) = peer else {
        return Err((Failure::Transient, "peer not active".to_owned()));
    };
    let secret = match peer.opt_text("pending_hmac_secret").ok().flatten() {
        Some(pending) => pending,
        None => peer.text("hmac_secret").unwrap_or_default(),
    };
    let ours = super::our_origin(&app.config);
    let body = json!({"version": 1, "sourceInstance": ours, "events": events}).to_string();
    let headers = crate::federation_auth::headers(&body, &secret, &ours, now_ms());
    let answer = fetch::request(
        &app.config,
        origin,
        "/api/federation/relay",
        Some(body),
        &headers,
        Trust::Approved,
        RELAY_TIMEOUT,
    )
    .await;
    match answer {
        Ok(answer) if answer.ok() => {
            let parsed: Value = serde_json::from_slice(&answer.body).unwrap_or(Value::Null);
            Ok(parsed
                .get("undeliverable")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|entry| entry.get("messageId")?.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default())
        }
        Ok(answer) => {
            let text = String::from_utf8_lossy(&answer.body).into_owned();
            let failure = if (400..500).contains(&answer.status) {
                Failure::PostFailed
            } else {
                Failure::Transient
            };
            Err((failure, format!("HTTP {}: {text}", answer.status)))
        }
        Err(FetchError::Timeout) => Err((Failure::Transient, "timeout".to_owned())),
        Err(FetchError::Failed(text)) => Err((Failure::Transient, text)),
    }
}

fn actor_json(actor: &Actor) -> Value {
    json!({"homeUserId": actor.home_user_id, "homeInstance": actor.home_instance})
}

/// Имя для события звонка, которое пир `target` примет от этого инстанса
/// (`callRelayActor`): первый из `candidates`, чей дом — этот инстанс или
/// сам пир.
///
/// # Errors
/// Ошибка базы.
pub fn relay_actor(
    tx: &mut dyn Tx,
    config: &crate::config::Config,
    target: &str,
    candidates: &[String],
) -> DbResult<Option<Actor>> {
    let ours = normalize_origin(Some(&super::our_origin(config)));
    let target = normalize_origin(Some(target));
    let mut seen: Vec<&String> = Vec::new();
    for id in candidates {
        if id.is_empty() || seen.contains(&id) {
            continue;
        }
        seen.push(id);
        let Some(row) = tx.query_one("SELECT * FROM users WHERE id = ?", params![id])? else {
            continue;
        };
        let Some(actor) = identity::actor_of_user(config, &row)? else {
            continue;
        };
        let home = normalize_origin(Some(&actor.home_instance));
        if home.is_some() && (home == ours || home == target) {
            return Ok(Some(actor));
        }
    }
    Ok(None)
}

/// Поле события с именем действующего (`ACTOR_FIELD`).
fn actor_field(kind: &str) -> &'static str {
    match kind {
        "dm_call_accept" => "acceptor",
        "dm_call_reject" => "rejector",
        _ => "endedBy",
    }
}

/// Событие принятия, отказа или конца (`buildCallRelayEvent`).
fn call_event(
    app: &AppState,
    kind: &str,
    federated: &str,
    actor: &Actor,
    per_member: bool,
    answered_by: Option<&Value>,
) -> Value {
    let mut call = serde_json::Map::new();
    call.insert(actor_field(kind).to_owned(), actor_json(actor));
    if per_member {
        call.insert("perMember".to_owned(), json!(true));
    }
    if kind == "dm_call_accept"
        && let Some(answered_by) = answered_by
    {
        call.insert("answeredBy".to_owned(), answered_by.clone());
    }
    json!({
        "eventType": kind,
        "messageId": app.ids.next(),
        "encryptionVersion": 0,
        "timestamp": now_ms(),
        "federatedId": federated,
        "call": call,
    })
}

/// Событие звонка одному пиру от имени первого подходящего из `actors`
/// (`relayCallEvent`).
async fn relay_call_event(
    app: &AppState,
    target: &str,
    kind: &str,
    federated: &str,
    actors: &[String],
    per_member: bool,
    answered_by: Option<&Value>,
) -> Relayed {
    let (target_tx, actors_tx) = (target.to_owned(), actors.to_vec());
    let config = app.config.clone();
    let actor = app
        .db
        .transaction(move |tx| relay_actor(tx, &config, &target_tx, &actors_tx))
        .await
        .ok()
        .flatten();
    let Some(actor) = actor else {
        return Err((
            Failure::IdentityNotAccepted,
            "the peer accepts none of the users this relay could name".to_owned(),
        ));
    };
    let event = call_event(app, kind, federated, &actor, per_member, answered_by);
    send(app, target, vec![event], Some(PEERING_TIMEOUT)).await
}

/// Инстансы участников переписки, кроме нашего (`memberHomeOrigins`).
fn member_homes(
    tx: &mut dyn Tx,
    config: &crate::config::Config,
    channel: &str,
) -> DbResult<Vec<String>> {
    let ours = normalize_origin(Some(&super::our_origin(config)));
    let rows = tx.query(
        "SELECT u.home_instance FROM dm_members m INNER JOIN users u ON m.user_id = u.id \
         WHERE m.dm_channel_id = ?",
        params![channel],
    )?;
    let mut keys = Vec::new();
    let mut targets = Vec::new();
    for row in rows {
        let Some(home) = row
            .opt_text("home_instance")?
            .filter(|home| !home.is_empty())
        else {
            continue;
        };
        let origin = if home.starts_with("http") {
            home
        } else {
            format!("https://{home}")
        };
        let key = normalize_origin(Some(&origin));
        if key.is_none() || key == ours || keys.contains(&key) {
            continue;
        }
        keys.push(key);
        targets.push(origin);
    }
    Ok(targets)
}

/// Неудача для клиента: `{reason, peerOrigin, peerLabel?}`.
fn failure_json(reason: &str, origin: &str, label: Option<&str>) -> Value {
    let mut failure = json!({"reason": reason, "peerOrigin": origin});
    if let Some(label) = label {
        failure["peerLabel"] = json!(label);
    }
    failure
}

async fn peer_labels(app: &AppState) -> Vec<(String, Option<String>)> {
    app.db
        .transaction(|tx| {
            tx.query("SELECT origin, instance_name FROM federation_peers", &[])?
                .iter()
                .map(|row| Ok((row.text("origin")?, row.opt_text("instance_name")?)))
                .collect::<DbResult<Vec<_>>>()
        })
        .await
        .unwrap_or_default()
}

fn label_of(labels: &[(String, Option<String>)], origin: &str) -> Option<String> {
    labels
        .iter()
        .find(|(o, _)| o == origin)
        .and_then(|(_, label)| label.clone())
}

/// Принятие или конец звонка, который ведёт этот инстанс, всем инстансам
/// участников, кроме `exclude` (`fanOutCallEvent`): неудачи по пирам.
async fn fan_out(
    app: &AppState,
    channel: &str,
    kind: &'static str,
    actors: &[String],
    exclude: Option<&str>,
    answered_by: Option<&Value>,
) -> (Option<String>, Vec<Value>) {
    let (channel_tx, actors_tx, exclude_tx) = (
        channel.to_owned(),
        actors.to_vec(),
        exclude.map(str::to_owned),
    );
    let config = app.config.clone();
    let planned = app
        .db
        .transaction(move |tx| {
            let Some(federated) = super::dm_federated_id(tx, &channel_tx)? else {
                return Ok((None, Vec::new()));
            };
            let exclude = exclude_tx.and_then(|origin| normalize_origin(Some(&origin)));
            let mut targets = Vec::new();
            for origin in member_homes(tx, &config, &channel_tx)? {
                if exclude.is_some() && normalize_origin(Some(&origin)) == exclude {
                    continue;
                }
                if relay_actor(tx, &config, &origin, &actors_tx)?.is_some() {
                    targets.push(origin);
                }
            }
            Ok::<_, crate::db::DbError>((Some(federated), targets))
        })
        .await;
    let Ok((Some(federated), targets)) = planned else {
        return (None, Vec::new());
    };
    if targets.is_empty() {
        return (Some(federated), Vec::new());
    }
    let labels = peer_labels(app).await;
    let mut tasks = tokio::task::JoinSet::new();
    for (index, origin) in targets.iter().enumerate() {
        let (app, origin, federated, actors) = (
            app.clone(),
            origin.clone(),
            federated.clone(),
            actors.to_vec(),
        );
        let answered_by = answered_by.cloned();
        tasks.spawn(async move {
            let result = relay_call_event(
                &app,
                &origin,
                kind,
                &federated,
                &actors,
                false,
                answered_by.as_ref(),
            )
            .await;
            (index, result)
        });
    }
    let mut results: Vec<(usize, Relayed)> = tasks.join_all().await;
    results.sort_by_key(|(index, _)| *index);
    let mut failures = Vec::new();
    for (index, result) in results {
        if let Err((failure, error)) = result {
            let origin = &targets[index];
            tracing::error!(
                kind,
                peer = origin,
                reason = failure.name(),
                error,
                "рассылка события звонка не дошла"
            );
            failures.push(failure_json(
                failure.event_reason(),
                origin,
                label_of(&labels, origin).as_deref(),
            ));
        }
    }
    (Some(federated), failures)
}

/// Сообщает участнику о неудачах рассылки (`emitFanoutUndeliverable`).
fn report_undeliverable(
    app: &AppState,
    user: &str,
    channel: Option<&str>,
    federated: &str,
    phase: &str,
    terminal: bool,
    failures: Vec<Value>,
) {
    if failures.is_empty() {
        return;
    }
    app.hub.send_to_user(
        user,
        &json!({
            "type": "dm_call_undeliverable",
            "dmChannelId": channel,
            "federatedCallId": federated,
            "terminal": terminal,
            "phase": phase,
            "failures": failures,
        }),
    );
}

/// Действие участника отсюда — ведущему звонка (`relayToCallHost`).
async fn to_host(
    app: &AppState,
    call: &FedCall,
    user: &str,
    kind: &'static str,
    answered_by: Option<&Value>,
    on_fail: OnFail,
) {
    let result = relay_call_event(
        app,
        &call.host,
        kind,
        &call.federated_id,
        &[user.to_owned()],
        call.group,
        answered_by,
    )
    .await;
    let Err((failure, error)) = result else {
        return;
    };
    tracing::error!(
        kind,
        host = call.host,
        reason = failure.name(),
        error,
        "событие звонка ведущему не дошло"
    );
    let (phase, terminal) = match on_fail {
        OnFail::Log => return,
        OnFail::Report(phase) => (phase, false),
        OnFail::Drop(phase) => {
            voice::forget_federated(&app.hub, &call.federated_id);
            (phase, true)
        }
    };
    let labels = peer_labels(app).await;
    report_undeliverable(
        app,
        user,
        call.dm_channel.as_deref(),
        &call.federated_id,
        phase,
        terminal,
        vec![failure_json(
            failure.event_reason(),
            &call.host,
            label_of(&labels, &call.host).as_deref(),
        )],
    );
}

/// Участник переписки для вызова.
struct Member {
    id: String,
    home_user_id: String,
    home_instance: Option<String>,
    name: String,
}

/// Вызов, начатый здесь, — инстансам участников (`sendFederatedCallStart`).
#[allow(clippy::too_many_lines, reason = "один проход по шагам оригинала")]
async fn start(app: &AppState, channel: &str, caller: &str, caller_name: &str) {
    let (channel_tx, config) = (channel.to_owned(), app.config.clone());
    let hub = app.hub.clone();
    let caller_tx = caller.to_owned();
    let loaded = app
        .db
        .transaction(move |tx| {
            let Some(federated) = super::dm_federated_id(tx, &channel_tx)? else {
                return Ok(None);
            };
            let rows = tx.query(
                "SELECT m.user_id, u.home_user_id, u.home_instance, u.username, u.display_name \
                 FROM dm_members m INNER JOIN users u ON m.user_id = u.id \
                 WHERE m.dm_channel_id = ?",
                params![&channel_tx],
            )?;
            let mut members = Vec::new();
            for row in &rows {
                let id = row.text("user_id")?;
                let name = match row.opt_text("display_name")?.filter(|n| !n.is_empty()) {
                    Some(name) => name,
                    None => row.opt_text("username")?.unwrap_or_default(),
                };
                members.push(Member {
                    home_user_id: row
                        .opt_text("home_user_id")?
                        .filter(|id| !id.is_empty())
                        .unwrap_or_else(|| id.clone()),
                    home_instance: row.opt_text("home_instance")?,
                    id,
                    name,
                });
            }
            let mut callers: Vec<(String, Option<Actor>)> = Vec::new();
            for member in &members {
                if member.id == caller_tx {
                    continue;
                }
                let Some(origin) = canonical_home_instance(member.home_instance.as_deref()) else {
                    continue;
                };
                if callers.iter().any(|(o, _)| *o == origin) {
                    continue;
                }
                let actor = relay_actor(tx, &config, &origin, std::slice::from_ref(&caller_tx))?;
                callers.push((origin, actor));
            }
            Ok::<_, crate::db::DbError>(Some((federated, members, callers)))
        })
        .await;
    let Ok(Some((federated, members, callers))) = loaded else {
        return;
    };
    let ours = super::our_origin(&app.config);
    let ours_key = normalize_origin(Some(&ours));
    let remote: Vec<&Member> = members
        .iter()
        .filter(|m| m.id != caller)
        .filter(|m| {
            let key = normalize_origin(m.home_instance.as_deref());
            key.is_some() && key != ours_key
        })
        .collect();
    if remote.is_empty() {
        return;
    }
    let local_ringee_online = members
        .iter()
        .filter(|m| m.id != caller)
        .filter(|m| {
            normalize_origin(m.home_instance.as_deref()).or_else(|| ours_key.clone()) == ours_key
        })
        .any(|m| hub.is_online(&m.id));
    let report = |terminal: bool, failures: Value| {
        let (app, caller, channel, federated) = (
            app.clone(),
            caller.to_owned(),
            channel.to_owned(),
            federated.clone(),
        );
        async move {
            let hub = app.hub.clone();
            let result = app
                .run(move |tx, out| {
                    Ok(voice::start_undeliverable(
                        tx, &hub, out, &caller, &channel, &federated, terminal, failures,
                    )?)
                })
                .await;
            if let Err(error) = result {
                tracing::error!(?error, "не удалось сообщить о недоставленном вызове");
            }
        }
    };
    if app.config.livekit.api_key.is_none() || app.config.livekit.api_secret.is_none() {
        tracing::warn!("вызов на другие инстансы не начат: медиасервер не настроен");
        report(
            !local_ringee_online,
            json!([{"reason": "livekit_unavailable"}]),
        )
        .await;
        return;
    }
    let livekit_url = app.config.livekit.url.clone().unwrap_or_else(|| {
        format!(
            "wss://{}/livekit",
            app.config.domain.clone().unwrap_or_default()
        )
    });
    let participants: Vec<Value> = members
        .iter()
        .map(|m| {
            json!({
                "homeUserId": m.home_user_id,
                "homeInstance": m.home_instance.clone().filter(|h| !h.is_empty()).unwrap_or_else(|| ours.clone()),
                "displayName": m.name,
            })
        })
        .collect();
    let per_member = voice::hosted_group(&app.hub, channel);
    let mut targeted: Vec<(String, Vec<&Member>)> = Vec::new();
    for member in &remote {
        let Some(origin) = canonical_home_instance(member.home_instance.as_deref()) else {
            continue;
        };
        match targeted.iter_mut().find(|(o, _)| *o == origin) {
            Some((_, bucket)) => bucket.push(member),
            None => targeted.push((origin, vec![member])),
        }
    }
    let labels = peer_labels(app).await;
    let mut tasks = tokio::task::JoinSet::new();
    for (index, (origin, recipients)) in targeted.iter().enumerate() {
        let actor = callers
            .iter()
            .find(|(o, _)| o == origin)
            .and_then(|(_, actor)| actor.clone());
        let Some(actor) = actor else {
            tracing::warn!(
                peer = origin,
                "вызов не отправлен: пир не примет имени звонящего"
            );
            tasks.spawn(async move {
                (
                    index,
                    Err((
                        Failure::IdentityNotAccepted,
                        "caller not accepted by the peer".to_owned(),
                    )),
                    String::new(),
                )
            });
            continue;
        };
        let mut tokens = serde_json::Map::new();
        let mut member_tokens = Vec::new();
        for member in recipients {
            let identity = format!("{}:{}", member.home_user_id, member.name);
            let token = crate::routes::livekit::access_token(
                app,
                &federated,
                &identity,
                crate::routes::livekit::Publish {
                    speak: true,
                    stream: true,
                },
                CALL_TOKEN_TTL_SECONDS,
            )
            .unwrap_or_default();
            tokens.insert(member.home_user_id.clone(), json!(token));
            if let Some(home) = canonical_home_instance(member.home_instance.as_deref()) {
                member_tokens.push(json!({"homeUserId": member.home_user_id, "homeInstance": home, "token": token}));
            }
        }
        let message_id = app.ids.next();
        let mut call = json!({
            "livekitUrl": livekit_url,
            "tokens": tokens,
            "memberTokens": member_tokens,
            "caller": {
                "homeUserId": actor.home_user_id,
                "homeInstance": actor.home_instance,
                "displayName": caller_name,
            },
            "participants": participants,
        });
        if per_member {
            call["perMember"] = json!(true);
        }
        let event = json!({
            "eventType": "dm_call_start",
            "messageId": message_id,
            "encryptionVersion": 0,
            "timestamp": now_ms(),
            "federatedId": federated,
            "call": call,
        });
        let (app, origin) = (app.clone(), origin.clone());
        tasks.spawn(async move {
            let result = send(&app, &origin, vec![event], Some(PEERING_TIMEOUT)).await;
            (index, result, message_id)
        });
    }
    let mut results = tasks.join_all().await;
    results.sort_by_key(|(index, ..)| *index);
    let mut any_success = false;
    let mut failures = Vec::new();
    for (index, result, message_id) in results {
        let (origin, recipients) = &targeted[index];
        let reason = match result {
            Ok(undeliverable) if undeliverable.contains(&message_id) => {
                tracing::warn!(peer = origin, "вызов дошёл, но там некому ответить");
                "no_recipient"
            }
            Ok(_) => {
                any_success = true;
                continue;
            }
            Err((failure, error)) => {
                tracing::error!(
                    peer = origin,
                    reason = failure.name(),
                    error,
                    "вызов на другой инстанс не дошёл"
                );
                failure.event_reason()
            }
        };
        let mut failure = failure_json(reason, origin, label_of(&labels, origin).as_deref());
        failure["affectedUserIds"] =
            json!(recipients.iter().map(|m| m.id.clone()).collect::<Vec<_>>());
        failures.push(failure);
    }
    if failures.is_empty() {
        return;
    }
    report(
        !(any_success || local_ringee_online),
        Value::Array(failures),
    )
    .await;
}

/// «Печатает» — инстансам участников, без ожидания (`sendTypingRelay`).
async fn typing(app: &AppState, channel: &str, stop: bool, user: &str) {
    let (channel_tx, user_tx, config) = (channel.to_owned(), user.to_owned(), app.config.clone());
    let planned = app
        .db
        .transaction(move |tx| {
            if !super::relay_enabled(tx)? {
                return Ok(None);
            }
            let ours = super::our_origin(&config);
            let participants = super::relay::participants(tx, &ours, &channel_tx)?;
            let Some(user) = tx.query_one("SELECT * FROM users WHERE id = ?", params![&user_tx])?
            else {
                return Ok(None);
            };
            let Some(federated) = super::dm_federated_id(tx, &channel_tx)? else {
                return Ok(None);
            };
            let mut origins: Vec<String> = Vec::new();
            for participant in &participants {
                let home = participant["homeInstance"].as_str().unwrap_or_default();
                let normalized = if home.starts_with("http") {
                    home.to_owned()
                } else {
                    format!("https://{home}")
                };
                if normalized != ours && !origins.contains(&normalized) {
                    origins.push(normalized);
                }
            }
            if origins.is_empty() {
                return Ok(None);
            }
            let id = user.text("id")?;
            let home_user = user
                .opt_text("home_user_id")?
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| id.clone());
            let home = user
                .opt_text("home_instance")?
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| super::inbound::raw_domain(&ours));
            let now = now_ms();
            let event = json!({
                "eventType": if stop { "dm_typing_stop" } else { "dm_typing_start" },
                "contextType": "dm",
                "messageId": format!("typing:{id}:{now}"),
                "federatedId": federated,
                "participants": participants,
                "encryptionVersion": 0,
                "timestamp": now,
                "typing": {
                    "homeUserId": home_user,
                    "homeInstance": home,
                    "username": user.opt_text("username")?.unwrap_or_default(),
                },
            });
            Ok::<_, crate::db::DbError>(Some((event, origins)))
        })
        .await;
    let Ok(Some((event, origins))) = planned else {
        return;
    };
    for origin in origins {
        let (app, event) = (app.clone(), event.clone());
        tokio::spawn(async move {
            if let Err((failure, error)) = send(&app, &origin, vec![event], None).await {
                tracing::debug!(
                    peer = origin,
                    reason = failure.name(),
                    error,
                    "«печатает» не дошло"
                );
            }
        });
    }
}

/// Выполняет одну работу со звонком.
pub async fn run(app: &AppState, task: CallTask) {
    match task {
        CallTask::Start {
            dm_channel,
            caller,
            caller_name,
        } => start(app, &dm_channel, &caller, &caller_name).await,
        CallTask::FanOut {
            dm_channel,
            kind,
            actors,
            exclude,
            answered_by,
            report,
        } => {
            let (federated, failures) = fan_out(
                app,
                &dm_channel,
                kind,
                &actors,
                exclude.as_deref(),
                answered_by.as_ref(),
            )
            .await;
            if let (Some((user, phase)), Some(federated)) = (report, federated) {
                report_undeliverable(
                    app,
                    &user,
                    Some(&dm_channel),
                    &federated,
                    phase,
                    false,
                    failures,
                );
            }
        }
        CallTask::ToHost {
            call,
            user,
            kind,
            answered_by,
            on_fail,
        } => to_host(app, &call, &user, kind, answered_by.as_ref(), on_fail).await,
        CallTask::Typing {
            dm_channel,
            stop,
            user,
        } => typing(app, &dm_channel, stop, &user).await,
    }
}

/// Ведущие звонков, которые больше не действующие пиры, теряют свои звонки
/// здесь (`runFederatedCallSentinelTick`).
async fn sentinel(app: &AppState) {
    for host in voice::federated_hosts(&app.hub) {
        let row = peer_row(app, &host).await;
        let status = row.as_ref().and_then(|row| row.text("status").ok());
        if status.as_deref() == Some("active") {
            continue;
        }
        let reason = if matches!(status.as_deref(), Some("rejected" | "revoked")) {
            "peer_rejected"
        } else {
            "peer_transient_failure"
        };
        let label = row.and_then(|row| row.opt_text("instance_name").ok().flatten());
        let hub = app.hub.clone();
        let mut out = crate::hub::Outbox::new();
        voice::evict_for_host(&hub, &mut out, &host, reason, label.as_deref());
        hub.flush(out);
    }
}

/// Запускает исполнителя звонков и проверку их ведущих.
pub fn spawn(app: &AppState) {
    let worker = app.clone();
    tokio::spawn(async move {
        loop {
            for task in worker.hub.next_call_tasks().await {
                let app = worker.clone();
                tokio::spawn(async move { run(&app, task).await });
            }
        }
    });
    let checker = app.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SENTINEL_EVERY);
        tick.tick().await;
        loop {
            tick.tick().await;
            sentinel(&checker).await;
        }
    });
}

/// Пир перестал быть действующим (`onPeerDeactivated`): звонки, которые он
/// ведёт, забываются, а его участники выходят из звонков отсюда.
///
/// # Errors
/// Ошибка базы.
pub fn peer_deactivated(
    tx: &mut dyn Tx,
    app: &AppState,
    out: &mut crate::hub::Outbox,
    origin: &str,
    status: Option<&str>,
    label: Option<&str>,
) -> DbResult<()> {
    let reason = if matches!(status, Some("rejected" | "revoked")) {
        "peer_rejected"
    } else {
        "peer_transient_failure"
    };
    let evicted = voice::evict_for_host(&app.hub, out, origin, reason, label);
    if evicted > 0 {
        tracing::info!(peer = origin, evicted, "звонки ведущего забыты");
    }
    let dropped = voice::drop_remote_participants(tx, &app.hub, out, origin)?;
    if dropped > 0 {
        tracing::info!(
            peer = origin,
            dropped,
            "участники с инстанса пира вышли из звонков"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_reasons_for_clients() {
        assert_eq!(Failure::PostFailed.event_reason(), "peer_transient_failure");
        assert_eq!(
            Failure::AdminRequired.event_reason(),
            "peer_transient_failure"
        );
        assert_eq!(
            Failure::AwaitingApproval.event_reason(),
            "peer_awaiting_approval"
        );
        assert_eq!(
            Failure::IdentityNotAccepted.event_reason(),
            "identity_not_accepted"
        );
        assert_eq!(Failure::PeerRejected.event_reason(), "peer_rejected");
    }

    #[test]
    fn actor_fields() {
        assert_eq!(actor_field("dm_call_accept"), "acceptor");
        assert_eq!(actor_field("dm_call_reject"), "rejector");
        assert_eq!(actor_field("dm_call_end"), "endedBy");
        let failure = failure_json("peer_rejected", "https://y.test", None);
        assert!(failure.get("peerLabel").is_none());
        assert_eq!(failure_json("x", "o", Some("Y"))["peerLabel"], json!("Y"));
    }
}
