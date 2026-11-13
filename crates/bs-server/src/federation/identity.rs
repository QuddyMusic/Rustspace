//! Чьё это событие и кто это у нас (`routes/federation/identity.ts`,
//! `routes/federation/stubName.ts`).
//!
//! Пользователь другого инстанса представлен здесь копией — строкой `users`
//! с `home_instance` (имя узла дома) и `home_user_id` (его идентификатор
//! дома). Событие от пира принимается, только если пир говорит за своих
//! пользователей (или за наших, которых он доказуемо знает).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::our_origin;
use super::reset::domain_of;
use crate::config::Config;
use crate::db::{DbResult, Row, Tx};
use crate::federation_auth::normalize_origin;
use crate::hub::Outbox;
use crate::params;
use crate::routes::users::sanitize_user;
use crate::state::{AppState, now_ms};
use crate::ws::profile_targets;

/// Хеш пароля копии: войти под ней нельзя.
pub const REPLICATED_PASSWORD_HASH: &str = "!federation-replicated";
/// Сколько номерных суффиксов пробуется до случайного.
const MAX_NUMBERED_SUFFIX: u32 = 10;

static HANDLE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new("^[a-z0-9_]+$").ok());
static SUFFIXED: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new("^([a-z0-9_]+)~([0-9a-f]+)$").ok());

fn is_handle(text: &str) -> bool {
    HANDLE.as_ref().is_some_and(|re| re.is_match(text))
}

/// Имя из подсказки, если это допустимое имя (`handleFromHint`).
#[must_use]
pub fn handle_from_hint(name: Option<&str>) -> Option<String> {
    let handle = name?.trim().to_lowercase();
    (!handle.is_empty() && is_handle(&handle)).then_some(handle)
}

fn suffixed_handle_of(local: &str) -> Option<String> {
    SUFFIXED
        .as_ref()?
        .captures(local)?
        .get(1)
        .map(|m| m.as_str().to_owned())
}

/// Участник события: идентификатор и инстанс дома (`RelayActor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// Идентификатор пользователя на его инстансе.
    pub home_user_id: String,
    /// Инстанс (адрес или имя узла).
    pub home_instance: String,
}

impl Actor {
    /// Из объекта события `{homeUserId, homeInstance}`; `None` — поля не строки.
    #[must_use]
    pub fn from_value(value: Option<&Value>) -> Option<Self> {
        let value = value?;
        Some(Self {
            home_user_id: value.get("homeUserId")?.as_str()?.to_owned(),
            home_instance: value.get("homeInstance")?.as_str()?.to_owned(),
        })
    }
}

/// Узел нашего инстанса, как он входит в идентичность (`getOurIdentityDomain`).
#[must_use]
pub fn our_identity_domain(config: &Config) -> String {
    match config.domain.as_deref().filter(|domain| !domain.is_empty()) {
        Some(domain) => domain.to_lowercase(),
        None => domain_of(&our_origin(config)).to_lowercase(),
    }
}

/// Наш ли это узел (`isOwnDomain`).
#[must_use]
pub fn is_own_domain(config: &Config, domain: &str) -> bool {
    !domain.is_empty()
        && (domain == domain_of(&our_origin(config)).to_lowercase()
            || domain == our_identity_domain(config))
}

/// Одна ли это идентичность (`sameRelayActor`).
#[must_use]
pub fn same_actor(a: &Actor, b: &Actor) -> bool {
    if a.home_user_id != b.home_user_id {
        return false;
    }
    let left = domain_of(&a.home_instance).to_lowercase();
    !left.is_empty() && left == domain_of(&b.home_instance).to_lowercase()
}

/// Идентичность пользователя для событий (`relayActorOfUser`).
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn actor_of_user(config: &Config, user: &Row) -> DbResult<Option<Actor>> {
    let Some(home) = user
        .opt_text("home_instance")?
        .filter(|home| !home.is_empty())
    else {
        return Ok(Some(Actor {
            home_user_id: user.text("id")?,
            home_instance: our_origin(config),
        }));
    };
    Ok(user
        .opt_text("home_user_id")?
        .filter(|id| !id.is_empty())
        .map(|id| Actor {
            home_user_id: id,
            home_instance: home,
        }))
}

