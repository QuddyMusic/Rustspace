//! Стикерпаки (расширение Rustspace): свои наборы стикеров, которые можно
//! составлять, дополнять, упорядочивать и давать другим добавить к себе.
//!
//! Стикер в паке — та же картинка `sticker_assets`, что и личный стикер,
//! поэтому в сообщении это обычный `sticker:<url>`: клиенты оригинала
//! видят его как стикер. Пак живёт на инстансе владельца.

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::params;
use crate::routes::stickers::{is_sticker_id, normalize_name, token_for};
use crate::state::{AppState, now_ms};

/// Своих паков у пользователя не больше.
const MAX_OWN_PACKS: i64 = 50;
/// Добавленных чужих паков не больше.
const MAX_ADDED_PACKS: i64 = 100;
/// Стикеров в паке не больше.
const MAX_PACK_STICKERS: i64 = 120;
const MAX_PACK_NAME: usize = 64;

fn pack_name(value: Option<&Value>) -> ApiResult<String> {
    let raw = value.and_then(Value::as_str).unwrap_or_default();
    normalize_name(raw)
        .filter(|name| name.chars().count() <= MAX_PACK_NAME)
        .ok_or_else(|| {
            ApiError::bad_request("sticker_pack_name_invalid").detail("max", MAX_PACK_NAME)
        })
}

/// Пак целиком: что видит `viewer`.
fn pack_json(
    state: &AppState,
    tx: &mut dyn Tx,
    viewer: &str,
    id: &str,
) -> ApiResult<Option<Value>> {
    let Some(row) = tx.query_one(
        "SELECT p.id, p.owner_id, p.name, p.shared, p.created_at, \
         COALESCE(u.display_name, u.username) AS owner_name \
         FROM bs_sticker_packs p JOIN users u ON u.id = p.owner_id WHERE p.id = ?",
        params![id],
    )?
    else {
        return Ok(None);
    };
    let owner = row.text("owner_id")?;
    let shared = row.int("shared")? != 0;
    let added = tx
        .query_one(
            "SELECT 1 AS one FROM bs_sticker_pack_users WHERE user_id = ? AND pack_id = ?",
            params![viewer, id],
        )?
        .is_some();
    let mine = owner == viewer;
    if !mine && !shared && !added {
        return Ok(None);
    }
    let mut stickers = Vec::new();
    for item in tx.query(
        "SELECT sticker_id, name FROM bs_sticker_pack_items WHERE pack_id = ? ORDER BY position, name",
        params![id],
    )? {
        let sticker = item.text("sticker_id")?;
        stickers.push(json!({"id": sticker, "name": item.text("name")?, "token": token_for(state, &sticker)}));
    }
    Ok(Some(json!({
        "id": id,
        "name": row.text("name")?,
        "ownerId": owner,
        "ownerName": row.opt_text("owner_name")?,
        "shared": shared,
        "mine": mine,
        "added": added,
        "createdAt": row.int("created_at")?,
        "stickers": stickers,
    })))
}

fn own_pack(tx: &mut dyn Tx, user: &str, id: &str) -> ApiResult<()> {
    let owner = tx.query_one(
        "SELECT owner_id FROM bs_sticker_packs WHERE id = ?",
        params![id],
    )?;
    match owner {
        None => Err(ApiError::not_found("sticker_pack_not_found")),
        Some(row) if row.text("owner_id")? != user => {
            Err(ApiError::forbidden("sticker_pack_not_yours"))
        }
        Some(_) => Ok(()),
    }
}

fn touch(tx: &mut dyn Tx, id: &str) -> DbResult<()> {
    tx.execute(
        "UPDATE bs_sticker_packs SET updated_at = ? WHERE id = ?",
        params![now_ms(), id],
    )?;
    Ok(())
}

/// `GET /api/rustspace/sticker-packs` — свои и добавленные паки.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, _| {
            let mut own = Vec::new();
            for row in tx.query(
                "SELECT id FROM bs_sticker_packs WHERE owner_id = ? ORDER BY created_at",
                params![&user.id],
            )? {
                if let Some(pack) = pack_json(&app, tx, &user.id, &row.text("id")?)? {
                    own.push(pack);
                }
            }
            let mut added = Vec::new();
            for row in tx.query(
                "SELECT pack_id FROM bs_sticker_pack_users WHERE user_id = ? ORDER BY added_at",
                params![&user.id],
            )? {
                if let Some(pack) = pack_json(&app, tx, &user.id, &row.text("pack_id")?)? {
                    added.push(pack);
                }
            }
            Ok(Json(json!({"own": own, "added": added})))
        })
        .await
}

