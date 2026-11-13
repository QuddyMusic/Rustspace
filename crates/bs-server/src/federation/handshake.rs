//! Рукопожатие между инстансами (`utils/federationHandshake.ts`,
//! `utils/federationPeering.ts`).
//!
//! Инстанс, начинающий пиринг, создаёт у себя строку пира в статусе
//! `pending` с новым секретом и шлёт его на `/api/federation/peer/accept`.
//! Ответ решает судьбу строки; принятый секрет перед активацией
//! проверяется подписанным запросом эпохи.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use serde_json::{Value, json};

use super::fetch::{self, Epoch, FetchError, Trust};
use super::peers::{self, Moved, NewPeer, Peer, Transition};
use crate::db::{DbResult, Tx};
use crate::error::ApiResult;
use crate::federation_auth;
use crate::params;
use crate::state::{AppState, now_ms};

/// Текст отказа отозванному пиру.
pub const REMOTE_REVOKED: &str = "Peering with this instance has been revoked";
const CHANGED: &str = "The peer changed while the handshake was in flight";
const THIRTY_DAYS_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Итог обращения «обеспечь пиринг» (`EnsurePeeredResult`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ensured {
    /// Пиринг есть.
    Active(String),
    /// Отказано окончательно.
    Rejected(String),
    /// Не вышло, можно повторить.
    Failed(String),
    /// Ждём одобрения на той стороне.
    Pending(String),
    /// Нужно решение нашего администратора.
    AdminRequired(String),
}

/// Кто просит пиринг.
#[derive(Debug, Clone)]
pub enum Intent {
    /// Сам сервер.
    System,
    /// Пользователь; его запрос попадёт администратору на одобрение.
    User {
        /// Кто просит.
        user_id: String,
        /// Зачем (`PeerEnsureReason`).
        reason: String,
        /// К чему относится просьба.
        target: String,
    },
}

/// Незавершённые рукопожатия и защита от повторов между инстансами.
#[derive(Default)]
pub struct Runtime {
    /// Статус копии пользователя, как его объявил дом (`projections`).
    pub projections: Mutex<HashMap<String, String>>,
    /// Запросы профиля копий у их домов.
    pub pulls: super::lookup::Pulls,
    windows: Mutex<HashMap<String, std::collections::VecDeque<i64>>>,
    admin: Mutex<HashSet<String>>,
    in_flight: Mutex<HashMap<String, Shared<BoxFuture<'static, Ensured>>>>,
    nonces: Mutex<HashMap<String, HashMap<String, i64>>>,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime").finish_non_exhaustive()
    }
}

/// Срок памяти об одноразовых числах.
const NONCE_MAX_AGE_MS: i64 = 15 * 60 * 1000;

impl Runtime {
    /// Скользящее окно частоты (`createLimiter`): `true` — предел исчерпан;
    /// иначе запрос учитывается.
    pub fn limited(&self, key: &str, max: usize, window_ms: i64, now: i64) -> bool {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        if windows.len() > 10_000 {
            windows.retain(|_, stamps| stamps.back().is_some_and(|last| *last >= now - window_ms));
        }
        let stamps = windows.entry(key.to_owned()).or_default();
        while stamps.front().is_some_and(|first| *first < now - window_ms) {
            stamps.pop_front();
        }
        if stamps.len() >= max {
            return true;
        }
        stamps.push_back(now);
        false
    }

    /// Идёт ли рукопожатие с этим инстансом (`isHandshakeInFlight`).
    #[must_use]
    pub fn in_flight(&self, origin: &str) -> bool {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(origin)
            || self.admin_claimed(origin)
    }

    fn admin_claimed(&self, origin: &str) -> bool {
        self.admin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(origin)
    }

    /// Занимает рукопожатие за администратором (`claimAdminHandshake`).
    #[must_use]
    pub fn claim_admin(&self, origin: &str) -> bool {
        if self.in_flight(origin) {
            return false;
        }
        self.admin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(origin.to_owned())
    }

    /// Освобождает рукопожатие администратора.
    pub fn release_admin(&self, origin: &str) {
        self.admin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(origin);
    }

    /// Встречалось ли одноразовое число пира (`isNonceDuplicate`); новое
    /// запоминается.
    #[must_use]
    pub fn nonce_seen(&self, origin: &str, nonce: &str, now: i64) -> bool {
        let mut store = self.nonces.lock().unwrap_or_else(PoisonError::into_inner);
        let known = store.entry(origin.to_owned()).or_default();
        if known.contains_key(nonce) {
            return true;
        }
        known.insert(nonce.to_owned(), now);
        false
    }

    /// Забывает одноразовые числа старше срока подписи.
    pub fn sweep_nonces(&self, now: i64) {
        let mut store = self.nonces.lock().unwrap_or_else(PoisonError::into_inner);
        store.retain(|_, known| {
            known.retain(|_, seen| *seen >= now - NONCE_MAX_AGE_MS);
            !known.is_empty()
        });
    }
}

