//! Поиск пользователей на других инстансах и запрос профиля копии у её дома
//! (`utils/federationLookup.ts`, `utils/federationStubBackfill.ts`,
//! `utils/federationOriginResolve.ts`, `utils/federationAttach.ts`).

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};
use url::Url;

use super::fetch::{self, Trust};
use super::identity::{self, apply_home_handle, may_carry_legacy_suffix};
use super::inbound::{self, Delivery, profile::HomeProfile};
use super::our_origin;
use super::reset::domain_of;
use crate::config::Config;
use crate::db::{DbResult, Row, Tx};
use crate::federation_auth::{self, canonical_home_instance};
use crate::params;
use crate::state::{AppState, now_ms};

/// Ответ пира на поиск пользователя (`LookupResult`).
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    /// Найден.
    Found {
        /// Идентификатор на его инстансе.
        home_user_id: String,
        /// Имя на его инстансе.
        username: Value,
        /// Профиль.
        profile: Value,
    },
    /// Такого нет.
    NotFound,
    /// Пир не ответил или ответил непонятно.
    Unreachable,
    /// Пир ограничил частоту; сколько секунд ждать.
    RateLimited(f64),
}

/// `Number(text)` в JavaScript для десятичной записи.
fn js_number(text: &str) -> f64 {
    let text = text.trim();
    if text.is_empty() {
        return 0.0;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

fn retry_after(answer: &fetch::Answer) -> f64 {
    let raw = answer
        .headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("60");
    let parsed = js_number(raw);
    if parsed.is_finite() { parsed } else { 60.0 }
}

fn found(json: &serde_json::Map<String, Value>) -> Option<(String, Value, Value)> {
    if json.get("found") != Some(&Value::Bool(true)) {
        return None;
    }
    let user = json.get("user").filter(|user| inbound::truthy(user))?;
    let id = user.get("homeUserId")?.as_str()?.to_owned();
    Some((
        id,
        user.get("username").cloned().unwrap_or(Value::Null),
        user.get("profile").cloned().unwrap_or(Value::Null),
    ))
}

async fn peer_secret(app: &AppState, origin: &str) -> Option<String> {
    let at = origin.to_owned();
    app.db
        .transaction(move |tx: &mut dyn Tx| super::peers::by_origin(tx, &at))
        .await
        .ok()
        .flatten()
        .map(|peer| peer.secret)
}

/// Ищет пользователя по имени на пире (`lookupRemoteUser`).
pub async fn user(app: &AppState, origin: &str, username: &str) -> Lookup {
    let Some(secret) = peer_secret(app, origin).await else {
        tracing::warn!(origin, "поиск пользователя: пира нет");
        return Lookup::Unreachable;
    };
    let Ok(answer) = fetch::signed_post(
        &app.config,
        origin,
        "/api/federation/users/lookup",
        &json!({"username": username}),
        &secret,
        Trust::Approved,
    )
    .await
    else {
        return Lookup::Unreachable;
    };
    match answer.status {
        404 => return Lookup::NotFound,
        429 => return Lookup::RateLimited(retry_after(&answer)),
        _ if !answer.ok() => {
            tracing::warn!(
                origin,
                status = answer.status,
                "поиск пользователя: пир ответил ошибкой"
            );
            return Lookup::Unreachable;
        }
        _ => {}
    }
    match found(&answer.object()) {
        Some((home_user_id, username, profile)) => Lookup::Found {
            home_user_id,
            username,
            profile,
        },
        None => Lookup::Unreachable,
    }
}

/// Профиль с версией (`versionedProfileOf`).
fn versioned(profile: Value) -> Value {
    let Value::Object(mut fields) = profile else {
        return profile;
    };
    let version = fields
        .remove("profileUpdatedAt")
        .filter(|v| v.as_f64().is_some_and(f64::is_finite))
        .unwrap_or(Value::Null);
    let accent = fields.remove("accentColor");
    fields.insert("profileUpdatedAt".into(), version);
    if let Some(accent) = accent.filter(|a| a.is_string() || a.is_null()) {
        fields.insert("accentColor".into(), accent);
    }
    Value::Object(fields)
}

/// Ищет пользователя по идентификатору на его доме (`lookupRemoteUserByHomeId`).
pub async fn by_home_id(app: &AppState, origin: &str, home_user_id: &str) -> Lookup {
    let Some(secret) = peer_secret(app, origin).await else {
        tracing::warn!(origin, "поиск по идентификатору: пира нет");
        return Lookup::Unreachable;
    };
    let Ok(answer) = fetch::signed_post(
        &app.config,
        origin,
        "/api/federation/users/by-home-id",
        &json!({"homeUserId": home_user_id}),
        &secret,
        Trust::Approved,
    )
    .await
    else {
        return Lookup::Unreachable;
    };
    if answer.status == 429 {
        return Lookup::RateLimited(retry_after(&answer));
    }
    if !answer.ok() {
        tracing::warn!(
            origin,
            status = answer.status,
            "поиск по идентификатору: пир ответил ошибкой"
        );
        return Lookup::Unreachable;
    }
    let json = answer.object();
    if json.get("found") == Some(&Value::Bool(false)) {
        return Lookup::NotFound;
    }
    match found(&json) {
        Some((home_user_id, username, profile)) => Lookup::Found {
            home_user_id,
            username,
            profile: versioned(profile),
        },
        None => Lookup::Unreachable,
    }
}

/// Профиль из ответа поиска (`homeProfileFromAnswer`).
#[must_use]
pub fn home_profile(username: &Value, profile: &Value) -> HomeProfile {
    let mut fields = match profile {
        Value::Object(fields) => fields.clone(),
        _ => serde_json::Map::new(),
    };
    fields.insert("username".into(), username.clone());
    HomeProfile::from_event(&Value::Object(fields))
}

/// Проверяет у пира одноразовое доказательство (`verifyAttachProofWithPeer`):
/// идентификатор и имя на его доме.
pub async fn verify_attach_proof(
    config: &Config,
    origin: &str,
    secret: &str,
    token: &str,
) -> Option<(String, String)> {
    let answer = fetch::signed_post(
        config,
        origin,
        "/api/federation/verify-attach-proof",
        &json!({"token": token}),
        secret,
        Trust::Approved,
    )
    .await
    .ok()
    .filter(fetch::Answer::ok)?;
    let header = |name: &str| {
        answer
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    let signature = header("x-federation-signature").unwrap_or_default();
    let signature = signature.strip_prefix("sha256=").unwrap_or(signature);
    let timestamp = js_number(header("x-federation-timestamp").unwrap_or(""));
    if signature.is_empty() || !timestamp.is_finite() {
        return None;
    }
    let text = std::str::from_utf8(&answer.body).ok()?;
    #[allow(clippy::cast_possible_truncation, reason = "время в миллисекундах")]
    let timestamp = timestamp as i64;
    if !federation_auth::verify(
        text,
        signature,
        secret,
        timestamp,
        header("x-federation-nonce"),
        now_ms(),
    ) {
        return None;
    }
    let parsed: Value = serde_json::from_str(text).ok()?;
    if parsed.get("valid") != Some(&Value::Bool(true)) {
        return None;
    }
    Some((
        parsed.get("homeUserId")?.as_str()?.to_owned(),
        parsed.get("username")?.as_str()?.to_owned(),
    ))
}

// ─── Адрес пира по имени узла (`federationOriginResolve.ts`) ────────────────

fn has_port(target: &str) -> bool {
    target
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
}

/// Адрес пира по имени узла (`resolveOriginFromHostname`).
///
/// # Errors
/// Ошибка базы.
pub fn origin_from_hostname(
    tx: &mut dyn Tx,
    config: &Config,
    host: &str,
) -> DbResult<Option<String>> {
    let target = host.trim().to_lowercase();
    if target.is_empty() {
        return Ok(None);
    }
    let mut active = Vec::new();
    for row in tx.query("SELECT origin, status FROM federation_peers", params![])? {
        let origin = row.text("origin")?;
        let Ok(url) = Url::parse(&origin) else {
            continue;
        };
        let hostname = url.host_str().unwrap_or_default().to_lowercase();
        let with_port = match url.port() {
            Some(port) => format!("{hostname}:{port}"),
            None => hostname.clone(),
        };
        if with_port == target {
            return Ok(Some(origin));
        }
        if row.opt_text("status")?.as_deref() == Some("active") && hostname == target {
            active.push(origin);
        }
    }
    if active.len() == 1 && !has_port(&target) {
        return Ok(active.pop());
    }
    let scheme = if our_origin(config).starts_with("https://") {
        "https://"
    } else {
        "http://"
    };
    Ok(fetch::validate_origin(&format!("{scheme}{target}")))
}

/// Адрес действующего пира — дома копии (`activePeerOriginForHome`).
///
/// # Errors
/// Ошибка базы.
pub fn active_peer_for_home(
    tx: &mut dyn Tx,
    config: &Config,
    home: &str,
) -> DbResult<Option<String>> {
    let Some(canonical) = canonical_home_instance(Some(home)) else {
        return Ok(None);
    };
    let Ok(url) = Url::parse(&canonical) else {
        return Ok(None);
    };
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_owned(),
    };
    let Some(origin) = origin_from_hostname(tx, config, &host)? else {
        return Ok(None);
    };
    let active = super::peers::by_origin(tx, &origin)?.is_some_and(|peer| peer.status == "active");
    Ok(active.then_some(origin))
}

// ─── Запрос профиля копии у дома (`federationStubBackfill.ts`) ─────────────

/// Что известно о запросах к домам (`inFlight`, `handleConfirmed`,
/// `answeredUnversioned`, `passInFlight`).
#[derive(Debug, Default)]
pub struct Pulls {
    in_flight: Mutex<HashSet<String>>,
    handle_confirmed: Mutex<HashSet<String>>,
    answered_unversioned: Mutex<HashSet<String>>,
    passes: Mutex<HashSet<String>>,
    queued: Mutex<Vec<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Pulls {
    /// Просит спросить дом о новой копии (`scheduleHomeRecordPull`): запрос
    /// уйдёт после фиксации транзакции.
    pub fn schedule(&self, user_id: &str) {
        lock(&self.queued).push(user_id.to_owned());
    }

    fn claim(&self, key: &str) -> bool {
        lock(&self.in_flight).insert(key.to_owned())
    }

    fn release(&self, key: &str) {
        lock(&self.in_flight).remove(key);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refresh {
    Refreshed,
    RateLimited,
    NoAnswer,
}

fn pull_key(origin: &str, home_user_id: &str) -> String {
    format!("{origin}\n{home_user_id}")
}

fn answered_key(row: &Row) -> DbResult<String> {
    Ok(format!(
        "{}\n{}",
        row.text("id")?,
        row.opt_text("home_user_id")?.unwrap_or_default()
    ))
}

fn live_row(tx: &mut dyn Tx, id: &str, home_user_id: &str) -> DbResult<Option<Row>> {
    let Some(row) = tx.query_one("SELECT * FROM users WHERE id = ?", params![id])? else {
        return Ok(None);
    };
    if row.opt_flag("is_deleted")? == Some(true)
        || row.opt_flag("federation_home_orphaned")? == Some(true)
        || row.opt_text("home_instance")?.is_none_or(|h| h.is_empty())
        || row.opt_text("home_user_id")?.as_deref() != Some(home_user_id)
    {
        return Ok(None);
    }
    Ok(Some(row))
}

async fn refresh(app: &AppState, row: &Row, origin: &str) -> Refresh {
    let (Ok(id), Ok(Some(home_user_id))) = (row.text("id"), row.opt_text("home_user_id")) else {
        return Refresh::NoAnswer;
    };
    let (answer_id, username, profile) = match by_home_id(app, origin, &home_user_id).await {
        Lookup::Found {
            home_user_id,
            username,
            profile,
        } => (home_user_id, username, profile),
        Lookup::RateLimited(_) => return Refresh::RateLimited,
        _ => return Refresh::NoAnswer,
    };
    if answer_id != home_user_id {
        return Refresh::NoAnswer;
    }
    let source: Arc<str> = origin.into();
    let applied = inbound::with_assets(app, source.clone(), source, Delivery::Live, move |cx| {
        let Some(current) = live_row(cx.tx, &id, &home_user_id)? else {
            return Ok(None);
        };
        let (named, moved) = apply_home_handle(cx.tx, &current, username.as_str())?;
        if let Some(moved) = &moved {
            identity::announce_user_updated(cx.tx, cx.out, moved)?;
        }
        if named.text("username")? != current.text("username")? {
            identity::announce_user_updated(cx.tx, cx.out, &named)?;
        }
        let legacy = may_carry_legacy_suffix(&named)?;
        let profile = home_profile(&username, &profile);
        let origin = cx.source.to_owned();
        inbound::profile::apply_home_profile(cx, &named, &profile, &origin)?;
        Ok(Some((
            legacy.then(|| named.text("id")).transpose()?,
            profile
                .updated_at
                .is_none()
                .then(|| answered_key(&named))
                .transpose()?,
        )))
    })
    .await;
    match applied {
        Ok(Some((confirmed, unversioned))) => {
            if let Some(id) = confirmed {
                lock(&app.federation.pulls.handle_confirmed).insert(id);
            }
            if let Some(key) = unversioned {
                lock(&app.federation.pulls.answered_unversioned).insert(key);
            }
            Refresh::Refreshed
        }
        Ok(None) => Refresh::NoAnswer,
        Err(error) => {
            tracing::warn!(%error, "профиль копии от дома не записан");
            Refresh::NoAnswer
        }
    }
}

async fn pull(app: AppState, user_id: String) {
    let id = user_id.clone();
    let config = app.config.clone();
    let found = app
        .db
        .transaction(move |tx: &mut dyn Tx| {
            let Some(row) = tx.query_one("SELECT * FROM users WHERE id = ?", params![&id])? else {
                return Ok(None);
            };
            let (Some(home), Some(_)) = (
                row.opt_text("home_instance")?.filter(|h| !h.is_empty()),
                row.opt_text("home_user_id")?.filter(|h| !h.is_empty()),
            ) else {
                return Ok(None);
            };
            Ok::<_, crate::db::DbError>(
                active_peer_for_home(tx, &config, &home)?.map(|origin| (row, origin)),
            )
        })
        .await;
    let Ok(Some((row, origin))) = found else {
        return;
    };
    let Ok(Some(home_user_id)) = row.opt_text("home_user_id") else {
        return;
    };
    let key = pull_key(&origin, &home_user_id);
    if !app.federation.pulls.claim(&key) {
        return;
    }
    refresh(&app, &row, &origin).await;
    app.federation.pulls.release(&key);
}

/// Отправляет накопленные запросы профиля новых копий.
pub fn start_home_pulls(app: &AppState) {
    let queued = std::mem::take(&mut *lock(&app.federation.pulls.queued));
    for user_id in queued {
        tokio::spawn(pull(app.clone(), user_id));
    }
}

fn needs_home_answer(app: &AppState, row: &Row) -> DbResult<bool> {
    if identity::is_placeholder_named(row)? {
        return Ok(true);
    }
    if row.opt_int("profile_updated_at")?.is_none()
        && !lock(&app.federation.pulls.answered_unversioned).contains(&answered_key(row)?)
    {
        return Ok(true);
    }
    Ok(may_carry_legacy_suffix(row)?
        && !lock(&app.federation.pulls.handle_confirmed).contains(&row.text("id")?))
}

/// Спрашивает дом о копиях с неполной записью после включения пира
/// (`backfillStubUsernamesForPeer`).
pub async fn backfill_for_peer(app: AppState, origin: String) {
    if !lock(&app.federation.pulls.passes).insert(origin.clone()) {
        return;
    }
    run_pass(&app, &origin).await;
    lock(&app.federation.pulls.passes).remove(&origin);
}

async fn run_pass(app: &AppState, origin: &str) {
    let domain = domain_of(origin);
    let at = origin.to_owned();
    let candidates = app
        .db
        .transaction(move |tx: &mut dyn Tx| {
            if super::peers::by_origin(tx, &at)?.is_none_or(|peer| peer.status != "active") {
                return Ok(Vec::new());
            }
            tx.query(
                "SELECT * FROM users WHERE home_instance = ? AND is_deleted = 0 \
                 AND federation_home_orphaned = 0 AND username LIKE ?",
                params![&domain, format!("%@{domain}")],
            )
        })
        .await
        .unwrap_or_default();
    for row in candidates {
        let Ok(Some(home_user_id)) = row.opt_text("home_user_id") else {
            continue;
        };
        if !needs_home_answer(app, &row).unwrap_or(false) {
            continue;
        }
        let key = pull_key(origin, &home_user_id);
        if !app.federation.pulls.claim(&key) {
            continue;
        }
        let outcome = refresh(app, &row, origin).await;
        app.federation.pulls.release(&key);
        if outcome == Refresh::RateLimited {
            tracing::info!(
                origin,
                "пир ограничил частоту: проход по копиям продолжится при следующем включении"
            );
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_ports() {
        assert!((js_number(" 60 ") - 60.0).abs() < f64::EPSILON);
        assert!(js_number("").abs() < f64::EPSILON);
        assert!(js_number("x").is_nan());
        assert!(has_port("y.test:8443"));
        assert!(!has_port("y.test"));
    }

    #[test]
    fn versioned_profiles() {
        let profile = versioned(json!({"bio": "b", "profileUpdatedAt": "x", "accentColor": 5}));
        assert_eq!(profile["profileUpdatedAt"], Value::Null);
        assert!(profile.get("accentColor").is_none());
        let profile = versioned(json!({"profileUpdatedAt": 7, "accentColor": null}));
        assert_eq!(profile["profileUpdatedAt"], 7);
        assert_eq!(profile["accentColor"], Value::Null);
    }
}
