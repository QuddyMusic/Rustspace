//! Раздача собранного веб-клиента и политика содержимого страницы
//! (`index.ts`, `utils/csp.ts`).

use std::path::{Component, Path, PathBuf};

use axum::Json;
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use url::Url;

use crate::state::AppState;

/// Значение `Content-Security-Policy` для страниц и ответов API
/// (`buildCspHeaderValue`).
#[must_use]
pub fn csp_header(livekit_url: Option<&str>) -> String {
    let livekit = livekit_url
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .and_then(|url| Url::parse(url).ok())
        .filter(|url| matches!(url.scheme(), "ws" | "wss" | "http" | "https"))
        .map(|url| url.origin().ascii_serialization());
    let mut connect = vec!["'self'", "https:", "http:", "wss:", "ws:", "blob:", "data:"];
    if let Some(origin) = livekit.as_deref() {
        connect.push(origin);
    }
    let connect = connect.join(" ");
    [
        "default-src 'self'",
        "script-src 'self' 'wasm-unsafe-eval'",
        "style-src 'self' 'unsafe-inline'",
        "img-src 'self' data: blob: https: http:",
        "media-src 'self' data: blob: https: http:",
        "font-src 'self' data:",
        &format!("connect-src {connect}"),
        "worker-src 'self' blob:",
        "child-src 'self' blob:",
        "frame-src https://www.youtube-nocookie.com https://player.vimeo.com https://open.spotify.com",
        "manifest-src 'self'",
        "object-src 'none'",
        "base-uri 'self'",
        "form-action 'self'",
        "frame-ancestors 'none'",
        "report-uri /api/csp-report",
        "report-to csp",
    ]
    .join("; ")
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Путь внутри каталога клиента; `None`, если запрос пытается выйти наружу.
fn inside(root: &Path, request_path: &str) -> Option<PathBuf> {
    let decoded = percent_decode(request_path)?;
    let relative = Path::new(decoded.trim_start_matches('/'));
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(relative))
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = text.get(at + 1..at + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    let decoded = String::from_utf8(out).ok()?;
    (!decoded.contains('\0') && !decoded.contains('\\')).then_some(decoded)
}

async fn file_response(path: &Path, immutable: bool) -> Option<Response> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type(path)));
    let cache = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache));
    Some(response)
}

/// Всё, что не нашло маршрута: файлы веб-клиента и его страница для
/// остальных адресов; для API — ответ «не найдено».
pub async fn fallback(State(state): State<AppState>, method: Method, uri: Uri) -> Response {
    let path = uri.path();
    let api = path.starts_with("/api/") || path.starts_with("/ws");
    let Some(root) = state.config.web_dist.as_deref().filter(|_| !api) else {
        let shown = uri
            .path_and_query()
            .map_or(path, axum::http::uri::PathAndQuery::as_str);
        let body = if state.config.web_dist.is_some() {
            json!({"error": "Not found", "statusCode": 404})
        } else {
            json!({"message": format!("Route {method}:{shown} not found"), "error": "Not Found", "statusCode": 404})
        };
        return (StatusCode::NOT_FOUND, Json(body)).into_response();
    };
    if method != Method::GET && method != Method::HEAD {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Not found", "statusCode": 404})),
        )
            .into_response();
    }
    if path != "/"
        && let Some(file) = inside(root, path)
        && let Some(response) = file_response(&file, path.starts_with("/assets/")).await
    {
        return response;
    }
    match file_response(&root.join("index.html"), false).await {
        Some(page) => page,
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Not found", "statusCode": 404})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_names_the_voice_server() {
        let policy = csp_header(Some("wss://voice.example:7443/rtc"));
        assert!(policy.contains(
            "connect-src 'self' https: http: wss: ws: blob: data: wss://voice.example:7443;"
        ));
        assert!(policy.starts_with("default-src 'self'; script-src 'self' 'wasm-unsafe-eval';"));
        assert!(policy.ends_with("report-uri /api/csp-report; report-to csp"));
        assert!(csp_header(Some("javascript:alert(1)")).contains("data:; worker-src"));
        assert!(csp_header(None).contains("data:; worker-src"));
    }

    #[test]
    fn requests_cannot_leave_the_client_directory() {
        let root = Path::new("/srv/web");
        assert_eq!(
            inside(root, "/assets/app.js"),
            Some(PathBuf::from("/srv/web/assets/app.js"))
        );
        assert_eq!(
            inside(root, "/a%20b.png"),
            Some(PathBuf::from("/srv/web/a b.png"))
        );
        for path in [
            "/../etc/passwd",
            "/a/../../b",
            "/%2e%2e/secret",
            "/a/%2E%2E/%2E%2E/b",
            "/x%00.js",
            "/..%5cwin",
            "/%zz",
            "/./x",
        ] {
            assert_eq!(inside(root, path), None, "{path}");
        }
    }
}
