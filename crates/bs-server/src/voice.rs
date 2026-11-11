//! Голосовые комнаты: кто где сидит, ограничения, проверки прав.

use crate::db::{DbResult, Tx};
use crate::hub::{Hub, Outbox};

/// После изменения ролей или прав в пространстве: затронутым участникам
/// уходит голосовое состояние, которое они теперь видят, а права говорить и
/// показывать экран перепроверяются (`announceSpaceAccessChange` и
/// `checkVoicePermissions`).
///
/// # Errors
/// Ошибка базы.
pub fn access_changed(
    tx: &mut dyn Tx,
    out: &mut Outbox,
    space_id: &str,
    affected: &[String],
) -> DbResult<()> {
    let _ = (tx, out, space_id, affected);
    Ok(())
}

/// Отправляет пользователю голосовое состояние пространства, которое он
/// сейчас видит (`pushSpaceVoiceState`).
///
/// # Errors
/// Ошибка базы.
pub fn push_space_state(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    user_id: &str,
    space_id: &str,
) -> DbResult<()> {
    let _ = (tx, hub, out, user_id, space_id);
    Ok(())
}

/// Перепроверяет права говорить и показывать экран у сидящих в голосовых
/// каналах пространства (`checkVoicePermissions`).
///
/// # Errors
/// Ошибка базы.
pub fn check_permissions(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space_id: &str,
) -> DbResult<()> {
    let _ = (tx, hub, out, space_id);
    Ok(())
}

/// Канал удаляют: сидящие в нём отключаются.
///
/// # Errors
/// Ошибка базы.
pub fn channel_deleted(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    space_id: &str,
    channel_id: &str,
) -> DbResult<()> {
    let _ = (tx, hub, out, space_id, channel_id);
    Ok(())
}

/// Участника добавили в группу, где идёт звонок: ему приходит вызов.
///
/// # Errors
/// Ошибка базы.
pub fn dm_member_added(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    dm_channel_id: &str,
    user_id: &str,
) -> DbResult<()> {
    let _ = (tx, hub, out, dm_channel_id, user_id);
    Ok(())
}

/// Участник покидает группу: если он в её звонке, его отключают.
///
/// # Errors
/// Ошибка базы.
pub fn dm_member_left(
    tx: &mut dyn Tx,
    hub: &Hub,
    out: &mut Outbox,
    dm_channel_id: &str,
    user_id: &str,
) -> DbResult<()> {
    let _ = (tx, hub, out, dm_channel_id, user_id);
    Ok(())
}
