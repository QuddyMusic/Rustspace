//! Друзья, заявки и поиск людей (`routes/social.ts`).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Row, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ClientIp, JsonBody};
use crate::hub::{Hub, Outbox};
use crate::ratelimit::Limit;
use crate::routes::users::{USER_COLUMNS, sanitize_user, user_row};
use crate::state::{AppState, now_ms};
use crate::util::{field, js_trim, str_field};
use crate::{dm, federation, params, ready, ws};

const SEARCH_LIMIT: Limit = Limit {
    name: "social.search",
    max: 30,
    window: Duration::from_mins(1),
};
const DISCOVER_LIMIT: Limit = Limit {
    name: "social.discover",
    max: 30,
    window: Duration::from_mins(1),
};
const MAX_HOME_USER_ID_LENGTH: usize = 128;

/// Адрес инстанса для сравнения: без схемы и хвостовых `/`, строчными.
fn host_of(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &trimmed[8..]
    } else if lower.starts_with("http://") {
        &trimmed[7..]
    } else {
        trimmed
    };
    let rest = rest.trim_end_matches('/');
    (!rest.is_empty()).then(|| rest.to_lowercase())
}

fn friend_json(user: &Row, added_at: i64) -> DbResult<Value> {
    let mut friend = sanitize_user(user, false)?;
    friend["addedAt"] = json!(added_at);
    Ok(friend)
}

/// `GET /api/social/friends`.
pub async fn friends(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let rows = tx.query(
                &format!(
                    "SELECT f.created_at AS f_created_at, {} FROM friends f JOIN users u \
                     ON u.id = CASE WHEN f.user_id = ? THEN f.friend_id ELSE f.user_id END \
                     WHERE f.user_id = ? OR f.friend_id = ?",
                    crate::routes::users::user_columns("u")
                ),
                params![&user.id, &user.id, &user.id],
            )?;
            let list = rows
                .iter()
                .map(|row| friend_json(row, row.int("f_created_at")?))
                .collect::<DbResult<Vec<_>>>()?;
            Ok(Json(json!(list)))
        })
        .await
}

/// `GET /api/social/requests`.
pub async fn requests(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            let rows = tx.query(
                "SELECT id, from_id, to_id, status, created_at FROM friend_requests \
                 WHERE (from_id = ? OR to_id = ?) AND status = 'pending'",
                params![&user.id, &user.id],
            )?;
            let mut list = Vec::with_capacity(rows.len());
            for row in rows {
                let from = row.text("from_id")?;
                let to = row.text("to_id")?;
                let other = if from == user.id { &to } else { &from };
                let mut request = json!({
                    "id": row.text("id")?,
                    "fromId": from,
                    "toId": to,
                    "status": row.opt_text("status")?.unwrap_or_else(|| "pending".to_owned()),
                    "createdAt": row.int("created_at")?,
                });
                if let Some(other) = user_row(tx, other)? {
                    request["user"] = sanitize_user(&other, false)?;
                }
                list.push(request);
            }
            Ok(Json(json!(list)))
        })
        .await
}

/// Куда идёт заявка в дружбу.
enum Addressee {
    /// Пользователь этого инстанса по идентификатору.
    LocalId(String),
    /// Пользователь этого инстанса по имени.
    LocalName(String),
    /// Пользователь другого инстанса.
    Remote {
        domain: String,
        handle: String,
        by_id: Option<String>,
    },
}

