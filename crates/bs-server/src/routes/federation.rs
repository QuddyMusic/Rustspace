//! Пиринг между инстансами: рукопожатие, ведение пиров и запросы на
//! одобрение (`routes/federation/handlers/{peerHandshake,peerAdmin,approvals}.ts`
//! и запрос эпохи из `relay.ts`).

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Row, Tx};
use crate::error::ApiError;
use crate::extract::{ClientIp, JsonBody, read_json_with_text};
use crate::federation::fetch::{self, FetchError, Trust, validate_origin};
use crate::federation::handshake::{
    self, Ensured, Intent, OnFailure, Outbound, Outcome as Shake, Prepared,
};
use crate::federation::peers::{self, Inbound, InboundInput, NewPeer, Peer, Transition};
use crate::federation::{our_origin, reset, worker};
use crate::federation_auth::{self, PeerSecrets, Signed};
use crate::params;
use crate::routes::invites::{admin_only, is_admin};
use crate::state::{AppState, now_ms};
use crate::util::{field, str_field};

type Outcome = Result<Response, ApiError>;

const THIRTY_DAYS_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const BAD_ORIGIN: &str = "must be a valid HTTPS URL (HTTP is only allowed for localhost)";
const IN_PROGRESS: &str = "A peering handshake with this instance is already in progress";
const TOO_MANY: &str = "Too many peering requests — try again later";

/// Ответ с произвольным телом и кодом.
fn send(status: u16, body: Value) -> Response {
    (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(body),
    )
        .into_response()
}

/// Отказ в виде `{error, statusCode}`.
fn fail(status: u16, text: &str) -> Response {
    send(status, json!({"error": text, "statusCode": status}))
}

/// Пропускает только администратора инстанса.
async fn require_admin(state: &AppState, user: &AuthUser) -> Result<Option<Response>, ApiError> {
    let id = user.id.clone();
    let admin = state.run(move |tx, _| Ok(is_admin(tx, &id)?)).await?;
    Ok((!admin).then(admin_only))
}

async fn peer_by_id(state: &AppState, id: &str) -> Result<Option<Peer>, ApiError> {
    let id = id.to_owned();
    state.run(move |tx, _| Ok(peers::by_id(tx, &id)?)).await
}

async fn public_peer(state: &AppState, id: Option<&str>) -> Result<Option<Value>, ApiError> {
    match id {
        Some(id) => Ok(peer_by_id(state, id).await?.map(|peer| peer.public)),
        None => Ok(None),
    }
}

/// Освобождает рукопожатие администратора при выходе из обработчика.
struct AdminClaim<'a> {
    state: &'a AppState,
    origin: String,
}

impl<'a> AdminClaim<'a> {
    fn take(state: &'a AppState, origin: &str) -> Option<Self> {
        state.federation.claim_admin(origin).then(|| Self {
            state,
            origin: origin.to_owned(),
        })
    }
}

impl Drop for AdminClaim<'_> {
    fn drop(&mut self) {
        self.state.federation.release_admin(&self.origin);
    }
}

/// `POST /api/federation/peer/initiate`: администратор начинает пиринг.
pub async fn initiate(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> Outcome {
    if let Some(refusal) = require_admin(&state, &user).await? {
        return Ok(refusal);
    }
    let Some(raw) = str_field(&body, "remoteOrigin").filter(|raw| !raw.is_empty()) else {
        return Ok(fail(400, "remoteOrigin is required"));
    };
    let Some(remote) = validate_origin(raw) else {
        return Ok(fail(400, &format!("remoteOrigin {BAD_ORIGIN}")));
    };
    if our_origin(&state.config) == remote {
        return Ok(fail(400, "Cannot peer with yourself"));
    }
    let Some(_claim) = AdminClaim::take(&state, &remote) else {
        return Ok(fail(409, IN_PROGRESS));
    };
    let started_at = now_ms();
    let (peer_id, secret, on_failure) =
        match handshake::prepare_admin_row(&state, &remote, false, None).await? {
            Prepared::AlreadyActive(id) => {
                return Ok(match public_peer(&state, Some(&id)).await? {
                    Some(peer) => send(200, json!({"peer": peer})),
                    None => fail(409, "The peer changed; try again"),
                });
            }
            Prepared::Busy(text) => return Ok(fail(409, text)),
            Prepared::Ready {
                peer_id,
                secret,
                on_failure,
            } => (peer_id, secret, on_failure),
        };
    let outcome = handshake::run_outbound(
        &state,
        &Outbound {
            peer_id,
            origin: remote,
            secret,
            token: None,
            trust: Trust::Approved,
            activation: "initiate_accepted",
            started_at,
            on_failure,
        },
    )
    .await?;
    let peer = public_peer(&state, outcome.peer_id()).await?;
    Ok(match outcome {
        Shake::Active { verified: true, .. } => send(200, json!({"peer": peer, "verified": true})),
        Shake::Active { .. } => send(200, json!({"peer": peer})),
        Shake::NeedsAttention { .. } => send(200, json!({"peer": peer, "verified": false})),
        Shake::Awaiting(_) => send(202, json!({"peer": peer})),
        Shake::Rejected { reason, error, .. } => {
            if reason.as_deref() == Some("stale_peering_on_remote") {
                send(
                    409,
                    json!({
                        "error": "The remote instance still holds stale peering for you. Ask its admin to reset (or Re-peer) their side; this instance checks every 15 minutes and completes the peering once they have.",
                        "code": "PEER_EXISTS_RESET_REQUIRED",
                        "statusCode": 409,
                        "peer": peer,
                    }),
                )
            } else {
                send(
                    502,
                    json!({"error": error, "statusCode": 502, "peer": peer}),
                )
            }
        }
        Shake::Revoked(_) => fail(409, "This peer was revoked meanwhile"),
        Shake::Failed {
            error, timed_out, ..
        } => fail(if timed_out { 504 } else { 502 }, &error),
    })
}

/// Ставит входящее рукопожатие в очередь на одобрение (`queueApprovalRequest`).
fn queue_approval(
    tx: &mut dyn Tx,
    state: &AppState,
    out: &mut crate::hub::Outbox,
    origin: &str,
    secret: &str,
    instance_name: Option<&str>,
) -> DbResult<Response> {
    let now = now_ms();
    let token = federation_auth::generate_secret();
    let existing = tx.query_one(
        "SELECT id FROM peer_approval_requests WHERE origin = ?",
        params![origin],
    )?;
    match existing {
        Some(row) => {
            tx.execute(
                "UPDATE peer_approval_requests SET instance_name = ?, hmac_secret = ?, \
                 requested_at = ?, expires_at = ?, approval_token = ? WHERE id = ?",
                params![
                    instance_name,
                    secret,
                    now,
                    now + THIRTY_DAYS_MS,
                    &token,
                    row.text("id")?
                ],
            )?;
        }
        None => {
            tx.execute(
                "INSERT INTO peer_approval_requests (id, origin, instance_name, hmac_secret, \
                 requested_at, expires_at, approval_token) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    state.ids.next(),
                    origin,
                    instance_name,
                    secret,
                    now,
                    now + THIRTY_DAYS_MS,
                    &token
                ],
            )?;
        }
    }
    let mut event = json!({"type": "federation_approval_request_received", "origin": origin});
    if let (Some(name), Some(fields)) = (instance_name, event.as_object_mut()) {
        fields.insert("instanceName".to_owned(), name.into());
    }
    peers::to_admins(tx, &state.hub, out, &event)?;
    Ok(send(
        202,
        json!({
            "queued": true,
            "message": "Request queued for admin approval",
            "approvalToken": token,
        }),
    ))
}

