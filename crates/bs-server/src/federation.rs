//! Федерация: обмен между инстансами.
//!
//! Перенесена часть, которая нужна локальным изменениям: журнал мутаций
//! (по нему новый пир догоняет историю), постановка событий в очередь
//! исходящих, рукопожатие и ведение пиров. Доставка очереди и приём событий —
//! в работе; пока их нет, события для удалённых участников только отмечаются
//! в журнале.

pub mod fetch;
pub mod handshake;
pub mod identity;
pub mod inbound;
pub mod lookup;
pub mod outbox;
pub mod peers;
pub mod presence;
pub mod relay;
pub mod reset;
pub mod worker;

use serde_json::{Value, json};

use crate::config::Config;
use crate::db::{DbResult, Tx};
use crate::params;
use crate::snowflake::Snowflake;
use crate::state::now_ms;

/// Включена ли пересылка между инстансами (`federation_relay_enabled`).
///
/// # Errors
/// Ошибка базы.
pub fn relay_enabled(tx: &mut dyn Tx) -> DbResult<bool> {
    Ok(tx
        .query_one(
            "SELECT federation_relay_enabled FROM instance_settings WHERE id = 1",
            params![],
        )?
        .map(|row| row.opt_flag("federation_relay_enabled"))
        .transpose()?
        .flatten()
        == Some(true))
}

/// Наш адрес, как его видят другие инстансы (`getOurOrigin`).
#[must_use]
pub fn our_origin(config: &Config) -> String {
    if let Some(origin) = config
        .public_origin
        .as_deref()
        .map(str::trim)
        .filter(|o| !o.is_empty())
    {
        return origin.strip_suffix('/').unwrap_or(origin).to_owned();
    }
    match config.domain.as_deref().filter(|domain| !domain.is_empty()) {
        Some(domain) => format!("https://{domain}"),
        None => format!("http://localhost:{}", config.port),
    }
}

/// Запись в журнал мутаций (`appendMutationLog`).
///
/// # Errors
/// Ошибка базы.
pub fn append_mutation_log(
    tx: &mut dyn Tx,
    ids: &Snowflake,
    entity_id: &str,
    context_id: &str,
    mutation_type: &str,
    payload: Option<&Value>,
) -> DbResult<()> {
    append_mutation_log_in(tx, ids, entity_id, context_id, "dm", mutation_type, payload)
}

/// Запись в журнал мутаций с явным видом контекста (`dm`, `profile`, `friend`).
///
/// # Errors
/// Ошибка базы.
pub fn append_mutation_log_in(
    tx: &mut dyn Tx,
    ids: &Snowflake,
    entity_id: &str,
    context_id: &str,
    context_type: &str,
    mutation_type: &str,
    payload: Option<&Value>,
) -> DbResult<()> {
    if !relay_enabled(tx)? {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO federation_mutation_log \
         (id, entity_id, context_id, context_type, mutation_type, mutated_at, payload) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        params![
            ids.next(),
            entity_id,
            context_id,
            context_type,
            mutation_type,
            now_ms(),
            payload.map(Value::to_string)
        ],
    )?;
    Ok(())
}

/// Федеративная личность строки пользователя: `(homeUserId, homeInstance)`.
///
/// # Errors
/// Ошибка базы.
pub fn identity(tx: &mut dyn Tx, config: &Config, user_id: &str) -> DbResult<(String, String)> {
    let row = tx.query_one(
        "SELECT home_user_id, home_instance FROM users WHERE id = ?",
        params![user_id],
    )?;
    let (home_user, home_instance) = match row {
        Some(row) => (
            row.opt_text("home_user_id")?,
            row.opt_text("home_instance")?,
        ),
        None => (None, None),
    };
    Ok((
        home_user
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| user_id.to_owned()),
        home_instance
            .filter(|home| !home.is_empty())
            .unwrap_or_else(|| our_origin(config)),
    ))
}

/// Инстансы удалённых участников переписки (`getGroupDmTargetOrigins`).
///
/// # Errors
/// Ошибка базы.
pub fn dm_target_origins(
    tx: &mut dyn Tx,
    config: &Config,
    dm_channel_id: &str,
) -> DbResult<Vec<String>> {
    let ours = our_origin(config);
    let mut origins = Vec::new();
    for row in tx.query(
        "SELECT u.home_instance FROM dm_members m JOIN users u ON u.id = m.user_id \
         WHERE m.dm_channel_id = ?",
        params![dm_channel_id],
    )? {
        let Some(home) = row
            .opt_text("home_instance")?
            .filter(|home| !home.is_empty())
        else {
            continue;
        };
        let origin = if home.starts_with("http") {
            home
        } else {
            format!("https://{home}")
        };
        if origin != ours && !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    Ok(origins)
}

/// Событие звонка для других инстансов. Звонки между инстансами уходят не
/// через очередь, а прямым запросом (`sendCallRelay`); он ещё не перенесён.
pub fn call_relay_pending(context_id: &str, event_type: &str, targets: &[String]) {
    if !targets.is_empty() {
        tracing::warn!(
            context = context_id,
            event = event_type,
            targets = targets.len(),
            "событие звонка для других инстансов не отправлено: звонки между инстансами ещё не перенесены"
        );
    }
}

/// Ссылка на личное сообщение для других инстансов (`dmMessageFederationRef`).
///
/// # Errors
/// Ошибка базы.
pub fn message_ref(tx: &mut dyn Tx, config: &Config, message_id: &str) -> DbResult<Value> {
    let row = tx.query_one(
        "SELECT source_instance, source_message_id FROM dm_messages WHERE id = ?",
        params![message_id],
    )?;
    if let Some(row) = row
        && let (Some(instance), Some(source)) = (
            row.opt_text("source_instance")?,
            row.opt_text("source_message_id")?,
        )
        && !instance.is_empty()
        && !source.is_empty()
    {
        return Ok(json!({"messageId": source, "messageHomeInstance": instance}));
    }
    Ok(json!({"messageId": message_id, "messageHomeInstance": our_origin(config)}))
}

/// Ставит в очередь пересылку позиции чтения переписки (`queueReadStateRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn queue_read_state(
    tx: &mut dyn Tx,
    config: &Config,
    ids: &Snowflake,
    channel_id: &str,
    message_id: &str,
    user_id: &str,
) -> DbResult<()> {
    relay::read_state(
        tx,
        relay::Relay { config, ids },
        channel_id,
        message_id,
        user_id,
    )
}

