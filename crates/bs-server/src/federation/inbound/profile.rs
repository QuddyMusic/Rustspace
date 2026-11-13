//! Профили копий пользователей (`routes/federation/profile.ts`).

use std::net::SocketAddr;
use std::time::Duration;

use serde_json::{Value, json};
use url::{Host, Url};

use super::{Ctx, Step, accept, domain, non_empty, reject, text, user_row};
use crate::db::{DbResult, Row, Tx};
use crate::federation::identity::{self, Resolved, apply_placeholder_rename, handle_from_hint};
use crate::params;
use crate::routes::users::sanitize_user;
use crate::state::AppState;
use crate::ws::profile_targets;

/// Наибольший файл профиля (`MAX_PROFILE_ASSET_BYTES`).
pub const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;
const ASSET_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REDIRECTS: usize = 5;

/// Проверяет адрес для исходящего запроса (`validateExternalUrl`): `http(s)`,
/// имя разрешается в публичный адрес. Возвращает адрес для соединения.
async fn external(url: &Url) -> Result<(String, SocketAddr), String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Invalid URL scheme".to_owned());
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let (name, ip) = match url.host() {
        Some(Host::Ipv4(ip)) => (ip.to_string(), ip.into()),
        Some(Host::Ipv6(ip)) => (ip.to_string(), ip.into()),
        Some(Host::Domain(name)) => {
            let found = tokio::net::lookup_host((name, port))
                .await
                .map_err(|_| "DNS lookup failed".to_owned())?
                .next()
                .ok_or_else(|| "DNS lookup failed".to_owned())?;
            (name.to_owned(), found.ip())
        }
        None => return Err("Invalid URL".to_owned()),
    };
    if !crate::net::is_public(ip) {
        return Err("Private IP not allowed".to_owned());
    }
    Ok((name, SocketAddr::new(ip, port)))
}

/// Ответ на запрос к внешнему адресу.
#[derive(Debug)]
pub struct Fetched {
    /// Код ответа.
    pub status: u16,
    /// Заголовки.
    pub headers: reqwest::header::HeaderMap,
    /// Ответ, тело ещё не прочитано.
    pub response: reqwest::Response,
}

/// Запрос без доступа во внутреннюю сеть (`safeFetch`): адрес и каждое
/// перенаправление проверяются, соединение идёт на проверенный адрес.
///
/// # Errors
/// Адрес недопустим, соединение не удалось или перенаправлений слишком много.
pub async fn safe_fetch(app: &AppState, url: &str, timeout: Duration) -> Result<Fetched, String> {
    let mut current = Url::parse(url).map_err(|_| "Invalid URL".to_owned())?;
    for _ in 0..=MAX_REDIRECTS {
        let (name, address) = external(&current).await?;
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(timeout)
            .resolve(&name, address);
        if let Some(pem) = &app.config.extra_ca_certs {
            for certificate in
                reqwest::Certificate::from_pem_bundle(pem).map_err(|error| error.to_string())?
            {
                builder = builder.add_root_certificate(certificate);
            }
        }
        let client = builder.build().map_err(|error| error.to_string())?;
        let response = client
            .get(current.clone())
            .send()
            .await
            .map_err(|_| "fetch failed".to_owned())?;
        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            let Some(location) = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
            else {
                let headers = response.headers().clone();
                return Ok(Fetched {
                    status,
                    headers,
                    response,
                });
            };
            current = current
                .join(location)
                .map_err(|_| "Invalid URL".to_owned())?;
            continue;
        }
        let headers = response.headers().clone();
        return Ok(Fetched {
            status,
            headers,
            response,
        });
    }
    Err("Too many redirects".to_owned())
}

