//! Дружба между инстансами (`routes/federation/events/friends.ts`).

use serde_json::{Value, json};

use super::{Ctx, Step, accept, actor, present, profile, reject, timestamp};
use crate::db::{DbResult, Row};
use crate::federation::identity::{self, Hints, Resolved};
use crate::federation::relay::{claim_subject, friend_subject, record_subject, subject_stale};
use crate::federation_auth::normalize_origin;
use crate::params;
use crate::routes::users::sanitize_user;
use crate::state::now_ms;

fn homed_here(cx: &Ctx<'_>, who: &Value) -> bool {
    let home = who
        .get("homeInstance")
        .and_then(Value::as_str)
        .unwrap_or_default();
    identity::is_own_domain(&cx.app.config, &super::domain(home))
}

/// `true` — пара менялась здесь позже, событие пропускается (`claimFriendPair`).
fn pair_changed_later(cx: &mut Ctx<'_>, event: &Value, from: &Value, to: &Value) -> DbResult<bool> {
    let Some(subject) = friend_subject(&cx.app.config, from, to) else {
        return Ok(false);
    };
    Ok(!claim_subject(cx.tx, &subject, timestamp(event))?)
}

fn are_friends(cx: &mut Ctx<'_>, a: &str, b: &str) -> DbResult<bool> {
    Ok(cx
        .tx
        .query_one(
            "SELECT user_id FROM friends WHERE (user_id = ? AND friend_id = ?) OR (user_id = ? AND friend_id = ?)",
            params![a, b, b, a],
        )?
        .is_some())
}

fn pending(cx: &mut Ctx<'_>, from: &str, to: &str) -> DbResult<Option<Row>> {
    cx.tx.query_one(
        "SELECT id, from_id, to_id, status, created_at FROM friend_requests \
         WHERE from_id = ? AND to_id = ? AND status = 'pending'",
        params![from, to],
    )
}

fn created_at(friendship: &Value) -> i64 {
    Some(super::millis(friendship.get("createdAt")))
        .filter(|at| *at != 0)
        .unwrap_or_else(now_ms)
}

fn friend_json(user: &Row, added_at: i64) -> DbResult<Value> {
    let mut friend = sanitize_user(user, false)?;
    friend["addedAt"] = json!(added_at);
    Ok(friend)
}

struct Pair<'a> {
    friendship: &'a Value,
    from: &'a Value,
    to: &'a Value,
}

fn pair(event: &Value) -> Option<Pair<'_>> {
    let friendship = present(event, "friendship")?;
    Some(Pair {
        friendship,
        from: friendship.get("from").unwrap_or(&Value::Null),
        to: friendship.get("to").unwrap_or(&Value::Null),
    })
}

/// Запрос дружбы с другого инстанса (`processFriendRequestCreateEvent`).
pub fn request_create(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let Some(Pair {
        friendship,
        from,
        to,
    }) = pair(event)
    else {
        return reject("missing_friendship_payload");
    };
    if let Some(refusal) = cx.refusal(Some(from))? {
        return reject(refusal);
    }
    let (Some(from_actor), Some(to_actor)) = (actor(from), actor(to)) else {
        return reject("attribution_mismatch");
    };
    if from_actor.home_user_id == to_actor.home_user_id
        && normalize_origin(Some(&from_actor.home_instance))
            == normalize_origin(Some(&to_actor.home_instance))
    {
        return reject("self_target_invalid");
    }
    let from_profile = friendship.get("fromProfile");
    let hints = Hints::from_profile(from_profile);
    let Some(replica) = cx.replica(&from_actor, &hints)? else {
        return accept();
    };
    let from_user = match from_profile.filter(|p| super::truthy(p)) {
        Some(snapshot) => profile::hydrate(cx, replica.user, snapshot)?,
        None => replica.user,
    };
    let Resolved::Found(to_user) = cx.resolve(to)? else {
        return reject("recipient_not_found");
    };
    if pair_changed_later(cx, event, from, to)? {
        return accept();
    }
    let (from_id, to_id) = (from_user.text("id")?, to_user.text("id")?);
    if are_friends(cx, &from_id, &to_id)? {
        return accept();
    }
    if pending(cx, &from_id, &to_id)?.is_some() || pending(cx, &to_id, &from_id)?.is_some() {
        return accept();
    }
    let id = cx.app.ids.next();
    let now = created_at(friendship);
    cx.tx.execute(
        "INSERT INTO friend_requests (id, from_id, to_id, status, created_at) VALUES (?, ?, ?, 'pending', ?)",
        params![&id, &from_id, &to_id, now],
    )?;
    cx.to_user(
        &to_id,
        &json!({
            "type": "friend_request_received",
            "request": {
                "id": id,
                "fromId": from_id,
                "toId": to_id,
                "status": "pending",
                "createdAt": now,
                "user": sanitize_user(&from_user, false)?,
            },
        }),
    )?;
    accept()
}

