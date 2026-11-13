//! Сброс удалённого инстанса (`utils/federationReset.ts`).
//!
//! Пир, переустановленный с нуля, приходит с новой эпохой. Старый пиринг
//! тогда недействителен: пир уходит в `needs_attention`, его пользователи
//! помечаются до решения администратора, а случай записывается в журнал.

use serde_json::json;

use super::peers::{self, Effects, Moved, Transition};
use crate::db::{DbResult, Tx, Value as Sql};
use crate::params;
use crate::state::{AppState, now_ms};

/// Хеш пароля у копии удалённого пользователя.
const REPLICATED_STUB: &str = "!federation-replicated";

/// Причины активации, после которых эпоха пира считается подтверждённой.
const HANDSHAKE_CAUSES: [&str; 9] = [
    "initiate_accepted",
    "accept_new",
    "accept_pending",
    "accept_rejected_override",
    "accept_awaiting_approval",
    "accept_awaiting_approval_fallback",
    "approval_handshake",
    "ensure_peered",
    "stale_peering_verified",
];

/// Имя узла из адреса (`extractDomain`): у полного адреса — без порта.
#[must_use]
pub fn domain_of(origin: &str) -> String {
    // `new URL(…).hostname`: у разобранного адреса без узла (`localhost:3000`
    // разбирается как схема `localhost:`) имя пустое.
    if let Ok(url) = url::Url::parse(origin) {
        return url.host_str().unwrap_or_default().to_owned();
    }
    let rest = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
        .unwrap_or(origin);
    rest.split('/').next().unwrap_or(rest).to_owned()
}

/// Условие «столбец указывает на этот инстанс» и его параметры: домашний
/// инстанс хранится то именем узла, то полным адресом.
fn instance_match(column: &str, origin: &str) -> (String, [Sql; 3]) {
    let domain = domain_of(origin);
    (
        format!("({column} = ? OR {column} = ? OR {column} = ?)"),
        [
            Sql::from(domain.as_str()),
            Sql::from(format!("https://{domain}")),
            Sql::from(format!("http://{domain}")),
        ],
    )
}

fn count(tx: &mut dyn Tx, sql: &str, values: &[Sql]) -> DbResult<i64> {
    tx.query_one(sql, values)?.map_or(Ok(0), |row| row.int("n"))
}

/// Записывает обнаруженный сброс пира (`markPeerReset`): переход в
/// `needs_attention`, пометка его пользователей и строка журнала.
///
/// # Errors
/// Ошибка базы.
pub fn mark(
    tx: &mut dyn Tx,
    peer_id: &str,
    origin: &str,
    dead_epoch: &str,
    observed_epoch: &str,
) -> DbResult<Option<Effects>> {
    let current = peers::by_id(tx, peer_id)?;
    let seen = tx
        .query_one(
            "SELECT observed_peer_instance_id FROM federation_peers WHERE id = ?",
            params![peer_id],
        )?
        .map(|row| row.opt_text("observed_peer_instance_id"))
        .transpose()?
        .flatten();
    let repeat = current.as_ref().is_some_and(|peer| {
        peer.status == "needs_attention" && peer.reason.as_deref() == Some("peer_reset_detected")
    }) && seen.as_deref() == Some(observed_epoch);
    if repeat {
        return Ok(None);
    }
    let change = Transition::new(
        &["active", "unreachable", "needs_attention"],
        "needs_attention",
        "reset_detected",
    )
    .reason("peer_reset_detected")
    .set("observed_peer_instance_id", observed_epoch);
    let Moved::Applied { effects, .. } = peers::apply(tx, peer_id, &change)? else {
        return Ok(None);
    };
    let (home, home_values) = instance_match("home_instance", origin);
    tx.execute(
        &format!("UPDATE users SET federation_heal_pending = 1 WHERE is_deleted = 0 AND {home}"),
        &home_values,
    )?;
    let mut stub_values = vec![Sql::from(REPLICATED_STUB)];
    stub_values.extend(home_values.iter().cloned());
    let stubs = count(
        tx,
        &format!(
            "SELECT count(*) AS n FROM users WHERE federation_heal_pending = 1 \
             AND password_hash = ? AND {home}"
        ),
        &stub_values,
    )?;
    let orphans = count(
        tx,
        &format!(
            "SELECT count(*) AS n FROM users WHERE federation_heal_pending = 1 \
             AND password_hash != ? AND {home}"
        ),
        &stub_values,
    )?;
    let open = tx
        .query_one(
            "SELECT resolved_at FROM federation_reset_events WHERE origin = ?",
            params![origin],
        )?
        .map(|row| row.opt_int("resolved_at"))
        .transpose()?;
    match open {
        Some(None) => {
            tx.execute(
                "UPDATE federation_reset_events SET stub_count = ?, orphaned_account_count = ?, \
                 acknowledged_at = NULL WHERE origin = ?",
                params![stubs, orphans, origin],
            )?;
        }
        Some(Some(_)) => {
            tx.execute(
                "UPDATE federation_reset_events SET dead_epoch = ?, new_epoch = NULL, \
                 detected_at = ?, resolved_at = NULL, stub_count = ?, orphaned_account_count = ?, \
                 acknowledged_at = NULL WHERE origin = ?",
                params![dead_epoch, now_ms(), stubs, orphans, origin],
            )?;
        }
        None => {
            tx.execute(
                "INSERT INTO federation_reset_events (origin, dead_epoch, new_epoch, detected_at, \
                 resolved_at, stub_count, orphaned_account_count) VALUES (?, ?, NULL, ?, NULL, ?, ?)",
                params![origin, dead_epoch, now_ms(), stubs, orphans],
            )?;
        }
    }
    Ok(Some(*effects))
}

