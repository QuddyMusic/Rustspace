//! Личные коллекции стикеров (`routes/stickers.ts`).

use std::path::PathBuf;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::auth::AuthUser;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::{JsonBody, read_json, schema_error};
use crate::federation::our_origin;
use crate::params;
use crate::state::{AppState, now_ms};
use crate::stickers::{MAX_STICKER_BYTES, encode_sticker};
use crate::util::{is_visible_single_line, js_trim, utf16_len};

const MAX_PERSONAL_STICKERS: i64 = 200;
const MAX_NAME_LENGTH: usize = 100;
const UPLOADS_PER_MINUTE: u32 = 30;
/// Картинка в base64 плюс остальное тело запроса.
/// Предел тела загрузки стикера (`bodyLimit` маршрута у оригинала).
pub const UPLOAD_BODY_LIMIT: usize = MAX_STICKER_BYTES.div_ceil(3) * 4 + 4096;

fn token_for(state: &AppState, id: &str) -> String {
    format!(
        "sticker:{}/api/stickers/assets/{id}.webp",
        our_origin(&state.config)
    )
}

fn is_sticker_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn asset_dir(state: &AppState) -> PathBuf {
    state.config.upload_dir.join("stickers")
}

fn name_invalid() -> ApiError {
    ApiError::bad_request("sticker_name_invalid").detail("max", MAX_NAME_LENGTH)
}

fn collection_full() -> ApiError {
    ApiError::conflict("sticker_collection_full").detail("max", MAX_PERSONAL_STICKERS)
}

/// Имя стикера (`normalizeStickerName`).
fn normalize_name(value: &str) -> Option<String> {
    let trimmed = js_trim(value);
    (!trimmed.is_empty()
        && utf16_len(trimmed) <= MAX_NAME_LENGTH
        && is_visible_single_line(trimmed))
    .then(|| trimmed.to_owned())
}

/// Строковое поле по схеме маршрута: приведение типов и пределы длины, как у
/// проверки Fastify (Ajv с `coerceTypes`).
fn schema_string(
    body: &Value,
    name: &str,
    min: usize,
    max: Option<usize>,
) -> Result<String, Response> {
    // `coerceTypes: 'array'`: массив из одного значения считается этим значением.
    let value = match &body[name] {
        Value::Array(items) if items.len() == 1 => &items[0],
        other => other,
    };
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::new(),
        _ => return Err(schema_error(&format!("body/{name} must be string"))),
    };
    let length = text.chars().count();
    if length < min {
        return Err(schema_error(&format!(
            "body/{name} must NOT have fewer than {min} characters"
        )));
    }
    if let Some(max) = max.filter(|max| length > *max) {
        return Err(schema_error(&format!(
            "body/{name} must NOT have more than {max} characters"
        )));
    }
    Ok(text)
}

fn schema_object(body: &Value, required: &[&str]) -> Result<(), Response> {
    let Some(fields) = body.as_object() else {
        return Err(schema_error("body must be object"));
    };
    match required.iter().find(|name| !fields.contains_key(**name)) {
        Some(missing) => Err(schema_error(&format!(
            "body must have required property '{missing}'"
        ))),
        None => Ok(()),
    }
}

fn schema_id(id: &str) -> Result<(), Response> {
    if is_sticker_id(id) {
        Ok(())
    } else {
        Err(schema_error(
            "params/id must match pattern \"^[a-f0-9]{64}$\"",
        ))
    }
}

fn collection_size(tx: &mut dyn Tx, user: &str) -> DbResult<i64> {
    tx.query_one(
        "SELECT count(*) AS n FROM personal_stickers WHERE user_id = ?",
        params![user],
    )?
    .map_or(Ok(0), |row| row.int("n"))
}

/// Добавляет стикер в коллекцию (`collectSticker`); `None` — коллекция полна.
fn collect(tx: &mut dyn Tx, user: &str, id: &str, name: &str) -> DbResult<Option<String>> {
    let existing = |tx: &mut dyn Tx| -> DbResult<Option<String>> {
        tx.query_one(
            "SELECT name FROM personal_stickers WHERE user_id = ? AND sticker_id = ?",
            params![user, id],
        )?
        .map(|row| row.text("name"))
        .transpose()
    };
    if let Some(name) = existing(tx)? {
        return Ok(Some(name));
    }
    if collection_size(tx, user)? >= MAX_PERSONAL_STICKERS {
        return Ok(None);
    }
    tx.execute(
        "INSERT INTO personal_stickers (user_id, sticker_id, name, created_at) VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
        params![user, id, name, now_ms()],
    )?;
    existing(tx)
}

