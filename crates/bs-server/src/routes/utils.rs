//! Служебные запросы клиента (`routes/utils.ts`).

use axum::Json;
use axum::extract::{Query, State};
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::embeds;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Параметры запроса описания ссылки.
#[derive(Debug, serde::Deserialize)]
pub struct MetadataQuery {
    url: Option<String>,
}

/// `GET /api/utils/metadata`: заголовок, описание и картинка страницы.
pub async fn metadata(
    State(_state): State<AppState>,
    _user: AuthUser,
    Query(query): Query<MetadataQuery>,
) -> ApiResult<Json<Value>> {
    let Some(url) = query.url.filter(|url| !url.is_empty()) else {
        return Err(ApiError::bad_request("validation_failed"));
    };
    Ok(Json(
        embeds::fetch_metadata(&url)
            .await
            .map_or_else(|| json!({}), |meta| meta.to_json(&url)),
    ))
}