/// Название инстанса и принимает ли он пиринг без одобрения.
///
/// # Errors
/// Ошибка базы.
pub fn settings(tx: &mut dyn Tx) -> DbResult<(Option<String>, bool)> {
    let Some(row) = tx.query_one(
        "SELECT instance_name, auto_accept_peering FROM instance_settings WHERE id = 1",
        params![],
    )?
    else {
        return Ok((None, true));
    };
    Ok((
        row.opt_text("instance_name")?,
        row.opt_int("auto_accept_peering")?.unwrap_or(1) == 1,
    ))
}

/// Ответ удалённой стороны на рукопожатие (`AcceptAnswer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accept {
    /// Принято.
    Accepted {
        /// Название инстанса.
        instance_name: Option<String>,
    },
    /// Поставлено в очередь на одобрение.
    Queued(Option<String>),
    /// Отказано: причина и текст.
    Refused(&'static str, String),
    /// У той стороны остался прежний пиринг.
    Exists(String),
    /// Та сторона завершает своё рукопожатие с нами.
    InProgress(String),
    /// Отказ, который может пройти сам.
    Transient(String, u16),
    /// Ответа нет.
    Unreachable(String, bool),
}

fn text_of(body: &serde_json::Map<String, Value>, name: &str) -> Option<String> {
    body.get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// Разбор ответа на рукопожатие (`interpretAcceptAnswer`).
#[must_use]
pub fn interpret(answer: &fetch::Answer) -> Accept {
    let body = answer.object();
    let code = text_of(&body, "code");
    let error = text_of(&body, "error");
    let code = code.as_deref();
    match answer.status {
        202 => Accept::Queued(text_of(&body, "approvalToken")),
        200..=299 => Accept::Accepted {
            instance_name: text_of(&body, "instanceName"),
        },
        409 if code == Some("PEER_EXISTS_RESET_REQUIRED") => {
            Accept::Exists(error.unwrap_or_else(|| {
                "The remote instance already holds peering for this instance".to_owned()
            }))
        }
        409 if code == Some("PEER_HANDSHAKE_IN_PROGRESS") => {
            Accept::InProgress(error.unwrap_or_else(|| {
                "The remote instance is completing its own handshake with this instance".to_owned()
            }))
        }
        403 if code == Some("PEERING_REQUIRES_APPROVAL") => Accept::Refused(
            "denied_by_remote",
            error.unwrap_or_else(|| {
                "The remote instance requires manual peering approval".to_owned()
            }),
        ),
        403 if code == Some("PEER_REVOKED") || error.as_deref() == Some(REMOTE_REVOKED) => {
            Accept::Refused(
                "revoked_by_remote",
                error.unwrap_or_else(|| REMOTE_REVOKED.to_owned()),
            )
        }
        status => Accept::Transient(
            error.unwrap_or_else(|| format!("Remote rejected peering (HTTP {status})")),
            status,
        ),
    }
}

async fn request_accept(
    state: &AppState,
    origin: &str,
    secret: &str,
    token: Option<&str>,
    trust: Trust,
) -> Accept {
    let name = state.run(|tx, _| Ok(settings(tx)?.0)).await.ok().flatten();
    let mut body = json!({
        "sourceOrigin": crate::federation::our_origin(&state.config),
        "hmacSecret": secret,
        "instanceId": &*state.instance_id,
    });
    if let Some(fields) = body.as_object_mut() {
        if let Some(name) = name {
            fields.insert("instanceName".to_owned(), name.into());
        }
        if let Some(token) = token.filter(|token| !token.is_empty()) {
            fields.insert("approvalToken".to_owned(), token.into());
        }
    }
    let sent = fetch::request(
        &state.config,
        origin,
        "/api/federation/peer/accept",
        Some(crate::jsjson::stringify(&body)),
        &[("Content-Type", "application/json".to_owned())],
        trust,
        fetch::TIMEOUT,
    )
    .await;
    match sent {
        Ok(answer) => interpret(&answer),
        Err(FetchError::Timeout) => Accept::Unreachable(
            "Remote instance did not respond within 10 seconds".to_owned(),
            true,
        ),
        Err(FetchError::Failed(text)) => {
            Accept::Unreachable(format!("Failed to reach remote instance: {text}"), false)
        }
    }
}

/// Итог рукопожатия (`HandshakeOutcome`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Пиринг действует.
    Active {
        /// Пир.
        peer_id: String,
        /// Подтверждён ли новый секрет подписью.
        verified: bool,
    },
    /// Ждём одобрения на той стороне.
    Awaiting(String),
    /// Отказано.
    Rejected {
        /// Пир.
        peer_id: String,
        /// Причина отказа.
        reason: Option<String>,
        /// Текст.
        error: String,
    },
    /// Нужен разбор администратором.
    NeedsAttention {
        /// Пир.
        peer_id: String,
        /// Причина.
        reason: Option<String>,
    },
    /// Пир тем временем отозван.
    Revoked(String),
    /// Не вышло.
    Failed {
        /// Пир, если строка осталась.
        peer_id: Option<String>,
        /// Текст.
        error: String,
        /// Не дождались ответа.
        timed_out: bool,
        /// Код ответа той стороны.
        http_status: Option<u16>,
    },
}