/// Кто это у нас.
#[derive(Debug, Clone)]
pub enum Resolved {
    /// Пользователь найден.
    Found(Box<Row>),
    /// Такого нет.
    Unknown,
    /// Идентификатор принадлежит другой идентичности.
    Mismatch,
}

impl Resolved {
    /// Найденный пользователь.
    #[must_use]
    pub fn user(&self) -> Option<&Row> {
        match self {
            Self::Found(user) => Some(user),
            _ => None,
        }
    }
}

fn former_identity(user: &Row) -> DbResult<Option<Actor>> {
    match (
        user.opt_text("detached_home_user_id")?,
        user.opt_text("detached_home_instance")?,
    ) {
        (Some(id), Some(home)) if !id.is_empty() && !home.is_empty() => Ok(Some(Actor {
            home_user_id: id,
            home_instance: home,
        })),
        _ => Ok(None),
    }
}

/// Пользователь, о котором говорит событие (`resolveRelayActor`).
///
/// # Errors
/// Ошибка базы.
pub fn resolve_actor(tx: &mut dyn Tx, config: &Config, actor: &Actor) -> DbResult<Resolved> {
    let candidates = tx.query(
        "SELECT * FROM users WHERE (home_user_id = ? OR (id = ? AND home_instance IS NULL) \
         OR detached_home_user_id = ?) AND is_deleted = 0",
        params![
            &actor.home_user_id,
            &actor.home_user_id,
            &actor.home_user_id
        ],
    )?;
    if candidates.is_empty() {
        return Ok(Resolved::Unknown);
    }
    let actor_domain = domain_of(&actor.home_instance).to_lowercase();
    for candidate in candidates {
        if former_identity(&candidate)?.is_some_and(|former| same_actor(&former, actor)) {
            return Ok(Resolved::Found(Box::new(candidate)));
        }
        if candidate
            .opt_text("home_instance")?
            .is_none_or(|home| home.is_empty())
        {
            if candidate.text("id")? == actor.home_user_id && is_own_domain(config, &actor_domain) {
                return Ok(Resolved::Found(Box::new(candidate)));
            }
            continue;
        }
        if actor_of_user(config, &candidate)?.is_some_and(|identity| same_actor(&identity, actor)) {
            return Ok(Resolved::Found(Box::new(candidate)));
        }
    }
    Ok(Resolved::Mismatch)
}

/// Знает ли пир нашего пользователя (`localUserStandingOnPeer`).
fn standing_on_peer(tx: &mut dyn Tx, home_user_id: &str, peer: &str) -> DbResult<Standing> {
    let Some(user) = tx.query_one(
        "SELECT id, replicated_instances FROM users WHERE id = ? AND home_instance IS NULL AND is_deleted = 0",
        params![home_user_id],
    )?
    else {
        return Ok(Standing::NoSuchUser);
    };
    let Some(host) = normalize_origin(Some(peer)) else {
        return Ok(Standing::Unproven);
    };
    for row in tx.query(
        "SELECT origin FROM user_federation_registry WHERE user_id = ?",
        params![user.text("id")?],
    )? {
        if normalize_origin(Some(&row.text("origin")?)).as_ref() == Some(&host) {
            return Ok(Standing::Proven);
        }
    }
    if let Some(list) = user.opt_text("replicated_instances")?
        && let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&list)
    {
        for item in items {
            if let Some(origin) = item.get("origin").and_then(Value::as_str)
                && normalize_origin(Some(origin)).as_ref() == Some(&host)
            {
                return Ok(Standing::Proven);
            }
        }
    }
    Ok(Standing::Unproven)
}

enum Standing {
    Proven,
    Unproven,
    NoSuchUser,
}

