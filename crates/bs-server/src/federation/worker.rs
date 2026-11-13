//! Фоновые задачи ведения пиров (`utils/federationWorker.ts`,
//! `utils/federationRecovery.ts`, часть `utils/storageJanitor.ts`).
//!
//! Доставка очереди исходящих сюда не входит: она переносится отдельным шагом.

use std::time::Duration;

use serde_json::json;

use super::fetch::{self, Epoch, Trust};
use super::handshake;
use super::peers::{self, Peer, Transition};
use super::reset;
use crate::federation_auth::{self, ROTATION_GRACE_MS};
use crate::params;
use crate::state::{AppState, now_ms};

/// Раз в столько проверяются недоступные пиры без очереди и идёт общий обход.
const HEALTH_INTERVAL: Duration = Duration::from_mins(15);
/// Раз в столько ищутся недоступные пиры, которым пора повторить попытку.
const RECOVERY_INTERVAL: Duration = Duration::from_secs(5);
/// Паузы между попытками, когда пиру есть что отправить.
const RECOVERY_BACKOFF_MS: [i64; 4] = [30_000, 60_000, 300_000, 900_000];
/// Сколько живёт никем не использованная автоматическая заготовка пира.
const AUTO_PENDING_GRACE_MS: i64 = 60 * 60 * 1000;
/// Сколько хранятся прочитанные уведомления о пиринге.
const NOTIFICATION_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Пора ли снова пробовать связаться с пиром (`isPeerAttemptDue`).
#[must_use]
pub fn attempt_due(peer: &Peer, has_queue: bool, now: i64, divisor: u32) -> bool {
    let Some(last) = peer.last_probe_at else {
        return true;
    };
    let interval = if has_queue {
        let step = usize::try_from((peer.probe_attempts - 1).max(0))
            .unwrap_or(0)
            .min(RECOVERY_BACKOFF_MS.len() - 1);
        // `retryWait`: паузы с очередью сокращаются делителем для тестов.
        let divisor = i64::from(divisor.max(1));
        (RECOVERY_BACKOFF_MS[step] + divisor - 1) / divisor
    } else {
        i64::try_from(HEALTH_INTERVAL.as_millis()).unwrap_or(i64::MAX)
    };
    now - last >= interval
}

/// Что вышло из проверки недоступного пира.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// Пир снова активен.
    Recovered,
    /// У пира новая эпоха: он переустановлен.
    ResetDetected,
    /// Ничего не изменилось.
    Unchanged,
}

/// Возвращает ответивший пир в строй или записывает его сброс
/// (`recoverOrDetectReset`).
pub async fn recover_or_detect_reset(
    state: &AppState,
    peer: &Peer,
    observed: Option<&str>,
) -> Recovery {
    if let (Some(known), Some(observed)) = (peer.epoch.as_deref(), observed)
        && known != observed
    {
        reset::mark_and_notify(state, &peer.id, &peer.origin, known, observed).await;
        return Recovery::ResetDetected;
    }
    let change = Transition::new(&["unreachable"], "active", "health_check_recovery")
        .set("consecutive_failures", 0_i64)
        .set("last_seen_at", now_ms());
    match peers::transition(state, &peer.id, change).await {
        Ok(moved) if moved.applied() => Recovery::Recovered,
        _ => Recovery::Unchanged,
    }
}

