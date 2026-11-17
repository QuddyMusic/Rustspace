//! Иконки игр для активности (`routes/gameIcons.ts` из PR оригинала #482,
//! авторы — QuddyMusic и cirzear).
//!
//! Настольный клиент достаёт иконку найденной игры с машины игрока (кэш
//! библиотеки Steam, Flatpak, .desktop, сам .exe) и загружает её на свой же
//! инстанс: у активности есть картинка, которую не раздаёт никто посторонний.

use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_FRAME_OPTIONS};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ExtendedColorType, ImageDecoder, ImageFormat, ImageReader};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::auth::AuthUser;
use crate::config::Config;
use crate::db::{DbResult, Tx};
use crate::error::{ApiError, ApiResult};
use crate::extract::read_json;
use crate::federation::our_origin;
use crate::params;
use crate::routes::stickers::{schema_object, schema_string};
use crate::state::{AppState, now_ms};
use crate::storage::Cleanup;

/// Наибольший размер загружаемой иконки (до перекодирования).
pub const MAX_GAME_ICON_BYTES: usize = 512 * 1024;
/// Длинная сторона хранимой иконки.
const DIMENSION: u32 = 256;
/// Предел числа иконок одного аккаунта: от злоупотреблений, не от игры.
const MAX_ICONS_PER_USER: i64 = 25_600;
const UPLOADS_PER_MINUTE: u32 = 10;
const MAX_DECODED_PIXELS: u64 = 16_777_216;
/// Иконку, которую никто не загружал год, сервер забывает; клиент
/// обновляет загрузку раз в 30 дней.
const RETENTION_MS: i64 = 365 * 24 * 60 * 60 * 1000;
const ASSET_GRACE_MS: i64 = 24 * 60 * 60 * 1000;
const TEMP_AGE_MS: i64 = 60 * 60 * 1000;
/// Предел тела запроса: base64 плюс запас.
pub const UPLOAD_BODY_LIMIT: usize = MAX_GAME_ICON_BYTES.div_ceil(3) * 4 + 4096;

fn asset_dir(config: &Config) -> PathBuf {
    config.upload_dir.join("game-icons")
}

fn is_icon_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn url_for(state: &AppState, id: &str) -> String {
    format!(
        "{}/api/game-icons/assets/{id}.webp",
        our_origin(&state.config)
    )
}

fn collection_full() -> ApiError {
    ApiError::conflict("game_icon_collection_full").detail("max", MAX_ICONS_PER_USER)
}

/// Неподвижная PNG, JPEG или WebP → WebP не больше 256 точек по длинной
/// стороне. GIF не принимается: иконка активности — картинка, не анимация.
///
/// # Errors
/// Не картинка, не тот формат, слишком велика.
pub fn encode_icon(bytes: &[u8]) -> Result<Vec<u8>, ()> {
    if bytes.is_empty() || bytes.len() > MAX_GAME_ICON_BYTES {
        return Err(());
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ())?;
    let format = reader.format().ok_or(())?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err(());
    }
    let (width, height) = reader.into_dimensions().map_err(|_| ())?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_DECODED_PIXELS {
        return Err(());
    }
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .map_err(|_| ())?;
    let orientation = decoder.orientation().map_err(|_| ())?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| ())?;
    image.apply_orientation(orientation);
    if image.width().max(image.height()) > DIMENSION {
        image = image.resize(DIMENSION, DIMENSION, FilterType::Lanczos3);
    }
    let frame = image.to_rgba8();
    let mut file = Vec::new();
    WebPEncoder::new_lossless(&mut file)
        .encode(
            frame.as_raw(),
            frame.width(),
            frame.height(),
            ExtendedColorType::Rgba8,
        )
        .map_err(|_| ())?;
    if file.len() > MAX_GAME_ICON_BYTES {
        return Err(());
    }
    Ok(file)
}

fn icon_count(tx: &mut dyn Tx, user: &str) -> DbResult<i64> {
    tx.query_one(
        "SELECT count(*) AS n FROM bs_game_icons WHERE user_id = ?",
        params![user],
    )?
    .map_or(Ok(0), |row| row.int("n"))
}