/// Ответ на наш запрос дружбы (`processFriendRequestUpdateEvent`).
pub fn request_update(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let Some(Pair {
        friendship,
        from,
        to,
    }) = pair(event)
    else {
        return reject("missing_friendship_payload");
    };
    let Some(status) = present(friendship, "status") else {
        return reject("missing_friendship_payload");
    };
    if let Some(refusal) = cx.refusal(Some(to))? {
        return reject(refusal);
    }
    let Resolved::Found(from_user) = cx.resolve(from)? else {
        return reject("sender_not_found");
    };
    if pair_changed_later(cx, event, from, to)? {
        return accept();
    }
    let Resolved::Found(to_user) = cx.resolve(to)? else {
        return accept();
    };
    let (from_id, to_id) = (from_user.text("id")?, to_user.text("id")?);
    let Some(request) = pending(cx, &from_id, &to_id)? else {
        return accept();
    };
    let request_id = request.text("id")?;
    let now = created_at(friendship);
    let status_text = status
        .as_str()
        .map_or_else(|| status.to_string(), str::to_owned);
    cx.tx.execute(
        "UPDATE friend_requests SET status = ? WHERE id = ?",
        params![&status_text, &request_id],
    )?;
    let mut formed = false;
    if status_text == "accepted" && !are_friends(cx, &from_id, &to_id)? {
        cx.tx.execute(
            "INSERT INTO friends (user_id, friend_id, created_at) VALUES (?, ?, ?)",
            params![&from_id, &to_id, now],
        )?;
        formed = true;
    }
    if status_text == "accepted" {
        cx.to_user(
            &from_id,
            &json!({
                "type": "friend_request_accepted",
                "friend": friend_json(&to_user, now)?,
                "requestId": request_id,
            }),
        )?;
        if formed {
            crate::routes::social::exchange_friend_presence(
                cx.tx, cx.app, cx.out, &from_id, &to_id,
            )?;
        }
    } else if status_text == "declined" {
        cx.to_user(
            &from_id,
            &json!({"type": "friend_request_declined", "requestId": request_id, "userId": to_id}),
        )?;
    }
    accept()
}

/// Запрос дружбы отозван (`processFriendRequestCancelEvent`).
pub fn request_cancel(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let Some(Pair { from, to, .. }) = pair(event) else {
        return reject("missing_friendship_payload");
    };
    if let Some(refusal) = cx.refusal(Some(from))? {
        return reject(refusal);
    }
    let from_resolved = cx.resolve(from)?;
    if matches!(from_resolved, Resolved::Mismatch) {
        return reject("attribution_mismatch");
    }
    let to_resolved = cx.resolve(to)?;
    if pair_changed_later(cx, event, from, to)? {
        return accept();
    }
    let (Resolved::Found(from_user), Resolved::Found(to_user)) = (from_resolved, to_resolved)
    else {
        return accept();
    };
    let (from_id, to_id) = (from_user.text("id")?, to_user.text("id")?);
    let Some(request) = pending(cx, &from_id, &to_id)? else {
        return accept();
    };
    let request_id = request.text("id")?;
    cx.tx.execute(
        "DELETE FROM friend_requests WHERE id = ?",
        params![&request_id],
    )?;
    cx.to_user(
        &to_id,
        &json!({"type": "friend_request_cancelled", "requestId": request_id, "userId": from_id}),
    )?;
    accept()
}