/// Ключ переписки, общий для всех инстансов.
///
/// # Errors
/// Ошибка базы.
pub fn dm_federated_id(tx: &mut dyn Tx, dm_channel_id: &str) -> DbResult<Option<String>> {
    Ok(tx
        .query_one(
            "SELECT federated_id FROM dm_channels WHERE id = ?",
            params![dm_channel_id],
        )?
        .map(|row| row.opt_text("federated_id"))
        .transpose()?
        .flatten()
        .filter(|id| !id.is_empty()))
}

/// Пересылка закрытия и открытия переписки (`queueDmCloseRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn queue_dm_close(
    tx: &mut dyn Tx,
    config: &Config,
    ids: &Snowflake,
    dm_channel_id: &str,
    user_id: &str,
    reopen: bool,
) -> DbResult<()> {
    relay::dm_close(
        tx,
        relay::Relay { config, ids },
        dm_channel_id,
        user_id,
        reopen,
    )
}

/// Пересылка создания или правки личного сообщения (`queueDmRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn queue_dm_message(
    tx: &mut dyn Tx,
    config: &Config,
    ids: &Snowflake,
    dm_channel_id: &str,
    message_id: &str,
    update: bool,
) -> DbResult<()> {
    relay::dm_message(
        tx,
        relay::Relay { config, ids },
        dm_channel_id,
        message_id,
        update,
    )
}

/// На что указывает изменение личного сообщения (`dmMessageMutationTarget`).
/// Берётся до удаления сообщения.
///
/// # Errors
/// Ошибка базы.
pub fn mutation_target(
    tx: &mut dyn Tx,
    config: &Config,
    dm_channel_id: &str,
    message_id: &str,
    actor_id: &str,
) -> DbResult<Option<Value>> {
    let Some(federated) = dm_federated_id(tx, dm_channel_id)? else {
        return Ok(None);
    };
    let row = tx.query_one(
        "SELECT home_user_id, home_instance FROM users WHERE id = ?",
        params![actor_id],
    )?;
    let Some(row) = row else { return Ok(None) };
    let actor = match row
        .opt_text("home_instance")?
        .filter(|home| !home.is_empty())
    {
        None => json!({"homeUserId": actor_id, "homeInstance": our_origin(config)}),
        Some(home) => match row.opt_text("home_user_id")?.filter(|id| !id.is_empty()) {
            Some(home_user) => json!({"homeUserId": home_user, "homeInstance": home}),
            None => return Ok(None),
        },
    };
    Ok(Some(json!({
        "message": message_ref(tx, config, message_id)?,
        "federatedId": federated,
        "actor": actor,
    })))
}

/// Пересылка удаления личного сообщения (`queueDmMessageDeleteRelay`).
///
/// # Errors
/// Ошибка базы.
pub fn queue_dm_delete(
    tx: &mut dyn Tx,
    config: &Config,
    ids: &Snowflake,
    dm_channel_id: &str,
    message_id: &str,
    target: Option<Value>,
) -> DbResult<()> {
    relay::dm_delete(
        tx,
        relay::Relay { config, ids },
        dm_channel_id,
        message_id,
        target,
    )
}

/// Профиль пользователя для других инстансов (`FederationProfileUpdatePayload`).
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn profile_payload(config: &Config, user: &crate::db::Row) -> DbResult<Value> {
    let origin = our_origin(config);
    let asset = |column: &str| -> DbResult<Option<String>> {
        Ok(user.opt_text(column)?.map(|value| {
            if value.is_empty() || value.starts_with("http") {
                value
            } else {
                format!("{origin}/api/uploads/{value}")
            }
        }))
    };
    Ok(json!({"profileUpdate": {
        "homeUserId": user.text("id")?,
        "homeInstance": origin,
        "profileUpdatedAt": user.opt_int("profile_updated_at")?.unwrap_or_else(now_ms),
        "username": user.text("username")?,
        "displayName": user.opt_text("display_name")?,
        "avatar": asset("avatar")?,
        "banner": asset("banner")?,
        "accentColor": user.opt_text("accent_color")?,
        "avatarColor": user.opt_text("avatar_color")?,
        "bio": user.opt_text("bio")?,
    }}))
}