/// `POST /api/rustspace/sticker-packs` `{name}` — новый пустой пак.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let name = pack_name(body.get("name"))?;
    let app = state.clone();
    state
        .run(move |tx, _| {
            let count = tx
                .query_one("SELECT count(*) AS n FROM bs_sticker_packs WHERE owner_id = ?", params![&user.id])?
                .map_or(Ok(0), |row| row.int("n"))?;
            if count >= MAX_OWN_PACKS {
                return Err(ApiError::conflict("sticker_pack_limit").detail("max", MAX_OWN_PACKS));
            }
            let id = app.ids.next();
            let now = now_ms();
            tx.execute(
                "INSERT INTO bs_sticker_packs (id, owner_id, name, shared, created_at, updated_at) VALUES (?, ?, ?, 1, ?, ?)",
                params![&id, &user.id, &name, now, now],
            )?;
            pack_json(&app, tx, &user.id, &id)?.map(Json).ok_or_else(ApiError::internal)
        })
        .await
}

/// `GET /api/rustspace/sticker-packs/:id`.
pub async fn get_pack(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, _| {
            pack_json(&app, tx, &user.id, &id)?
                .map(Json)
                .ok_or_else(|| ApiError::not_found("sticker_pack_not_found"))
        })
        .await
}

/// `PATCH /api/rustspace/sticker-packs/:id` `{name?, shared?}` — владелец.
pub async fn update(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let name = body.get("name").map(|n| pack_name(Some(n))).transpose()?;
    let shared = body.get("shared").and_then(Value::as_bool);
    let app = state.clone();
    state
        .run(move |tx, _| {
            own_pack(tx, &user.id, &id)?;
            if let Some(name) = &name {
                tx.execute(
                    "UPDATE bs_sticker_packs SET name = ? WHERE id = ?",
                    params![name, &id],
                )?;
            }
            if let Some(shared) = shared {
                tx.execute(
                    "UPDATE bs_sticker_packs SET shared = ? WHERE id = ?",
                    params![i64::from(shared), &id],
                )?;
                if !shared {
                    // Закрытый пак уходит и у тех, кто его добавил.
                    tx.execute(
                        "DELETE FROM bs_sticker_pack_users WHERE pack_id = ?",
                        params![&id],
                    )?;
                }
            }
            touch(tx, &id)?;
            pack_json(&app, tx, &user.id, &id)?
                .map(Json)
                .ok_or_else(ApiError::internal)
        })
        .await
}

/// `DELETE /api/rustspace/sticker-packs/:id` — удалить свой пак.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            own_pack(tx, &user.id, &id)?;
            tx.execute(
                "DELETE FROM bs_sticker_pack_users WHERE pack_id = ?",
                params![&id],
            )?;
            tx.execute(
                "DELETE FROM bs_sticker_pack_items WHERE pack_id = ?",
                params![&id],
            )?;
            tx.execute("DELETE FROM bs_sticker_packs WHERE id = ?", params![&id])?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `PUT /api/rustspace/sticker-packs/:id/stickers/:sticker` `{name?}` —
/// положить стикер в свой пак (картинка уже есть на инстансе).
pub async fn put_sticker(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, sticker)): Path<(String, String)>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    if !is_sticker_id(&sticker) {
        return Err(ApiError::bad_request("validation_failed"));
    }
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .and_then(normalize_name);
    let app = state.clone();
    state
        .run(move |tx, _| {
            own_pack(tx, &user.id, &id)?;
            if tx.query_one("SELECT id FROM sticker_assets WHERE id = ?", params![&sticker])?.is_none() {
                return Err(ApiError::not_found("sticker_not_found"));
            }
            let name = match name {
                Some(name) => name,
                None => tx
                    .query_one(
                        "SELECT name FROM personal_stickers WHERE user_id = ? AND sticker_id = ?",
                        params![&user.id, &sticker],
                    )?
                    .map(|row| row.text("name"))
                    .transpose()?
                    .unwrap_or_else(|| "sticker".to_owned()),
            };
            let exists = tx
                .query_one(
                    "SELECT 1 AS one FROM bs_sticker_pack_items WHERE pack_id = ? AND sticker_id = ?",
                    params![&id, &sticker],
                )?
                .is_some();
            if exists {
                tx.execute(
                    "UPDATE bs_sticker_pack_items SET name = ? WHERE pack_id = ? AND sticker_id = ?",
                    params![&name, &id, &sticker],
                )?;
            } else {
                let count = tx
                    .query_one(
                        "SELECT count(*) AS n, COALESCE(max(position), -1) AS last FROM bs_sticker_pack_items WHERE pack_id = ?",
                        params![&id],
                    )?
                    .map_or(Ok((0, -1)), |row| Ok::<_, crate::db::DbError>((row.int("n")?, row.int("last")?)))?;
                if count.0 >= MAX_PACK_STICKERS {
                    return Err(ApiError::conflict("sticker_pack_full").detail("max", MAX_PACK_STICKERS));
                }
                tx.execute(
                    "INSERT INTO bs_sticker_pack_items (pack_id, sticker_id, name, position) VALUES (?, ?, ?, ?)",
                    params![&id, &sticker, &name, count.1 + 1],
                )?;
            }
            touch(tx, &id)?;
            pack_json(&app, tx, &user.id, &id)?.map(Json).ok_or_else(ApiError::internal)
        })
        .await
}