/// Почему пир не может говорить за этого участника (`attributionRefusal`).
/// `None` — может.
///
/// # Errors
/// Ошибка базы.
pub fn attribution_refusal(
    tx: &mut dyn Tx,
    config: &Config,
    actor: Option<&Value>,
    source: &str,
) -> DbResult<Option<&'static str>> {
    const MISMATCH: Option<&str> = Some("attribution_mismatch");
    let Some(actor) = actor.and_then(Value::as_object) else {
        return Ok(MISMATCH);
    };
    let (Some(id), Some(home)) = (
        actor
            .get("homeUserId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty()),
        actor
            .get("homeInstance")
            .and_then(Value::as_str)
            .filter(|home| !home.is_empty()),
    ) else {
        return Ok(MISMATCH);
    };
    let author_domain = domain_of(home).to_lowercase();
    let source_domain = domain_of(source).to_lowercase();
    if author_domain.is_empty() || source_domain.is_empty() {
        return Ok(MISMATCH);
    }
    if author_domain == source_domain {
        let found = resolve_actor(
            tx,
            config,
            &Actor {
                home_user_id: id.to_owned(),
                home_instance: home.to_owned(),
            },
        )?;
        return Ok(match found {
            Resolved::Mismatch => MISMATCH,
            Resolved::Found(user) if user.opt_flag("federation_home_orphaned")? == Some(true) => {
                MISMATCH
            }
            _ => None,
        });
    }
    let ours = domain_of(&our_origin(config)).to_lowercase();
    if !ours.is_empty() && author_domain == ours {
        return Ok(match standing_on_peer(tx, id, source)? {
            Standing::Proven => None,
            Standing::Unproven => Some("attribution_unproven"),
            Standing::NoSuchUser => MISMATCH,
        });
    }
    Ok(MISMATCH)
}

/// Наш пользователь или копия по идентификатору дома (`resolveLocalUser`).
///
/// # Errors
/// Ошибка базы.
pub fn resolve_local_user(tx: &mut dyn Tx, home_user_id: &str) -> DbResult<Option<Row>> {
    let candidates = tx.query(
        "SELECT * FROM users WHERE (home_user_id = ? OR (id = ? AND home_instance IS NULL)) AND is_deleted = 0",
        params![home_user_id, home_user_id],
    )?;
    if candidates.len() <= 1 {
        return Ok(candidates.into_iter().next());
    }
    let mut first = None;
    for candidate in candidates {
        if candidate.opt_text("home_user_id")?.as_deref() == Some(home_user_id) {
            return Ok(Some(candidate));
        }
        first.get_or_insert(candidate);
    }
    Ok(first)
}

fn local_part_of(user: &Row) -> DbResult<Option<String>> {
    let Some(home) = user
        .opt_text("home_instance")?
        .filter(|home| !home.is_empty())
    else {
        return Ok(None);
    };
    let suffix = format!("@{home}").to_lowercase();
    Ok(user
        .text("username")?
        .to_lowercase()
        .strip_suffix(&suffix)
        .map(str::to_owned))
}

fn is_renameable_replica(user: &Row) -> DbResult<bool> {
    Ok(user
        .opt_text("home_instance")?
        .is_some_and(|home| !home.is_empty())
        && user
            .opt_text("home_user_id")?
            .is_some_and(|id| !id.is_empty())
        && user.opt_text("password_hash")?.as_deref() == Some(REPLICATED_PASSWORD_HASH)
        && user.opt_flag("federation_home_orphaned")? != Some(true))
}

/// Копия с именем-заглушкой (`isPlaceholderNamedStub`).
pub fn is_placeholder_named(user: &Row) -> DbResult<bool> {
    if !is_renameable_replica(user)? {
        return Ok(false);
    }
    let Some(local) = local_part_of(user)? else {
        return Ok(false);
    };
    if suffixed_handle_of(&local).is_some() {
        return Ok(false);
    }
    let home_user = user.opt_text("home_user_id")?.unwrap_or_default();
    Ok(local == home_user.to_lowercase() || !is_handle(&local))
}

fn is_suffixed_for(user: &Row, handle: &str) -> DbResult<bool> {
    Ok(is_renameable_replica(user)?
        && local_part_of(user)?
            .is_some_and(|local| suffixed_handle_of(&local).as_deref() == Some(handle)))
}