/// Что сделать после записи решения о входящем рукопожатии.
enum After {
    Nothing,
    Effects(peers::Effects),
    Reset {
        peer_id: String,
        dead: String,
        observed: String,
    },
}

/// `POST /api/federation/peer/accept`: другой инстанс предлагает пиринг.
pub async fn accept(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    JsonBody(body): JsonBody,
) -> Outcome {
    if !state.hub.allow(
        &format!("federation-accept:{ip}"),
        10,
        Duration::from_mins(1),
    ) {
        return Ok(fail(429, TOO_MANY));
    }
    let Some(raw) = str_field(&body, "sourceOrigin").filter(|raw| !raw.is_empty()) else {
        return Ok(fail(400, "sourceOrigin is required"));
    };
    let Some(secret) = str_field(&body, "hmacSecret").filter(|secret| !secret.is_empty()) else {
        return Ok(fail(400, "hmacSecret is required"));
    };
    let Some(source) = validate_origin(raw) else {
        return Ok(fail(400, &format!("sourceOrigin {BAD_ORIGIN}")));
    };
    let secret = secret.to_owned();
    let their_name = str_field(&body, "instanceName").map(str::to_owned);
    let their_epoch = str_field(&body, "instanceId")
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    let token = str_field(&body, "approvalToken").map(str::to_owned);
    let app = state.clone();
    let at = source.clone();
    let own_in_flight = state.federation.in_flight(&source);
    let (reply, after) = state
        .run(move |tx, out| {
            let (our_name, auto_accept) = handshake::settings(tx)?;
            let existing = peers::by_origin(tx, &at)?;
            let ours = our_origin(&app.config);
            let decision = peers::decide_inbound(InboundInput {
                row: existing.as_ref(),
                auto_accept,
                token: token.as_deref(),
                own_in_flight,
                our_origin: &ours,
                source_origin: &at,
            });
            let accepted = json!({
                "accepted": true,
                "instanceName": our_name,
                "instanceId": &*app.instance_id,
            });
            let busy = || {
                send(
                    409,
                    json!({"error": IN_PROGRESS, "code": "PEER_HANDSHAKE_IN_PROGRESS", "statusCode": 409}),
                )
            };
            Ok(match decision {
                Inbound::Create => {
                    let new = NewPeer {
                        origin: at.clone(),
                        secret,
                        initiated_by: "remote",
                        instance_name: their_name,
                        epoch: their_epoch,
                        last_seen_at: Some(now_ms()),
                        status: "active",
                        cause: Some("accept_new"),
                        reason: None,
                    };
                    match peers::insert(tx, &app.ids, &new)? {
                        Some((_, effects)) => (send(200, accepted), After::Effects(effects)),
                        None => (busy(), After::Nothing),
                    }
                }
                Inbound::Activate { from, cause } => {
                    let Some(row) = existing else {
                        return Ok((fail(500, "Peer row missing"), After::Nothing));
                    };
                    let change = Transition::new(&[from], "active", cause)
                        .expect(&row.secret)
                        .set("hmac_secret", secret)
                        .set("instance_name", their_name)
                        .set("peer_instance_id", their_epoch)
                        .set("last_seen_at", now_ms())
                        .set("approval_token", None::<String>);
                    match peers::apply(tx, &row.id, &change)? {
                        peers::Moved::Applied { effects, .. } => {
                            if from == "awaiting_approval" {
                                tx.execute(
                                    "DELETE FROM peer_approval_requests WHERE origin = ?",
                                    params![&at],
                                )?;
                            }
                            (send(200, accepted), After::Effects(*effects))
                        }
                        peers::Moved::Refused(_) => (busy(), After::Nothing),
                    }
                }
                Inbound::Queue => (
                    queue_approval(tx, &app, out, &at, &secret, their_name.as_deref())?,
                    After::Nothing,
                ),
                Inbound::RefuseExists => {
                    // Пир пришёл с другой эпохой: он переустановлен.
                    let after = match (existing, their_epoch) {
                        (Some(row), Some(observed)) => match row.epoch {
                            Some(dead) if dead != observed => After::Reset {
                                peer_id: row.id,
                                dead,
                                observed,
                            },
                            _ => After::Nothing,
                        },
                        _ => After::Nothing,
                    };
                    (
                        send(
                            409,
                            json!({
                                "accepted": false,
                                "code": "PEER_EXISTS_RESET_REQUIRED",
                                "error": "This instance already holds peering for you; its admin must reset that peering before a new handshake can be accepted.",
                                "instanceName": our_name,
                                "instanceId": &*app.instance_id,
                                "statusCode": 409,
                            }),
                        ),
                        after,
                    )
                }
                Inbound::RefuseRevoked => (
                    send(
                        403,
                        json!({"error": handshake::REMOTE_REVOKED, "code": "PEER_REVOKED", "statusCode": 403}),
                    ),
                    After::Nothing,
                ),
                Inbound::RefuseDenied => (
                    send(
                        403,
                        json!({
                            "error": "This instance requires manual peering approval",
                            "code": "PEERING_REQUIRES_APPROVAL",
                            "statusCode": 403,
                        }),
                    ),
                    After::Nothing,
                ),
                Inbound::RefuseInProgress => (
                    send(
                        409,
                        json!({
                            "error": "This instance is completing its own peering handshake with you",
                            "code": "PEER_HANDSHAKE_IN_PROGRESS",
                            "statusCode": 409,
                        }),
                    ),
                    After::Nothing,
                ),
            })
        })
        .await?;
    match after {
        After::Nothing => {}
        After::Effects(effects) => {
            // Ответ не ждёт последствий: удалённая сторона проверяет секрет
            // сразу после него.
            let app = state.clone();
            tokio::spawn(async move { peers::run_effects(&app, effects).await });
        }
        After::Reset {
            peer_id,
            dead,
            observed,
        } => reset::mark_and_notify(&state, &peer_id, &source, &dead, &observed).await,
    }
    Ok(reply)
}