/// `GET /api/stickers`.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let inner = state.clone();
    state
        .run(move |tx, _| {
            let rows = tx.query(
                "SELECT sticker_id, name FROM personal_stickers WHERE user_id = ? ORDER BY created_at DESC",
                params![&user.id],
            )?;
            let mut list = Vec::with_capacity(rows.len());
            for row in rows {
                let id = row.text("sticker_id")?;
                list.push(json!({"id": id, "name": row.text("name")?, "token": token_for(&inner, &id)}));
            }
            Ok(Json(Value::Array(list)))
        })
        .await
}

/// `POST /api/stickers`: новая картинка в коллекцию.
pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    request: Request,
) -> Result<Json<Value>, Response> {
    let body = read_json(request).await?;
    schema_object(&body, &["name", "image"])?;
    let raw_name = schema_string(&body, "name", 1, Some(MAX_NAME_LENGTH))?;
    let image = schema_string(&body, "image", 4, None)?;
    let refuse = |error: ApiError| error.into_response();
    let name = normalize_name(&raw_name).ok_or_else(|| refuse(name_invalid()))?;
    if !state.hub.allow(
        &format!("sticker-upload:{}", user.id),
        UPLOADS_PER_MINUTE,
        Duration::from_mins(1),
    ) {
        return Err(refuse(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
        )));
    }
    let owner = user.id.clone();
    let size = state
        .run(move |tx, _| Ok(collection_size(tx, &owner)?))
        .await
        .map_err(refuse)?;
    if size >= MAX_PERSONAL_STICKERS {
        return Err(refuse(collection_full()));
    }
    // Только каноническая запись base64: то же, что сравнение с обратным кодированием.
    let bytes = STANDARD
        .decode(&image)
        .map_err(|_| refuse(ApiError::bad_request("validation_failed")))?;
    let invalid_image = || {
        refuse(
            ApiError::bad_request("sticker_invalid_image")
                .detail("max", MAX_STICKER_BYTES / (1024 * 1024)),
        )
    };
    let encoded = tokio::task::spawn_blocking(move || encode_sticker(&bytes))
        .await
        .map_err(|_| refuse(ApiError::internal()))?
        .map_err(|_| invalid_image())?;
    let id = hex::encode(Sha256::digest(&encoded));

    let asset = id.clone();
    state
        .run(move |tx, _| {
            let now = now_ms();
            tx.execute(
                "INSERT INTO sticker_assets (id, created_at) VALUES (?, ?) ON CONFLICT (id) DO UPDATE SET created_at = ?",
                params![&asset, now, now],
            )?;
            Ok(())
        })
        .await
        .map_err(refuse)?;
    let dir = asset_dir(&state);
    let target = dir.join(format!("{id}.webp"));
    let temporary = dir.join(format!("{id}.webp.{}.tmp", uuid::Uuid::new_v4()));
    let written = async {
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(&temporary, &encoded).await?;
        tokio::fs::rename(&temporary, &target).await
    };
    if written.await.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(refuse(ApiError::internal()));
    }
    let (owner, asset) = (user.id.clone(), id.clone());
    let kept = state
        .run(move |tx, _| Ok(collect(tx, &owner, &asset, &name)?))
        .await
        .map_err(refuse)?;
    let name = kept.ok_or_else(|| refuse(collection_full()))?;
    Ok(Json(
        json!({"id": id, "name": name, "token": token_for(&state, &id)}),
    ))
}