/// Скачивает картинку профиля с инстанса-источника (`downloadProfileAsset`).
/// Имя локального файла или `None`.
pub async fn download_asset(app: &AppState, url: &str, source: &str) -> Option<String> {
    let (Ok(parsed), Ok(base)) = (Url::parse(url), Url::parse(source)) else {
        return None;
    };
    if parsed.host_str() != base.host_str() {
        tracing::warn!(url, source, "файл профиля с чужого узла не скачивается");
        return None;
    }
    let ext = std::path::Path::new(parsed.path())
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
        .map_or_else(|| ".webp".to_owned(), |ext| format!(".{ext}"));
    let name = format!("{}{ext}", app.ids.next());
    let fetched = match safe_fetch(app, url, ASSET_TIMEOUT).await {
        Ok(fetched) => fetched,
        Err(error) => {
            tracing::warn!(url, error, "файл профиля не скачан");
            return None;
        }
    };
    if !(200..300).contains(&fetched.status) {
        return None;
    }
    let kind = fetched
        .headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !kind.starts_with("image/") {
        tracing::warn!(url, kind, "файл профиля не картинка");
        return None;
    }
    if let Some(declared) = fetched
        .headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        && declared > MAX_ASSET_BYTES as u64
    {
        return None;
    }
    let (body, truncated) =
        crate::net::read_limited(fetched.response, MAX_ASSET_BYTES + 1, ASSET_TIMEOUT).await;
    if truncated || body.len() > MAX_ASSET_BYTES {
        tracing::warn!(url, "файл профиля больше предела");
        return None;
    }
    let path = crate::files::upload_path(&app.config, &name)?;
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    match tokio::fs::write(&path, &body).await {
        Ok(()) => Some(name),
        Err(error) => {
            tracing::warn!(url, %error, "файл профиля не записан");
            None
        }
    }
}

fn base_url(home: &str) -> String {
    if home.starts_with("http") {
        home.to_owned()
    } else {
        format!("https://{home}")
    }
}

/// Абсолютный адрес картинки на доме и её локальная копия, если удалось.
fn resolve_asset(
    cx: &mut Ctx<'_>,
    snapshot: &str,
    base: &str,
    uploads_only: bool,
) -> Result<String, super::Stop> {
    let absolute = if snapshot.starts_with("http") {
        snapshot.to_owned()
    } else if !uploads_only && snapshot.starts_with('/') {
        format!("{base}{snapshot}")
    } else {
        format!("{base}/api/uploads/{snapshot}")
    };
    Ok(cx.asset(&absolute, base)?.unwrap_or(absolute))
}

/// Объявляет строку пользователя всем, кто её видит.
fn announce(cx: &mut Ctx<'_>, user: &Row, with_self: bool) -> DbResult<()> {
    let event = json!({"type": "user_updated", "user": sanitize_user(user, false)?});
    let id = user.text("id")?;
    let mut targets = profile_targets(cx.tx, &id)?;
    if with_self && !targets.contains(&id) {
        targets.push(id);
    }
    for target in targets {
        cx.to_user(&target, &event)?;
    }
    Ok(())
}

const FILL_COLUMNS: [(&str, &str); 5] = [
    ("display_name", "displayName"),
    ("avatar", "avatar"),
    ("avatar_color", "avatarColor"),
    ("banner", "banner"),
    ("bio", "bio"),
];

/// Заполняет пустые поля профиля копии из снимка в событии
/// (`hydrateReplicatedUserProfile`).
///
/// # Errors
/// Ошибка базы или нужен файл.
pub fn hydrate(cx: &mut Ctx<'_>, user_in: Row, profile: &Value) -> Result<Row, super::Stop> {
    let Some(home) = user_in.opt_text("home_instance")?.filter(|h| !h.is_empty()) else {
        return Ok(user_in);
    };
    if user_in.opt_flag("federation_home_orphaned")? == Some(true) {
        return Ok(user_in);
    }
    let username = text(profile, "username");
    let (user, renamed) = match apply_placeholder_rename(cx.tx, &user_in, username)? {
        Some(row) => (row, true),
        None => (user_in, false),
    };
    let base = base_url(&home);
    let mut fills: Vec<(&str, String)> = Vec::new();
    if user.opt_int("profile_updated_at")?.is_none() {
        let empty = |column: &str| -> DbResult<bool> {
            Ok(user.opt_text(column)?.is_none_or(|v| v.is_empty()))
        };
        let display = non_empty(profile, "displayName")
            .map(str::to_owned)
            .or_else(|| handle_from_hint(username));
        if let Some(display) = display
            && empty("display_name")?
        {
            fills.push(("display_name", display));
        }
        if let Some(avatar) = non_empty(profile, "avatar")
            && empty("avatar")?
        {
            fills.push(("avatar", resolve_asset(cx, avatar, &base, false)?));
        }
        if let Some(color) = non_empty(profile, "avatarColor")
            && empty("avatar_color")?
        {
            fills.push(("avatar_color", color.to_owned()));
        }
        if let Some(banner) = non_empty(profile, "banner")
            && empty("banner")?
        {
            fills.push(("banner", resolve_asset(cx, banner, &base, false)?));
        }
        if let Some(bio) = non_empty(profile, "bio")
            && empty("bio")?
        {
            fills.push(("bio", bio.to_owned()));
        }
    }
    if fills.is_empty() {
        if renamed {
            identity::announce_user_updated(cx.tx, cx.out, &user)?;
        }
        return Ok(user);
    }
    let id = user.text("id")?;
    let mut sets = Vec::new();
    let mut values: Vec<crate::db::Value> = Vec::new();
    for (column, value) in &fills {
        sets.push(format!("{column} = COALESCE(NULLIF({column}, ''), ?)"));
        values.push(value.clone().into());
    }
    values.push(id.clone().into());
    cx.tx.execute(
        &format!(
            "UPDATE users SET {} WHERE id = ? AND profile_updated_at IS NULL",
            sets.join(", ")
        ),
        &values,
    )?;
    let stored = user_row(cx.tx, &id)?;
    for (column, file) in &fills {
        if matches!(*column, "avatar" | "banner") && !file.starts_with("http") {
            let kept = stored
                .as_ref()
                .map(|row| row.opt_text(column))
                .transpose()?
                .flatten()
                .as_deref()
                == Some(file.as_str());
            if kept {
                cx.keep(file);
            }
        }
    }
    let Some(stored) = stored else {
        return Ok(user);
    };
    let mut filled = false;
    for (column, value) in &fills {
        let now = stored.opt_text(column)?;
        let before = user.opt_text(column)?;
        if now.as_deref() == Some(value.as_str()) && before.as_deref() != Some(value.as_str()) {
            filled = true;
        }
    }
    let _ = FILL_COLUMNS;
    if renamed || filled {
        identity::announce_user_updated(cx.tx, cx.out, &stored)?;
    }
    Ok(stored)
}