/// Запрос принят на другом инстансе (`processFriendAddEvent`).
pub fn add(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let Some(Pair {
        friendship,
        from,
        to,
    }) = pair(event)
    else {
        return reject("missing_friendship_payload");
    };
    if let Some(refusal) = cx.refusal(Some(to))? {
        return reject(refusal);
    }
    let subject = friend_subject(&cx.app.config, from, to);
    let at = timestamp(event);
    if let Some(subject) = &subject
        && subject_stale(cx.tx, subject, at)?
    {
        return accept();
    }
    let from_resolved = cx.resolve(from)?;
    let to_resolved = cx.resolve(to)?;
    let both = match (from_resolved, to_resolved) {
        (Resolved::Found(a), Resolved::Found(b)) => Some((*a, *b)),
        _ => None,
    };
    if let Some((a, b)) = &both
        && are_friends(cx, &a.text("id")?, &b.text("id")?)?
    {
        if let Some(subject) = &subject {
            record_subject(cx.tx, subject, at)?;
        }
        return accept();
    }
    let request = match &both {
        Some((a, b)) => pending(cx, &a.text("id")?, &b.text("id")?)?,
        None => None,
    };
    let (Some((a, b)), Some(request)) = (both, request) else {
        tracing::warn!(
            source = cx.source,
            "friend_add без ожидающего запроса отклонён"
        );
        return reject("invalid_target");
    };
    let from_user = match friendship.get("fromProfile").filter(|p| super::truthy(p)) {
        Some(snapshot) => profile::hydrate(cx, a, snapshot)?,
        None => a,
    };
    let to_user = match friendship.get("toProfile").filter(|p| super::truthy(p)) {
        Some(snapshot) => profile::hydrate(cx, b, snapshot)?,
        None => b,
    };
    if pair_changed_later(cx, event, from, to)? {
        return accept();
    }
    let (from_id, to_id) = (from_user.text("id")?, to_user.text("id")?);
    let now = created_at(friendship);
    cx.tx.execute(
        "INSERT INTO friends (user_id, friend_id, created_at) VALUES (?, ?, ?)",
        params![&from_id, &to_id, now],
    )?;
    cx.tx.execute(
        "UPDATE friend_requests SET status = 'accepted' WHERE \
         ((from_id = ? AND to_id = ?) OR (from_id = ? AND to_id = ?)) AND status = 'pending'",
        params![&from_id, &to_id, &to_id, &from_id],
    )?;
    let (local, remote) = if homed_here(cx, from) {
        (&from_user, &to_user)
    } else {
        (&to_user, &from_user)
    };
    cx.to_user(
        &local.text("id")?,
        &json!({
            "type": "friend_request_accepted",
            "friend": friend_json(remote, now)?,
            "requestId": request.text("id")?,
        }),
    )?;
    crate::routes::social::exchange_friend_presence(cx.tx, cx.app, cx.out, &from_id, &to_id)?;
    accept()
}

/// Дружба прекращена (`processFriendRemoveEvent`).
pub fn remove(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let Some(Pair { from, to, .. }) = pair(event) else {
        return reject("missing_friendship_payload");
    };
    let from_refusal = cx.refusal(Some(from))?;
    let to_refusal = match from_refusal {
        Some(_) => cx.refusal(Some(to))?,
        None => None,
    };
    if let (Some(a), Some(b)) = (from_refusal, to_refusal) {
        let refusal = if a == "attribution_unproven" || b == "attribution_unproven" {
            "attribution_unproven"
        } else {
            "attribution_mismatch"
        };
        return reject(refusal);
    }
    let from_resolved = cx.resolve(from)?;
    let to_resolved = cx.resolve(to)?;
    let acting = if from_refusal.is_some() {
        &to_resolved
    } else {
        &from_resolved
    };
    if matches!(acting, Resolved::Mismatch) {
        return reject("attribution_mismatch");
    }
    if pair_changed_later(cx, event, from, to)? {
        return accept();
    }
    let (Resolved::Found(from_user), Resolved::Found(to_user)) = (from_resolved, to_resolved)
    else {
        return accept();
    };
    let (from_id, to_id) = (from_user.text("id")?, to_user.text("id")?);
    cx.tx.execute(
        "DELETE FROM friends WHERE (user_id = ? AND friend_id = ?) OR (user_id = ? AND friend_id = ?)",
        params![&from_id, &to_id, &to_id, &from_id],
    )?;
    let (local, removing) = if homed_here(cx, from) {
        (from_id, to_id)
    } else {
        (to_id, from_id)
    };
    cx.to_user(
        &local,
        &json!({"type": "friend_removed", "userId": removing}),
    )?;
    accept()
}
