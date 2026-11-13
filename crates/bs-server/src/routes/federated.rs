//! Аккаунт пользователя и другие инстансы: доказательство для привязки,
//! повторная привязка отделённого аккаунта, учётные данные для инстансов,
//! удаление своих копий на них (`routes/auth.ts`, `routes/users.ts`,
//! `routes/federation/handlers/attach.ts`).

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use url::Url;

use super::federation::{fail, send};
use crate::auth::AuthUser;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ClientIp, JsonBody};
use crate::federation::identity::{self, Claim, REPLICATED_PASSWORD_HASH, handle_from_hint};
use crate::federation::reset::domain_of;
use crate::federation::{fetch, lookup, our_origin, peers};
use crate::federation_auth::{self, canonical_home_instance, normalize_origin};
use crate::params;
use crate::ratelimit::Limit;
use crate::routes::users::sanitize_user;
use crate::state::{AppState, now_ms};

const ATTACH_PROOF_LIMIT: Limit = Limit {
    name: "auth.attach-proof",
    max: 5,
    window: Duration::from_mins(15),
};
const REATTACH_LIMIT: Limit = Limit {
    name: "users.reattach",
    max: 5,
    window: Duration::from_mins(15),
};
const IDENTITY_DELETE_LIMIT: Limit = Limit {
    name: "users.federation-identity.delete",
    max: 5,
    window: Duration::from_mins(15),
};

fn is_hex64(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `POST /api/auth/attach-proof`: одноразовое доказательство владения
/// аккаунтом для привязки на другом инстансе.
pub async fn attach_proof(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    state.limiter.check(ATTACH_PROOF_LIMIT, ip)?;
    let raw = body.get("targetDomain").and_then(Value::as_str);
    let Some(raw) = raw
        .filter(|raw| !crate::util::js_trim(raw).is_empty() && crate::util::utf16_len(raw) <= 255)
    else {
        return Err(ApiError::bad_request("target_domain_required"));
    };
    let lowered = crate::util::js_trim(raw).to_lowercase();
    let bare = lowered
        .strip_prefix("https://")
        .or_else(|| lowered.strip_prefix("http://"))
        .unwrap_or(&lowered);
    let target = federation_auth::strip_trailing_slashes(bare).to_owned();
    if target.is_empty() {
        return Err(ApiError::bad_request("target_domain_required"));
    }
    if user.home_instance.as_deref().is_some_and(|h| !h.is_empty()) {
        return Err(ApiError::forbidden("native_account_required"));
    }
    let token = hex::encode(rand::random::<[u8; 32]>());
    let issued = token.clone();
    state
        .run(move |tx, _| {
            let now = now_ms();
            tx.execute(
                "DELETE FROM federation_attach_proofs WHERE expires_at < ?",
                params![now],
            )?;
            tx.execute(
                "INSERT INTO federation_attach_proofs (token, home_user_id, target_domain, created_at, \
                 expires_at, used_at) VALUES (?, ?, ?, ?, ?, NULL)",
                params![&issued, &user.id, &target, now, now + 60_000],
            )?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({"token": token})))
}

/// Адрес инстанса для учётных данных (`canonicalRemoteOrigin`).
#[must_use]
pub fn canonical_remote_origin(value: &str) -> Option<String> {
    let canonical = canonical_home_instance(Some(value))?;
    if canonical.len() > 512 {
        return None;
    }
    let url = Url::parse(&canonical).ok()?;
    if !matches!(url.scheme(), "https" | "http") {
        return None;
    }
    let host = url.host_str()?;
    if host.len() > 253
        || !host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
    {
        return None;
    }
    Some(url.origin().ascii_serialization().to_lowercase())
}

/// `POST /api/users/@me/federation-credential`: секрет для входа на другой
/// инстанс.
pub async fn federation_credential(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let Some(origin) = body
        .get("origin")
        .and_then(Value::as_str)
        .filter(|o| !o.is_empty())
    else {
        return Err(ApiError::bad_request("origin_required"));
    };
    let Some(target) = canonical_remote_origin(origin) else {
        return Err(ApiError::bad_request("origin_invalid"));
    };
    if normalize_origin(Some(&target)) == normalize_origin(Some(&our_origin(&state.config))) {
        return Err(ApiError::bad_request("federation_credential_remote_only"));
    }
    let mark = body.get("markProvisioned") == Some(&Value::Bool(true));
    state
        .run(move |tx, _| {
            let row = crate::routes::users::user_row(tx, &user.id)?;
            let Some(row) = row.filter(|row| row.opt_flag("is_deleted").ok().flatten() != Some(true))
            else {
                return Err(ApiError::unauthorized("account_deleted"));
            };
            if row.opt_text("home_instance")?.is_some_and(|h| !h.is_empty())
                && row.opt_flag("federation_home_orphaned")? != Some(true)
            {
                return Err(ApiError::conflict("federation_credential_home_only"));
            }
            let now = now_ms();
            let secret = base64::Engine::encode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                rand::random::<[u8; 32]>(),
            );
            tx.execute(
                "INSERT INTO user_federation_credentials (user_id, origin, secret, created_at, provisioned_at) \
                 VALUES (?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
                params![&user.id, &target, secret, now, mark.then_some(now)],
            )?;
            if mark {
                tx.execute(
                    "UPDATE user_federation_credentials SET provisioned_at = ? \
                     WHERE user_id = ? AND origin = ? AND provisioned_at IS NULL",
                    params![now, &user.id, &target],
                )?;
            }
            let Some(stored) = tx.query_one(
                "SELECT origin, secret, provisioned_at FROM user_federation_credentials \
                 WHERE user_id = ? AND origin = ?",
                params![&user.id, &target],
            )?
            else {
                return Err(ApiError::new(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "federation_credential_failed",
                ));
            };
            Ok(Json(json!({
                "origin": stored.text("origin")?,
                "secret": stored.text("secret")?,
                "provisioned": stored.opt_int("provisioned_at")?.is_some(),
            })))
        })
        .await
}