/// Свободное имя `handle@domain` (`firstFreeUsername`).
///
/// # Errors
/// Ошибка базы.
pub fn first_free_username(
    tx: &mut dyn Tx,
    handle: &str,
    domain: &str,
    owner: Option<(&str, &str)>,
) -> DbResult<String> {
    let name_for = |local: &str| format!("{local}@{domain}").to_lowercase();
    let taken = |tx: &mut dyn Tx, candidate: &str| -> DbResult<bool> {
        Ok(tx
            .query_one(
                "SELECT id FROM users WHERE username = ?",
                params![candidate],
            )?
            .map(|row| row.text("id"))
            .transpose()?
            .is_some_and(|holder| owner.is_none_or(|(id, _)| id != holder)))
    };
    let mut username = name_for(handle);
    let mut attempt = 0;
    while taken(tx, &username)? {
        attempt += 1;
        if attempt > MAX_NUMBERED_SUFFIX {
            if let Some((_, own)) = owner {
                let own = own.to_lowercase();
                let suffix = format!("@{domain}").to_lowercase();
                if own
                    .strip_suffix(&suffix)
                    .and_then(suffixed_handle_of)
                    .as_deref()
                    == Some(handle)
                {
                    return Ok(own);
                }
            }
            let random: [u8; 4] = rand::random();
            return Ok(name_for(&format!("{handle}~{}", hex::encode(random))));
        }
        username = name_for(&format!("{handle}~{attempt}"));
    }
    Ok(username)
}

/// Переименовывает копию с именем-заглушкой в её настоящее имя
/// (`applyPlaceholderRename`). Возвращает обновлённую строку, если имя
/// изменилось.
///
/// # Errors
/// Ошибка базы.
pub fn apply_placeholder_rename(
    tx: &mut dyn Tx,
    user: &Row,
    username: Option<&str>,
) -> DbResult<Option<Row>> {
    let Some(home) = user
        .opt_text("home_instance")?
        .filter(|home| !home.is_empty())
    else {
        return Ok(None);
    };
    let Some(handle) = handle_from_hint(username) else {
        return Ok(None);
    };
    if !is_placeholder_named(user)? && !is_suffixed_for(user, &handle)? {
        return Ok(None);
    }
    let id = user.text("id")?;
    let current = user.text("username")?;
    let renamed = first_free_username(tx, &handle, &home, Some((&id, &current)))?;
    if renamed == current {
        return Ok(None);
    }
    tx.execute(
        "UPDATE users SET username = ? WHERE id = ?",
        params![&renamed, &id],
    )?;
    tracing::info!(
        user = id,
        from = current,
        to = renamed,
        "копия переименована"
    );
    tx.query_one("SELECT * FROM users WHERE id = ?", params![&id])
}

/// Объявляет обновлённую строку всем, кому она видна (`announceUserUpdated`).
///
/// # Errors
/// Ошибка базы.
pub fn announce_user_updated(tx: &mut dyn Tx, out: &mut Outbox, user: &Row) -> DbResult<()> {
    let event = serde_json::json!({"type": "user_updated", "user": sanitize_user(user, false)?});
    let id = user.text("id")?;
    for target in profile_targets(tx, &id)? {
        out.to_user(&target, &event);
    }
    Ok(())
}

static LEGACY_SUFFIXED: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new("^[a-z0-9_]+_(?:[1-9]|10|[0-9a-f]{8})$").ok());

/// Имя копии может нести старый суффикс `_N` (`mayCarryLegacySuffix`).
///
/// # Errors
/// В строке нет нужных столбцов.
pub fn may_carry_legacy_suffix(user: &Row) -> DbResult<bool> {
    if user.opt_text("home_instance")?.is_none_or(|h| h.is_empty())
        || user.opt_text("home_user_id")?.is_none_or(|h| h.is_empty())
        || user.opt_flag("federation_home_orphaned")? == Some(true)
    {
        return Ok(false);
    }
    Ok(local_part_of(user)?.is_some_and(|local| {
        LEGACY_SUFFIXED
            .as_ref()
            .is_some_and(|re| re.is_match(&local))
    }))
}

