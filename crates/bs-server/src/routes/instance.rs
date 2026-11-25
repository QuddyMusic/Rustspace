//! Сведения об инстансе и проверка живости.

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use crate::db::{DbResult, Tx};
use crate::error::ApiResult;
use crate::params;
use crate::state::{AppState, now_ms};

/// Открыта ли регистрация: решение администратора, а пока его нет —
/// значение из окружения.
///
/// # Errors
/// Ошибка базы.
pub fn registration_open(state: &AppState, tx: &mut dyn Tx) -> DbResult<bool> {
    let row = tx.query_one(
        "SELECT registration_open FROM instance_settings WHERE id = 1",
        params![],
    )?;
    Ok(row
        .map(|row| row.opt_flag("registration_open"))
        .transpose()?
        .flatten()
        .unwrap_or(state.config.registration_open))
}

/// Браузер (`User-Agent: Mozilla/…`): ванильный веб-клиент или десктоп
/// Backspace (Electron).
fn browser(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|agent| agent.starts_with("Mozilla/"))
}

/// `GET /api/instance/info`.
///
/// `version` браузеру (ванильный веб и десктоп Backspace) — версия Backspace,
/// протокол которой сервер реализует: ванильный клиент строит по ней ссылки
/// на свой десктоп-клиент, и версия Rustspace дала бы ссылки на
/// несуществующий релиз Backspace. Остальным (клиент Rustspace, помощник
/// обновления, пиры) — версия Rustspace, как раньше. Версия Rustspace всегда
/// есть и в `rustspaceVersion`.
pub async fn info(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Json<Value>> {
    let version = if browser(&headers) {
        crate::config::BACKSPACE_COMPAT_VERSION.to_owned()
    } else {
        state.config.version.clone()
    };
    let db = state.db.clone();
    db.transaction(move |tx| {
        let open = registration_open(&state, tx)?;
        let row = tx.query_one(
            "SELECT instance_name, federated_registration_open, directory_enabled, \
             directory_browse_enabled, support_card_enabled FROM instance_settings WHERE id = 1",
            params![],
        )?;
        let flag = |name: &str| -> DbResult<Option<bool>> {
            row.as_ref().map_or(Ok(None), |row| row.opt_flag(name))
        };
        let configured = !state.config.directory_endpoint.is_empty();
        Ok(Json(json!({
            "name": row
                .as_ref()
                .map(|row| row.opt_text("instance_name"))
                .transpose()?
                .flatten()
                .unwrap_or_else(|| "Rustspace".to_owned()),
            "version": version,
            "rustspaceVersion": state.config.version,
            "compatibleWith": crate::config::BACKSPACE_COMPAT_VERSION,
            "registrationOpen": open,
            "federatedRegistrationOpen": flag("federated_registration_open")? == Some(true),
            "instanceId": &*state.instance_id,
            "sourceCodeUrl": state.config.source_code_url,
            "commit": state.config.commit,
            "directoryConfigured": configured,
            "directoryAvailable": configured && flag("directory_browse_enabled")? == Some(true),
            "directoryEnabled": flag("directory_enabled")? == Some(true),
            "supportCardEnabled": flag("support_card_enabled")?.unwrap_or(true),
        })))
    })
    .await
}

/// `GET /api/health`.
pub async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "timestamp": now_ms() }))
}