/// `DELETE /api/rustspace/sticker-packs/:id/stickers/:sticker`.
pub async fn remove_sticker(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, sticker)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, _| {
            own_pack(tx, &user.id, &id)?;
            tx.execute(
                "DELETE FROM bs_sticker_pack_items WHERE pack_id = ? AND sticker_id = ?",
                params![&id, &sticker],
            )?;
            touch(tx, &id)?;
            pack_json(&app, tx, &user.id, &id)?
                .map(Json)
                .ok_or_else(ApiError::internal)
        })
        .await
}

/// `PUT /api/rustspace/sticker-packs/:id/order` `{stickers: [id…]}` — новый
/// порядок (неупомянутые остаются в конце).
pub async fn reorder(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    let order: Vec<String> = body
        .get("stickers")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::bad_request("validation_failed"))?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .take(usize::try_from(MAX_PACK_STICKERS).unwrap_or(120))
        .collect();
    let app = state.clone();
    state
        .run(move |tx, _| {
            own_pack(tx, &user.id, &id)?;
            tx.execute(
                "UPDATE bs_sticker_pack_items SET position = position + 100000 WHERE pack_id = ?",
                params![&id],
            )?;
            for (position, sticker) in order.iter().enumerate() {
                tx.execute(
                    "UPDATE bs_sticker_pack_items SET position = ? WHERE pack_id = ? AND sticker_id = ?",
                    params![i64::try_from(position).unwrap_or(0), &id, sticker],
                )?;
            }
            touch(tx, &id)?;
            pack_json(&app, tx, &user.id, &id)?.map(Json).ok_or_else(ApiError::internal)
        })
        .await
}

/// `PUT /api/rustspace/sticker-packs/:id/added` — добавить чужой открытый пак к себе.
pub async fn add(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let app = state.clone();
    state
        .run(move |tx, _| {
            let pack = pack_json(&app, tx, &user.id, &id)?
                .ok_or_else(|| ApiError::not_found("sticker_pack_not_found"))?;
            if pack["mine"] == true {
                return Ok(Json(pack));
            }
            if pack["shared"] != true {
                return Err(ApiError::not_found("sticker_pack_not_found"));
            }
            let count = tx
                .query_one(
                    "SELECT count(*) AS n FROM bs_sticker_pack_users WHERE user_id = ?",
                    params![&user.id],
                )?
                .map_or(Ok(0), |row| row.int("n"))?;
            if count >= MAX_ADDED_PACKS {
                return Err(ApiError::conflict("sticker_pack_limit").detail("max", MAX_ADDED_PACKS));
            }
            tx.execute(
                "INSERT INTO bs_sticker_pack_users (user_id, pack_id, added_at) VALUES (?, ?, ?) \
                 ON CONFLICT (user_id, pack_id) DO NOTHING",
                params![&user.id, &id, now_ms()],
            )?;
            pack_json(&app, tx, &user.id, &id)?
                .map(Json)
                .ok_or_else(ApiError::internal)
        })
        .await
}

/// `DELETE /api/rustspace/sticker-packs/:id/added` — убрать чужой пак у себя.
pub async fn unadd(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            tx.execute(
                "DELETE FROM bs_sticker_pack_users WHERE user_id = ? AND pack_id = ?",
                params![&user.id, &id],
            )?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `GET /api/rustspace/sticker-packs/with/:sticker` — открытые паки с этим
/// стикером: из сообщения можно открыть пак и добавить его.
pub async fn containing(
    State(state): State<AppState>,
    user: AuthUser,
    Path(sticker): Path<String>,
) -> ApiResult<Json<Value>> {
    if !is_sticker_id(&sticker) {
        return Ok(Json(json!([])));
    }
    let app = state.clone();
    state
        .run(move |tx, _| {
            let mut packs = Vec::new();
            for row in tx.query(
                "SELECT p.id FROM bs_sticker_pack_items i JOIN bs_sticker_packs p ON p.id = i.pack_id \
                 WHERE i.sticker_id = ? AND (p.shared = 1 OR p.owner_id = ?) ORDER BY p.created_at LIMIT 5",
                params![&sticker, &user.id],
            )? {
                if let Some(pack) = pack_json(&app, tx, &user.id, &row.text("id")?)? {
                    packs.push(pack);
                }
            }
            Ok(Json(Value::Array(packs)))
        })
        .await
}