/// Профиль, как его объявил дом (`HomeProfile`).
#[derive(Debug, Clone, Default)]
pub struct HomeProfile {
    /// Версия профиля.
    pub updated_at: Option<i64>,
    /// Имя на доме.
    pub username: Option<String>,
    /// Отображаемое имя.
    pub display_name: Option<String>,
    /// Аватар.
    pub avatar: Option<String>,
    /// Баннер.
    pub banner: Option<String>,
    /// Цвет акцента; `None` — дом его не прислал.
    pub accent_color: Option<Option<String>>,
    /// Цвет аватара.
    pub avatar_color: Option<String>,
    /// О себе.
    pub bio: Option<String>,
}

fn opt_string(value: &Value, key: &str) -> Option<String> {
    text(value, key).map(str::to_owned)
}

impl HomeProfile {
    /// Из события `profile_update`.
    #[must_use]
    pub fn from_event(payload: &Value) -> Self {
        Self {
            updated_at: payload
                .get("profileUpdatedAt")
                .filter(|v| !v.is_null())
                .map(|v| super::millis(Some(v))),
            username: opt_string(payload, "username"),
            display_name: opt_string(payload, "displayName"),
            avatar: opt_string(payload, "avatar"),
            banner: opt_string(payload, "banner"),
            accent_color: payload
                .get("accentColor")
                .map(|v| v.as_str().map(str::to_owned)),
            avatar_color: opt_string(payload, "avatarColor"),
            bio: opt_string(payload, "bio"),
        }
    }
}

fn is_newer(incoming: Option<i64>, stored: Option<i64>) -> bool {
    match incoming {
        None => stored.is_none(),
        Some(incoming) => stored.is_none_or(|stored| incoming > stored),
    }
}