impl Outcome {
    /// Пир, которого касается итог.
    #[must_use]
    pub fn peer_id(&self) -> Option<&str> {
        match self {
            Self::Active { peer_id, .. }
            | Self::Awaiting(peer_id)
            | Self::Rejected { peer_id, .. }
            | Self::NeedsAttention { peer_id, .. }
            | Self::Revoked(peer_id) => Some(peer_id),
            Self::Failed { peer_id, .. } => peer_id.as_deref(),
        }
    }
}

/// Итог по текущему состоянию строки (`outcomeForRow`).
fn outcome_for(
    row: Option<&Peer>,
    error: &str,
    timed_out: bool,
    http_status: Option<u16>,
) -> Outcome {
    let Some(row) = row else {
        return Outcome::Failed {
            peer_id: None,
            error: error.to_owned(),
            timed_out,
            http_status,
        };
    };
    let peer_id = row.id.clone();
    match row.status {
        "active" | "unreachable" => Outcome::Active {
            peer_id,
            verified: false,
        },
        "awaiting_approval" => Outcome::Awaiting(peer_id),
        "rejected" => Outcome::Rejected {
            peer_id,
            reason: row.reason.clone(),
            error: error.to_owned(),
        },
        "needs_attention" => Outcome::NeedsAttention {
            peer_id,
            reason: row.reason.clone(),
        },
        "revoked" => Outcome::Revoked(peer_id),
        _ => Outcome::Failed {
            peer_id: Some(peer_id),
            error: error.to_owned(),
            timed_out,
            http_status,
        },
    }
}

async fn peer(state: &AppState, id: &str) -> ApiResult<Option<Peer>> {
    let id = id.to_owned();
    state.run(move |tx, _| Ok(peers::by_id(tx, &id)?)).await
}

fn refused_row(moved: Moved) -> Option<Peer> {
    match moved {
        Moved::Refused(row) => row.map(|row| *row),
        Moved::Applied { .. } => None,
    }
}

/// Что делать со строкой, если та сторона не ответила.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnFailure {
    /// Оставить как есть.
    Keep,
    /// Удалить, если пиру нечего отправлять.
    RemoveUnlessQueued,
    /// Вернуть автоматике.
    ReleaseToTraffic,
}

/// Исходящее рукопожатие.
#[derive(Debug, Clone)]
pub struct Outbound {
    /// Строка пира в статусе `pending`.
    pub peer_id: String,
    /// Адрес той стороны.
    pub origin: String,
    /// Секрет, который мы предлагаем.
    pub secret: String,
    /// Токен одобрения, выданный той стороной.
    pub token: Option<String>,
    /// Доверие к адресу.
    pub trust: Trust,
    /// Причина активации при успехе.
    pub activation: &'static str,
    /// Когда начали.
    pub started_at: i64,
    /// Что делать при неудаче.
    pub on_failure: OnFailure,
}

async fn attempt(state: &AppState, opts: &Outbound) -> ApiResult<()> {
    let (id, at) = (opts.peer_id.clone(), opts.started_at);
    state
        .run(move |tx, _| Ok(peers::record_attempt(tx, &id, &["pending"], at)?))
        .await
        .map(|_| ())
}

async fn activate_verified(
    state: &AppState,
    opts: &Outbound,
    epoch: &str,
    instance_name: Option<&str>,
) -> ApiResult<bool> {
    let mut change = Transition::new(&["pending"], "active", opts.activation)
        .expect(&opts.secret)
        .set("hmac_secret", opts.secret.as_str())
        .set("last_seen_at", now_ms())
        .set("peer_instance_id", epoch)
        .set("approval_token", None::<String>);
    if let Some(name) = instance_name {
        change = change.set("instance_name", name);
    }
    Ok(peers::transition(state, &opts.peer_id, change)
        .await?
        .applied())
}