async fn delete_on(
    state: &AppState,
    user_id: &str,
    origin: &str,
    mode: &str,
) -> Map<String, Value> {
    let mut result = Map::new();
    if mode == "leave" {
        result.insert("success".into(), json!(true));
    } else {
        let at = origin.to_owned();
        let peer = state
            .run(move |tx, _| Ok(peers::by_origin(tx, &at)?))
            .await
            .ok()
            .flatten();
        let Some(peer) = peer.filter(|peer| peer.status == "active") else {
            result.insert("success".into(), json!(false));
            result.insert("error".into(), json!("no_active_peer"));
            return result;
        };
        let ours = our_origin(&state.config);
        let body = crate::jsjson::stringify(&json!({
            "homeUserId": user_id,
            "homeInstance": domain_of(&ours),
            "mode": mode,
        }));
        let headers = federation_auth::headers(&body, &peer.secret, &ours, now_ms());
        let answer = fetch::request_with(
            &state.config,
            reqwest::Method::DELETE,
            origin,
            "/api/federation/identity",
            Some(body),
            &headers,
            fetch::Trust::Approved,
            Duration::from_secs(15),
        )
        .await;
        match answer {
            Ok(answer) => {
                let Ok(data) = serde_json::from_slice::<Value>(&answer.body) else {
                    result.insert("success".into(), json!(false));
                    result.insert("error".into(), json!("unreachable"));
                    return result;
                };
                if answer.ok() {
                    result.insert("success".into(), json!(true));
                } else if data.get("error").and_then(Value::as_str) == Some("owns_spaces") {
                    result.insert("success".into(), json!(false));
                    result.insert("error".into(), json!("owns_spaces"));
                    result.insert(
                        "ownedSpaces".into(),
                        data.get("ownedSpaces").cloned().unwrap_or(Value::Null),
                    );
                } else {
                    let error = data
                        .get("error")
                        .filter(|e| crate::federation::inbound::truthy(e))
                        .cloned()
                        .unwrap_or_else(|| json!(format!("HTTP {}", answer.status)));
                    result.insert("success".into(), json!(false));
                    result.insert("error".into(), error);
                }
            }
            Err(fetch::FetchError::Timeout) => {
                result.insert("success".into(), json!(false));
                result.insert("error".into(), json!("timeout"));
            }
            Err(fetch::FetchError::Failed(_)) => {
                result.insert("success".into(), json!(false));
                result.insert("error".into(), json!("unreachable"));
            }
        }
    }
    result
}