/// Проверка недоступных пиров (`processRecoveryTick`).
async fn recovery_tick(state: &AppState) {
    let now = now_ms();
    let divisor = state.config.federation_backoff_divisor;
    let due = state
        .run(move |tx, _| {
            let mut due = Vec::new();
            for peer in peers::with_status(tx, "unreachable")? {
                let queued = tx
                    .query_one(
                        "SELECT id FROM federation_outbox WHERE peer_id = ? LIMIT 1",
                        params![&peer.id],
                    )?
                    .is_some();
                if attempt_due(&peer, queued, now, divisor) {
                    due.push(peer);
                }
            }
            Ok(due)
        })
        .await
        .unwrap_or_default();
    for peer in due {
        let (reachable, epoch) = fetch::probe_reachable(&state.config, &peer.origin).await;
        if reachable {
            match recover_or_detect_reset(state, &peer, epoch.as_deref()).await {
                Recovery::ResetDetected => {
                    tracing::warn!(
                        origin = peer.origin,
                        "пир переустановлен: нужен разбор администратором"
                    );
                }
                Recovery::Recovered => tracing::info!(origin = peer.origin, "пир снова доступен"),
                Recovery::Unchanged => {}
            }
        } else {
            let id = peer.id.clone();
            let _ = state
                .run(move |tx, _| Ok(peers::record_attempt(tx, &id, &["unreachable"], now)?))
                .await;
        }
    }
}

/// Завершает смены секрета, у которых истёк срок совместного действия.
async fn finalize_rotations(state: &AppState) {
    let done = state
        .run(|tx, _| {
            let rows = tx.query(
                "SELECT id, origin, pending_hmac_secret, secret_rotation_at FROM federation_peers \
                 WHERE pending_hmac_secret IS NOT NULL AND secret_rotation_at IS NOT NULL",
                params![],
            )?;
            let now = now_ms();
            for row in rows {
                if now - row.int("secret_rotation_at")? <= ROTATION_GRACE_MS {
                    continue;
                }
                tx.execute(
                    "UPDATE federation_peers SET hmac_secret = ?, pending_hmac_secret = NULL, \
                     secret_rotation_at = NULL, secret_rotated_at = ? WHERE id = ?",
                    params![row.text("pending_hmac_secret")?, now, row.text("id")?],
                )?;
                tracing::info!(origin = row.text("origin")?, "смена секрета завершена");
            }
            Ok(())
        })
        .await;
    if let Err(error) = done {
        tracing::warn!(?error, "завершение смены секретов не удалось");
    }
}

/// Предлагает пиру новый секрет (`/api/federation/peer/rotate`) и запоминает
/// его как ожидающий. Ошибка — код ответа и текст для администратора.
///
/// # Errors
/// Пир отказал или не ответил.
pub async fn start_rotation(state: &AppState, peer: &Peer) -> Result<(), (u16, String)> {
    let secret = federation_auth::generate_secret();
    let sent = fetch::signed_post(
        &state.config,
        &peer.origin,
        "/api/federation/peer/rotate",
        &json!({"newSecret": secret}),
        &peer.secret,
        Trust::Approved,
    )
    .await;
    match sent {
        Ok(answer) if answer.ok() => {}
        Ok(answer) => {
            let text = answer
                .object()
                .get("error")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.is_empty())
                .map_or_else(
                    || format!("Remote instance rejected rotation (HTTP {})", answer.status),
                    str::to_owned,
                );
            return Err((502, text));
        }
        Err(fetch::FetchError::Timeout) => {
            return Err((
                504,
                "Remote instance did not respond within 10 seconds".to_owned(),
            ));
        }
        Err(fetch::FetchError::Failed(text)) => {
            return Err((502, format!("Failed to reach remote instance: {text}")));
        }
    }
    let id = peer.id.clone();
    state
        .run(move |tx, _| {
            tx.execute(
                "UPDATE federation_peers SET pending_hmac_secret = ?, secret_rotation_at = ? \
                 WHERE id = ?",
                params![secret, now_ms(), id],
            )?;
            Ok(())
        })
        .await
        .map_err(|_| (500, "Internal Server Error".to_owned()))
}

