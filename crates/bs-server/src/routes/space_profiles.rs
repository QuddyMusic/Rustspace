//! Свой профиль участника в пространстве: аватар, баннер и статус (запрос
//! оригинала #273). Пустое поле — берётся из общего профиля. Расширение
//! Rustspace: клиент оригинала этих запросов не делает и видит общий профиль.

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::routes::account::valid_asset;
use crate::state::{AppState, now_ms};
use crate::util::js_trim;
use crate::{params, perms};

/// Самый длинный статус (как у общего профиля).
const MAX_STATUS: usize = 128;

/// Картинка из запроса: `None` — поле не трогаем, `Some(None)` — сбросить.
fn image(body: &Value, name: &str, code: &'static str) -> ApiResult<Option<Option<String>>> {
    let Some(value) = body.get(name) else {
        return Ok(None);
    };
    if !valid_asset(value) {
        return Err(ApiError::bad_request(code));
    }
    Ok(Some(
        value
            .as_str()
            .map(js_trim)
            .filter(|url| !url.is_empty())
            .map(|url| url.strip_prefix("/api/uploads/").unwrap_or(url).to_owned()),
    ))
}

/// Профиль строки таблицы в JSON (картинки — путями загрузок, как у
/// общего профиля).
fn profile_json(row: &crate::db::Row) -> crate::db::DbResult<Value> {
    Ok(json!({
        "userId": row.text("user_id")?,
        "avatar": row.opt_text("avatar")?.map(upload_path),
        "banner": row.opt_text("banner")?.map(upload_path),
        "customStatus": row.opt_text("custom_status")?,
    }))
}

/// Имя загруженного файла — путём загрузок; внешний адрес — как есть.
fn upload_path(file: String) -> String {
    if file.starts_with("http://") || file.starts_with("https://") {
        file
    } else {
        format!("/api/uploads/{file}")
    }
}

/// `GET /api/rustspace/spaces/{id}/profiles` — свои профили участников.
pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Path(space_id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            if !perms::is_member(tx, &space_id, &user.id)? {
                return Err(ApiError::forbidden("not_member"));
            }
            let rows = tx.query(
                "SELECT p.user_id, p.avatar, p.banner, p.custom_status \
                 FROM bs_space_profiles p JOIN space_members m \
                 ON m.space_id = p.space_id AND m.user_id = p.user_id \
                 WHERE p.space_id = ?",
                params![&space_id],
            )?;
            let list = rows
                .iter()
                .map(profile_json)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Json(Value::Array(list)))
        })
        .await
}

/// `PUT /api/rustspace/spaces/{id}/profile` `{avatar?, banner?, customStatus?}`
/// — меняет свой профиль в пространстве; отсутствующее поле не трогается,
/// `null` или пустое — сбрасывается. Всё пустое — профиля нет.
pub async fn put(
    State(state): State<AppState>,
    user: AuthUser,
    Path(space_id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let avatar = image(&body, "avatar", "avatar_url_invalid")?;
    let banner = image(&body, "banner", "banner_url_invalid")?;
    let status = match body.get("customStatus") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(text)) => {
            let text = js_trim(text.as_str());
            if text.chars().count() > MAX_STATUS {
                return Err(ApiError::bad_request("custom_status_too_long"));
            }
            Some((!text.is_empty()).then(|| text.to_owned()))
        }
        Some(_) => return Err(ApiError::bad_request("custom_status_invalid")),
    };
    state
        .run(move |tx, _| {
            if !perms::is_member(tx, &space_id, &user.id)? {
                return Err(ApiError::forbidden("not_member"));
            }
            let current = tx.query_one(
                "SELECT avatar, banner, custom_status FROM bs_space_profiles \
                 WHERE space_id = ? AND user_id = ?",
                params![&space_id, &user.id],
            )?;
            let old = |name: &str| -> crate::db::DbResult<Option<String>> {
                current.as_ref().map_or(Ok(None), |row| row.opt_text(name))
            };
            let avatar = match avatar {
                Some(value) => value,
                None => old("avatar")?,
            };
            let banner = match banner {
                Some(value) => value,
                None => old("banner")?,
            };
            let status = match status {
                Some(value) => value,
                None => old("custom_status")?,
            };
            if avatar.is_none() && banner.is_none() && status.is_none() {
                tx.execute(
                    "DELETE FROM bs_space_profiles WHERE space_id = ? AND user_id = ?",
                    params![&space_id, &user.id],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO bs_space_profiles \
                     (space_id, user_id, avatar, banner, custom_status, updated_at) \
                     VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (space_id, user_id) DO UPDATE SET \
                     avatar = excluded.avatar, banner = excluded.banner, \
                     custom_status = excluded.custom_status, updated_at = excluded.updated_at",
                    params![&space_id, &user.id, &avatar, &banner, &status, now_ms()],
                )?;
            }
            Ok(Json(json!({
                "userId": user.id,
                "avatar": avatar.map(upload_path),
                "banner": banner.map(upload_path),
                "customStatus": status,
            })))
        })
        .await
}