/// Та сторона приняла рукопожатие: проверяем новый секрет и активируем
/// (`settleAccepted`).
async fn settle_accepted(
    state: &AppState,
    opts: &Outbound,
    instance_name: Option<&str>,
) -> ApiResult<Outcome> {
    let probe = fetch::probe_epoch(&state.config, &opts.origin, &opts.secret).await;
    let row = peer(state, &opts.peer_id).await?;
    let Some(current) = row.as_ref() else {
        return Ok(outcome_for(None, CHANGED, false, None));
    };
    if current.status == "pending" && current.secret == opts.secret {
        return match probe {
            Epoch::Verified(epoch) => {
                if activate_verified(state, opts, &epoch, instance_name).await? {
                    Ok(Outcome::Active {
                        peer_id: opts.peer_id.clone(),
                        verified: true,
                    })
                } else {
                    let row = peer(state, &opts.peer_id).await?;
                    Ok(outcome_for(row.as_ref(), CHANGED, false, None))
                }
            }
            Epoch::Unknown => {
                attempt(state, opts).await?;
                let row = peer(state, &opts.peer_id).await?;
                Ok(outcome_for(
                    row.as_ref(),
                    "The remote accepted the handshake, but the signed check of the new secret did not answer; the next attempt completes the peering",
                    false,
                    None,
                ))
            }
            Epoch::NotPeered | Epoch::SecretMismatch => {
                let mut change =
                    Transition::new(&["pending"], "needs_attention", "repeer_unverified")
                        .reason("repeer_incomplete")
                        .expect(&opts.secret)
                        .set("last_seen_at", now_ms());
                if let Some(name) = instance_name {
                    change = change.set("instance_name", name);
                }
                let parked = peers::transition(state, &opts.peer_id, change).await?;
                if parked.applied() {
                    Ok(Outcome::NeedsAttention {
                        peer_id: opts.peer_id.clone(),
                        reason: Some("repeer_incomplete".to_owned()),
                    })
                } else {
                    Ok(outcome_for(
                        refused_row(parked).as_ref(),
                        CHANGED,
                        false,
                        None,
                    ))
                }
            }
        };
    }
    if current.status == "active" && current.secret != opts.secret {
        if let Epoch::Verified(epoch) = probe {
            // Та сторона приняла оба встречных рукопожатия: берём секрет,
            // который остался у неё.
            let change = Transition::new(&["active"], "active", opts.activation)
                .expect(&current.secret)
                .set("hmac_secret", opts.secret.as_str())
                .set("peer_instance_id", epoch);
            peers::transition(state, &opts.peer_id, change).await?;
            tracing::warn!(
                origin = opts.origin,
                "оба встречных рукопожатия приняты; взят секрет той стороны"
            );
        }
        let row = peer(state, &opts.peer_id).await?;
        return Ok(outcome_for(row.as_ref(), CHANGED, false, None));
    }
    Ok(outcome_for(row.as_ref(), CHANGED, false, None))
}

/// Исходящее рукопожатие от запроса до записи итога (`runOutboundHandshake`).
///
/// # Errors
/// Ошибка базы.
pub async fn run_outbound(state: &AppState, opts: &Outbound) -> ApiResult<Outcome> {
    let answer = request_accept(
        state,
        &opts.origin,
        &opts.secret,
        opts.token.as_deref(),
        opts.trust,
    )
    .await;
    let id = &opts.peer_id;
    match answer {
        Accept::Queued(token) => {
            let change = Transition::new(&["pending"], "awaiting_approval", "handshake_queued")
                .expect(&opts.secret)
                .set("approval_token", token);
            let moved = peers::transition(state, id, change).await?;
            Ok(if moved.applied() {
                Outcome::Awaiting(id.clone())
            } else {
                outcome_for(refused_row(moved).as_ref(), CHANGED, false, None)
            })
        }
        Accept::Refused(reason, error) => {
            let change = Transition::new(&["pending"], "rejected", "remote_refused")
                .reason(reason)
                .expect(&opts.secret);
            let moved = peers::transition(state, id, change).await?;
            tracing::warn!(
                origin = opts.origin,
                reason,
                error,
                "в рукопожатии отказано"
            );
            Ok(if moved.applied() {
                Outcome::Rejected {
                    peer_id: id.clone(),
                    reason: Some(reason.to_owned()),
                    error,
                }
            } else {
                outcome_for(refused_row(moved).as_ref(), &error, false, None)
            })
        }
        Accept::Accepted { instance_name } => {
            settle_accepted(state, opts, instance_name.as_deref()).await
        }
        Accept::Exists(error) => {
            let row = peer(state, id).await?;
            let ours = row
                .as_ref()
                .is_some_and(|row| row.status == "pending" && row.secret == opts.secret);
            if !ours {
                return Ok(outcome_for(row.as_ref(), &error, false, None));
            }
            // Возможно, та сторона держит именно этот секрет: тогда пиринг
            // уже состоялся, и подписанная проверка это покажет.
            if let Epoch::Verified(epoch) =
                fetch::probe_epoch(&state.config, &opts.origin, &opts.secret).await
                && activate_verified(state, opts, &epoch, None).await?
            {
                return Ok(Outcome::Active {
                    peer_id: id.clone(),
                    verified: true,
                });
            }
            let change = Transition::new(&["pending"], "rejected", "stale_peering")
                .reason("stale_peering_on_remote")
                .expect(&opts.secret);
            let parked = peers::transition(state, id, change).await?;
            tracing::warn!(
                origin = opts.origin,
                "у той стороны остался прежний пиринг; ждём сброса её администратором"
            );
            Ok(if parked.applied() {
                Outcome::Rejected {
                    peer_id: id.clone(),
                    reason: Some("stale_peering_on_remote".to_owned()),
                    error,
                }
            } else {
                outcome_for(refused_row(parked).as_ref(), &error, false, None)
            })
        }
        Accept::InProgress(error) => {
            attempt(state, opts).await?;
            let row = peer(state, id).await?;
            Ok(outcome_for(row.as_ref(), &error, false, None))
        }
        Accept::Transient(error, status) => failed(state, opts, &error, false, Some(status)).await,
        Accept::Unreachable(error, timed_out) => failed(state, opts, &error, timed_out, None).await,
    }
}