/// [`mark`] в своей транзакции, с событиями администраторам.
pub async fn mark_and_notify(
    state: &AppState,
    peer_id: &str,
    origin: &str,
    dead_epoch: &str,
    observed_epoch: &str,
) {
    let (id, at, dead, observed) = (
        peer_id.to_owned(),
        origin.to_owned(),
        dead_epoch.to_owned(),
        observed_epoch.to_owned(),
    );
    let marked = state
        .run(move |tx, _| Ok(mark(tx, &id, &at, &dead, &observed)?))
        .await;
    match marked {
        Ok(Some(effects)) => {
            peers::run_effects(state, effects).await;
            let app = state.clone();
            let at = origin.to_owned();
            let _ = state
                .run(move |tx, out| {
                    let event = json!({"type": "federation_peer_reset_detected", "origin": at});
                    Ok(peers::to_admins(tx, &app.hub, out, &event)?)
                })
                .await;
        }
        Ok(None) => {}
        Err(error) => tracing::error!(origin, ?error, "сброс пира не записан"),
    }
}

/// Закрывает запись о сбросе после нового рукопожатия (`healResetIncarnation`).
///
/// Если пир вернулся с прежней эпохой (сброса на деле не было), пометки
/// снимаются. Настоящий сброс требует разбора копий пользователей и
/// осиротевших аккаунтов; он переносится вместе с репликами, а до тех пор
/// запись остаётся открытой и видна администратору.
///
/// # Errors
/// Ошибка базы.
pub fn heal(tx: &mut dyn Tx, origin: &str, new_epoch: &str, cause: &str) -> DbResult<()> {
    if !HANDSHAKE_CAUSES.contains(&cause) {
        return Ok(());
    }
    let Some(journal) = tx.query_one(
        "SELECT dead_epoch FROM federation_reset_events WHERE origin = ? AND resolved_at IS NULL",
        params![origin],
    )?
    else {
        return Ok(());
    };
    if journal.text("dead_epoch")? != new_epoch {
        tracing::warn!(
            origin,
            "пир вернулся с новой эпохой: запись о сбросе остаётся открытой до переноса реплик"
        );
        return Ok(());
    }
    let (home, values) = instance_match("home_instance", origin);
    tx.execute(
        &format!(
            "UPDATE users SET federation_heal_pending = 0 WHERE federation_heal_pending = 1 AND {home}"
        ),
        &values,
    )?;
    tx.execute(
        "UPDATE federation_reset_events SET new_epoch = ?, resolved_at = ? WHERE origin = ?",
        params![new_epoch, now_ms(), origin],
    )?;
    Ok(())
}

/// Условие для `detached_home_instance` (`detachedHomeInstanceMatch`).
#[must_use]
pub fn detached_match(origin: &str) -> (String, [Sql; 3]) {
    instance_match("detached_home_instance", origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_drops_scheme_and_slashes() {
        assert_eq!(domain_of("https://a.example//"), "a.example");
        assert_eq!(domain_of("http://localhost:3000"), "localhost");
        assert_eq!(domain_of("localhost:3000"), "");
        assert_eq!(domain_of("a.example"), "a.example");
        assert_eq!(domain_of("a.example/x"), "a.example");
    }
}