/// Плановая смена секретов у активных пиров.
async fn auto_rotate(state: &AppState) {
    let due = state
        .run(|tx, _| {
            let rows = tx.query(
                &format!(
                    "SELECT {} FROM federation_peers WHERE status = 'active' \
                     AND pending_hmac_secret IS NULL AND auto_rotate_interval_days > 0",
                    peers::COLUMNS
                ),
                params![],
            )?;
            let now = now_ms();
            let mut due = Vec::new();
            for row in rows {
                let last = row
                    .opt_int("secret_rotated_at")?
                    .map_or_else(|| row.int("created_at"), Ok)?;
                if now - last >= row.int("auto_rotate_interval_days")? * 86_400_000 {
                    due.push(Peer::from_row(&row)?);
                }
            }
            Ok(due)
        })
        .await
        .unwrap_or_default();
    for peer in due {
        match start_rotation(state, &peer).await {
            Ok(()) => tracing::info!(origin = peer.origin, "начата плановая смена секрета"),
            Err((_, text)) => {
                tracing::warn!(
                    origin = peer.origin,
                    text,
                    "плановая смена секрета не удалась"
                );
            }
        }
    }
}

/// Узнаёт эпоху активных пиров, у которых она ещё не записана
/// (`refreshPeerEpochs`).
async fn refresh_epochs(state: &AppState) {
    let unknown = state
        .run(|tx, _| {
            Ok(peers::with_status(tx, "active")?
                .into_iter()
                .filter(|peer| peer.epoch.is_none())
                .collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
    for peer in unknown {
        let Epoch::Verified(epoch) =
            fetch::probe_epoch(&state.config, &peer.origin, &peer.secret).await
        else {
            continue;
        };
        let id = peer.id;
        let _ = state
            .run(move |tx, _| {
                tx.execute(
                    "UPDATE federation_peers SET peer_instance_id = ? \
                     WHERE id = ? AND peer_instance_id IS NULL",
                    params![epoch, id],
                )?;
                Ok(())
            })
            .await;
    }
}

/// Ищет сброс у пиров, застрявших в `needs_attention` по другой причине
/// (`detectResetOnNeedsAttentionPeers`).
async fn detect_resets(state: &AppState) {
    let suspects = state
        .run(|tx, _| {
            Ok(peers::with_status(tx, "needs_attention")?
                .into_iter()
                .filter(|peer| {
                    peer.epoch.is_some() && peer.reason.as_deref() != Some("peer_reset_detected")
                })
                .collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
    for peer in suspects {
        let (reachable, observed) = fetch::probe_reachable(&state.config, &peer.origin).await;
        if let (true, Some(known), Some(observed)) = (reachable, peer.epoch.as_deref(), observed)
            && known != observed
        {
            reset::mark_and_notify(state, &peer.id, &peer.origin, known, &observed).await;
        }
    }
}

/// Удаляет автоматические заготовки пиров, которыми никто не воспользовался
/// (`cleanupUnusedAutoPendingPeers`).
async fn cleanup_unused_pending(state: &AppState) {
    let app = state.clone();
    let done = state
        .run(move |tx, out| {
            let cutoff = now_ms() - AUTO_PENDING_GRACE_MS;
            let rows = tx.query(
                "SELECT p.id, p.origin FROM federation_peers p WHERE p.status = 'pending' \
                 AND p.initiated_by = 'auto' AND p.created_at < ? AND NOT EXISTS \
                 (SELECT 1 FROM federation_outbox o WHERE o.peer_id = p.id)",
                params![cutoff],
            )?;
            let mut removed = 0_u32;
            for row in rows {
                let origin = row.text("origin")?;
                if app.federation.in_flight(&origin) {
                    continue;
                }
                tx.execute(
                    "DELETE FROM federation_peers WHERE id = ?",
                    params![row.text("id")?],
                )?;
                removed += 1;
                tracing::info!(origin, "удалена неиспользованная заготовка пира");
            }
            if removed > 0 {
                peers::to_admins(
                    tx,
                    &app.hub,
                    out,
                    &json!({"type": "federation_peers_changed"}),
                )?;
            }
            Ok(())
        })
        .await;
    if let Err(error) = done {
        tracing::warn!(?error, "уборка заготовок пиров не удалась");
    }
}

/// Закрывает просроченные запросы на пиринг (`cleanupExpiredApprovalRequests`)
/// и удаляет давно прочитанные уведомления.
async fn expire_approvals(state: &AppState) {
    let app = state.clone();
    // Исходящие закрываются на месте; входящим сначала уходит отказ.
    let inbound = state
        .run(move |tx, out| {
            let now = now_ms();
            let expired = tx.query(
                "SELECT id, origin, direction, hmac_secret FROM peer_approval_requests \
                 WHERE expires_at <= ?",
                params![now],
            )?;
            let mut inbound = Vec::new();
            for request in expired {
                let id = request.text("id")?;
                let origin = request.text("origin")?;
                if request.text("direction")? != "outbound" {
                    match request.opt_text("hmac_secret")? {
                        Some(secret) => inbound.push((id, origin, secret)),
                        None => tracing::warn!(origin, "у входящего запроса нет секрета: отказ не отправить"),
                    }
                    continue;
                }
                let subscribers = tx.query(
                    "SELECT user_id, trigger_reason, trigger_target FROM peer_approval_subscribers \
                     WHERE request_id = ?",
                    params![&id],
                )?;
                for sub in subscribers {
                    let user = sub.text("user_id")?;
                    tx.execute(
                        "INSERT INTO peer_approval_notifications (id, user_id, kind, peer_origin, \
                         trigger_reason, trigger_target, created_at, read_at) \
                         VALUES (?, ?, 'expired', ?, ?, ?, ?, NULL)",
                        params![
                            app.ids.next(),
                            &user,
                            &origin,
                            sub.text("trigger_reason")?,
                            sub.text("trigger_target")?,
                            now
                        ],
                    )?;
                    out.to_user(
                        &user,
                        &json!({"type": "peering_notification_received", "kind": "expired"}),
                    );
                    out.to_user(&user, &json!({"type": "peering_subscription_changed"}));
                }
                tx.execute(
                    "DELETE FROM peer_approval_subscribers WHERE request_id = ?",
                    params![&id],
                )?;
                tx.execute("DELETE FROM peer_approval_requests WHERE id = ?", params![&id])?;
            }
            tx.execute(
                "DELETE FROM peer_approval_notifications WHERE read_at IS NOT NULL AND read_at <= ?",
                params![now - NOTIFICATION_RETENTION_MS],
            )?;
            Ok(inbound)
        })
        .await
        .unwrap_or_default();
    let ours = crate::federation::our_origin(&state.config);
    for (id, origin, secret) in inbound {
        let body = json!({
            "origin": ours,
            "reason": "expired",
            "message": "Request expired — no response from admin within 30 days",
        });
        let sent = fetch::signed_post(
            &state.config,
            &origin,
            "/api/federation/peer/denied",
            &body,
            &secret,
            Trust::Asserted,
        )
        .await;
        if sent.is_ok_and(|answer| answer.ok()) {
            let _ = state
                .run(move |tx, _| {
                    tx.execute(
                        "DELETE FROM peer_approval_requests WHERE id = ?",
                        params![id],
                    )?;
                    Ok(())
                })
                .await;
        } else {
            tracing::warn!(
                origin,
                "отказ по сроку не доставлен: повтор в следующий обход"
            );
        }
    }
}

/// Общий обход раз в 15 минут (`processHealthCheckTick`).
async fn health_tick(state: &AppState) {
    finalize_rotations(state).await;
    auto_rotate(state).await;
    refresh_epochs(state).await;
    detect_resets(state).await;
    handshake::resolve_stale_parked(state).await;
}

/// Чистит журнал применённых событий, часы субъектов и очередь файлов
/// (`sweepAppliedEvents`, `sweepSubjectClocks`, `cleanupFederationFileQueue`).
///
/// # Errors
/// Ошибка базы.
pub fn sweep_inbound(tx: &mut dyn crate::db::Tx, now: i64) -> crate::db::DbResult<u64> {
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    let mut total = super::inbound::sweep_applied(tx, now)?;
    total += tx.execute(
        "DELETE FROM federation_subject_clocks WHERE recorded_at < ?",
        crate::params![now - 400 * DAY_MS],
    )?;
    total += tx.execute(
        "DELETE FROM federation_file_queue WHERE status = 'completed' AND created_at < ?",
        crate::params![now - 7 * DAY_MS],
    )?;
    total += tx.execute(
        "DELETE FROM federation_file_queue WHERE expires_at < ?",
        crate::params![now],
    )?;
    Ok(total)
}

/// Запускает фоновые задачи ведения пиров.
pub fn spawn(state: &AppState) {
    if state.config.disable_federation_workers {
        tracing::info!("фоновые задачи федерации отключены (DISABLE_FEDERATION_WORKERS)");
        return;
    }
    let app = state.clone();
    tokio::spawn(async move {
        refresh_epochs(&app).await;
        detect_resets(&app).await;
        let mut timer = tokio::time::interval(HEALTH_INTERVAL);
        timer.tick().await;
        loop {
            timer.tick().await;
            health_tick(&app).await;
        }
    });
    let app = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RECOVERY_INTERVAL).await;
            recovery_tick(&app).await;
        }
    });
    super::outbox::spawn(state);
    super::sync::spawn(state);
    let app = state.clone();
    tokio::spawn(async move { super::peers::startup_bootstrap(&app).await });
    let app = state.clone();
    tokio::spawn(async move {
        super::inbound::profile::backfill_assets(&app).await;
        loop {
            tokio::time::sleep(super::inbound::files::INTERVAL).await;
            super::inbound::files::tick(&app).await;
        }
    });
    let app = state.clone();
    tokio::spawn(async move {
        let mut minute = tokio::time::interval(Duration::from_mins(1));
        let mut passed = 0_u32;
        loop {
            minute.tick().await;
            app.federation.sweep_nonces(now_ms());
            passed += 1;
            if passed >= 60 {
                passed = 0;
                cleanup_unused_pending(&app).await;
                expire_approvals(&app).await;
                let expired = app
                    .run(|tx, _| Ok(super::outbox::expire(tx, now_ms())?))
                    .await
                    .unwrap_or(0);
                if expired > 0 {
                    tracing::info!(expired, "из очереди исходящих удалены просроченные события");
                }
                let swept = app
                    .run(|tx, _| Ok(sweep_inbound(tx, now_ms())?))
                    .await
                    .unwrap_or(0);
                if swept > 0 {
                    tracing::info!(swept, "удалены устаревшие записи приёма событий");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(attempts: i64, last: Option<i64>) -> Peer {
        Peer {
            id: "1".into(),
            origin: "https://b.example".into(),
            status: "unreachable",
            reason: None,
            secret: "s".into(),
            pending_secret: None,
            rotation_at: None,
            initiated_by: "auto".into(),
            instance_name: None,
            epoch: None,
            approval_token: None,
            nonce_supported: false,
            probe_attempts: attempts,
            last_probe_at: last,
            public: serde_json::Value::Null,
        }
    }

    #[test]
    fn attempts_back_off_only_when_mail_is_queued() {
        let now = 10_000_000;
        assert!(attempt_due(&peer(0, None), false, now, 1));
        // Есть что отправить: 30 с, 60 с, 5 мин, 15 мин.
        assert!(!attempt_due(&peer(1, Some(now - 29_000)), true, now, 1));
        assert!(attempt_due(&peer(1, Some(now - 30_000)), true, now, 1));
        assert!(!attempt_due(&peer(2, Some(now - 59_000)), true, now, 1));
        assert!(attempt_due(&peer(3, Some(now - 300_000)), true, now, 1));
        assert!(!attempt_due(&peer(9, Some(now - 899_000)), true, now, 1));
        assert!(attempt_due(&peer(9, Some(now - 900_000)), true, now, 1));
        // Очередь пуста: раз в 15 минут.
        assert!(!attempt_due(&peer(1, Some(now - 600_000)), false, now, 1));
        assert!(attempt_due(&peer(1, Some(now - 900_000)), false, now, 1));
        // Делитель для тестов сокращает только паузы с очередью.
        assert!(attempt_due(&peer(1, Some(now - 30)), true, now, 1000));
        assert!(!attempt_due(&peer(1, Some(now - 30)), false, now, 1000));
    }
}