/// `POST /api/social/requests`.
pub async fn send_request(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    let username = str_field(&body, "username")
        .map(js_trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    let by_identity = field(&body, "homeUserId").is_some_and(|v| !v.is_null())
        || field(&body, "homeInstance").is_some_and(|v| !v.is_null());
    let identity = if by_identity {
        let home_user = str_field(&body, "homeUserId")
            .map(js_trim)
            .unwrap_or_default();
        let home_instance = str_field(&body, "homeInstance").and_then(host_of);
        let Some(home_instance) = home_instance.filter(|_| {
            !home_user.is_empty() && home_user.encode_utf16().count() <= MAX_HOME_USER_ID_LENGTH
        }) else {
            return Err(ApiError::bad_request("validation_failed"));
        };
        Some((home_user.to_owned(), home_instance))
    } else {
        if username.is_none() {
            return Err(ApiError::bad_request("username_required"));
        }
        None
    };
    let owner = user.id.clone();
    let sender = state
        .run(move |tx, _| Ok(user_row(tx, &owner)?))
        .await?
        .ok_or_else(|| ApiError::unauthorized("unauthorized"))?;
    let ours = host_of(&federation::our_origin(&state.config));
    let sender_home = sender
        .opt_text("home_instance")?
        .as_deref()
        .and_then(host_of);
    if sender_home.is_some() && sender_home != ours {
        return Err(ApiError::forbidden("not_authoritative_for_sender"));
    }
    let addressee = match identity {
        Some((home_user, home_instance)) => {
            if Some(&home_instance) == ours.as_ref()
                || federation::identity::is_own_domain(&state.config, &home_instance)
            {
                Addressee::LocalId(home_user)
            } else {
                let typed = username.as_deref().unwrap_or_default().to_lowercase();
                let handle = match typed.rfind('@') {
                    Some(at)
                        if at > 0 && host_of(&typed[at + 1..]).as_ref() == Some(&home_instance) =>
                    {
                        typed[..at].to_owned()
                    }
                    _ => home_user.clone(),
                };
                Addressee::Remote {
                    domain: home_instance,
                    handle,
                    by_id: Some(home_user),
                }
            }
        }
        None => {
            let raw = username.unwrap_or_default();
            match raw.rfind('@') {
                Some(at) if at > 0 && at < raw.len() - 1 && host_of(&raw[at + 1..]) != ours => {
                    Addressee::Remote {
                        domain: raw[at + 1..].to_lowercase(),
                        handle: raw[..at].to_lowercase(),
                        by_id: None,
                    }
                }
                Some(at) if at > 0 => Addressee::LocalName(raw[..at].to_owned()),
                _ => Addressee::LocalName(raw),
            }
        }
    };
    if let Addressee::Remote {
        domain,
        handle,
        by_id,
    } = addressee
    {
        return remote_request(&state, sender, domain, handle, by_id).await;
    }
    let app = state.clone();
    state
        .run(move |tx, out| {
            let target = match &addressee {
                Addressee::LocalId(home_user) => tx.query_one(
                    &format!(
                        "SELECT {USER_COLUMNS} FROM users WHERE is_deleted = 0 AND home_instance IS NULL \
                         AND (id = ? OR home_user_id = ?)"
                    ),
                    params![home_user, home_user],
                )?,
                Addressee::LocalName(name) => tx.query_one(
                    &format!("SELECT {USER_COLUMNS} FROM users WHERE username = ?"),
                    params![name.to_lowercase()],
                )?,
                Addressee::Remote { .. } => None,
            };
            let target = target.ok_or_else(|| ApiError::not_found("user_not_found"))?;
            let target_id = target.text("id")?;
            if target_id == user.id {
                return Err(ApiError::bad_request("cannot_friend_self"));
            }
            if dm::are_friends(tx, &user.id, &target_id)? {
                return Err(ApiError::bad_request("already_friends"));
            }
            let pending = tx.query_one(
                "SELECT 1 AS one FROM friend_requests WHERE status = 'pending' AND \
                 ((from_id = ? AND to_id = ?) OR (from_id = ? AND to_id = ?))",
                params![&user.id, &target_id, &target_id, &user.id],
            )?;
            if pending.is_some() {
                return Err(ApiError::bad_request("friend_request_pending"));
            }
            let id = app.ids.next();
            let now = now_ms();
            tx.execute(
                "INSERT INTO friend_requests (id, from_id, to_id, status, created_at) \
                 VALUES (?, ?, ?, 'pending', ?)",
                params![&id, &user.id, &target_id, now],
            )?;
            let base = json!({
                "id": id, "fromId": user.id, "toId": target_id, "status": "pending", "createdAt": now,
            });
            let mut received = base.clone();
            received["user"] = sanitize_user(&sender, false)?;
            let mut sent = base;
            sent["user"] = sanitize_user(&target, false)?;
            out.to_user(&target_id, &json!({"type": "friend_request_received", "request": received}));
            out.to_user(&user.id, &json!({"type": "friend_request_sent", "request": sent}));
            relay_friend_event(tx, &app, federation::relay::Friendship::Request, &user.id, &target_id)?;
            Ok((StatusCode::CREATED, Json(json!({"success": true, "requestId": id}))).into_response())
        })
        .await
}

/// Итог заявки пользователю другого инстанса.
enum RemoteOutcome {
    Sent(String),
    AlreadySent(String),
    Incoming(String),
    AlreadyFriends,
    NotFound,
}

/// Заявка пользователю другого инстанса (`handleFederatedFriendRequest`):
/// пиринг, поиск на том инстансе, копия, событие.
#[allow(clippy::too_many_lines, reason = "порядок шагов оригинала")]
async fn remote_request(
    state: &AppState,
    sender: Row,
    domain: String,
    handle: String,
    by_id: Option<String>,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    use federation::handshake::{Ensured, Intent};
    use federation::lookup::{self, Lookup};
    let (config, at) = (state.config.clone(), domain.clone());
    let resolved = state
        .run(move |tx, _| {
            let Some(origin) = lookup::origin_from_hostname(tx, &config, &at)? else {
                return Ok(None);
            };
            let reset = tx.query_one(
                "SELECT origin FROM federation_reset_events WHERE origin = ? AND resolved_at IS NULL",
                params![&origin],
            )?;
            Ok(Some((origin, reset.is_some())))
        })
        .await?;
    let Some((peer_origin, reset)) = resolved else {
        return Err(ApiError::bad_request("invalid_target_domain").detail("domain", domain));
    };
    if reset {
        return Err(ApiError::conflict("peer_reset_pending"));
    }
    let sender_id = sender.text("id")?;
    let ensured = federation::handshake::ensure_peered(
        state,
        &peer_origin,
        Intent::User {
            user_id: sender_id.clone(),
            reason: "friend_add".to_owned(),
            target: format!("{handle}@{domain}"),
        },
    )
    .await;
    match ensured {
        Ensured::Active(_) => {}
        Ensured::Rejected(_) => {
            return Err(ApiError::forbidden("peer_rejected").detail("domain", domain));
        }
        Ensured::Failed(_) => {
            return Err(
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "peer_unreachable")
                    .detail("domain", domain),
            );
        }
        Ensured::Pending(_) => {
            let at = peer_origin.clone();
            let awaiting = state
                .run(move |tx, _| Ok(federation::peers::by_origin(tx, &at)?))
                .await?
                .is_some_and(|peer| peer.status == "awaiting_approval");
            let code = if awaiting {
                "peer_pending_approval"
            } else {
                "peer_pending"
            };
            return Err(ApiError::conflict(code).detail("domain", domain));
        }
        Ensured::AdminRequired(_) => {
            return Err(ApiError::conflict("peer_pending_local_admin").detail("domain", domain));
        }
    }
    let found = match &by_id {
        Some(id) => lookup::by_home_id(state, &peer_origin, id).await,
        None => lookup::user(state, &peer_origin, &handle).await,
    };
    let (home_user_id, username, profile) = match found {
        Lookup::Found {
            home_user_id,
            username,
            profile,
        } => (home_user_id, username, profile),
        Lookup::NotFound => {
            return Err(ApiError::not_found("user_not_found")
                .detail("domain", domain)
                .detail("handle", handle));
        }
        Lookup::Unreachable => {
            return Err(
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "peer_unreachable")
                    .detail("domain", domain),
            );
        }
        Lookup::RateLimited(seconds) => {
            let mut response =
                ApiError::new(StatusCode::TOO_MANY_REQUESTS, "lookup_rate_limited").into_response();
            if seconds != 0.0
                && !seconds.is_nan()
                && let Ok(value) =
                    axum::http::HeaderValue::from_str(&crate::jsjson::number(seconds))
            {
                response.headers_mut().insert("retry-after", value);
            }
            return Ok(response);
        }
    };
    let canonical_sender = sender
        .opt_text("home_user_id")?
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| sender_id.clone());
    if home_user_id == canonical_sender
        && federation_auth_normalize(&peer_origin)
            == federation_auth_normalize(&federation::our_origin(&state.config))
    {
        return Err(ApiError::bad_request("cannot_friend_self"));
    }
    let source: std::sync::Arc<str> = peer_origin.as_str().into();
    let (target_domain, origin) = (domain.clone(), peer_origin.clone());
    let outcome = federation::inbound::with_assets(
        state,
        source.clone(),
        source,
        federation::inbound::Delivery::Live,
        move |cx| {
            let actor = federation::identity::Actor {
                home_user_id: home_user_id.clone(),
                home_instance: target_domain.clone(),
            };
            let hints = federation::identity::Hints {
                username: username.as_str().map(str::to_owned),
                status: profile.get("status").and_then(Value::as_str).map(str::to_owned),
                deleted: false,
                home_asked: false,
            };
            let Some(replica) = cx.replica(&actor, &hints)? else {
                return Ok(RemoteOutcome::NotFound);
            };
            let stub = federation::inbound::profile::hydrate(cx, replica.user, &profile)?;
            let (sender_id, stub_id) = (sender.text("id")?, stub.text("id")?);
            let pending = cx.tx.query_one(
                "SELECT id, from_id FROM friend_requests WHERE status = 'pending' AND \
                 ((from_id = ? AND to_id = ?) OR (from_id = ? AND to_id = ?))",
                params![&sender_id, &stub_id, &stub_id, &sender_id],
            )?;
            if let Some(pending) = pending {
                let id = pending.text("id")?;
                return Ok(if pending.text("from_id")? == sender_id {
                    RemoteOutcome::AlreadySent(id)
                } else {
                    RemoteOutcome::Incoming(id)
                });
            }
            if dm::are_friends(cx.tx, &sender_id, &stub_id)? {
                return Ok(RemoteOutcome::AlreadyFriends);
            }
            let now = now_ms();
            let request_id = cx.app.ids.next();
            let relay = federation::relay::Relay {
                config: &cx.app.config,
                ids: &cx.app.ids,
            };
            let entity = federation::relay::remote_friend_request(
                cx.tx, relay, &sender, &stub, &origin, &profile, now,
            )?;
            cx.tx.execute(
                "INSERT INTO friend_requests (id, from_id, to_id, status, created_at, relay_message_id) \
                 VALUES (?, ?, ?, 'pending', ?, ?)",
                params![&request_id, &sender_id, &stub_id, now, &entity],
            )?;
            let request = json!({
                "id": request_id,
                "fromId": sender_id,
                "toId": stub_id,
                "status": "pending",
                "createdAt": now,
                "user": sanitize_user(&stub, false)?,
            });
            cx.to_user(&sender_id, &json!({"type": "friend_request_sent", "request": request}))?;
            Ok(RemoteOutcome::Sent(request_id))
        },
    )
    .await?;
    Ok(match outcome {
        RemoteOutcome::Sent(id) => (
            StatusCode::CREATED,
            Json(json!({"success": true, "requestId": id})),
        )
            .into_response(),
        RemoteOutcome::AlreadySent(id) => (
            StatusCode::OK,
            Json(json!({"success": true, "requestId": id})),
        )
            .into_response(),
        RemoteOutcome::Incoming(id) => {
            return Err(ApiError::conflict("incoming_request_exists").detail("requestId", id));
        }
        RemoteOutcome::AlreadyFriends => return Err(ApiError::conflict("already_friends")),
        RemoteOutcome::NotFound => {
            return Err(ApiError::not_found("user_not_found")
                .detail("domain", domain)
                .detail("handle", handle));
        }
    })
}

