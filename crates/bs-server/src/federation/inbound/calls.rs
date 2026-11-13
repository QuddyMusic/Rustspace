//! Звонки и «печатает» от пиров (`routes/federation/events/calls.ts`).

use std::collections::HashMap;

use serde_json::{Value, json};

use super::{Ctx, Step, accept, raw_domain, reject, text, truthy};
use crate::db::DbResult;
use crate::federation::identity::{Actor, Hints, Resolved};
use crate::params;
use crate::voice::{self, FedCall};

/// Источник со схемой.
fn with_scheme(source: &str) -> String {
    if source.starts_with("http") {
        source.to_owned()
    } else {
        format!("https://{source}")
    }
}

/// Строковое поле, непустое (как проверка `!value` в оригинале).
fn filled<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    text(value, key).filter(|v| !v.is_empty())
}

/// Токены звонка по пользователям здесь (`callTokensByLocalUser`).
fn tokens_by_local_user(cx: &mut Ctx<'_>, call: &Value) -> DbResult<HashMap<String, String>> {
    let ours = crate::federation::our_origin(&cx.app.config);
    let holders: Vec<Value> = match call.get("memberTokens").filter(|v| !v.is_null()) {
        Some(Value::Array(list)) => list.clone(),
        Some(_) => Vec::new(),
        None => call
            .get("tokens")
            .and_then(Value::as_object)
            .map(|tokens| {
                tokens
                    .iter()
                    .map(|(home, token)| json!({"homeUserId": home, "homeInstance": ours, "token": token}))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let mut by_user = HashMap::new();
    for holder in holders {
        let (Some(home_user), Some(home), Some(token)) = (
            text(&holder, "homeUserId"),
            text(&holder, "homeInstance"),
            text(&holder, "token"),
        ) else {
            continue;
        };
        let actor = json!({"homeUserId": home_user, "homeInstance": home});
        if let Resolved::Found(user) = cx.resolve(&actor)? {
            by_user.insert(user.text("id")?, token.to_owned());
        }
    }
    Ok(by_user)
}

/// Пользователь здесь для участника события, или пусто (`localActorId`).
fn local_actor(cx: &mut Ctx<'_>, actor: Option<&Value>) -> DbResult<String> {
    match actor {
        Some(actor) => match cx.resolve(actor)? {
            Resolved::Found(user) => user.text("id"),
            _ => Ok(String::new()),
        },
        None => Ok(String::new()),
    }
}

/// Участник здесь и в переписке.
fn local_member(
    cx: &mut Ctx<'_>,
    actor: Option<&Value>,
    channel: &str,
) -> DbResult<Option<String>> {
    let Some(actor) = actor else {
        return Ok(None);
    };
    let Resolved::Found(user) = cx.resolve(actor)? else {
        return Ok(None);
    };
    let id = user.text("id")?;
    Ok(crate::dm::is_member(cx.tx, channel, &id)?.then_some(id))
}

/// Кто ответил (`answererOf`).
fn answerer(call: &Value) -> Option<Value> {
    if let Some(named) = call.get("answeredBy")
        && let (Some(user), Some(home)) =
            (filled(named, "homeUserId"), filled(named, "homeInstance"))
    {
        return Some(json!({"homeUserId": user, "homeInstance": home}));
    }
    let acceptor = call.get("acceptor")?;
    Some(
        json!({"homeUserId": acceptor.get("homeUserId"), "homeInstance": acceptor.get("homeInstance")}),
    )
}

/// Вызов с другого инстанса (`processDmCallStartEvent`).
#[allow(clippy::too_many_lines, reason = "два пути оригинала")]
pub fn start(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let call = event.get("call").filter(|c| truthy(c));
    let federated = filled(event, "federatedId");
    let (Some(call), Some(federated)) = (call, federated) else {
        return reject("missing_call_payload");
    };
    let caller_value = call.get("caller").filter(|c| truthy(c));
    let (Some(caller_value), Some(livekit_url)) = (caller_value, filled(call, "livekitUrl")) else {
        return reject("missing_call_payload");
    };
    if !call.get("tokens").is_some_and(truthy) {
        return reject("missing_call_payload");
    }
    if let Some(refusal) = cx.refusal(Some(caller_value))? {
        tracing::warn!(
            caller = raw_domain(text(caller_value, "homeInstance").unwrap_or_default()),
            source = raw_domain(cx.source),
            "отказ в атрибуции dm_call_start"
        );
        return reject(refusal);
    }
    let channel = cx.tx.query_one(
        "SELECT id, owner_id FROM dm_channels WHERE federated_id = ?",
        params![federated],
    )?;
    let owner = match &channel {
        Some(row) => row.opt_text("owner_id")?,
        None => None,
    };
    let group = !crate::dm::may_be_one_on_one(owner.as_deref(), Some(federated))
        && call.get("perMember") == Some(&Value::Bool(true));
    let Some(caller_actor) = Actor::from_value(Some(caller_value)) else {
        return reject("participant_not_found");
    };
    let Some(replica) = cx.replica(&caller_actor, &Hints::default())? else {
        return reject("participant_not_found");
    };
    let caller = replica.user;
    let caller_id = caller.text("id")?;
    let caller_name = match caller.opt_text("display_name")? {
        Some(name) => name,
        None => caller.opt_text("username")?.unwrap_or_default(),
    };
    let tokens = tokens_by_local_user(cx, call)?;
    let caller_home = text(caller_value, "homeInstance")
        .unwrap_or_default()
        .to_owned();
    let mut ringed = Vec::new();
    let channel_id = match &channel {
        Some(row) => Some(row.text("id")?),
        None => None,
    };
    let candidates: Vec<String> = if let Some(channel_id) = &channel_id {
        cx.tx
            .query(
                "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
                params![channel_id],
            )?
            .iter()
            .map(|row| row.text("user_id"))
            .collect::<DbResult<_>>()?
    } else {
        let Some(participants) = call.get("participants").and_then(Value::as_array) else {
            return reject("channel_not_found");
        };
        let ours = raw_domain(&crate::federation::our_origin(&cx.app.config));
        let caller_user = text(caller_value, "homeUserId").unwrap_or_default();
        let caller_domain = raw_domain(&caller_home);
        let mut found = Vec::new();
        for participant in participants {
            let home_user = text(participant, "homeUserId").unwrap_or_default();
            let domain = raw_domain(text(participant, "homeInstance").unwrap_or_default());
            if home_user == caller_user && domain == caller_domain {
                continue;
            }
            // Копия или аккаунт с дома участника; свой пользователь — по
            // идентификатору, когда дом участника — этот инстанс.
            let mut row = cx.tx.query_one(
                "SELECT id FROM users WHERE home_user_id = ? \
                 AND replace(replace(coalesce(home_instance, ''), 'https://', ''), 'http://', '') = ? LIMIT 1",
                params![home_user, &domain],
            )?;
            if row.is_none() && domain == ours {
                row = cx.tx.query_one(
                    "SELECT id FROM users WHERE id = ? AND home_instance IS NULL",
                    params![home_user],
                )?;
            }
            if let Some(row) = row {
                found.push(row.text("id")?);
            }
        }
        found
    };
    for member in candidates {
        // Звонящему здесь не звонят; тем, кто не на связи, тоже; тем, для
        // кого ведущий не выдал токена (их дом — третий инстанс), нечем.
        if channel_id.is_some() && member == caller_id {
            continue;
        }
        if !cx.app.hub.is_online(&member) {
            continue;
        }
        let Some(token) = tokens.get(&member) else {
            continue;
        };
        cx.to_user(
            &member,
            &json!({
                "type": "dm_call_incoming",
                "dmChannelId": channel_id,
                "federatedCallId": federated,
                "callerId": caller_id,
                "callerName": caller_name,
                "livekitUrl": livekit_url,
                "livekitToken": token,
                "callOrigin": caller_home,
            }),
        )?;
        ringed.push(member);
    }
    if ringed.is_empty() {
        // Звонить некому: звонящий узнаёт об этом и не ждёт минуту.
        return Ok(super::Verdict::Undeliverable("no_recipient".to_owned()));
    }
    voice::create_federated(
        cx.app,
        FedCall {
            dm_channel: channel_id,
            federated_id: federated.to_owned(),
            caller: caller_id,
            host: with_scheme(cx.source),
            livekit_url: livekit_url.to_owned(),
            tokens,
            ringed,
            joined: Vec::new(),
            group,
            active: false,
            started_at: crate::state::now_ms(),
        },
    );
    accept()
}

/// Переписка здесь по ключу звонка.
fn channel_by_key(cx: &mut Ctx<'_>, federated: &str) -> DbResult<Option<String>> {
    cx.tx
        .query_one(
            "SELECT id FROM dm_channels WHERE federated_id = ?",
            params![federated],
        )?
        .map(|row| row.text("id"))
        .transpose()
}

/// Принятие звонка (`processDmCallAcceptEvent`).
pub fn answer(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let call = event.get("call").filter(|c| truthy(c));
    let acceptor = call.and_then(|c| c.get("acceptor")).filter(|a| truthy(a));
    let (Some(call), Some(acceptor), Some(federated)) =
        (call, acceptor, filled(event, "federatedId"))
    else {
        return reject("missing_call_payload");
    };
    if let Some(refusal) = cx.refusal(Some(acceptor))? {
        return reject(refusal);
    }
    let channel = channel_by_key(cx, federated)?;
    let hosted = channel
        .as_deref()
        .and_then(|channel| voice::hosted_call(&cx.app.hub, channel));
    let answered_by = answerer(call);
    if let (Some(channel), Some((_, _, group))) = (channel.as_deref(), hosted) {
        let per_member = call.get("perMember") == Some(&Value::Bool(true));
        let seat = if group && per_member {
            local_member(cx, Some(acceptor), channel)?.map(|user| (user, with_scheme(cx.source)))
        } else {
            None
        };
        let actor = local_actor(cx, Some(acceptor))?;
        let source = with_scheme(cx.source);
        voice::relayed_accept_hosted(
            cx.tx,
            &cx.app.hub,
            cx.out,
            channel,
            federated,
            seat,
            actor,
            answered_by,
            &source,
        )?;
    } else {
        voice::relayed_to_federated(
            &cx.app.hub,
            cx.out,
            federated,
            "dm_call_accept",
            answered_by,
        );
    }
    accept()
}

/// Отказ или конец звонка (`processDmCallRejectEvent`, `processDmCallEndEvent`).
pub fn leave(cx: &mut Ctx<'_>, event: &Value, decline: bool) -> Step {
    let field = if decline { "rejector" } else { "endedBy" };
    let call = event.get("call").filter(|c| truthy(c));
    let actor = call.and_then(|c| c.get(field)).filter(|a| truthy(a));
    let (Some(call), Some(actor), Some(federated)) = (call, actor, filled(event, "federatedId"))
    else {
        return reject("missing_call_payload");
    };
    if let Some(refusal) = cx.refusal(Some(actor))? {
        return reject(refusal);
    }
    let channel = channel_by_key(cx, federated)?;
    let hosted = channel
        .as_deref()
        .and_then(|channel| voice::hosted_call(&cx.app.hub, channel));
    if let (Some(channel), Some((_, _, group))) = (channel.as_deref(), hosted) {
        let per_member = call.get("perMember") == Some(&Value::Bool(true));
        let member = if group {
            local_member(cx, Some(actor), channel)?
        } else {
            None
        };
        let local = local_actor(cx, Some(actor))?;
        let source = with_scheme(cx.source);
        voice::relayed_leave_hosted(
            cx.tx,
            &cx.app.hub,
            cx.out,
            channel,
            decline,
            member,
            local,
            per_member,
            &source,
        )?;
    } else {
        let kind = if decline {
            "dm_call_reject"
        } else {
            "dm_call_end"
        };
        voice::relayed_to_federated(&cx.app.hub, cx.out, federated, kind, None);
    }
    accept()
}

/// «Печатает» (`processDmTypingStartEvent`, `processDmTypingStopEvent`).
pub fn typing(cx: &mut Ctx<'_>, event: &Value, stop: bool) -> Step {
    let payload = event.get("typing").filter(|t| truthy(t));
    let (Some(payload), Some(federated)) = (payload, filled(event, "federatedId")) else {
        return reject("missing_typing_payload");
    };
    if let Some(refusal) = cx.refusal(Some(payload))? {
        return reject(refusal);
    }
    let Some(channel) = super::live_channel(cx.tx, federated)? else {
        return accept();
    };
    let user = match cx.resolve(payload)? {
        Resolved::Found(user) => *user,
        Resolved::Mismatch => {
            tracing::warn!(
                stop,
                "«печатает» отклонено: идентификатор принадлежит другому пользователю"
            );
            return reject("attribution_mismatch");
        }
        Resolved::Unknown => return accept(),
    };
    let channel_id = channel.text("id")?;
    let user_id = user.text("id")?;
    let wire = if stop {
        json!({"type": "dm_typing_stop", "dmChannelId": channel_id, "userId": user_id})
    } else {
        let username = match user.opt_text("username")? {
            Some(name) => json!(name),
            None => payload.get("username").cloned().unwrap_or(Value::Null),
        };
        json!({"type": "dm_typing", "dmChannelId": channel_id, "userId": user_id, "username": username})
    };
    for row in cx.tx.query(
        "SELECT user_id FROM dm_members WHERE dm_channel_id = ?",
        params![&channel_id],
    )? {
        let member = row.text("user_id")?;
        if member != user_id {
            cx.to_user(&member, &wire)?;
        }
    }
    accept()
}