fn same_identity(a: &Row, b: &Row) -> DbResult<bool> {
    let pair = |row: &Row| -> DbResult<Option<(String, String)>> {
        Ok(
            match (
                row.opt_text("home_user_id")?,
                row.opt_text("home_instance")?,
            ) {
                (Some(id), Some(home)) if !id.is_empty() && !home.is_empty() => {
                    Some((id, home.to_lowercase()))
                }
                _ => None,
            },
        )
    };
    Ok(match (pair(a)?, pair(b)?) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    })
}

/// Итог присвоения имени (`HandleClaim`).
#[derive(Debug, Clone)]
pub enum Claim {
    /// Имя теперь у строки; `moved` — копия, уступившая имя.
    Claimed(String, Option<Row>),
    /// Имя занято аккаунтом, который входит под ним, или копией той же
    /// идентичности.
    Held(String),
}

/// Присваивает строке имя `handle@domain`, отодвигая копию, которая его
/// занимала (`claimHandleName`).
///
/// # Errors
/// Ошибка базы.
pub fn claim_handle_name(
    tx: &mut dyn Tx,
    row: &Row,
    handle: &str,
    domain: &str,
) -> DbResult<Claim> {
    let username = format!("{handle}@{domain}").to_lowercase();
    let id = row.text("id")?;
    let mut moved = None;
    if let Some(holder) =
        tx.query_one("SELECT * FROM users WHERE username = ?", params![&username])?
        && holder.text("id")? != id
    {
        if holder.opt_text("password_hash")?.as_deref() != Some(REPLICATED_PASSWORD_HASH)
            || same_identity(&holder, row)?
        {
            return Ok(Claim::Held(holder.text("id")?));
        }
        let aside = first_free_username(tx, handle, domain, None)?;
        let holder_id = holder.text("id")?;
        tx.execute(
            "UPDATE users SET username = ? WHERE id = ?",
            params![&aside, &holder_id],
        )?;
        tracing::info!(user = holder_id, to = aside, "копия уступила имя");
        moved = tx.query_one("SELECT * FROM users WHERE id = ?", params![&holder_id])?;
    }
    if row.text("username")? != username {
        tx.execute(
            "UPDATE users SET username = ? WHERE id = ?",
            params![&username, &id],
        )?;
    }
    Ok(Claim::Claimed(username, moved))
}

/// Имя, которое сообщил дом (`applyHomeHandle`): обновлённая строка и
/// копия, уступившая имя.
///
/// # Errors
/// Ошибка базы.
pub fn apply_home_handle(
    tx: &mut dyn Tx,
    user: &Row,
    home_username: Option<&str>,
) -> DbResult<(Row, Option<Row>)> {
    let unchanged = || Ok((user.clone(), None));
    let Some(handle) = handle_from_hint(home_username) else {
        return unchanged();
    };
    let (Some(home), Some(_)) = (
        user.opt_text("home_instance")?.filter(|h| !h.is_empty()),
        user.opt_text("home_user_id")?.filter(|h| !h.is_empty()),
    ) else {
        return unchanged();
    };
    if user.opt_flag("federation_home_orphaned")? == Some(true) {
        return unchanged();
    }
    let id = user.text("id")?;
    if is_renameable_replica(user)? {
        let current = user.text("username")?;
        let username = first_free_username(tx, &handle, &home, Some((&id, &current)))?;
        if username == current {
            return unchanged();
        }
        tx.execute(
            "UPDATE users SET username = ? WHERE id = ?",
            params![&username, &id],
        )?;
        let row = tx.query_one("SELECT * FROM users WHERE id = ?", params![&id])?;
        return Ok((row.unwrap_or_else(|| user.clone()), None));
    }
    match claim_handle_name(tx, user, &handle, &home)? {
        Claim::Held(holder) => {
            tracing::warn!(user = id, holder, "аккаунт сохраняет имя: имя дома занято");
            unchanged()
        }
        Claim::Claimed(_, moved) => {
            let row = tx.query_one("SELECT * FROM users WHERE id = ?", params![&id])?;
            Ok((row.unwrap_or_else(|| user.clone()), moved))
        }
    }
}