/// `POST /api/game-icons` `{image: base64}` → `{id, url}`. Та же картинка
/// повторно продлевает хранение.
pub async fn upload(
    State(state): State<AppState>,
    user: AuthUser,
    request: Request,
) -> Result<Json<Value>, Response> {
    let body = read_json(request).await?;
    schema_object(&body, &["image"])?;
    let image = schema_string(&body, "image", 4, None)?;
    let refuse = |error: ApiError| error.into_response();
    if !state.hub.allow(
        &format!("game-icon-upload:{}", user.id),
        UPLOADS_PER_MINUTE,
        Duration::from_mins(1),
    ) {
        return Err(refuse(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
        )));
    }
    let owner = user.id.clone();
    let count = state
        .run(move |tx, _| Ok(icon_count(tx, &owner)?))
        .await
        .map_err(refuse)?;
    if count >= MAX_ICONS_PER_USER {
        return Err(refuse(collection_full()));
    }
    let bytes = STANDARD
        .decode(&image)
        .map_err(|_| refuse(ApiError::bad_request("validation_failed")))?;
    let encoded = tokio::task::spawn_blocking(move || encode_icon(&bytes))
        .await
        .map_err(|_| refuse(ApiError::internal()))?
        .map_err(|()| {
            refuse(ApiError::bad_request("game_icon_invalid_image").detail("max", 0.5))
        })?;
    let id = hex::encode(Sha256::digest(&encoded));
    let asset = id.clone();
    state
        .run(move |tx, _| {
            let now = now_ms();
            tx.execute(
                "INSERT INTO bs_game_icon_assets (id, created_at) VALUES (?, ?) \
                 ON CONFLICT (id) DO UPDATE SET created_at = ?",
                params![&asset, now, now],
            )?;
            Ok(())
        })
        .await
        .map_err(refuse)?;
    // Запись о файле раньше самого файла; файл появляется целиком.
    let dir = asset_dir(&state.config);
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
        .run(move |tx, _| {
            let now = now_ms();
            let known = tx
                .query_one(
                    "SELECT 1 AS one FROM bs_game_icons WHERE user_id = ? AND icon_id = ?",
                    params![&owner, &asset],
                )?
                .is_some();
            if known {
                tx.execute(
                    "UPDATE bs_game_icons SET created_at = ? WHERE user_id = ? AND icon_id = ?",
                    params![now, &owner, &asset],
                )?;
                return Ok(true);
            }
            if icon_count(tx, &owner)? >= MAX_ICONS_PER_USER {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO bs_game_icons (user_id, icon_id, created_at) VALUES (?, ?, ?)",
                params![&owner, &asset, now],
            )?;
            Ok(true)
        })
        .await
        .map_err(refuse)?;
    if !kept {
        return Err(refuse(collection_full()));
    }
    Ok(Json(json!({"id": id, "url": url_for(&state, &id)})))
}

/// `GET /api/game-icons/assets/:id.webp` — открытая ссылка, как у стикеров.
pub async fn asset(State(state): State<AppState>, Path(filename): Path<String>) -> Response {
    let missing = || ApiError::not_found("game_icon_not_found").into_response();
    let Some(id) = filename
        .strip_suffix(".webp")
        .filter(|id| is_icon_id(id))
        .map(str::to_owned)
    else {
        return missing();
    };
    let lookup = id.clone();
    let known: ApiResult<bool> = state
        .run(move |tx, _| {
            Ok(tx
                .query_one(
                    "SELECT id FROM bs_game_icon_assets WHERE id = ?",
                    params![&lookup],
                )?
                .is_some())
        })
        .await;
    if !known.unwrap_or(false) {
        return missing();
    }
    let Ok(bytes) = tokio::fs::read(asset_dir(&state.config).join(format!("{id}.webp"))).await
    else {
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
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// Уборка: заявки старше года снимаются, картинки без заявок удаляются,
/// брошенные временные файлы — тоже.
///
/// # Errors
/// Ошибка базы.
pub fn cleanup(
    tx: &mut dyn Tx,
    config: &Config,
    dry_run: bool,
    result: &mut Cleanup,
) -> DbResult<()> {
    let now = now_ms();
    let cutoff = now - RETENTION_MS;
    let mut kept = std::collections::HashSet::new();
    for row in tx.query(
        "SELECT DISTINCT icon_id FROM bs_game_icons WHERE created_at >= ?",
        params![cutoff],
    )? {
        kept.insert(row.text("icon_id")?);
    }
    let dir = asset_dir(config);
    let mut removed = Vec::new();
    for row in tx.query(
        "SELECT id FROM bs_game_icon_assets WHERE created_at < ?",
        params![now - ASSET_GRACE_MS],
    )? {
        let id = row.text("id")?;
        if kept.contains(&id) || !is_icon_id(&id) {
            continue;
        }
        let path = dir.join(format!("{id}.webp"));
        let size = std::fs::metadata(&path).map_or(0, |meta| meta.len());
        if !dry_run
            && let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            result
                .errors
                .push(format!("Failed to delete game icon {id}: {error}"));
            continue;
        }
        result.deleted_files += u64::from(size > 0);
        result.freed_bytes += size;
        removed.push(id);
    }
    if !dry_run {
        tx.execute(
            "DELETE FROM bs_game_icons WHERE created_at < ?",
            params![cutoff],
        )?;
        for id in &removed {
            tx.execute("DELETE FROM bs_game_icons WHERE icon_id = ?", params![id])?;
            tx.execute("DELETE FROM bs_game_icon_assets WHERE id = ?", params![id])?;
        }
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.path().extension().is_none_or(|ext| ext != "tmp") {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let old = meta
                .modified()
                .ok()
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age.as_millis() > u128::try_from(TEMP_AGE_MS).unwrap_or(0));
            if !old {
                continue;
            }
            if !dry_run && std::fs::remove_file(entry.path()).is_err() {
                continue;
            }
            result.deleted_files += 1;
            result.freed_bytes += meta.len();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_shrunk_and_gifs_refused() {
        let mut png = Vec::new();
        DynamicImage::new_rgba8(600, 300)
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap_or_default();
        let webp = encode_icon(&png).unwrap_or_default();
        let decoded = image::load_from_memory(&webp).map(|i| (i.width(), i.height()));
        assert_eq!(decoded.ok(), Some((256, 128)));
        let mut gif = Vec::new();
        DynamicImage::new_rgba8(8, 8)
            .write_to(&mut Cursor::new(&mut gif), ImageFormat::Gif)
            .unwrap_or_default();
        assert!(encode_icon(&gif).is_err());
        assert!(encode_icon(b"not an image").is_err());
    }
}