async fn failed(
    state: &AppState,
    opts: &Outbound,
    error: &str,
    timed_out: bool,
    http_status: Option<u16>,
) -> ApiResult<Outcome> {
    attempt(state, opts).await?;
    match opts.on_failure {
        OnFailure::Keep => {}
        OnFailure::RemoveUnlessQueued => {
            let id = opts.peer_id.clone();
            let removed = state
                .run(move |tx, _| Ok(peers::remove(tx, &id, &["pending"], true)?))
                .await?;
            if let Some(effects) = removed {
                peers::run_effects(state, effects).await;
            }
        }
        OnFailure::ReleaseToTraffic => {
            let change = Transition::new(&["pending"], "pending", "handshake_released")
                .set("initiated_by", "auto");
            peers::transition(state, &opts.peer_id, change).await?;
        }
    }
    let row = peer(state, &opts.peer_id).await?;
    Ok(outcome_for(row.as_ref(), error, timed_out, http_status))
}

/// Готовность строки пира к рукопожатию администратора.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prepared {
    /// Строка готова.
    Ready {
        /// Пир.
        peer_id: String,
        /// Секрет строки.
        secret: String,
        /// Что делать при неудаче.
        on_failure: OnFailure,
    },
    /// Пиринг уже действует.
    AlreadyActive(String),
    /// Сейчас нельзя: текст для администратора.
    Busy(&'static str),
}

/// Готовит строку пира к рукопожатию от имени администратора
/// (`prepareAdminHandshakeRow`). `approve` — одобрение запроса, иначе —
/// начало пиринга.
///
/// # Errors
/// Ошибка базы.
pub async fn prepare_admin_row(
    state: &AppState,
    origin: &str,
    approve: bool,
    instance_name: Option<String>,
) -> ApiResult<Prepared> {
    const RETRY: &str = "The peer changed; try again";
    let at = origin.to_owned();
    let existing = state
        .run(move |tx, _| Ok(peers::by_origin(tx, &at)?))
        .await?;
    if let Some(existing) = existing {
        match existing.status {
            "active" | "unreachable" => return Ok(Prepared::AlreadyActive(existing.id)),
            "pending" => {
                if existing.initiated_by == "admin" {
                    return Ok(Prepared::Ready {
                        peer_id: existing.id,
                        secret: existing.secret,
                        on_failure: OnFailure::Keep,
                    });
                }
                if existing.initiated_by != "auto" {
                    return Ok(Prepared::Busy(
                        "A peering handshake with this instance is already in progress",
                    ));
                }
                let change = Transition::new(&["pending"], "pending", "admin_claimed")
                    .expect(&existing.secret)
                    .set("initiated_by", "admin");
                if !peers::transition(state, &existing.id, change)
                    .await?
                    .applied()
                {
                    return Ok(Prepared::Busy(RETRY));
                }
                return Ok(Prepared::Ready {
                    peer_id: existing.id,
                    secret: existing.secret,
                    on_failure: OnFailure::ReleaseToTraffic,
                });
            }
            "awaiting_approval" if !approve => {
                return Ok(Prepared::Busy(
                    "A peering handshake with this instance is awaiting the remote admin's approval",
                ));
            }
            "needs_attention" if approve => {
                return Ok(Prepared::Busy(
                    "This peer needs attention; reset it before approving a new handshake",
                ));
            }
            _ => {}
        }
        let (id, status) = (existing.id.clone(), existing.status);
        let removed = state
            .run(move |tx, _| Ok(peers::remove(tx, &id, &[status], false)?))
            .await?;
        match removed {
            Some(effects) => peers::run_effects(state, effects).await,
            None => return Ok(Prepared::Busy(RETRY)),
        }
    }
    let secret = federation_auth::generate_secret();
    let new = NewPeer {
        origin: origin.to_owned(),
        secret: secret.clone(),
        initiated_by: "admin",
        instance_name,
        status: "pending",
        ..NewPeer::default()
    };
    let app = state.clone();
    let inserted = state
        .run(move |tx, _| Ok(peers::insert(tx, &app.ids, &new)?))
        .await?;
    let Some((row, effects)) = inserted else {
        return Ok(Prepared::Busy(RETRY));
    };
    peers::run_effects(state, effects).await;
    Ok(Prepared::Ready {
        peer_id: row.id,
        secret,
        on_failure: OnFailure::RemoveUnlessQueued,
    })
}