/// Подсказки о пользователе из события.
#[derive(Debug, Clone, Default)]
pub struct Hints {
    /// Имя на его инстансе.
    pub username: Option<String>,
    /// Статус, который объявил дом.
    pub status: Option<String>,
    /// Пользователь удалён.
    pub deleted: bool,
    /// Дом только что спрашивали: новую копию не нужно спрашивать снова.
    pub home_asked: bool,
}

impl Hints {
    /// Из профиля участника события (`profile`).
    #[must_use]
    pub fn from_profile(profile: Option<&Value>) -> Self {
        let text = |key: &str| {
            profile
                .and_then(|p| p.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        Self {
            username: text("username"),
            status: text("status"),
            deleted: profile
                .and_then(|p| p.get("deleted"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            home_asked: false,
        }
    }
}

fn lookup(tx: &mut dyn Tx, config: &Config, actor: &Actor, hints: &Hints) -> DbResult<Resolved> {
    let identity = resolve_actor(tx, config, actor)?;
    if !matches!(identity, Resolved::Unknown) {
        return Ok(identity);
    }
    let Some(hint) = hints.username.as_deref().filter(|hint| !hint.is_empty()) else {
        return Ok(Resolved::Unknown);
    };
    let domain = domain_of(&actor.home_instance);
    let hint = hint.to_lowercase();
    let candidates = tx.query(
        "SELECT * FROM users WHERE home_instance = ? AND is_deleted = 0 AND home_user_id IS NULL \
         AND federation_home_orphaned = 0",
        params![&domain],
    )?;
    let mut matched = Vec::new();
    for candidate in candidates {
        let username = candidate.text("username")?;
        let local = match username.find('@') {
            Some(at) => username[..at].to_lowercase(),
            None => username.to_lowercase(),
        };
        if local == hint {
            matched.push(candidate);
        }
    }
    if matched.len() <= 1 {
        return Ok(matched
            .into_iter()
            .next()
            .map_or(Resolved::Unknown, |user| Resolved::Found(Box::new(user))));
    }
    let score = |user: &Row| -> (i32, usize) {
        let real = i32::from(
            user.opt_text("password_hash").ok().flatten().as_deref()
                != Some(REPLICATED_PASSWORD_HASH),
        );
        let filled = ["display_name", "avatar", "banner", "bio"]
            .iter()
            .filter(|column| {
                user.opt_text(column)
                    .ok()
                    .flatten()
                    .is_some_and(|value| !value.is_empty())
            })
            .count();
        (real, filled)
    };
    matched.sort_by_key(|user| std::cmp::Reverse(score(user)));
    Ok(matched
        .into_iter()
        .next()
        .map_or(Resolved::Unknown, |user| Resolved::Found(Box::new(user))))
}

/// Записывает идентификатор дома копии, у которой его не было
/// (`backfillHomeUserId`).
fn backfill_home_user_id(tx: &mut dyn Tx, user: Row, home_user_id: &str) -> DbResult<Row> {
    let current = user.opt_text("home_user_id")?;
    if current.as_deref() == Some(home_user_id)
        || user.opt_flag("federation_home_orphaned")? == Some(true)
    {
        return Ok(user);
    }
    let id = user.text("id")?;
    if current.is_some_and(|current| !current.is_empty()) {
        tracing::warn!(user = id, "идентификатор дома копии не перезаписывается");
        return Ok(user);
    }
    tx.execute(
        "UPDATE users SET home_user_id = ? WHERE id = ?",
        params![home_user_id, &id],
    )?;
    Ok(tx
        .query_one("SELECT * FROM users WHERE id = ?", params![&id])?
        .unwrap_or(user))
}

/// Итог поиска или создания копии.
#[derive(Debug, Clone)]
pub struct Replica {
    /// Строка пользователя.
    pub user: Row,
    /// Имя изменилось: о строке нужно объявить (`announceUserUpdated`).
    pub renamed: bool,
    /// Копия только что создана: её профиль стоит спросить у дома.
    pub created: bool,
}

/// Находит пользователя или создаёт копию (`resolveOrCreateReplicatedUser`).
///
/// # Errors
/// Ошибка базы.
pub fn resolve_or_create(
    tx: &mut dyn Tx,
    app: &AppState,
    out: &mut Outbox,
    actor: &Actor,
    hints: &Hints,
) -> DbResult<Option<Replica>> {
    let config = &app.config;
    match lookup(tx, config, actor, hints)? {
        Resolved::Found(user) => {
            let user = backfill_home_user_id(tx, *user, &actor.home_user_id)?;
            return Ok(Some(
                match apply_placeholder_rename(tx, &user, hints.username.as_deref())? {
                    Some(renamed) => {
                        announce_user_updated(tx, out, &renamed)?;
                        Replica {
                            user: renamed,
                            renamed: true,
                            created: false,
                        }
                    }
                    None => Replica {
                        user,
                        renamed: false,
                        created: false,
                    },
                },
            ));
        }
        Resolved::Mismatch => {
            tracing::warn!(
                home_user = actor.home_user_id,
                "идентификатор принадлежит другой идентичности"
            );
            return Ok(None);
        }
        Resolved::Unknown => {}
    }
    if hints.deleted {
        return Ok(None);
    }
    let domain = domain_of(&actor.home_instance);
    if domain.to_lowercase() == our_identity_domain(config) {
        tracing::info!(
            home_user = actor.home_user_id,
            "копия с нашим доменом не создаётся"
        );
        return Ok(None);
    }
    let tombstoned = tx.query_one(
        "SELECT is_deleted FROM users WHERE (home_user_id = ? AND home_instance = ?) \
         OR (detached_home_user_id = ? AND detached_home_instance = ?)",
        params![&actor.home_user_id, &domain, &actor.home_user_id, &domain],
    )?;
    if tombstoned.is_some_and(|row| row.opt_flag("is_deleted").ok().flatten() == Some(true)) {
        return Ok(None);
    }
    let local = handle_from_hint(hints.username.as_deref())
        .unwrap_or_else(|| actor.home_user_id.to_lowercase());
    let username = first_free_username(tx, &local, &domain, None)?;
    let id = app.ids.next();
    tx.execute(
        "INSERT INTO users (id, username, display_name, password_hash, status, is_admin, home_instance, \
         home_user_id, created_at) VALUES (?, ?, NULL, ?, 'offline', 0, ?, ?, ?)",
        params![
            &id,
            &username,
            REPLICATED_PASSWORD_HASH,
            &domain,
            &actor.home_user_id,
            now_ms()
        ],
    )?;
    if let Some(status) = hints
        .status
        .as_deref()
        .filter(|status| super::presence::is_status(status))
    {
        super::presence::project(tx, &app.hub, &app.federation.projections, &id, status)?;
    }
    tracing::info!(user = id, username, domain, "создана копия пользователя");
    if !hints.home_asked {
        app.federation.pulls.schedule(&id);
    }
    let Some(user) = tx.query_one("SELECT * FROM users WHERE id = ?", params![&id])? else {
        return Ok(None);
    };
    Ok(Some(Replica {
        user,
        renamed: false,
        created: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_and_suffixes() {
        assert_eq!(handle_from_hint(Some(" Kai ")).as_deref(), Some("kai"));
        assert_eq!(handle_from_hint(Some("k.ai")), None);
        assert_eq!(handle_from_hint(None), None);
        assert_eq!(suffixed_handle_of("kai~3").as_deref(), Some("kai"));
        assert_eq!(suffixed_handle_of("kai~zz"), None);
    }

    #[test]
    fn actors_compare_by_host() {
        let a = Actor {
            home_user_id: "1".into(),
            home_instance: "https://A.example".into(),
        };
        let b = Actor {
            home_user_id: "1".into(),
            home_instance: "a.example".into(),
        };
        let c = Actor {
            home_user_id: "2".into(),
            home_instance: "a.example".into(),
        };
        assert!(same_actor(&a, &b));
        assert!(!same_actor(&a, &c));
    }
}