/// `POST /api/stickers/:id/collect`: чужой стикер этого инстанса в свою коллекцию.
pub async fn collect_existing(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> Result<Json<Value>, Response> {
    schema_id(&id)?;
    schema_object(&body, &["token", "name"])?;
    let token = schema_string(&body, "token", 0, Some(2048))?;
    let raw_name = schema_string(&body, "name", 1, Some(MAX_NAME_LENGTH))?;
    let expected = token_for(&state, &id);
    if token != expected {
        return Err(ApiError::bad_request("sticker_not_local").into_response());
    }
    let asset = id.clone();
    let name = state
        .run(move |tx, _| {
            if tx
                .query_one(
                    "SELECT id FROM sticker_assets WHERE id = ?",
                    params![&asset],
                )?
                .is_none()
            {
                return Err(ApiError::not_found("sticker_not_found"));
            }
            let name = normalize_name(&raw_name).ok_or_else(name_invalid)?;
            collect(tx, &user.id, &asset, &name)?.ok_or_else(collection_full)
        })
        .await
        .map_err(IntoResponse::into_response)?;
    Ok(Json(json!({"id": id, "name": name, "token": expected})))
}

/// `DELETE /api/stickers/:id`.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    state
        .run(move |tx, _| {
            tx.execute(
                "DELETE FROM personal_stickers WHERE user_id = ? AND sticker_id = ?",
                params![&user.id, &id],
            )?;
            Ok(Json(json!({"success": true})))
        })
        .await
}

/// `PATCH /api/stickers/:id`: переименование.
pub async fn rename(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> Result<Json<Value>, Response> {
    schema_id(&id)?;
    schema_object(&body, &["name"])?;
    let raw_name = schema_string(&body, "name", 1, Some(MAX_NAME_LENGTH))?;
    let name = normalize_name(&raw_name).ok_or_else(|| name_invalid().into_response())?;
    let (asset, new_name) = (id.clone(), name.clone());
    state
        .run(move |tx, _| {
            let changed = tx.execute(
                "UPDATE personal_stickers SET name = ? WHERE user_id = ? AND sticker_id = ?",
                params![&new_name, &user.id, &asset],
            )?;
            if changed == 0 {
                Err(ApiError::not_found("sticker_not_found"))
            } else {
                Ok(())
            }
        })
        .await
        .map_err(IntoResponse::into_response)?;
    Ok(Json(
        json!({"id": id, "name": name, "token": token_for(&state, &id)}),
    ))
}

/// `GET /api/stickers/assets/:filename`.
pub async fn asset(State(state): State<AppState>, Path(filename): Path<String>) -> Response {
    let missing = || ApiError::not_found("sticker_not_found").into_response();
    let Some(id) = filename
        .strip_suffix(".webp")
        .filter(|id| is_sticker_id(id))
        .map(str::to_owned)
    else {
        return missing();
    };
    let lookup = id.clone();
    let known = state
        .run(move |tx, _| {
            Ok(tx
                .query_one(
                    "SELECT id FROM sticker_assets WHERE id = ?",
                    params![&lookup],
                )?
                .is_some())
        })
        .await
        .unwrap_or(false);
    if !known {
        return missing();
    }
    let Ok(bytes) = tokio::fs::read(asset_dir(&state).join(format!("{id}.webp"))).await else {
        return missing();
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("image/webp"));
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; img-src 'self'"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_ids() {
        assert_eq!(normalize_name("  Кот  ").as_deref(), Some("Кот"));
        assert_eq!(normalize_name("   "), None);
        assert_eq!(normalize_name("a\nb"), None);
        assert_eq!(normalize_name(&"я".repeat(101)), None);
        assert!(is_sticker_id(&"a1".repeat(32)));
        assert!(!is_sticker_id(&"A1".repeat(32)));
        assert!(!is_sticker_id("../x"));
    }

    #[test]
    fn schema_strings_are_coerced_like_ajv() {
        let body = json!({"a": 5, "b": null, "c": [1], "d": "текст", "e": true});
        assert_eq!(
            schema_string(&body, "a", 1, None).ok().as_deref(),
            Some("5")
        );
        assert!(schema_string(&body, "b", 1, None).is_err());
        assert_eq!(
            schema_string(&body, "c", 0, None).ok().as_deref(),
            Some("1")
        );
        assert!(schema_string(&json!({"c": [1, 2]}), "c", 0, None).is_err());
        assert!(schema_string(&body, "d", 1, Some(4)).is_err());
        assert_eq!(
            schema_string(&body, "e", 1, Some(5)).ok().as_deref(),
            Some("true")
        );
        assert!(schema_object(&body, &["a", "zz"]).is_err());
        assert!(schema_object(&json!([]), &[]).is_err());
    }
}
