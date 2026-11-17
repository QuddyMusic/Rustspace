//! Личные мосты (расширение Rustspace): `/api/rustspace/personal-bridges…`.
//! Свои аккаунты Matrix и Telegram и выбор чатов, которые становятся
//! личными переписками. См. [`crate::bridges::personal`].

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::bridges::Kind;
use crate::bridges::personal::{self, matrix, telegram};
use crate::error::{ApiError, ApiResult};
use crate::extract::JsonBody;
use crate::state::AppState;

fn require_enabled(state: &AppState) -> ApiResult<()> {
    if personal::enabled(state) {
        Ok(())
    } else {
        Err(ApiError::not_found("personal_bridges_disabled"))
    }
}

fn text<'a>(body: &'a Value, key: &str) -> Option<&'a str> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
}

/// `GET /api/rustspace/personal-bridges` — сети и свои аккаунты.
pub async fn list(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    require_enabled(&state)?;
    let app = state.clone();
    let accounts = state
        .run(move |tx, _| personal::accounts_json(tx, &app, &user.id))
        .await?;
    let bridges = &state.config.bridges;
    Ok(Json(json!({
        "networks": {
            "matrix": {"available": true},
            "telegram": {
                "available": true,
                // Без ключей сервера пользователь вводит свои (my.telegram.org).
                "serverKeys": bridges.telegram_api_id.is_some() && bridges.telegram_api_hash.is_some(),
            },
        },
        "accounts": accounts,
    })))
}

/// `POST /api/rustspace/personal-bridges/matrix`
/// `{homeserver, accessToken?} | {homeserver, login, password}`.
pub async fn add_matrix(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    require_enabled(&state)?;
    let server =
        text(&body, "homeserver").ok_or_else(|| ApiError::bad_request("validation_failed"))?;
    let (me, secret) = matrix::login(
        server,
        text(&body, "accessToken"),
        text(&body, "login"),
        body.get("password").and_then(Value::as_str),
    )
    .await
    .map_err(personal::remote_error)?;
    let id = personal::add_account(&state, &user.id, Kind::Matrix, &me, &me, &secret).await?;
    match matrix::resume(&state, &id, &secret) {
        Ok(link) => state.personal.insert(&id, personal::Link::Matrix(link)),
        Err(err) => personal::set_error(&state, &id, &err).await,
    }
    Ok(Json(personal::account_view(&state, &user.id, &id).await?))
}

/// `POST /api/rustspace/personal-bridges/telegram/login` `{phone, apiId?, apiHash?}`.
pub async fn telegram_login(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    require_enabled(&state)?;
    let phone = text(&body, "phone").ok_or_else(|| ApiError::bad_request("validation_failed"))?;
    let api_id = body.get("apiId").and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .and_then(|id| i32::try_from(id).ok())
    });
    let api_hash = text(&body, "apiHash").map(|h| h.trim().to_owned());
    let login = telegram::start_login(&state, &user.id, phone, api_id, api_hash)
        .await
        .map_err(personal::remote_error)?;
    Ok(Json(json!({"loginId": login})))
}

fn step_json(step: telegram::Step) -> Value {
    match step {
        telegram::Step::Password(hint) => json!({"passwordRequired": true, "hint": hint}),
        telegram::Step::Done(account) => json!({"account": account}),
    }
}

/// `POST /api/rustspace/personal-bridges/telegram/code` `{loginId, code}`.
pub async fn telegram_code(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    require_enabled(&state)?;
    let (Some(login), Some(code)) = (text(&body, "loginId"), text(&body, "code")) else {
        return Err(ApiError::bad_request("validation_failed"));
    };
    telegram::submit_code(&state, &user.id, login, code)
        .await
        .map(|step| Json(step_json(step)))
        .map_err(personal::remote_error)
}

/// `POST /api/rustspace/personal-bridges/telegram/password` `{loginId, password}`.
pub async fn telegram_password(
    State(state): State<AppState>,
    user: AuthUser,
    JsonBody(body): JsonBody,
) -> ApiResult<Json<Value>> {
    require_enabled(&state)?;
    let (Some(login), Some(password)) = (
        text(&body, "loginId"),
        body.get("password").and_then(Value::as_str),
    ) else {
        return Err(ApiError::bad_request("validation_failed"));
    };
    telegram::submit_password(&state, &user.id, login, password)
        .await
        .map(|step| Json(step_json(step)))
        .map_err(personal::remote_error)
}

/// `DELETE /api/rustspace/personal-bridges/:id`.
pub async fn remove(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_enabled(&state)?;
    personal::remove_account(&state, &user.id, &id).await?;
    Ok(Json(json!({"success": true})))
}

/// `GET /api/rustspace/personal-bridges/:id/remote-chats`.
pub async fn remote_chats(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_enabled(&state)?;
    Ok(Json(Value::Array(
        personal::remote_chats(&state, &user.id, &id).await?,
    )))
}

/// `PUT /api/rustspace/personal-bridges/:id/chats` `{remoteChat, title, group}`.
pub async fn link_chat(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<String>,
    JsonBody(body): JsonBody,
) -> ApiResult<(StatusCode, Json<Value>)> {
    require_enabled(&state)?;
    let remote = text(&body, "remoteChat")
        .filter(|chat| chat.len() <= 255)
        .ok_or_else(|| ApiError::bad_request("validation_failed"))?
        .to_owned();
    let title = text(&body, "title").unwrap_or(&remote).to_owned();
    let group = body.get("group").and_then(Value::as_bool).unwrap_or(false);
    let linked = personal::link_chat(&state, &user.id, &id, remote, title, group).await?;
    Ok((StatusCode::CREATED, Json(linked)))
}

/// `DELETE /api/rustspace/personal-bridges/:id/chats/:chat`.
pub async fn unlink_chat(
    State(state): State<AppState>,
    user: AuthUser,
    Path((id, chat)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    require_enabled(&state)?;
    personal::unlink_chat(&state, &user.id, &id, &chat).await?;
    Ok(Json(json!({"success": true})))
}
