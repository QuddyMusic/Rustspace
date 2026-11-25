//! Превью трансляции для наведения на голосовой канал (запрос оригинала
//! #288): клиент того, кто показывает экран, раз в несколько секунд кладёт
//! маленький кадр, зрители видят его, не заходя в канал. Кадры живут только
//! в памяти и минуту; показывать может лишь тот, кто сейчас транслирует в
//! этом канале, смотреть — тот, кто видит канал.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use bs_permissions::Permissions;

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::messages::channel_space;
use crate::perms;
use crate::state::{AppState, now_ms};

/// Самый большой кадр.
const MAX_FRAME: usize = 256 * 1024;
/// Сколько кадр считается свежим.
const FRESH_MS: i64 = 60_000;
/// Больше кадров разом не держим.
const MAX_FRAMES: usize = 512;

/// Кадры: (канал, пользователь) → (байты, тип, когда).
static FRAMES: Mutex<Option<HashMap<(String, String), (Vec<u8>, &'static str, i64)>>> =
    Mutex::new(None);

/// Тип картинки по первым байтам: только JPEG, PNG и WebP.
fn image_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// `PUT /api/rustspace/voice/{channel}/preview` — свой свежий кадр.
pub async fn put(
    State(state): State<AppState>,
    user: AuthUser,
    Path(channel): Path<String>,
    body: axum::body::Bytes,
) -> ApiResult<Response> {
    if body.len() > MAX_FRAME {
        return Err(ApiError::bad_request("preview_too_large"));
    }
    let kind = image_kind(&body).ok_or_else(|| ApiError::bad_request("preview_invalid"))?;
    if state.hub.voice.sharing_in(&user.id).as_deref() != Some(channel.as_str()) {
        return Err(ApiError::conflict("not_sharing"));
    }
    let now = now_ms();
    let mut frames = FRAMES.lock().unwrap_or_else(PoisonError::into_inner);
    let frames = frames.get_or_insert_with(HashMap::new);
    frames.retain(|_, (_, _, at)| now - *at < FRESH_MS);
    if frames.len() >= MAX_FRAMES && !frames.contains_key(&(channel.clone(), user.id.clone())) {
        return Err(ApiError::conflict("preview_busy"));
    }
    frames.insert((channel, user.id), (body.to_vec(), kind, now));
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

/// `GET /api/rustspace/voice/{channel}/preview/{user}` — свежий кадр того,
/// кто транслирует в канале.
pub async fn get(
    State(state): State<AppState>,
    user: AuthUser,
    Path((channel, streamer)): Path<(String, String)>,
) -> ApiResult<Response> {
    let viewer = user.id.clone();
    let lookup = channel.clone();
    let allowed = state
        .run(move |tx, _| {
            let Some(space) = channel_space(tx, &lookup)? else {
                return Ok(false);
            };
            Ok(perms::has_channel_permission(
                tx,
                &viewer,
                &space,
                &lookup,
                Permissions::VIEW_CHANNEL,
            )?)
        })
        .await?;
    if !allowed || state.hub.voice.sharing_in(&streamer).as_deref() != Some(channel.as_str()) {
        return Err(ApiError::not_found("preview_not_found"));
    }
    let now = now_ms();
    let frames = FRAMES.lock().unwrap_or_else(PoisonError::into_inner);
    let Some((bytes, kind, _)) = frames
        .as_ref()
        .and_then(|frames| frames.get(&(channel, streamer)))
        .filter(|(_, _, at)| now - *at < FRESH_MS)
    else {
        return Err(ApiError::not_found("preview_not_found"));
    };
    Ok((
        [(CONTENT_TYPE, *kind), (CACHE_CONTROL, "no-store")],
        bytes.clone(),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::image_kind;

    #[test]
    fn only_images_are_accepted() {
        assert_eq!(image_kind(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_kind(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(image_kind(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_kind(b"<html>"), None);
    }
}