fn federation_auth_normalize(origin: &str) -> Option<String> {
    crate::federation_auth::normalize_origin(Some(origin))
}

/// Событие дружбы для инстансов участников, если кто-то из них не здешний.
fn relay_friend_event(
    tx: &mut dyn Tx,
    app: &AppState,
    kind: federation::relay::Friendship<'_>,
    from: &str,
    to: &str,
) -> DbResult<()> {
    federation::relay::friendship(
        tx,
        federation::relay::Relay {
            config: &app.config,
            ids: &app.ids,
        },
        kind,
        from,
        to,
    )
    .map(|_| ())
}

/// Отправляет получателю текущее присутствие субъекта (`sendPresenceSnapshot`).
/// Дружба только что возникла: каждый получает присутствие другого, а дом
/// друга-копии — присутствие своего (`exchangeFriendPresence`).
///
/// # Errors
/// Ошибка базы.
pub fn exchange_friend_presence(
    tx: &mut dyn Tx,
    app: &AppState,
    out: &mut Outbox,
    a: &str,
    b: &str,
) -> DbResult<()> {
    presence_snapshot(tx, &app.hub, out, a, b)?;
    presence_snapshot(tx, &app.hub, out, b, a)?;
    relay_friend_presence(tx, app, a, b)
}

/// Присутствие новых друзей их домам (`snapshotPresenceForFriend`). Оригинал
/// ставит эти события в очередь после событий самой дружбы.
///
/// # Errors
/// Ошибка базы.
pub fn relay_friend_presence(tx: &mut dyn Tx, app: &AppState, a: &str, b: &str) -> DbResult<()> {
    let relay = federation::relay::Relay {
        config: &app.config,
        ids: &app.ids,
    };
    let first = app.hub.activities(a);
    let second = app.hub.activities(b);
    federation::relay::presence_for_friend(tx, relay, a, b, &first)?;
    federation::relay::presence_for_friend(tx, relay, b, a, &second)
}