/// Итог для уже существующей строки, если он окончателен (`settledResultFor`).
#[must_use]
pub fn settled_for(peer: &Peer) -> Option<Ensured> {
    let reason = peer.reason.as_deref().and_then(peers::reason_text);
    Some(match peer.status {
        "active" | "unreachable" => Ensured::Active(peer.id.clone()),
        "rejected" => Ensured::Rejected(
            reason
                .unwrap_or("Remote instance requires manual peering approval")
                .to_owned(),
        ),
        "revoked" => Ensured::Rejected("Peer was revoked by admin".to_owned()),
        "needs_attention" => Ensured::Rejected(
            reason
                .unwrap_or("Peer in needs_attention; an admin has to reset it")
                .to_owned(),
        ),
        "awaiting_approval" => {
            Ensured::Pending("Awaiting admin approval on remote instance".to_owned())
        }
        _ => return None,
    })
}

/// Записывает просьбу пользователя о пиринге в очередь администратора
/// (`queueOutboundApproval`).
fn queue_outbound_approval(
    tx: &mut dyn Tx,
    state: &AppState,
    out: &mut crate::hub::Outbox,
    origin: &str,
    user_id: &str,
    reason: &str,
    target: &str,
) -> DbResult<()> {
    let now = now_ms();
    let parent = tx
        .query_one(
            "SELECT id FROM peer_approval_requests WHERE origin = ? AND direction = 'outbound'",
            params![origin],
        )?
        .map(|row| row.text("id"))
        .transpose()?;
    let parent = match parent {
        Some(id) => id,
        None => {
            let id = state.ids.next();
            tx.execute(
                "INSERT INTO peer_approval_requests (id, origin, direction, instance_name, \
                 hmac_secret, requested_at, expires_at, approval_token) \
                 VALUES (?, ?, 'outbound', NULL, NULL, ?, ?, NULL)",
                params![&id, origin, now, now + THIRTY_DAYS_MS],
            )?;
            id
        }
    };
    let existing = tx.query_one(
        "SELECT id FROM peer_approval_subscribers WHERE request_id = ? AND user_id = ? \
         AND trigger_reason = ? AND trigger_target = ?",
        params![&parent, user_id, reason, target],
    )?;
    match existing {
        Some(row) => {
            tx.execute(
                "UPDATE peer_approval_subscribers SET created_at = ? WHERE id = ?",
                params![now, row.text("id")?],
            )?;
        }
        None => {
            tx.execute(
                "INSERT INTO peer_approval_subscribers (id, request_id, user_id, trigger_reason, \
                 trigger_target, created_at) VALUES (?, ?, ?, ?, ?, ?)",
                params![state.ids.next(), &parent, user_id, reason, target, now],
            )?;
        }
    }
    peers::to_admins(
        tx,
        &state.hub,
        out,
        &json!({"type": "federation_approval_request_received", "origin": origin}),
    )?;
    out.to_user(user_id, &json!({"type": "peering_subscription_changed"}));
    Ok(())
}

/// Что решено до начала рукопожатия.
enum Gate {
    Done(Ensured),
    Go(Option<(String, String)>),
}