/// `POST /api/users/@me/federation-identity/delete`: удалить свои копии на
/// других инстансах.
pub async fn federation_identity_delete(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    state.limiter.check(IDENTITY_DELETE_LIMIT, ip)?;
    let mode = body.get("mode").and_then(Value::as_str);
    let Some(mode) = mode.filter(|mode| matches!(*mode, "leave" | "soft" | "full")) else {
        return Err(ApiError::bad_request("validation_failed")
            .detail("field", "mode")
            .detail("reason", "must be \"leave\", \"soft\" or \"full\""));
    };
    let origins: Option<Vec<String>> = body
        .get("origins")
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
        .and_then(|list| list.iter().map(|o| o.as_str().map(str::to_owned)).collect());
    let Some(origins) = origins else {
        return Err(ApiError::bad_request("validation_failed")
            .detail("field", "origins")
            .detail("reason", "must be a non-empty array of strings"));
    };
    let mut results = Map::new();
    let mut succeeded = Vec::new();
    for origin in &origins {
        let mut result = delete_on(&state, &user.id, origin, mode).await;
        if result.get("success") == Some(&Value::Bool(true)) {
            let (owner, at, mode) = (user.id.clone(), origin.clone(), mode.to_owned());
            let cleaned = state
                .run(move |tx, _| {
                    tx.execute(
                        "DELETE FROM user_federation_registry WHERE user_id = ? AND origin = ?",
                        params![&owner, &at],
                    )?;
                    if mode != "leave" {
                        let canonical = canonical_remote_origin(&at);
                        tx.execute(
                            "DELETE FROM user_federation_credentials WHERE user_id = ? AND (origin = ? OR origin = ?)",
                            params![&owner, &at, canonical.unwrap_or_else(|| at.clone())],
                        )?;
                    }
                    tx.execute(
                        "UPDATE users SET federation_registry_updated_at = ? WHERE id = ?",
                        params![now_ms(), &owner],
                    )?;
                    Ok(())
                })
                .await;
            if cleaned.is_err() {
                result = Map::new();
                result.insert("success".into(), json!(false));
                result.insert("error".into(), json!("Unknown error"));
            } else {
                succeeded.push(origin.clone());
            }
        }
        results.insert(origin.clone(), Value::Object(result));
    }
    if !succeeded.is_empty() {
        let owner = user.id.clone();
        state
            .run(move |tx, _| {
                let row = tx.query_one(
                    "SELECT replicated_instances FROM users WHERE id = ?",
                    params![&owner],
                )?;
                if let Some(list) = row
                    .map(|row| row.opt_text("replicated_instances"))
                    .transpose()?
                    .flatten()
                    .filter(|list| !list.is_empty())
                    && let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&list)
                {
                    let kept: Vec<Value> = items
                        .into_iter()
                        .filter(|item| {
                            !item
                                .get("origin")
                                .and_then(Value::as_str)
                                .is_some_and(|o| succeeded.iter().any(|s| s == o))
                        })
                        .collect();
                    tx.execute(
                        "UPDATE users SET replicated_instances = ? WHERE id = ?",
                        params![Value::Array(kept).to_string(), &owner],
                    )?;
                }
                Ok(())
            })
            .await?;
    }
    Ok(Json(json!({"results": results})))
}