/// Записывает профиль, объявленный домом (`applyHomeProfile`).
///
/// # Errors
/// Ошибка базы или нужен файл.
pub fn apply_home_profile(
    cx: &mut Ctx<'_>,
    row: &Row,
    profile: &HomeProfile,
    home_origin: &str,
) -> Result<Row, super::Stop> {
    if row.opt_text("home_instance")?.is_none_or(|h| h.is_empty())
        || row.opt_flag("federation_home_orphaned")? == Some(true)
    {
        return Ok(row.clone());
    }
    if !is_newer(profile.updated_at, row.opt_int("profile_updated_at")?) {
        return Ok(row.clone());
    }
    let base = base_url(home_origin);
    let avatar = match profile.avatar.as_deref().filter(|v| !v.is_empty()) {
        Some(value) => Some(resolve_asset(cx, value, &base, true)?),
        None => None,
    };
    let banner = match profile.banner.as_deref().filter(|v| !v.is_empty()) {
        Some(value) => Some(resolve_asset(cx, value, &base, true)?),
        None => None,
    };
    let display = profile
        .display_name
        .clone()
        .or_else(|| handle_from_hint(profile.username.as_deref()));
    let id = row.text("id")?;
    let mut sql =
        "UPDATE users SET display_name = ?, avatar = ?, banner = ?, avatar_color = ?, bio = ?, \
                   profile_updated_at = ?"
            .to_owned();
    let mut values: Vec<crate::db::Value> = vec![
        display.into(),
        avatar.clone().into(),
        banner.clone().into(),
        profile.avatar_color.clone().into(),
        profile.bio.clone().into(),
        profile.updated_at.into(),
    ];
    if let Some(accent) = &profile.accent_color {
        sql.push_str(", accent_color = ?");
        values.push(accent.clone().into());
    }
    values.push(id.clone().into());
    match profile.updated_at {
        None => sql.push_str(" WHERE id = ? AND profile_updated_at IS NULL"),
        Some(at) => {
            sql.push_str(
                " WHERE id = ? AND (profile_updated_at IS NULL OR profile_updated_at < ?)",
            );
            values.push(at.into());
        }
    }
    let written = cx.tx.execute(&sql, &values)?;
    let old_avatar = row.opt_text("avatar")?;
    let old_banner = row.opt_text("banner")?;
    if written == 0 {
        return Ok(user_row(cx.tx, &id)?.unwrap_or_else(|| row.clone()));
    }
    for file in [&avatar, &banner].into_iter().flatten() {
        if !file.starts_with("http") {
            cx.keep(file);
        }
    }
    if let Some(old) =
        old_avatar.filter(|old| !old.starts_with("http") && Some(old) != avatar.as_ref())
    {
        cx.discard(&old);
    }
    if let Some(old) =
        old_banner.filter(|old| !old.starts_with("http") && Some(old) != banner.as_ref())
    {
        cx.discard(&old);
    }
    let Some(updated) = user_row(cx.tx, &id)? else {
        return Ok(row.clone());
    };
    announce(cx, &updated, true)?;
    Ok(updated)
}

/// Профиль изменился на доме (`processProfileUpdateEvent`).
pub fn update(cx: &mut Ctx<'_>, event: &Value) -> Step {
    let Some(payload) = super::present(event, "profileUpdate") else {
        return reject("missing_profile_update_payload");
    };
    let home = text(payload, "homeInstance").unwrap_or_default();
    if super::raw_domain(home) != super::raw_domain(cx.source) {
        tracing::warn!(
            home = domain(home),
            source = cx.source,
            "profile_update не от дома"
        );
        return reject("attribution_mismatch");
    }
    let user = match cx.resolve(payload)? {
        Resolved::Found(user) => *user,
        _ => return accept(),
    };
    if user.opt_text("home_instance")?.is_none_or(|h| h.is_empty()) {
        return accept();
    }
    if user.opt_flag("federation_home_orphaned")? == Some(true) {
        return accept();
    }
    let profile = HomeProfile::from_event(payload);
    let source = cx.source.to_owned();
    apply_home_profile(cx, &user, &profile, &source)?;
    accept()
}

/// Скачивает файлы профилей копий, оставшиеся ссылками
/// (`backfillReplicatedProfileAssets`).
pub async fn backfill_assets(app: &AppState) {
    let rows = app
        .db
        .transaction(|tx: &mut dyn Tx| {
            tx.query(
                "SELECT id, home_instance, avatar, banner FROM users WHERE home_instance IS NOT NULL \
                 AND is_deleted = 0 AND (avatar LIKE 'http%' OR banner LIKE 'http%')",
                params![],
            )
        })
        .await
        .unwrap_or_default();
    for row in rows {
        let (Ok(id), Ok(Some(home))) = (row.text("id"), row.opt_text("home_instance")) else {
            continue;
        };
        let base = base_url(&home);
        let mut sets: Vec<(&str, String)> = Vec::new();
        for column in ["avatar", "banner"] {
            if let Ok(Some(url)) = row.opt_text(column)
                && url.starts_with("http")
                && let Some(file) = download_asset(app, &url, &base).await
            {
                sets.push((column, file));
            }
        }
        if sets.is_empty() {
            continue;
        }
        let result = app
            .db
            .transaction(move |tx: &mut dyn Tx| {
                for (column, file) in &sets {
                    tx.execute(
                        &format!("UPDATE users SET {column} = ? WHERE id = ?"),
                        params![file, &id],
                    )?;
                }
                Ok::<_, crate::db::DbError>(())
            })
            .await;
        if let Err(error) = result {
            tracing::warn!(%error, "файлы профиля копии не записаны");
        }
    }
}