/// Обеспечивает пиринг с инстансом (`ensurePeered`): готовый итог для уже
/// решённых случаев, иначе — рукопожатие. Одновременные просьбы об одном
/// инстансе ждут одного и того же рукопожатия.
pub async fn ensure_peered(state: &AppState, origin: &str, intent: Intent) -> Ensured {
    let Some(normalized) = fetch::validate_origin(origin) else {
        return Ensured::Failed(format!("Invalid origin: {origin}"));
    };
    if normalized == crate::federation::our_origin(&state.config) {
        return Ensured::Failed("Cannot peer with self".to_owned());
    }
    let app = state.clone();
    let at = normalized.clone();
    let admin_busy = state.federation.admin_claimed(&normalized);
    let gate = state
        .run(move |tx, out| {
            let existing = peers::by_origin(tx, &at)?;
            if let Some(settled) = existing.as_ref().and_then(settled_for) {
                return Ok(Gate::Done(settled));
            }
            if admin_busy {
                return Ok(Gate::Done(Ensured::Failed(
                    "An admin-initiated handshake with this instance is in progress".to_owned(),
                )));
            }
            let inbound = tx.query_one(
                "SELECT id FROM peer_approval_requests WHERE origin = ? AND direction = 'inbound'",
                params![&at],
            )?;
            if inbound.is_some() {
                return Ok(Gate::Done(Ensured::AdminRequired(
                    "Local admin must resolve pending peering approval before initiating"
                        .to_owned(),
                )));
            }
            let ungated = existing
                .as_ref()
                .is_some_and(|peer| peer.status == "pending" && peer.initiated_by != "admin");
            if (existing.is_none() || ungated) && !settings(tx)?.1 {
                if let Some(peer) = existing.as_ref().filter(|_| ungated) {
                    peers::remove(tx, &peer.id, &["pending"], false)?;
                    tracing::warn!(
                        origin = at,
                        "заготовка пира удалена: исходящий пиринг требует одобрения администратора"
                    );
                }
                return Ok(Gate::Done(match &intent {
                    Intent::User {
                        user_id,
                        reason,
                        target,
                    } => {
                        queue_outbound_approval(tx, &app, out, &at, user_id, reason, target)?;
                        Ensured::AdminRequired(
                            "Awaiting your admin's approval to initiate peering".to_owned(),
                        )
                    }
                    Intent::System => Ensured::AdminRequired(
                        "Outbound peering requires admin approval on this instance".to_owned(),
                    ),
                }));
            }
            Ok(Gate::Go(existing.map(|peer| (peer.id, peer.secret))))
        })
        .await;
    let existing = match gate {
        Ok(Gate::Done(result)) => return result,
        Ok(Gate::Go(existing)) => existing,
        Err(_) => return Ensured::Failed("Database error".to_owned()),
    };
    let shared = {
        let mut flights = state
            .federation
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(running) = flights.get(&normalized) {
            running.clone()
        } else {
            let app = state.clone();
            let at = normalized.clone();
            let handshake = async move {
                let result = perform(&app, &at, existing).await;
                app.federation
                    .in_flight
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&at);
                result
            }
            .boxed()
            .shared();
            flights.insert(normalized.clone(), handshake.clone());
            // Рукопожатие доходит до конца, даже если просивший ушёл.
            tokio::spawn(handshake.clone());
            handshake
        }
    };
    shared.await
}

async fn perform(state: &AppState, origin: &str, existing: Option<(String, String)>) -> Ensured {
    let started_at = now_ms();
    let had_row = existing.is_some();
    let (peer_id, secret) = if let Some(found) = existing {
        found
    } else {
        let secret = federation_auth::generate_secret();
        let new = NewPeer {
            origin: origin.to_owned(),
            secret: secret.clone(),
            initiated_by: "auto",
            status: "pending",
            ..NewPeer::default()
        };
        let app = state.clone();
        let inserted = state
            .run(move |tx, _| Ok(peers::insert(tx, &app.ids, &new)?))
            .await;
        match inserted {
            Ok(Some((row, effects))) => {
                peers::run_effects(state, effects).await;
                (row.id, secret)
            }
            _ => {
                return Ensured::Failed(
                    "A peer row for this instance appeared meanwhile; try again".to_owned(),
                );
            }
        }
    };
    let outcome = run_outbound(
        state,
        &Outbound {
            peer_id,
            origin: origin.to_owned(),
            secret,
            token: None,
            trust: Trust::Asserted,
            activation: "ensure_peered",
            started_at,
            on_failure: if had_row {
                OnFailure::Keep
            } else {
                OnFailure::RemoveUnlessQueued
            },
        },
    )
    .await;
    let text = |reason: Option<String>, fallback: String| {
        reason
            .as_deref()
            .and_then(peers::reason_text)
            .map_or(fallback, str::to_owned)
    };
    match outcome {
        Ok(Outcome::Active { peer_id, .. }) => Ensured::Active(peer_id),
        Ok(Outcome::Awaiting(_)) => {
            Ensured::Pending("Awaiting admin approval on remote instance".to_owned())
        }
        Ok(Outcome::Rejected { reason, error, .. }) => Ensured::Rejected(text(reason, error)),
        Ok(Outcome::NeedsAttention { reason, .. }) => Ensured::Rejected(text(
            reason,
            "Peer in needs_attention; an admin has to reset it".to_owned(),
        )),
        Ok(Outcome::Revoked(_)) => Ensured::Rejected("Peer was revoked by admin".to_owned()),
        Ok(Outcome::Failed { error, .. }) => Ensured::Failed(error),
        Err(_) => Ensured::Failed("Database error".to_owned()),
    }
}