fn presence_snapshot(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    recipient: &str,
    subject: &str,
) -> DbResult<()> {
    let Some(row) = tx.query_one(
        "SELECT status, custom_status FROM users WHERE id = ?",
        params![subject],
    )?
    else {
        return Ok(());
    };
    let status = row
        .opt_text("status")?
        .unwrap_or_else(|| "offline".to_owned());
    let activities = if status == "offline" {
        Vec::new()
    } else {
        ready::snapshot_activities(
            hub.activities(subject),
            row.opt_text("custom_status")?.as_deref(),
        )
    };
    let event = ws::presence_update(tx, subject, &status, Some(&activities))?;
    out.to_user(recipient, &event);
    Ok(())
}

fn request_row(tx: &mut dyn Tx, id: &str) -> ApiResult<Row> {
    tx.query_one(
        "SELECT id, from_id, to_id, status, created_at FROM friend_requests WHERE id = ?",
        params![id],
    )?
    .ok_or_else(|| ApiError::not_found("friend_request_not_found"))
}

/// `PATCH /api/social/requests/:id` — принять или отклонить.
pub async fn answer_request(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let status = str_field(&body, "status")
        .filter(|status| matches!(*status, "accepted" | "declined"))
        .ok_or_else(|| ApiError::bad_request("validation_failed"))?
        .to_owned();
    let app = state.clone();
    state
        .run(move |tx, out| {
            let request = request_row(tx, &id)?;
            let from = request.text("from_id")?;
            if request.text("to_id")? != user.id {
                return Err(ApiError::forbidden("friend_request_not_recipient"));
            }
            // Оригинал статус не проверяет: получатель давно принятой заявки
            // может «принять» её снова и вернуть себе дружбу, которую другая
            // сторона разорвала. Ответить можно только на ожидающую заявку.
            if request.opt_text("status")?.as_deref().unwrap_or("pending") != "pending" {
                return Err(ApiError::bad_request("friend_request_not_pending"));
            }
            tx.execute(
                "UPDATE friend_requests SET status = ? WHERE id = ?",
                params![&status, &id],
            )?;
            if status == "accepted" {
                let now = now_ms();
                tx.execute(
                    "INSERT INTO friends (user_id, friend_id, created_at) VALUES (?, ?, ?) \
                     ON CONFLICT DO NOTHING",
                    params![&from, &user.id, now],
                )?;
                if let Some(me) = user_row(tx, &user.id)? {
                    out.to_user(
                        &from,
                        &json!({
                            "type": "friend_request_accepted",
                            "friend": friend_json(&me, now)?,
                            "requestId": id,
                        }),
                    );
                }
                presence_snapshot(tx, &app.hub, out, &from, &user.id)?;
                presence_snapshot(tx, &app.hub, out, &user.id, &from)?;
                let created = request.int("created_at")?;
                relay_friend_event(
                    tx,
                    &app,
                    federation::relay::Friendship::Answer("accepted", created),
                    &from,
                    &user.id,
                )?;
                relay_friend_presence(tx, &app, &from, &user.id)?;
            } else {
                out.to_user(
                    &from,
                    &json!({"type": "friend_request_declined", "requestId": id, "userId": user.id}),
                );
                let created = request.int("created_at")?;
                relay_friend_event(
                    tx,
                    &app,
                    federation::relay::Friendship::Answer("declined", created),
                    &from,
                    &user.id,
                )?;
            }
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `DELETE /api/social/requests/:id` — отозвать свою заявку.
pub async fn cancel_request(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, out| {
            let request = request_row(tx, &id)?;
            if request.text("from_id")? != user.id {
                return Err(ApiError::forbidden("friend_request_not_sender"));
            }
            if request.opt_text("status")?.as_deref() != Some("pending") {
                return Err(ApiError::bad_request("friend_request_not_pending"));
            }
            let to = request.text("to_id")?;
            tx.execute("DELETE FROM friend_requests WHERE id = ?", params![&id])?;
            out.to_user(
                &to,
                &json!({"type": "friend_request_cancelled", "requestId": id, "userId": user.id}),
            );
            let created = request.int("created_at")?;
            relay_friend_event(
                tx,
                &app,
                federation::relay::Friendship::Cancel(created),
                &user.id,
                &to,
            )?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `DELETE /api/social/friends/:id`.
pub async fn remove_friend(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, out| {
            let removed = tx.execute(
                "DELETE FROM friends WHERE (user_id = ? AND friend_id = ?) OR (user_id = ? AND friend_id = ?)",
                params![&user.id, &id, &id, &user.id],
            )?;
            if removed == 0 {
                return Err(ApiError::not_found("not_friends"));
            }
            out.to_user(&id, &json!({"type": "friend_removed", "userId": user.id}));
            relay_friend_event(tx, &app, federation::relay::Friendship::Remove, &user.id, &id)?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// Параметры поиска людей.
#[derive(Debug, serde::Deserialize)]
pub struct DiscoverQuery {
    q: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
}

/// `parseInt(text, 10) || fallback`.
fn js_int(text: Option<&str>, fallback: i64) -> i64 {
    let text = text.unwrap_or_default().trim_start();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let digits: String = digits
        .chars()
        .take_while(char::is_ascii_digit)
        .take(15)
        .collect();
    match digits.parse::<i64>() {
        Ok(0) | Err(_) => fallback,
        Ok(value) => sign * value,
    }
}

const DISCOVERABLE: &str = "discoverable = 1 AND is_deleted = 0 AND id <> ? \
    AND (home_instance IS NULL OR home_instance = '')";

fn neighbours(tx: &mut dyn Tx, user_id: &str) -> DbResult<(HashSet<String>, HashSet<String>)> {
    let friends = tx
        .query(
            "SELECT CASE WHEN user_id = ? THEN friend_id ELSE user_id END AS id FROM friends \
             WHERE user_id = ? OR friend_id = ?",
            params![user_id, user_id, user_id],
        )?
        .iter()
        .map(|row| row.text("id"))
        .collect::<DbResult<_>>()?;
    let spaces = tx
        .query(
            "SELECT space_id FROM space_members WHERE user_id = ?",
            params![user_id],
        )?
        .iter()
        .map(|row| row.text("space_id"))
        .collect::<DbResult<_>>()?;
    Ok((friends, spaces))
}

/// `GET /api/social/discover`.
pub async fn discover(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    Query(query): Query<DiscoverQuery>,
) -> ApiResult<Json<Value>> {
    state.limiter.check(DISCOVER_LIMIT, ip)?;
    state
        .run(move |tx, _| {
            let q = query.q.as_deref().map(js_trim).unwrap_or_default().to_owned();
            let limit = js_int(query.limit.as_deref(), 24).clamp(1, 100);
            let offset = js_int(query.offset.as_deref(), 0).max(0);
            let mut filter = DISCOVERABLE.to_owned();
            let mut values: Vec<crate::db::Value> = vec![user.id.as_str().into()];
            if !q.is_empty() {
                filter.push_str(" AND (username LIKE ? OR display_name LIKE ?)");
                let pattern = format!("%{q}%");
                values.push(pattern.clone().into());
                values.push(pattern.into());
            }
            let total = tx
                .query_one(&format!("SELECT count(*) AS n FROM users WHERE {filter}"), &values)?
                .map_or(Ok(0), |row| row.int("n"))?;
            if total == 0 {
                return Ok(Json(json!({"users": [], "total": 0})));
            }
            let (my_friends, my_spaces) = neighbours(tx, &user.id)?;
            let mut pending: HashMap<String, (&str, String)> = HashMap::new();
            for row in tx.query(
                "SELECT id, from_id, to_id FROM friend_requests WHERE status = 'pending' \
                 AND (from_id = ? OR to_id = ?)",
                params![&user.id, &user.id],
            )? {
                let from = row.text("from_id")?;
                let id = row.text("id")?;
                if from == user.id {
                    pending.insert(row.text("to_id")?, ("outbound_pending", id));
                } else {
                    pending.entry(from).or_insert(("inbound_pending", id));
                }
            }
            values.push(limit.into());
            values.push(offset.into());
            let rows = tx.query(
                &format!(
                    "SELECT {USER_COLUMNS} FROM users WHERE {filter} ORDER BY created_at DESC LIMIT ? OFFSET ?"
                ),
                &values,
            )?;
            let mut users = Vec::with_capacity(rows.len());
            for row in &rows {
                let id = row.text("id")?;
                let (their_friends, their_spaces) = neighbours(tx, &id)?;
                let mutual_friends = my_friends.intersection(&their_friends).count();
                let mutual_spaces = my_spaces.intersection(&their_spaces).count();
                let user_json = sanitize_user(row, false)?;
                let mut entry = json!({
                    "id": id,
                    "username": user_json["username"],
                    "displayName": user_json["displayName"],
                    "avatar": user_json["avatar"],
                    "banner": user_json["banner"],
                    "avatarColor": user_json["avatarColor"],
                    "bio": user_json["bio"],
                    "status": user_json["status"],
                    "customStatus": user_json["customStatus"],
                    "createdAt": user_json["createdAt"],
                    "homeInstance": user_json["homeInstance"],
                    "homeUserId": user_json["homeUserId"],
                    "mutualFriendCount": mutual_friends,
                    "mutualSpaceCount": mutual_spaces,
                    "relationship": "none",
                });
                if my_friends.contains(&id) {
                    entry["relationship"] = json!("friends");
                } else if let Some((relationship, request)) = pending.get(&id) {
                    entry["relationship"] = json!(relationship);
                    entry["requestId"] = json!(request);
                }
                users.push(entry);
            }
            users.sort_by(|a, b| {
                let key = |v: &Value| (v["mutualFriendCount"].as_u64().unwrap_or(0), v["createdAt"].as_i64().unwrap_or(0));
                key(b).cmp(&key(a))
            });
            Ok(Json(json!({"users": users, "total": total})))
        })
        .await
}

/// Параметр поиска по имени.
#[derive(Debug, serde::Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
}

/// `GET /api/social/search`.
pub async fn search(
    State(state): State<AppState>,
    user: AuthUser,
    ClientIp(ip): ClientIp,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    state.limiter.check(SEARCH_LIMIT, ip)?;
    let Some(q) = query.q.filter(|q| !q.is_empty()) else {
        return Ok(Json(json!([])));
    };
    state
        .run(move |tx, _| {
            let pattern = format!("%{q}%");
            let rows = tx.query(
                &format!(
                    "SELECT {USER_COLUMNS} FROM users WHERE {DISCOVERABLE} \
                     AND (username LIKE ? OR display_name LIKE ?) LIMIT 10"
                ),
                params![&user.id, &pattern, &pattern],
            )?;
            let list = rows
                .iter()
                .map(|row| sanitize_user(row, false))
                .collect::<DbResult<Vec<_>>>()?;
            Ok(Json(json!(list)))
        })
        .await
}