/// `POST /api/federation/peer/ensure`: пользователь просит пиринг с инстансом.
pub async fn ensure(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> Outcome {
    let Some(raw) = str_field(&body, "remoteOrigin").filter(|raw| !raw.is_empty()) else {
        return Ok(fail(400, "remoteOrigin is required"));
    };
    let Some(remote) = validate_origin(raw) else {
        return Ok(fail(400, &format!("remoteOrigin {BAD_ORIGIN}")));
    };
    // Клиенты до появления поля звали маршрут только ради подключения инстанса.
    let reason = match field(&body, "reason") {
        None => "instance_connect",
        Some(Value::String(text)) if text == "instance_connect" => "instance_connect",
        Some(_) => return Err(ApiError::bad_request("validation_failed")),
    };
    let at = remote.clone();
    let settled = state
        .run(move |tx, _| Ok(handshake::settled(tx, &at)?))
        .await?;
    if settled.is_none()
        && !state.hub.allow(
            &format!("federation-ensure:{}", user.id),
            3,
            Duration::from_mins(15),
        )
    {
        return Ok(fail(429, TOO_MANY));
    }
    let result = handshake::ensure_peered(
        &state,
        &remote,
        Intent::User {
            user_id: user.id,
            reason: reason.to_owned(),
            target: remote.clone(),
        },
    )
    .await;
    Ok(send(
        200,
        match result {
            Ensured::Active(peer_id) => json!({"peeringStatus": "active", "peerId": peer_id}),
            Ensured::Rejected(error) => json!({"peeringStatus": "rejected", "error": error}),
            Ensured::Pending(error) => {
                json!({"peeringStatus": "awaiting_approval", "error": error})
            }
            Ensured::Failed(error) => json!({"peeringStatus": "pending", "error": error}),
            Ensured::AdminRequired(_) => json!({"peeringStatus": "admin_required"}),
        },
    ))
}

fn signed_headers(headers: &HeaderMap) -> Option<Signed> {
    federation_auth::parse_headers(|name| headers.get(name).and_then(|value| value.to_str().ok()))
}

/// Подписанное тело: запись для подписи и разобранное значение.
struct SignedBody {
    value: Value,
    text: Option<String>,
}

impl SignedBody {
    /// `JSON.stringify(request.body)`: у запроса без тела — `undefined`.
    fn stringified(&self) -> &str {
        self.text.as_deref().unwrap_or("undefined")
    }
}

async fn signed_body(request: Request) -> Result<(HeaderMap, SignedBody), Response> {
    let headers = request.headers().clone();
    let (value, text) = read_json_with_text(request).await?;
    Ok((headers, SignedBody { value, text }))
}

fn verified(body: &str, signed: &Signed, secrets: PeerSecrets<'_>) -> bool {
    federation_auth::verify_peer(
        body,
        &signed.signature,
        signed.timestamp,
        signed.nonce.as_deref(),
        secrets,
        now_ms(),
    )
}

/// `POST /api/federation/peer/rotate`: пир предлагает новый общий секрет.
pub async fn rotate_inbound(State(state): State<AppState>, request: Request) -> Outcome {
    let (headers, body) = match signed_body(request).await {
        Ok(read) => read,
        Err(refusal) => return Ok(refusal),
    };
    let Some(signed) = signed_headers(&headers) else {
        return Ok(fail(401, "Missing or malformed federation headers"));
    };
    let origin = signed.origin.clone();
    let peer = state
        .run(move |tx, _| Ok(peers::by_origin(tx, &origin)?))
        .await?;
    let Some(peer) = peer.filter(|peer| peer.status == "active") else {
        return Ok(fail(403, "Unknown or inactive peer"));
    };
    if !verified(body.stringified(), &signed, peer.secrets()) {
        return Ok(fail(401, "Invalid signature"));
    }
    let Some(secret) = str_field(&body.value, "newSecret").filter(|secret| {
        secret.len() == 64
            && secret
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) else {
        return Ok(fail(400, "newSecret must be a 64-character hex string"));
    };
    if peer.pending_secret.is_some() {
        return Ok(fail(
            409,
            "A secret rotation is already in progress — wait for it to complete",
        ));
    }
    let (id, secret) = (peer.id, secret.to_owned());
    state
        .run(move |tx, _| {
            tx.execute(
                "UPDATE federation_peers SET pending_hmac_secret = ?, secret_rotation_at = ? \
                 WHERE id = ?",
                params![secret, now_ms(), id],
            )?;
            Ok(())
        })
        .await?;
    tracing::info!(origin = peer.origin, "принята смена секрета от пира");
    Ok(send(
        200,
        json!({"accepted": true, "gracePeriodMs": 900_000}),
    ))
}

/// `POST /api/federation/peer/denied`: та сторона отклонила наш запрос.
pub async fn denied(State(state): State<AppState>, request: Request) -> Outcome {
    let (headers, body) = match signed_body(request).await {
        Ok(read) => read,
        Err(refusal) => return Ok(refusal),
    };
    let Some(signed) = signed_headers(&headers) else {
        return Ok(fail(401, "Missing or malformed federation headers"));
    };
    let origin = signed.origin.clone();
    let peer = state
        .run(move |tx, _| Ok(peers::by_origin(tx, &origin)?))
        .await?;
    let Some(peer) = peer else {
        return Ok(fail(404, "No peer record for this origin"));
    };
    if peer.status != "awaiting_approval" {
        // В тексте — статус как записан в базе.
        let status = peer
            .public
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or(peer.status);
        return Ok(fail(
            409,
            &format!("Peer is in '{status}' state, not awaiting_approval"),
        ));
    }
    // Только действующий секрет: смены секрета до пиринга не бывает.
    let secrets = PeerSecrets {
        current: &peer.secret,
        pending: None,
        rotation_at: None,
    };
    if !verified(body.stringified(), &signed, secrets) {
        return Ok(fail(401, "Invalid HMAC signature"));
    }
    let reason = if str_field(&body.value, "reason") == Some("expired") {
        "expired_on_remote"
    } else {
        "denied_by_remote"
    };
    let change =
        Transition::new(&["awaiting_approval"], "rejected", "remote_denied_request").reason(reason);
    peers::transition(&state, &peer.id, change).await?;
    Ok(send(200, json!({"acknowledged": true})))
}

/// `POST /api/federation/epoch`: эпоха инстанса, подписанная общим секретом.
/// Отвечает любому неотозванному пиру: по этому запросу недоступный пир
/// возвращается в строй, а новый секрет проверяется до активации.
pub async fn epoch(State(state): State<AppState>, request: Request) -> Outcome {
    let (headers, body) = match signed_body(request).await {
        Ok(read) => read,
        Err(refusal) => return Ok(refusal),
    };
    let Some(signed) = signed_headers(&headers) else {
        return Ok(fail(400, "Missing or malformed federation headers"));
    };
    let origin = signed.origin.clone();
    let peer = state
        .run(move |tx, _| Ok(peers::by_origin(tx, &origin)?))
        .await?;
    // Статус сравнивается как записан: неизвестный статус не равен `revoked`.
    let Some(peer) = peer.filter(|peer| peer.status != "revoked") else {
        return Ok(fail(403, "Not peered"));
    };
    let text = body.text.as_deref().unwrap_or("{}");
    if !verified(text, &signed, peer.secrets()) {
        return Ok(fail(401, "Invalid signature"));
    }
    Ok(signed_json(
        &state,
        &json!({"instanceId": &*state.instance_id}),
        &peer.secret,
    ))
}

/// Проверка подписанного запроса пира (`authenticateS2SPeer`): заголовки,
/// действующий пир, предел частоты (до проверки подписи), подпись,
/// одноразовое число. Возвращает пира и одноразовое число запроса.
fn authenticate_s2s(
    state: &AppState,
    headers: &HeaderMap,
    body: &SignedBody,
    peer: Option<Peer>,
    signed: Option<&Signed>,
    rate_limit: Option<(u32, Duration)>,
) -> Result<(Peer, Option<String>), Response> {
    let Some(signed) = signed else {
        return Err(fail(401, "Missing or malformed federation headers"));
    };
    let _ = headers;
    let Some(peer) = peer.filter(|peer| peer.status == "active") else {
        return Err(fail(403, "Unknown or inactive peer"));
    };
    if let Some((max, window)) = rate_limit
        && !state
            .hub
            .allow(&format!("federation-relay:{}", peer.origin), max, window)
    {
        return Err(fail(429, "Rate limit exceeded"));
    }
    if !verified(body.stringified(), signed, peer.secrets()) {
        return Err(fail(401, "Invalid signature"));
    }
    match &signed.nonce {
        Some(nonce) => {
            if state.federation.nonce_seen(&peer.origin, nonce, now_ms()) {
                return Err(fail(409, "Duplicate nonce — possible replay"));
            }
        }
        None if peer.nonce_supported => {
            return Err(fail(
                401,
                "Nonce required — peer previously supported nonces",
            ));
        }
        None => {
            tracing::warn!(
                origin = peer.origin,
                "пир не поддерживает защиту от повторов (нет nonce)"
            );
        }
    }
    let nonce = signed.nonce.clone();
    Ok((peer, nonce))
}

async fn s2s_peer(state: &AppState, signed: Option<&Signed>) -> Result<Option<Peer>, ApiError> {
    let Some(origin) = signed.map(|signed| signed.origin.clone()) else {
        return Ok(None);
    };
    state
        .run(move |tx, _| Ok(peers::by_origin(tx, &origin)?))
        .await
}

/// `POST /api/federation/relay`: события от пира.
pub async fn relay(State(state): State<AppState>, request: Request) -> Outcome {
    let (headers, body) = match signed_body(request).await {
        Ok(read) => read,
        Err(refusal) => return Ok(refusal),
    };
    let signed = signed_headers(&headers);
    let peer = s2s_peer(&state, signed.as_ref()).await?;
    let (peer, nonce) = match authenticate_s2s(
        &state,
        &headers,
        &body,
        peer,
        signed.as_ref(),
        Some((90, Duration::from_mins(1))),
    ) {
        Ok(found) => found,
        Err(refusal) => return Ok(refusal),
    };
    let claimed = body
        .value
        .get("sourceInstanceId")
        .and_then(Value::as_str)
        .filter(|epoch| !epoch.is_empty())
        .map(str::to_owned);
    if let Some(claimed) = claimed
        && peer.epoch.is_none()
    {
        let id = peer.id.clone();
        state
            .run(move |tx, _| {
                tx.execute(
                    "UPDATE federation_peers SET peer_instance_id = ? WHERE id = ? AND peer_instance_id IS NULL",
                    params![claimed, id],
                )?;
                Ok(())
            })
            .await?;
    }
    let events = body
        .value
        .get("version")
        .filter(|version| version.as_f64() == Some(1.0))
        .and_then(|_| body.value.get("events"))
        .and_then(Value::as_array);
    let Some(events) = events else {
        return Ok(fail(400, "Invalid relay request format"));
    };
    if events.len() > 50 {
        return Ok(fail(400, "Maximum 50 events per batch"));
    }
    let Some(source) = str_field(&body.value, "sourceInstance").filter(|s| !s.is_empty()) else {
        return Ok(fail(400, "sourceInstance is required"));
    };
    if federation_auth::normalize_origin(Some(source))
        != federation_auth::normalize_origin(Some(&peer.origin))
    {
        tracing::warn!(
            peer = peer.origin,
            source,
            "источник пачки не совпадает с пиром"
        );
        return Ok(fail(
            403,
            "sourceInstance does not match the authenticated peer",
        ));
    }
    let outcome = crate::federation::inbound::process_events(
        &state,
        events,
        source,
        &peer.origin,
        crate::federation::inbound::Delivery::Live,
    )
    .await;
    let (id, ratchet) = (peer.id.clone(), nonce.is_some() && !peer.nonce_supported);
    let configured = i64::try_from(state.config.max_upload_size).unwrap_or(i64::MAX);
    let max_upload = state
        .run(move |tx, _| {
            let mut sql =
                "UPDATE federation_peers SET last_seen_at = ?, consecutive_failures = 0".to_owned();
            if ratchet {
                sql.push_str(", nonce_supported = 1");
            }
            sql.push_str(" WHERE id = ?");
            tx.execute(&sql, params![now_ms(), id])?;
            let row = tx.query_one(
                "SELECT max_upload_size_bytes FROM instance_settings WHERE id = 1",
                params![],
            )?;
            Ok(row
                .map(|row| row.opt_int("max_upload_size_bytes"))
                .transpose()?
                .flatten()
                .unwrap_or(configured))
        })
        .await?;
    let mut response = json!({
        "accepted": outcome.accepted,
        "rejected": crate::federation::inbound::rejections_for_sender(
            &outcome.rejected,
            body.value.get("capabilities"),
        ),
        "maxUploadSize": max_upload,
    });
    if !outcome.undeliverable.is_empty() {
        response["undeliverable"] = Value::Array(
            outcome
                .undeliverable
                .iter()
                .map(|(id, reason)| json!({"messageId": id, "reason": reason}))
                .collect(),
        );
    }
    Ok(send(200, response))
}

/// `DELETE /api/federation/identity`: дом удаляет своего пользователя здесь.
pub async fn delete_identity(State(state): State<AppState>, request: Request) -> Outcome {
    let (headers, body) = match signed_body(request).await {
        Ok(read) => read,
        Err(refusal) => return Ok(refusal),
    };
    let signed = signed_headers(&headers);
    let peer = s2s_peer(&state, signed.as_ref()).await?;
    let (peer, _) = match authenticate_s2s(&state, &headers, &body, peer, signed.as_ref(), None) {
        Ok(found) => found,
        Err(refusal) => return Ok(refusal),
    };
    let home_user = str_field(&body.value, "homeUserId").filter(|s| !s.is_empty());
    let home = str_field(&body.value, "homeInstance").filter(|s| !s.is_empty());
    let mode = str_field(&body.value, "mode").filter(|mode| matches!(*mode, "soft" | "full"));
    let (Some(home_user), Some(_), Some(mode)) = (home_user, home, mode) else {
        return Ok(fail(
            400,
            "Invalid request: homeUserId, homeInstance, and mode (soft|full) required",
        ));
    };
    let (home_user, purge, origin) = (home_user.to_owned(), mode == "full", peer.origin.clone());
    let app = state.clone();
    let outcome = state
        .run(move |tx, out| {
            let Some(user) = tx.query_one(
                "SELECT * FROM users WHERE home_user_id = ? AND is_deleted = 0",
                params![&home_user],
            )?
            else {
                return Ok(Ok(Vec::new()));
            };
            let home = user.opt_text("home_instance")?.unwrap_or_default();
            if home.is_empty() || reset::domain_of(&home) != reset::domain_of(&origin) {
                return Ok(Err(fail(
                    403,
                    "Attribution mismatch: you can only delete users from your own instance",
                )));
            }
            if user.opt_flag("federation_home_orphaned")? == Some(true) {
                return Ok(Ok(Vec::new()));
            }
            let id = user.text("id")?;
            let owned = tx.query(
                "SELECT id, name FROM spaces WHERE owner_id = ?",
                params![&id],
            )?;
            if !owned.is_empty() {
                let spaces = owned
                    .iter()
                    .map(|row| Ok(json!({"id": row.text("id")?, "name": row.text("name")?})))
                    .collect::<DbResult<Vec<_>>>()?;
                return Ok(Err(send(
                    409,
                    json!({"error": "owns_spaces", "ownedSpaces": spaces, "statusCode": 409}),
                )));
            }
            let spaces: Vec<String> = tx
                .query(
                    "SELECT space_id FROM space_members WHERE user_id = ?",
                    params![&id],
                )?
                .iter()
                .map(|row| row.text("space_id"))
                .collect::<DbResult<_>>()?;
            let audience = crate::ws::profile_targets(tx, &id)?;
            let stale = crate::routes::account::tombstone(tx, &id, purge)?;
            crate::federation::presence::forget(&app.federation.projections, &id);
            for space in &spaces {
                out.to_space(
                    space,
                    &json!({"type": "member_left", "spaceId": space, "userId": id}),
                );
            }
            if let Some(row) = crate::routes::users::user_row(tx, &id)? {
                let event = json!({"type": "user_updated", "user": crate::routes::users::sanitize_user(&row, false)?});
                for target in &audience {
                    out.to_user(target, &event);
                }
            }
            out.close_user(&id);
            tracing::info!(user = id, origin, mode = if purge { "full" } else { "soft" }, "пользователь удалён по запросу дома");
            Ok(Ok(stale))
        })
        .await?;
    match outcome {
        Ok(stale) => {
            for file in stale.iter().filter(|file| !file.starts_with("http")) {
                crate::files::delete_upload_file(&state.config, file).await;
            }
            Ok(send(200, json!({"success": true})))
        }
        Err(refusal) => Ok(refusal),
    }
}

/// Ответ JSON, подписанный общим секретом (`sendSignedJson`).
#[must_use]
pub fn signed_json(state: &AppState, payload: &Value, secret: &str) -> Response {
    let body = payload.to_string();
    let built = federation_auth::headers(&body, secret, &our_origin(&state.config), now_ms());
    let mut response = (StatusCode::OK, body).into_response();
    for (name, value) in built {
        if name == "X-Federation-Origin" {
            continue;
        }
        if let (Ok(name), Ok(value)) = (
            axum::http::HeaderName::from_bytes(name.as_bytes()),
            axum::http::HeaderValue::from_str(&value),
        ) {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

/// `GET /api/federation/peers`.
pub async fn list_peers(State(state): State<AppState>, user: AuthUser) -> Outcome {
    if let Some(refusal) = require_admin(&state, &user).await? {
        return Ok(refusal);
    }
    let listed = state
        .run(|tx, _| {
            let rows = tx.query(
                &format!("SELECT {} FROM federation_peers", peers::COLUMNS),
                params![],
            )?;
            let mut listed = Vec::new();
            for row in rows {
                listed.push(Peer::from_row(&row)?.public);
            }
            Ok(listed)
        })
        .await?;
    Ok(send(200, json!({"peers": listed})))
}

fn count(tx: &mut dyn Tx, sql: &str, id: &str) -> DbResult<i64> {
    tx.query_one(sql, params![id])?
        .map_or(Ok(0), |row| row.int("n"))
}

/// `GET /api/federation/reset-events`: журнал сбросов удалённых инстансов.
pub async fn reset_events(State(state): State<AppState>, user: AuthUser) -> Outcome {
    if let Some(refusal) = require_admin(&state, &user).await? {
        return Ok(refusal);
    }
    let events = state
        .run(|tx, _| {
            let rows = tx.query(
                "SELECT origin, dead_epoch, new_epoch, detected_at, resolved_at, stub_count, \
                 orphaned_account_count, acknowledged_at FROM federation_reset_events",
                params![],
            )?;
            let mut events = Vec::new();
            for event in rows {
                let origin = event.text("origin")?;
                let (condition, values) = reset::detached_match(&origin);
                let accounts = tx.query(
                    &format!(
                        "SELECT id, username, display_name, avatar_color FROM users \
                         WHERE federation_home_orphaned = 1 AND is_deleted = 0 AND {condition}"
                    ),
                    &values,
                )?;
                let mut orphaned = Vec::new();
                for account in accounts {
                    let id = account.text("id")?;
                    let spaces: Vec<Value> = tx
                        .query(
                            "SELECT id, name FROM spaces WHERE owner_id = ?",
                            params![&id],
                        )?
                        .iter()
                        .map(|space| {
                            Ok(json!({"id": space.text("id")?, "name": space.text("name")?}))
                        })
                        .collect::<DbResult<_>>()?;
                    orphaned.push(json!({
                        "id": id,
                        "username": account.text("username")?,
                        "displayName": account.opt_text("display_name")?,
                        "avatarColor": account.opt_text("avatar_color")?,
                        "ownedSpaces": spaces,
                        "spaceMemberCount": count(
                            tx,
                            "SELECT count(*) AS n FROM space_members WHERE user_id = ?",
                            &id,
                        )?,
                        "messageCount": count(
                            tx,
                            "SELECT count(*) AS n FROM messages WHERE user_id = ?",
                            &id,
                        )?,
                    }));
                }
                events.push(json!({
                    "origin": origin,
                    "deadEpoch": event.text("dead_epoch")?,
                    "newEpoch": event.opt_text("new_epoch")?,
                    "detectedAt": event.int("detected_at")?,
                    "resolvedAt": event.opt_int("resolved_at")?,
                    "stubCount": event.int("stub_count")?,
                    "orphanedAccountCount": event.int("orphaned_account_count")?,
                    "acknowledgedAt": event.opt_int("acknowledged_at")?,
                    "orphanedAccounts": orphaned,
                }));
            }
            Ok(events)
        })
        .await?;
    Ok(send(200, json!({"events": events})))
}

/// `POST /api/federation/reset-events/acknowledge`.
pub async fn acknowledge_reset(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> Outcome {
    if let Some(refusal) = require_admin(&state, &user).await? {
        return Ok(refusal);
    }
    let Some(origin) = str_field(&body, "origin").filter(|origin| !origin.is_empty()) else {
        return Ok(fail(400, "origin is required"));
    };
    let origin = origin.to_owned();
    let found = state
        .run(move |tx, _| {
            let Some(event) = tx.query_one(
                "SELECT acknowledged_at FROM federation_reset_events WHERE origin = ?",
                params![&origin],
            )?
            else {
                return Ok(false);
            };
            if event.opt_int("acknowledged_at")?.is_none() {
                tx.execute(
                    "UPDATE federation_reset_events SET acknowledged_at = ? WHERE origin = ?",
                    params![now_ms(), &origin],
                )?;
            }
            Ok(true)
        })
        .await?;
    Ok(if found {
        send(200, json!({"success": true}))
    } else {
        fail(404, "No reset event for this origin")
    })
}

/// Пир для маршрута администратора или готовый отказ.
async fn admin_peer(
    state: &AppState,
    user: &AuthUser,
    id: &str,
) -> Result<Result<Peer, Response>, ApiError> {
    if let Some(refusal) = require_admin(state, user).await? {
        return Ok(Err(refusal));
    }
    Ok(peer_by_id(state, id)
        .await?
        .ok_or_else(|| fail(404, "Peer not found")))
}

/// `DELETE /api/federation/peers/:id`: отзыв пира.
pub async fn revoke_peer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Outcome {
    if let Err(refusal) = admin_peer(&state, &user, &id).await? {
        return Ok(refusal);
    }
    let change = Transition::new(
        &[
            "pending",
            "awaiting_approval",
            "active",
            "unreachable",
            "needs_attention",
            "rejected",
        ],
        "revoked",
        "admin_revoked",
    );
    peers::transition(&state, &id, change).await?;
    Ok(send(200, json!({"success": true})))
}

async fn remove_peer(
    state: &AppState,
    id: &str,
    from: &'static [&'static str],
) -> Result<bool, ApiError> {
    let peer_id = id.to_owned();
    let removed = state
        .run(move |tx, _| Ok(peers::remove(tx, &peer_id, from, false)?))
        .await?;
    match removed {
        Some(effects) => {
            peers::run_effects(state, effects).await;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// `POST /api/federation/peers/:id/reset`: забыть пира, требующего внимания.
pub async fn reset_peer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(_): JsonBody,
) -> Outcome {
    let peer = match admin_peer(&state, &user, &id).await? {
        Ok(peer) => peer,
        Err(refusal) => return Ok(refusal),
    };
    if peer.status != "needs_attention" {
        return Ok(fail(
            400,
            "Reset is only available for peers in the needs_attention state. Use revoke for active peers.",
        ));
    }
    Ok(if remove_peer(&state, &id, &["needs_attention"]).await? {
        send(200, json!({"success": true}))
    } else {
        fail(409, "The peer changed; reload and try again")
    })
}

/// `POST /api/federation/peers/:id/recheck`: проверить недоступного пира сейчас.
pub async fn recheck_peer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(_): JsonBody,
) -> Outcome {
    let peer = match admin_peer(&state, &user, &id).await? {
        Ok(peer) => peer,
        Err(refusal) => return Ok(refusal),
    };
    if peer.status != "unreachable" {
        return Ok(fail(
            400,
            "Recheck is only available for unreachable peers.",
        ));
    }
    let (reachable, observed) = fetch::probe_reachable(&state.config, &peer.origin).await;
    if !reachable {
        let peer_id = id.clone();
        state
            .run(move |tx, _| {
                Ok(peers::record_attempt(
                    tx,
                    &peer_id,
                    &["unreachable"],
                    now_ms(),
                )?)
            })
            .await?;
        return Ok(send(
            200,
            json!({"recovered": false, "status": "unreachable"}),
        ));
    }
    Ok(
        match worker::recover_or_detect_reset(&state, &peer, observed.as_deref()).await {
            worker::Recovery::ResetDetected => send(
                200,
                json!({"recovered": false, "status": "needs_attention"}),
            ),
            worker::Recovery::Recovered => {
                send(200, json!({"recovered": true, "status": "active"}))
            }
            worker::Recovery::Unchanged => {
                let status = peer_by_id(&state, &id).await?.map_or_else(
                    || json!("removed"),
                    |peer| peer.public.get("status").cloned().unwrap_or_default(),
                );
                send(200, json!({"recovered": false, "status": status}))
            }
        },
    )
}

/// `PATCH /api/federation/peers/:id`: срок плановой смены секрета.
pub async fn update_peer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> Outcome {
    if let Err(refusal) = admin_peer(&state, &user, &id).await? {
        return Ok(refusal);
    }
    let Some(raw) = field(&body, "autoRotateIntervalDays") else {
        return Ok(fail(400, "No valid fields to update"));
    };
    let days = crate::routes::admin::js_number(raw);
    if days.is_nan() || days.fract() != 0.0 || !(1.0..=365.0).contains(&days) {
        return Ok(fail(
            400,
            "autoRotateIntervalDays must be an integer between 1 and 365",
        ));
    }
    // Целое из 1..=365: приведение точное (в PostgreSQL столбец целочисленный).
    #[allow(clippy::cast_possible_truncation, reason = "проверено выше")]
    let days = days as i64;
    let peer_id = id.clone();
    state
        .run(move |tx, _| {
            tx.execute(
                "UPDATE federation_peers SET auto_rotate_interval_days = ? WHERE id = ?",
                params![days, peer_id],
            )?;
            Ok(())
        })
        .await?;
    Ok(send(
        200,
        json!({"peer": public_peer(&state, Some(&id)).await?}),
    ))
}

/// `DELETE /api/federation/peers/:id/permanent`: удалить отозванного пира.
pub async fn delete_peer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Outcome {
    let peer = match admin_peer(&state, &user, &id).await? {
        Ok(peer) => peer,
        Err(refusal) => return Ok(refusal),
    };
    if !matches!(peer.status, "revoked" | "rejected") {
        return Ok(fail(
            400,
            "Only revoked or rejected peers can be permanently deleted. Revoke the peer first.",
        ));
    }
    remove_peer(&state, &id, &["revoked", "rejected"]).await?;
    Ok(send(200, json!({"success": true})))
}

/// `POST /api/federation/peers/:id/rotate`: сменить общий секрет.
pub async fn rotate_peer(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(_): JsonBody,
) -> Outcome {
    let peer = match admin_peer(&state, &user, &id).await? {
        Ok(peer) => peer,
        Err(refusal) => return Ok(refusal),
    };
    if peer.status != "active" {
        return Ok(fail(400, "Can only rotate secrets for active peers"));
    }
    if peer.pending_secret.is_some() {
        return Ok(fail(
            409,
            "A secret rotation is already in progress — wait for it to complete",
        ));
    }
    Ok(match worker::start_rotation(&state, &peer).await {
        Ok(()) => {
            tracing::info!(origin = peer.origin, "начата смена секрета");
            send(200, json!({"success": true, "gracePeriodMs": 900_000}))
        }
        Err((status, text)) => fail(status, &text),
    })
}

/// Запрос на одобрение пиринга.
#[derive(Debug, Clone)]
struct Approval {
    id: String,
    origin: String,
    outbound: bool,
    instance_name: Option<String>,
    secret: Option<String>,
    token: Option<String>,
}

fn approval(tx: &mut dyn Tx, id: &str) -> DbResult<Option<Approval>> {
    let Some(row) = tx.query_one(
        "SELECT id, origin, direction, instance_name, hmac_secret, approval_token \
         FROM peer_approval_requests WHERE id = ?",
        params![id],
    )?
    else {
        return Ok(None);
    };
    Ok(Some(Approval {
        id: row.text("id")?,
        origin: row.text("origin")?,
        outbound: row.text("direction")? == "outbound",
        instance_name: row.opt_text("instance_name")?,
        secret: row.opt_text("hmac_secret")?,
        token: row.opt_text("approval_token")?,
    }))
}

/// `GET /api/federation/approval-requests`.
pub async fn approval_requests(State(state): State<AppState>, user: AuthUser) -> Outcome {
    if let Some(refusal) = require_admin(&state, &user).await? {
        return Ok(refusal);
    }
    let requests = state
        .run(|tx, _| {
            let rows = tx.query(
                "SELECT id, origin, direction, instance_name, requested_at, expires_at \
                 FROM peer_approval_requests ORDER BY requested_at DESC",
                params![],
            )?;
            let mut requests = Vec::new();
            for row in rows {
                let id = row.text("id")?;
                let direction = row.text("direction")?;
                let mut request = json!({
                    "id": id,
                    "origin": row.text("origin")?,
                    "direction": direction,
                    "instanceName": row.opt_text("instance_name")?,
                    "requestedAt": row.int("requested_at")?,
                    "expiresAt": row.int("expires_at")?,
                });
                if direction == "outbound" {
                    let subscribers: Vec<Value> = tx
                        .query(
                            "SELECT s.user_id, u.username, s.trigger_reason, s.trigger_target \
                             FROM peer_approval_subscribers s INNER JOIN users u ON u.id = s.user_id \
                             WHERE s.request_id = ?",
                            params![&id],
                        )?
                        .iter()
                        .map(|sub| {
                            Ok(json!({
                                "userId": sub.text("user_id")?,
                                "username": sub.text("username")?,
                                "triggerReason": sub.text("trigger_reason")?,
                                "triggerTarget": sub.text("trigger_target")?,
                            }))
                        })
                        .collect::<DbResult<_>>()?;
                    if let Some(fields) = request.as_object_mut() {
                        fields.insert("subscribers".to_owned(), subscribers.into());
                    }
                }
                requests.push(request);
            }
            Ok(requests)
        })
        .await?;
    Ok(send(200, json!({"requests": requests})))
}

async fn admin_approval(
    state: &AppState,
    user: &AuthUser,
    id: &str,
) -> Result<Result<Approval, Response>, ApiError> {
    if let Some(refusal) = require_admin(state, user).await? {
        return Ok(Err(refusal));
    }
    let id = id.to_owned();
    Ok(state
        .run(move |tx, _| Ok(approval(tx, &id)?))
        .await?
        .ok_or_else(|| fail(404, "Approval request not found")))
}

async fn delete_approval(state: &AppState, id: &str) -> Result<(), ApiError> {
    let id = id.to_owned();
    state
        .run(move |tx, _| {
            tx.execute(
                "DELETE FROM peer_approval_subscribers WHERE request_id = ?",
                params![&id],
            )?;
            tx.execute(
                "DELETE FROM peer_approval_requests WHERE id = ?",
                params![&id],
            )?;
            Ok(())
        })
        .await
}

/// Рукопожатие по одобренному запросу (`runApprovalHandshake`).
async fn approval_handshake(
    state: &AppState,
    request: &Approval,
) -> Result<Result<Shake, Response>, ApiError> {
    let Some(_claim) = AdminClaim::take(state, &request.origin) else {
        return Ok(Err(fail(409, IN_PROGRESS)));
    };
    let started_at = now_ms();
    let prepared =
        handshake::prepare_admin_row(state, &request.origin, true, request.instance_name.clone())
            .await?;
    let (peer_id, secret, on_failure): (String, String, OnFailure) = match prepared {
        Prepared::Busy(text) => return Ok(Err(fail(409, text))),
        Prepared::AlreadyActive(peer_id) => {
            return Ok(Ok(Shake::Active {
                peer_id,
                verified: false,
            }));
        }
        Prepared::Ready {
            peer_id,
            secret,
            on_failure,
        } => (peer_id, secret, on_failure),
    };
    let outcome = handshake::run_outbound(
        state,
        &Outbound {
            peer_id,
            origin: request.origin.clone(),
            secret,
            token: if request.outbound {
                None
            } else {
                request.token.clone()
            },
            trust: Trust::Asserted,
            activation: "approval_handshake",
            started_at,
            on_failure,
        },
    )
    .await?;
    Ok(Ok(outcome))
}

/// Ответ на неудачное рукопожатие по запросу (`failureReply`).
async fn approval_failure(
    state: &AppState,
    outcome: Shake,
    outbound: bool,
) -> Result<Response, ApiError> {
    let peer = public_peer(state, outcome.peer_id()).await?;
    let with_peer = |mut body: Value| {
        if let (Some(peer), Some(fields)) = (&peer, body.as_object_mut()) {
            fields.insert("peer".to_owned(), peer.clone());
        }
        body
    };
    Ok(match outcome {
        Shake::Failed {
            error,
            timed_out,
            http_status,
            ..
        } => {
            let code = if timed_out {
                504
            } else if http_status.is_none() && outbound {
                503
            } else {
                502
            };
            let mut body = json!({"error": error, "statusCode": code});
            if let (Some(status), Some(fields)) = (http_status, body.as_object_mut()) {
                fields.insert("remoteStatus".to_owned(), status.into());
            }
            send(code, body)
        }
        Shake::Rejected { error, .. } => {
            send(502, with_peer(json!({"error": error, "statusCode": 502})))
        }
        Shake::NeedsAttention { .. } => send(
            502,
            with_peer(json!({
                "error": "The remote accepted, but the new peering could not be verified; the peer needs attention",
                "statusCode": 502,
            })),
        ),
        _ => fail(409, "This peer was revoked meanwhile"),
    })
}

const ALSO_QUEUED: &str =
    "Remote instance also requires admin approval. Your request has been queued on their side.";

/// `POST /api/federation/approval-requests/:id/approve`.
pub async fn approve_request(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(_): JsonBody,
) -> Outcome {
    let request = match admin_approval(&state, &user, &id).await? {
        Ok(request) => request,
        Err(refusal) => return Ok(refusal),
    };
    let outcome = match approval_handshake(&state, &request).await? {
        Ok(outcome) => outcome,
        Err(refusal) => return Ok(refusal),
    };
    let peer = public_peer(&state, outcome.peer_id()).await?;
    let with_peer = |mut body: Value| {
        if let (Some(peer), Some(fields)) = (&peer, body.as_object_mut()) {
            fields.insert("peer".to_owned(), peer.clone());
        }
        body
    };
    match (&outcome, request.outbound) {
        // Исходящий запрос закрывает сама активация: подписчики получают
        // уведомление вместе с ней.
        (Shake::Active { .. }, true) => Ok(send(
            200,
            with_peer(json!({"success": true, "peerStatus": "active"})),
        )),
        (Shake::Awaiting(_), true) => Ok(send(
            200,
            with_peer(json!({
                "success": true,
                "peerStatus": "awaiting_approval",
                "awaitingRemoteApproval": true,
                "message": ALSO_QUEUED,
            })),
        )),
        (Shake::Active { .. }, false) => {
            delete_approval(&state, &request.id).await?;
            Ok(send(200, with_peer(json!({"success": true}))))
        }
        (Shake::Awaiting(_), false) => {
            delete_approval(&state, &request.id).await?;
            Ok(send(
                200,
                json!({"success": true, "awaitingRemoteApproval": true, "message": ALSO_QUEUED}),
            ))
        }
        _ => approval_failure(&state, outcome, request.outbound).await,
    }
}

/// Отказ во входящем запросе: сначала весть той стороне, потом запись
/// (`handleInboundDeny`).
async fn deny_inbound(state: &AppState, request: Approval) -> Outcome {
    let Some(secret) = request.secret.clone() else {
        return Ok(fail(
            500,
            "Inbound approval request is missing hmacSecret — cannot deliver /peer/denied notification.",
        ));
    };
    let body = json!({
        "origin": our_origin(&state.config),
        "reason": "denied_by_admin",
        "message": "Request denied by admin",
    });
    let sent = fetch::signed_post(
        &state.config,
        &request.origin,
        "/api/federation/peer/denied",
        &body,
        &secret,
        Trust::Asserted,
    )
    .await;
    let delivered = match sent {
        Ok(answer) => answer.ok(),
        Err(FetchError::Timeout | FetchError::Failed(_)) => false,
    };
    if !delivered {
        return Ok(fail(
            502,
            "Denial notification could not be delivered to the remote instance. The request is still pending — you can retry or wait for it to expire.",
        ));
    }
    let app = state.clone();
    let effects = state
        .run(move |tx, out| {
            let effects = match peers::by_origin(tx, &request.origin)? {
                None => {
                    let new = NewPeer {
                        origin: request.origin.clone(),
                        secret,
                        initiated_by: "admin",
                        instance_name: request.instance_name.clone(),
                        status: "rejected",
                        reason: Some("denied_by_local_admin"),
                        ..NewPeer::default()
                    };
                    peers::insert(tx, &app.ids, &new)?.map(|(_, effects)| effects)
                }
                Some(existing) => {
                    let change = Transition::new(
                        &["pending", "awaiting_approval", "rejected"],
                        "rejected",
                        "local_admin_denied",
                    )
                    .reason("denied_by_local_admin");
                    match peers::apply(tx, &existing.id, &change)? {
                        peers::Moved::Applied { effects, .. } => Some(*effects),
                        peers::Moved::Refused(_) => None,
                    }
                }
            };
            peers::to_admins(
                tx,
                &app.hub,
                out,
                &json!({"type": "federation_peers_changed"}),
            )?;
            tx.execute(
                "DELETE FROM peer_approval_requests WHERE id = ?",
                params![&request.id],
            )?;
            Ok(effects)
        })
        .await?;
    if let Some(effects) = effects {
        peers::run_effects(state, effects).await;
    }
    Ok(send(200, json!({"success": true})))
}

/// Отказ в исходящем запросе: уведомления просившим (`handleOutboundDeny`).
async fn deny_outbound(state: &AppState, request: Approval) -> Outcome {
    let app = state.clone();
    state
        .run(move |tx, out| {
            let subscribers = tx.query(
                "SELECT user_id, trigger_reason, trigger_target FROM peer_approval_subscribers \
                 WHERE request_id = ?",
                params![&request.id],
            )?;
            let now = now_ms();
            for sub in subscribers {
                let user = sub.text("user_id")?;
                tx.execute(
                    "INSERT INTO peer_approval_notifications (id, user_id, kind, peer_origin, \
                     trigger_reason, trigger_target, created_at, read_at) \
                     VALUES (?, ?, 'denied', ?, ?, ?, ?, NULL)",
                    params![
                        app.ids.next(),
                        &user,
                        &request.origin,
                        sub.text("trigger_reason")?,
                        sub.text("trigger_target")?,
                        now
                    ],
                )?;
                out.to_user(
                    &user,
                    &json!({"type": "peering_notification_received", "kind": "denied"}),
                );
                out.to_user(&user, &json!({"type": "peering_subscription_changed"}));
            }
            tx.execute(
                "DELETE FROM peer_approval_subscribers WHERE request_id = ?",
                params![&request.id],
            )?;
            tx.execute(
                "DELETE FROM peer_approval_requests WHERE id = ?",
                params![&request.id],
            )?;
            peers::to_admins(
                tx,
                &app.hub,
                out,
                &json!({"type": "federation_peers_changed"}),
            )?;
            Ok(())
        })
        .await?;
    Ok(send(200, json!({"success": true})))
}

/// `POST /api/federation/approval-requests/:id/deny`.
pub async fn deny_request(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(_): JsonBody,
) -> Outcome {
    let request = match admin_approval(&state, &user, &id).await? {
        Ok(request) => request,
        Err(refusal) => return Ok(refusal),
    };
    if request.outbound {
        deny_outbound(&state, request).await
    } else {
        deny_inbound(&state, request).await
    }
}

/// `GET /api/federation/peering-subscriptions`: просьбы пользователя о пиринге.
pub async fn subscriptions(State(state): State<AppState>, user: AuthUser) -> Outcome {
    let listed = state
        .run(move |tx, _| {
            tx.query(
                "SELECT s.id, s.request_id, r.origin, r.instance_name, s.trigger_reason, \
                 s.trigger_target, s.created_at FROM peer_approval_subscribers s \
                 INNER JOIN peer_approval_requests r ON r.id = s.request_id \
                 WHERE s.user_id = ? ORDER BY s.created_at DESC",
                params![&user.id],
            )?
            .iter()
            .map(|row| {
                Ok(json!({
                    "id": row.text("id")?,
                    "requestId": row.text("request_id")?,
                    "peerOrigin": row.text("origin")?,
                    "peerInstanceName": row.opt_text("instance_name")?,
                    "triggerReason": row.text("trigger_reason")?,
                    "triggerTarget": row.text("trigger_target")?,
                    "createdAt": row.int("created_at")?,
                }))
            })
            .collect::<DbResult<Vec<Value>>>()
            .map_err(ApiError::from)
        })
        .await?;
    Ok(send(200, json!({"subscriptions": listed})))
}

/// `DELETE /api/federation/peering-subscriptions/:id`.
pub async fn cancel_subscription(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> Outcome {
    let app = state.clone();
    state
        .run(move |tx, out| {
            let Some(sub) = tx.query_one(
                "SELECT user_id, request_id FROM peer_approval_subscribers WHERE id = ?",
                params![&id],
            )?
            else {
                return Ok(fail(404, "subscription_not_found"));
            };
            if sub.text("user_id")? != user.id {
                return Ok(fail(403, "forbidden"));
            }
            let request = sub.text("request_id")?;
            tx.execute(
                "DELETE FROM peer_approval_subscribers WHERE id = ?",
                params![&id],
            )?;
            let remaining = tx.query_one(
                "SELECT id FROM peer_approval_subscribers WHERE request_id = ? LIMIT 1",
                params![&request],
            )?;
            if remaining.is_none() {
                tx.execute(
                    "DELETE FROM peer_approval_requests WHERE id = ?",
                    params![&request],
                )?;
                peers::to_admins(
                    tx,
                    &app.hub,
                    out,
                    &json!({"type": "federation_peers_changed"}),
                )?;
            }
            out.to_user(&user.id, &json!({"type": "peering_subscription_changed"}));
            Ok(send(200, json!({"success": true})))
        })
        .await
}

/// Параметры списка уведомлений.
#[derive(Debug, serde::Deserialize)]
pub struct NotificationsQuery {
    unread: Option<String>,
}

fn notification(row: &Row) -> DbResult<Value> {
    Ok(json!({
        "id": row.text("id")?,
        "kind": row.text("kind")?,
        "peerOrigin": row.text("peer_origin")?,
        "triggerReason": row.text("trigger_reason")?,
        "triggerTarget": row.text("trigger_target")?,
        "createdAt": row.int("created_at")?,
        "readAt": row.opt_int("read_at")?,
    }))
}

/// `GET /api/federation/peering-notifications`.
pub async fn notifications(
    State(state): State<AppState>,
    user: AuthUser,
    Query(query): Query<NotificationsQuery>,
) -> Outcome {
    let unread = query.unread.as_deref() == Some("1");
    let listed = state
        .run(move |tx, _| {
            let filter = if unread { " AND read_at IS NULL" } else { "" };
            tx.query(
                &format!(
                    "SELECT id, kind, peer_origin, trigger_reason, trigger_target, created_at, \
                     read_at FROM peer_approval_notifications WHERE user_id = ?{filter} \
                     ORDER BY created_at DESC"
                ),
                params![&user.id],
            )?
            .iter()
            .map(notification)
            .collect::<DbResult<Vec<Value>>>()
            .map_err(ApiError::from)
        })
        .await?;
    Ok(send(200, json!({"notifications": listed})))
}

/// `POST /api/federation/peering-notifications/:id/read`.
pub async fn read_notification(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(_): JsonBody,
) -> Outcome {
    state
        .run(move |tx, _| {
            let Some(row) = tx.query_one(
                "SELECT user_id FROM peer_approval_notifications WHERE id = ?",
                params![&id],
            )?
            else {
                return Ok(fail(404, "notification_not_found"));
            };
            if row.text("user_id")? != user.id {
                return Ok(fail(403, "forbidden"));
            }
            tx.execute(
                "UPDATE peer_approval_notifications SET read_at = ? WHERE id = ?",
                params![now_ms(), &id],
            )?;
            Ok(send(200, json!({"success": true})))
        })
        .await
}

/// `POST /api/federation/peering-notifications/read-all`.
pub async fn read_all_notifications(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(_): JsonBody,
) -> Outcome {
    let changed = state
        .run(move |tx, _| {
            Ok(tx.execute(
                "UPDATE peer_approval_notifications SET read_at = ? \
                 WHERE user_id = ? AND read_at IS NULL",
                params![now_ms(), &user.id],
            )?)
        })
        .await?;
    Ok(send(200, json!({"success": true, "count": changed})))
}