// ─── Повторная привязка (`/api/users/@me/reattach`) ─────────────────────────

fn merge_stub(tx: &mut dyn Tx, stub: &str, target: &str) -> DbResult<()> {
    let statements: [(&str, bool); 18] = [
        (
            "DELETE FROM dm_members WHERE user_id = ? AND dm_channel_id IN (SELECT dm_channel_id FROM dm_members WHERE user_id = ?)",
            false,
        ),
        ("UPDATE dm_members SET user_id = ? WHERE user_id = ?", true),
        ("UPDATE dm_messages SET user_id = ? WHERE user_id = ?", true),
        ("UPDATE messages SET user_id = ? WHERE user_id = ?", true),
        (
            "UPDATE attachments SET uploader_id = ? WHERE uploader_id = ?",
            true,
        ),
        (
            "DELETE FROM dm_reactions WHERE user_id = ? AND EXISTS (SELECT 1 FROM dm_reactions r2 WHERE r2.user_id = ? \
          AND r2.dm_message_id = dm_reactions.dm_message_id AND r2.emoji = dm_reactions.emoji)",
            false,
        ),
        (
            "UPDATE dm_reactions SET user_id = ? WHERE user_id = ?",
            true,
        ),
        (
            "DELETE FROM reactions WHERE user_id = ? AND EXISTS (SELECT 1 FROM reactions r2 WHERE r2.user_id = ? \
          AND r2.message_id = reactions.message_id AND r2.emoji = reactions.emoji)",
            false,
        ),
        ("UPDATE reactions SET user_id = ? WHERE user_id = ?", true),
        (
            "DELETE FROM friends WHERE user_id = ? AND friend_id IN (SELECT friend_id FROM friends WHERE user_id = ?)",
            false,
        ),
        (
            "DELETE FROM friends WHERE friend_id = ? AND user_id IN (SELECT user_id FROM friends WHERE friend_id = ?)",
            false,
        ),
        ("UPDATE friends SET user_id = ? WHERE user_id = ?", true),
        ("UPDATE friends SET friend_id = ? WHERE friend_id = ?", true),
        ("DELETE FROM friends WHERE user_id = friend_id", false),
        (
            "UPDATE friend_requests SET from_id = ? WHERE from_id = ?",
            true,
        ),
        ("UPDATE friend_requests SET to_id = ? WHERE to_id = ?", true),
        ("DELETE FROM friend_requests WHERE from_id = to_id", false),
        ("", false),
    ];
    for (sql, target_first) in statements {
        if sql.is_empty() {
            continue;
        }
        let count = sql.matches('?').count();
        match (count, target_first) {
            (0, _) => tx.execute(sql, params![])?,
            (_, true) => tx.execute(sql, params![target, stub])?,
            (_, false) => tx.execute(sql, params![stub, target])?,
        };
    }
    let channels: Vec<String> = tx
        .query(
            "SELECT channel_id FROM read_states WHERE user_id = ?",
            params![stub],
        )?
        .iter()
        .map(|row| row.text("channel_id"))
        .collect::<DbResult<_>>()?;
    for channel in channels {
        crate::dm::keep_newer_read_pointer(tx, (stub, &channel), (target, &channel))?;
    }
    tx.execute(
        "UPDATE dm_channels SET owner_id = ? WHERE owner_id = ?",
        params![target, stub],
    )?;
    tx.execute("DELETE FROM users WHERE id = ?", params![stub])?;
    Ok(())
}

enum Rebind {
    Done(Vec<crate::dm::Reconciled>),
    Conflict,
}

