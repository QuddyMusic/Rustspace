//! Токены LiveKit для голосовых каналов и звонков (`routes/livekit.ts`).

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use bs_permissions::Permissions;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::messages::channel_space;
use crate::state::{AppState, now_ms};
use crate::util::str_field;
use crate::{dm, federation, perms};

/// Что участник может публиковать в комнате.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Publish {
    /// Микрофон и камера.
    pub speak: bool,
    /// Показ экрана со звуком.
    pub stream: bool,
}

/// Подписывает токен доступа LiveKit (как `AccessToken` из `livekit-server-sdk`).
///
/// # Errors
/// Голос не настроен или подпись не удалась.
pub fn access_token(
    state: &AppState,
    room: &str,
    identity: &str,
    publish: Publish,
    ttl_seconds: i64,
) -> ApiResult<String> {
    let (Some(key), Some(secret)) = (
        &state.config.livekit.api_key,
        &state.config.livekit.api_secret,
    ) else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "voice_disabled",
        ));
    };
    let mut sources = Vec::new();
    if publish.speak {
        sources.extend(["microphone", "camera"]);
    }
    if publish.stream {
        sources.extend(["screen_share", "screen_share_audio"]);
    }
    let now = now_ms() / 1000;
    let claims = json!({
        "iss": key,
        "sub": identity,
        "nbf": now,
        "exp": now + ttl_seconds,
        "video": {
            "room": room,
            "roomJoin": true,
            "canPublish": publish.speak || publish.stream,
            "canPublishSources": sources,
            "canSubscribe": true,
            "canPublishData": true,
        },
    });
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|_| ApiError::internal())
}

/// `POST /api/livekit/token`.
pub async fn token(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    if state.config.livekit.api_key.is_none() || state.config.livekit.api_secret.is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "voice_disabled",
        ));
    }
    let app = state.clone();
    let identity = format!("{}:{}", user.id, user.username);
    let (room, publish) = state
        .run(move |tx, _| {
            if let Some(dm_channel) = str_field(&body, "dmChannelId").filter(|id| !id.is_empty()) {
                if !dm::is_member(tx, dm_channel, &user.id)? {
                    return Err(ApiError::forbidden("not_dm_member"));
                }
                let room = federation::dm_federated_id(tx, dm_channel)?
                    .unwrap_or_else(|| format!("dm-{dm_channel}"));
                return Ok((
                    room,
                    Publish {
                        speak: true,
                        stream: true,
                    },
                ));
            }
            let Some(channel) = str_field(&body, "channelId").filter(|id| !id.is_empty()) else {
                return Err(ApiError::bad_request("validation_failed"));
            };
            let space = channel_space(tx, channel)?
                .ok_or_else(|| ApiError::not_found("channel_not_found"))?;
            let held = perms::compute(tx, &user.id, &space, Some(channel))?;
            // Оригинал спрашивает только `CONNECT`. Приватный канал закрыт
            // запретом `VIEW_CHANNEL`, а `CONNECT` у участника остаётся, и
            // токен скрытой комнаты позволял её слушать. Здесь нужны оба права.
            if !perms::has(held, Permissions::CONNECT) || !perms::can_view(held) {
                return Err(ApiError::forbidden("voice_connect_forbidden"));
            }
            let restricted = crate::voice::restrictions(tx, &space, &user.id)?;
            Ok((
                channel.to_owned(),
                Publish {
                    speak: perms::has(held, Permissions::SPEAK) && !restricted.muted,
                    stream: perms::has(held, Permissions::STREAM) && !restricted.muted,
                },
            ))
        })
        .await?;
    let jwt = access_token(&app, &room, &identity, publish, 3600)?;
    Ok(Json(json!({
        "token": jwt,
        "url": app.config.livekit.url.clone().unwrap_or_default(),
    })))
}