/// Итог для инстанса без нового рукопожатия (`settledPeeringResult`).
///
/// # Errors
/// Ошибка базы.
pub fn settled(tx: &mut dyn Tx, origin: &str) -> DbResult<Option<Ensured>> {
    let Some(normalized) = fetch::validate_origin(origin) else {
        return Ok(None);
    };
    Ok(peers::by_origin(tx, &normalized)?
        .as_ref()
        .and_then(settled_for))
}

/// Перепроверяет пиров, отложенных из-за прежнего пиринга на той стороне
/// (`resolveStaleParkedPeers`).
pub async fn resolve_stale_parked(state: &AppState) {
    let parked = state
        .run(|tx, _| Ok(peers::with_status(tx, "rejected")?))
        .await
        .unwrap_or_default();
    for row in parked {
        if row.reason.as_deref() != Some("stale_peering_on_remote")
            || state.federation.in_flight(&row.origin)
        {
            continue;
        }
        match fetch::probe_epoch(&state.config, &row.origin, &row.secret).await {
            Epoch::Verified(epoch) => {
                let change = Transition::new(&["rejected"], "active", "stale_peering_verified")
                    .expect(&row.secret)
                    .set("peer_instance_id", epoch)
                    .set("last_seen_at", now_ms())
                    .set("approval_token", None::<String>);
                let _ = peers::transition(state, &row.id, change).await;
            }
            Epoch::NotPeered => {
                let reopen = Transition::new(&["rejected"], "pending", "retry_after_remote_reset")
                    .expect(&row.secret);
                let reopened = peers::transition(state, &row.id, reopen).await;
                if !reopened.is_ok_and(|moved| moved.applied()) {
                    continue;
                }
                if let Ensured::Failed(_) =
                    Box::pin(ensure_peered(state, &row.origin, Intent::System)).await
                {
                    let park = Transition::new(&["pending"], "rejected", "stale_peering")
                        .reason("stale_peering_on_remote");
                    let _ = peers::transition(state, &row.id, park).await;
                }
            }
            Epoch::SecretMismatch | Epoch::Unknown => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, body: &str) -> fetch::Answer {
        fetch::Answer {
            status,
            headers: axum::http::HeaderMap::new(),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn accept_answers_are_classified() {
        assert_eq!(
            interpret(&answer(
                200,
                r#"{"accepted":true,"instanceName":"B","instanceId":"e"}"#
            )),
            Accept::Accepted {
                instance_name: Some("B".into())
            }
        );
        assert_eq!(
            interpret(&answer(202, r#"{"queued":true,"approvalToken":"t"}"#)),
            Accept::Queued(Some("t".into()))
        );
        assert!(matches!(
            interpret(&answer(409, r#"{"code":"PEER_EXISTS_RESET_REQUIRED"}"#)),
            Accept::Exists(_)
        ));
        assert!(matches!(
            interpret(&answer(
                409,
                r#"{"code":"PEER_HANDSHAKE_IN_PROGRESS","error":"x"}"#
            )),
            Accept::InProgress(_)
        ));
        assert!(matches!(
            interpret(&answer(403, r#"{"code":"PEERING_REQUIRES_APPROVAL"}"#)),
            Accept::Refused("denied_by_remote", _)
        ));
        // Старые версии отзывают без кода, только текстом.
        assert!(matches!(
            interpret(&answer(403, &format!(r#"{{"error":"{REMOTE_REVOKED}"}}"#))),
            Accept::Refused("revoked_by_remote", _)
        ));
        assert_eq!(
            interpret(&answer(500, "not json")),
            Accept::Transient("Remote rejected peering (HTTP 500)".into(), 500)
        );
        assert_eq!(
            interpret(&answer(429, r#"{"error":"slow down"}"#)),
            Accept::Transient("slow down".into(), 429)
        );
    }

    #[test]
    fn admin_claim_excludes_other_handshakes() {
        let runtime = Runtime::default();
        assert!(runtime.claim_admin("https://b.example"));
        assert!(runtime.in_flight("https://b.example"));
        assert!(!runtime.claim_admin("https://b.example"));
        assert!(runtime.claim_admin("https://c.example"));
        runtime.release_admin("https://b.example");
        assert!(!runtime.in_flight("https://b.example"));
    }

    #[test]
    fn nonces_are_remembered_per_peer_and_expire() {
        let runtime = Runtime::default();
        assert!(!runtime.nonce_seen("https://b.example", "n1", 1_000));
        assert!(runtime.nonce_seen("https://b.example", "n1", 2_000));
        assert!(!runtime.nonce_seen("https://c.example", "n1", 2_000));
        runtime.sweep_nonces(1_000 + NONCE_MAX_AGE_MS + 1);
        assert!(!runtime.nonce_seen("https://b.example", "n1", 3_000));
    }
}