/// `POST /api/users/@me/reattach`: отделённый аккаунт снова привязывается к
/// своему дому по доказательству.
#[allow(clippy::too_many_lines, reason = "порядок шагов оригинала")]
pub async fn reattach(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Response> {
    state.limiter.check(REATTACH_LIMIT, ip)?;
    let Some(token) = body
        .get("token")
        .and_then(Value::as_str)
        .filter(|t| is_hex64(t))
        .map(str::to_owned)
    else {
        return Ok(fail(400, "token is required (64-char hex)"));
    };
    let owner = user.id.clone();
    let found = state
        .run(move |tx, _| {
            let row = crate::routes::users::user_row(tx, &owner)?;
            let peers = tx.query("SELECT * FROM federation_peers", params![])?;
            Ok((row, peers))
        })
        .await?;
    let (detached, peer_rows) = found;
    let Some(detached) =
        detached.filter(|row| row.opt_flag("is_deleted").ok().flatten() != Some(true))
    else {
        return Ok(fail(404, "User not found"));
    };
    let former_home = detached
        .opt_text("detached_home_instance")?
        .filter(|h| !h.is_empty());
    let Some(former_home) = former_home
        .filter(|_| detached.opt_flag("federation_home_orphaned").ok().flatten() == Some(true))
    else {
        return Ok(fail(403, "Only detached accounts can re-attach"));
    };
    let home_domain = domain_of(&former_home).to_lowercase();
    let mut peer = None;
    for row in &peer_rows {
        if domain_of(&row.text("origin")?).to_lowercase() == home_domain
            && row.opt_text("status")?.as_deref() == Some("active")
        {
            peer = Some((row.text("origin")?, row.text("hmac_secret")?));
            break;
        }
    }
    let Some((peer_origin, secret)) = peer else {
        return Ok(fail(409, "Home instance is not an active peer"));
    };
    let Some((home_user_id, username)) =
        lookup::verify_attach_proof(&state.config, &peer_origin, &secret, &token).await
    else {
        return Ok(fail(401, "Attach proof could not be verified"));
    };
    let Some(handle) = handle_from_hint(Some(&username)) else {
        return Ok(fail(401, "Attach proof could not be verified"));
    };
    let detached_id = detached.text("id")?;
    let (id, verified, home, domain, held_handle) = (
        detached_id.clone(),
        home_user_id.clone(),
        former_home.clone(),
        home_domain.clone(),
        handle.clone(),
    );
    let outcome = state
        .run(move |tx, out| {
            let existing = tx.query_one(
                "SELECT id, password_hash FROM users WHERE home_user_id = ? \
                 AND lower(replace(replace(coalesce(home_instance, ''), 'https://', ''), 'http://', '')) = ? \
                 AND is_deleted = 0 AND id <> ?",
                params![&verified, &domain, &id],
            )?;
            if let Some(existing) = &existing
                && existing.opt_text("password_hash")?.as_deref() != Some(REPLICATED_PASSWORD_HASH)
            {
                tracing::error!(identity = verified, holder = existing.text("id")?, "повторная привязка: идентичность занята");
                return Ok(Rebind::Conflict);
            }
            if let Some(existing) = existing {
                merge_stub(tx, &existing.text("id")?, &id)?;
            }
            tx.execute(
                "UPDATE dm_channels SET owner_home_user_id = ?, owner_home_instance = ? \
                 WHERE owner_id = ? AND owner_home_user_id IS NOT NULL",
                params![&verified, canonical_home_instance(Some(&home)), &id],
            )?;
            tx.execute(
                "UPDATE users SET home_instance = ?, home_user_id = ?, detached_home_instance = NULL, \
                 detached_home_user_id = NULL, federation_home_orphaned = 0, profile_updated_at = NULL \
                 WHERE id = ?",
                params![&home, &verified, &id],
            )?;
            let Some(row) = crate::routes::users::user_row(tx, &id)? else {
                return Ok(Rebind::Conflict);
            };
            let moved = match identity::claim_handle_name(tx, &row, &held_handle, &domain)? {
                Claim::Held(holder) => {
                    // Откат всей транзакции: аккаунт остаётся отделённым.
                    return Err(crate::error::ApiError::conflict("reattach_handle_taken")
                        .extra("holder", holder));
                }
                Claim::Claimed(_, moved) => moved,
            };
            let mut reconciled = Vec::new();
            for channel in tx.query(
                "SELECT c.id FROM dm_channels c WHERE c.deleted_at IS NULL AND c.federated_id IS NOT NULL \
                 AND EXISTS (SELECT 1 FROM dm_members m WHERE m.dm_channel_id = c.id AND m.user_id = ?) \
                 AND (SELECT count(*) FROM dm_members m2 WHERE m2.dm_channel_id = c.id) = 2",
                params![&id],
            )? {
                if let Some(result) = crate::dm::reconcile_federated_id(tx, &channel.text("id")?)? {
                    reconciled.push(result);
                }
            }
            if let Some(moved) = &moved {
                identity::announce_user_updated(tx, out, moved)?;
            }
            Ok(Rebind::Done(reconciled))
        })
        .await;
    let reconciled = match outcome {
        Ok(Rebind::Done(reconciled)) => reconciled,
        Ok(Rebind::Conflict) => {
            return Ok(fail(
                409,
                "The new identity is already bound to another account on this instance",
            ));
        }
        Err(error) if error.code() == "reattach_handle_taken" => {
            tracing::warn!(
                user = detached_id,
                handle,
                "повторная привязка отклонена: имя занято"
            );
            return Err(ApiError::conflict("reattach_handle_taken"));
        }
        Err(error) => return Err(error),
    };
    if let lookup::Lookup::Found {
        home_user_id: answered,
        username,
        profile,
    } = lookup::by_home_id(&state, &peer_origin, &home_user_id).await
        && answered == home_user_id
    {
        let id = detached_id.clone();
        let source: std::sync::Arc<str> = peer_origin.as_str().into();
        let applied = crate::federation::inbound::with_assets(
            &state,
            source.clone(),
            source,
            crate::federation::inbound::Delivery::Live,
            move |cx| {
                if let Some(row) = crate::routes::users::user_row(cx.tx, &id)? {
                    let profile = lookup::home_profile(&username, &profile);
                    let origin = cx.source.to_owned();
                    crate::federation::inbound::profile::apply_home_profile(
                        cx, &row, &profile, &origin,
                    )?;
                }
                Ok(())
            },
        )
        .await;
        if let Err(error) = applied {
            tracing::warn!(%error, "профиль от дома после привязки не записан");
        }
    }
    let id = detached_id.clone();
    let updated = state
        .run(move |tx, out| {
            let Some(updated) = crate::routes::users::user_row(tx, &id)? else {
                return Err(ApiError::not_found("user_not_found"));
            };
            let mut targets = crate::ws::profile_targets(tx, &id)?;
            if !targets.contains(&id) {
                targets.push(id.clone());
            }
            for target in targets {
                let event =
                    json!({"type": "user_updated", "user": sanitize_user(&updated, target == id)?});
                crate::broadcast::to_user(tx, out, &target, &event)?;
            }
            crate::dm::announce_reconciled(tx, out, &reconciled)?;
            Ok(updated)
        })
        .await?;
    tracing::info!(
        user = detached_id,
        home = home_domain,
        "аккаунт снова привязан к дому"
    );
    Ok(send(
        200,
        json!({"success": true, "user": sanitize_user(&updated, true)?}),
    )
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_origins() {
        assert_eq!(
            canonical_remote_origin("Y.Test/").as_deref(),
            Some("https://y.test")
        );
        assert_eq!(
            canonical_remote_origin("http://y.test:8080/x").as_deref(),
            Some("http://y.test:8080")
        );
        assert_eq!(canonical_remote_origin("https://y_x.test"), None);
        assert!(is_hex64(&"a".repeat(64)));
        assert!(!is_hex64("zz"));
    }
}
