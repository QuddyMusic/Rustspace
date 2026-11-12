//! Ответы об ошибках в формате оригинала: `{ error, code, statusCode, details? }`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::db::DbError;
use crate::error_messages::message;

/// Ошибка обработчика.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    details: Option<Map<String, Value>>,
    /// Свой текст вместо табличного (так оригинал отвечает в проверке токена).
    text: Option<&'static str>,
    /// Дополнительные поля верхнего уровня (например, `retryAfter`).
    extra: Map<String, Value>,
}

impl ApiError {
    /// Ошибка с кодом из таблицы оригинала.
    #[must_use]
    pub fn new(status: StatusCode, code: &'static str) -> Self {
        Self {
            status,
            code,
            details: None,
            text: None,
            extra: Map::new(),
        }
    }

    /// 400 с кодом.
    #[must_use]
    pub fn bad_request(code: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code)
    }

    /// 401 с кодом.
    #[must_use]
    pub fn unauthorized(code: &'static str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code)
    }

    /// 403 с кодом.
    #[must_use]
    pub fn forbidden(code: &'static str) -> Self {
        Self::new(StatusCode::FORBIDDEN, code)
    }

    /// 404 с кодом.
    #[must_use]
    pub fn not_found(code: &'static str) -> Self {
        Self::new(StatusCode::NOT_FOUND, code)
    }

    /// 409 с кодом.
    #[must_use]
    pub fn conflict(code: &'static str) -> Self {
        Self::new(StatusCode::CONFLICT, code)
    }

    /// 500 без подробностей наружу.
    #[must_use]
    pub fn internal() -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal")
    }

    /// Добавляет значение в `details` (оно же подставляется в текст).
    #[must_use]
    pub fn detail(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.details
            .get_or_insert_with(Map::new)
            .insert(name.to_owned(), value.into());
        self
    }

    /// Свой текст ошибки.
    #[must_use]
    pub fn text(mut self, text: &'static str) -> Self {
        self.text = Some(text);
        self
    }

    /// Дополнительное поле верхнего уровня.
    #[must_use]
    pub fn extra(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.extra.insert(name.to_owned(), value.into());
        self
    }

    /// Код ошибки.
    #[must_use]
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// Свой текст, если он задан.
    #[must_use]
    pub fn custom_text(&self) -> Option<&'static str> {
        self.text
    }

    /// HTTP-статус.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        self.status
    }
}

/// Тексты ошибок расширений этого сервера, которых нет у оригинала.
fn extension_message(code: &str) -> Option<&'static str> {
    Some(match code {
        "two_factor_required" => "Two-factor authentication code required",
        "two_factor_invalid" => "Invalid two-factor authentication code",
        "two_factor_already_enabled" => "Two-factor authentication is already enabled",
        "two_factor_not_enabled" => "Two-factor authentication is not enabled",
        "two_factor_setup_missing" => "Start two-factor setup first",
        "two_factor_rate_limited" => "Too many two-factor attempts; try again later",
        "e2ee_key_invalid" => "Invalid encryption key bundle",
        "e2ee_key_not_found" => "This user has not published an encryption key",
        "e2ee_payload_invalid" => "Invalid encrypted message",
        _ => return None,
    })
}

/// Текст ошибки по коду с подстановкой `{{имя}}` из `details`.
#[must_use]
pub fn error_text(code: &str, details: Option<&Map<String, Value>>) -> String {
    let template = message(code)
        .or_else(|| extension_message(code))
        .unwrap_or(code);
    let Some(details) = details else {
        return template.to_owned();
    };
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = &after[..end];
        match details.get(name) {
            Some(Value::String(text)) => out.push_str(text),
            Some(value) => out.push_str(&value.to_string()),
            None => {
                out.push_str("{{");
                out.push_str(name);
                out.push_str("}}");
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status == StatusCode::INTERNAL_SERVER_ERROR && self.code == "internal" {
            // Как ответ Fastify на необработанное исключение, но без текста
            // исключения: он бывает с путями и SQL.
            return (
                self.status,
                Json(json!({
                    "statusCode": 500,
                    "error": "Internal Server Error",
                    "message": "Internal Server Error",
                })),
            )
                .into_response();
        }
        let mut body = Map::new();
        let text = self.text.map_or_else(
            || error_text(self.code, self.details.as_ref()),
            str::to_owned,
        );
        body.insert("error".to_owned(), Value::String(text));
        body.insert("code".to_owned(), Value::String(self.code.to_owned()));
        body.insert("statusCode".to_owned(), Value::from(self.status.as_u16()));
        if let Some(details) = self.details {
            body.insert("details".to_owned(), Value::Object(details));
        }
        body.extend(self.extra);
        (self.status, Json(Value::Object(body))).into_response()
    }
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        tracing::error!(%err, "ошибка базы данных");
        Self::internal()
    }
}

/// Результат обработчика.
pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_filled_from_details() {
        let error = ApiError::bad_request("password_too_short").detail("min", 8);
        assert_eq!(
            error_text(error.code, error.details.as_ref()),
            "Password must be at least 8 characters"
        );
        assert_eq!(
            error_text("username_length_invalid", None),
            "Username must be between {{min}} and {{max}} characters"
        );
    }

    #[test]
    fn every_code_is_sorted_for_lookup() {
        assert_eq!(message("not_found"), Some("Not found"));
        assert_eq!(message("username_taken"), Some("Username is already taken"));
        assert!(message("PEER_REVOKED").is_some());
        assert_eq!(message("no_such_code"), None);
    }
}
