//! Раздача собранного веб-клиента и политика содержимого страницы
//! (`index.ts`, `utils/csp.ts`).

use std::path::{Component, Path, PathBuf};

use axum::Json;
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, Method, StatusCode};
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

/// Плитка Rustspace 512×512 — значок веб-версии на экране и в PWA.
const TILE_PNG: &[u8] = include_bytes!("../../bs-app/assets/brand-tile.png");
/// Плашка «Rustspace» поверх веб-клиента оригинала.
const BRAND_CSS: &str = "#rustspace-badge{position:fixed;top:6px;left:50%;\
transform:translateX(-50%);z-index:2147483000;display:flex;align-items:center;gap:6px;\
padding:2px 10px 2px 4px;border-radius:999px;background:rgba(183,65,14,.92);color:#fff;\
font:600 11px/18px system-ui,-apple-system,'Segoe UI',sans-serif;text-decoration:none;\
box-shadow:0 2px 10px rgba(0,0,0,.35);opacity:.85;transition:opacity .15s}\
#rustspace-badge:hover{opacity:1}#rustspace-badge img{width:16px;height:16px;\
border-radius:4px}\
body:has(nav) #rustspace-badge{top:auto;bottom:72px;left:16px;transform:none;\
padding:0;width:40px;height:40px;border-radius:12px;background:none;font-size:0;gap:0;\
opacity:.75}\
body:has(nav) #rustspace-badge img{width:40px;height:40px;border-radius:12px}\
@media (max-width:640px){#rustspace-badge{display:none}}";

/// Файлы брендирования веб-версии (`/rustspace/brand/…`).
fn brand_asset(path: &str) -> Option<Response> {
    let (body, kind): (Vec<u8>, &'static str) = match path {
        "/rustspace/brand/icon.svg" => (ICON.as_bytes().to_vec(), "image/svg+xml"),
        "/rustspace/brand/icon-512.png" => (TILE_PNG.to_vec(), "image/png"),
        "/rustspace/brand/web.css" => (BRAND_CSS.as_bytes().to_vec(), "text/css; charset=utf-8"),
        _ => return None,
    };
    Some(
        (
            [
                (CONTENT_TYPE, kind),
                (CACHE_CONTROL, "public, max-age=86400"),
            ],
            body,
        )
            .into_response(),
    )
}

/// `index.html` веб-клиента оригинала с приметами Rustspace: заголовок
/// вкладки, значки, имя приложения и плашка-ссылка вверху. Сама сборка не
/// меняется — подмена при раздаче, поэтому годится и чужая `--web-dist`.
fn brand_index(page: &str, config: &crate::config::Config) -> String {
    let title = "Rustspace";
    let tip = format!(
        "Rustspace {} — неофициальный сервер на Rust, совместимый с Backspace. \
         Веб-клиент — оригинальный Backspace. / Unofficial Rust server compatible \
         with Backspace; the web client is the original Backspace.",
        config.version
    );
    let head = format!(
        r#"<link rel="icon" type="image/svg+xml" href="/rustspace/brand/icon.svg" /><link rel="apple-touch-icon" href="/rustspace/brand/icon-512.png" /><meta name="application-name" content="{title}" /><link rel="stylesheet" href="/rustspace/brand/web.css" /></head>"#
    );
    let badge = format!(
        r#"<a id="rustspace-badge" href="{}" target="_blank" rel="noopener" title="{}"><img src="/rustspace/brand/icon.svg" alt="" />Rustspace</a>"#,
        escape_html(&config.source_code_url),
        escape_html(&tip)
    );
    let mut out = page
        // Значки оригинала уступают своим.
        .replace(r#"rel="icon""#, r#"rel="x-backspace-icon""#)
        .replace(r#"rel="apple-touch-icon""#, r#"rel="x-backspace-touch-icon""#)
        .replace("<title>Backspace</title>", "<title>Rustspace</title>")
        .replace(
            r#"name="apple-mobile-web-app-title" content="Backspace""#,
            r#"name="apple-mobile-web-app-title" content="Rustspace""#,
        )
        .replacen("</head>", &head, 1);
    if let Some(start) = out.find("<body")
        && let Some(end) = out[start..].find('>')
    {
        out.insert_str(start + end + 1, &badge);
    }
    out
}

/// Манифест PWA: установленное приложение называется Rustspace и носит его
/// значок. Не JSON — отдаётся как есть.
fn brand_manifest(text: &str) -> String {
    let Ok(mut manifest) = serde_json::from_str::<serde_json::Value>(text) else {
        return text.to_owned();
    };
    if let Some(object) = manifest.as_object_mut() {
        object.insert("name".into(), json!("Rustspace"));
        object.insert("short_name".into(), json!("Rustspace"));
        object.insert(
            "description".into(),
            json!("Rustspace — unofficial Rust server compatible with Backspace, with the Backspace web client."),
        );
        object.insert(
            "icons".into(),
            json!([
                {"src": "/rustspace/brand/icon-512.png", "sizes": "512x512", "type": "image/png"},
                {"src": "/rustspace/brand/icon-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable"},
                {"src": "/rustspace/brand/icon.svg", "sizes": "any", "type": "image/svg+xml"}
            ]),
        );
    }
    manifest.to_string()
}

/// Отдаёт файл клиента; `index.html` и манифест — с приметами Rustspace.
async fn client_file(
    path: &Path,
    immutable: bool,
    config: &crate::config::Config,
) -> Option<Response> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if name != "index.html" && name != "manifest.webmanifest" {
        return file_response(path, immutable).await;
    }
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let (body, kind) = if name == "index.html" {
        (brand_index(&text, config), "text/html; charset=utf-8")
    } else {
        (brand_manifest(&text), "application/manifest+json")
    };
    Some(([(CONTENT_TYPE, kind), (CACHE_CONTROL, "no-cache")], body).into_response())
}

/// Страница «здесь нет веб-версии» для `/`, когда веб-клиент не подключён.
const LANDING: &str = include_str!("landing.html");
/// Значок Rustspace (`packaging/brand/rustspace/icon.svg`).
const ICON: &str = include_str!("../../../packaging/brand/rustspace/icon.svg");

/// Текст, безопасный внутри HTML и его атрибутов.
fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Заглушка на `/`: инстанс принимает только десктоп-клиент на Rust.
/// Язык страницы-заглушки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Ru,
    En,
}

impl Lang {
    const fn code(self) -> &'static str {
        match self {
            Self::Ru => "ru",
            Self::En => "en",
        }
    }
}

/// Язык заглушки: `?lang=ru|en`, иначе первый по весу язык из
/// `Accept-Language`. Русский — тем, кто предпочитает русский (а также
/// белорусский и казахский интерфейс, где русский обычно понимают),
/// остальным — английский.
fn pick_lang(query: Option<&str>, accept_language: Option<&str>) -> Lang {
    let forced = query.and_then(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == "lang")
            .map(|(_, value)| value.to_ascii_lowercase())
    });
    match forced.as_deref() {
        Some("ru") => return Lang::Ru,
        Some("en") => return Lang::En,
        _ => {}
    }
    let best = accept_language
        .into_iter()
        .flat_map(|header| header.split(','))
        .filter_map(|item| {
            let mut parts = item.split(';');
            let tag = parts.next()?.trim().to_ascii_lowercase();
            let weight = parts
                .find_map(|param| param.trim().strip_prefix("q="))
                .map_or(Some(1.0), |q| q.trim().parse::<f32>().ok())?;
            (!tag.is_empty() && weight > 0.0).then_some((tag, weight))
        })
        // Первый из равных по весу — как в заголовке.
        .fold(None::<(String, f32)>, |best, (tag, weight)| match best {
            Some((_, top)) if top >= weight => best,
            _ => Some((tag, weight)),
        });
    match best {
        Some((tag, _))
            if ["ru", "be", "kk"].contains(&tag.split('-').next().unwrap_or_default()) =>
        {
            Lang::Ru
        }
        _ => Lang::En,
    }
}

/// Убирает из шаблона блок другого языка (`<!--RU-->…<!--/RU-->`).
fn keep_language(page: &str, lang: Lang) -> String {
    let (drop_open, drop_close) = match lang {
        Lang::Ru => ("<!--EN-->", "<!--/EN-->"),
        Lang::En => ("<!--RU-->", "<!--/RU-->"),
    };
    let mut out = page.to_owned();
    if let (Some(start), Some(end)) = (out.find(drop_open), out.find(drop_close)) {
        out.replace_range(start..end + drop_close.len(), "");
    }
    out.replace("<!--RU-->", "")
        .replace("<!--/RU-->", "")
        .replace("<!--EN-->", "")
        .replace("<!--/EN-->", "")
}

fn landing_page(config: &crate::config::Config, lang: Lang) -> String {
    use base64::Engine as _;
    let origin = crate::federation::our_origin(config);
    let host = origin
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_owned();
    keep_language(LANDING, lang)
        .replace("{{LANG}}", lang.code())
        .replace(
            "{{ICON_B64}}",
            &base64::engine::general_purpose::STANDARD.encode(ICON),
        )
        .replace("{{ICON_SVG}}", ICON.trim())
        .replace("{{HOST}}", &escape_html(&host))
        .replace("{{ORIGIN}}", &escape_html(&origin))
        .replace(
            "{{CLIENT_LINK_RU}}",
            &client_link(
                config,
                "исходный код и сборки",
                "ссылку на сборки даст администратор инстанса",
            ),
        )
        .replace(
            "{{CLIENT_LINK_EN}}",
            &client_link(
                config,
                "source and builds",
                "ask the instance admin for a build",
            ),
        )
        .replace("{{SOURCE}}", &escape_html(&config.source_code_url))
        .replace("{{VERSION}}", &escape_html(&config.version))
}

/// Ссылка на клиент: на свой репозиторий, если он задан
/// (`BACKSPACE_SOURCE_URL`), — в репозитории оригинала клиента на Rust нет.
fn client_link(config: &crate::config::Config, linked: &str, otherwise: &str) -> String {
    if config.source_code_url == crate::config::UPSTREAM_SOURCE_URL {
        otherwise.to_owned()
    } else {
        format!(
            r#"<a href="{}" rel="noopener">{linked}</a>"#,
            escape_html(&config.source_code_url)
        )
    }
}

fn landing(config: &crate::config::Config, lang: Lang) -> Response {
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (axum::http::header::CACHE_CONTROL, "no-cache"),
            (axum::http::header::VARY, "Accept-Language"),
        ],
        landing_page(config, lang),
    )
        .into_response()
}

/// Файл сборки веб-клиента: в общий лимит запросов не идёт. Сервис-воркер
/// ванильного клиента при первом открытии разом загружает всю сборку
/// (больше сотни файлов), и несколько новых устройств за одним NAT
/// исчерпали бы лимит в 200 запросов в минуту. API, загрузки и `/ws`
/// считаются как раньше.
#[must_use]
pub fn is_static_file(method: &Method, path: &str) -> bool {
    if method != Method::GET && method != Method::HEAD {
        return false;
    }
    if path.starts_with("/assets/") {
        return true;
    }
    let name = path.rsplit('/').next().unwrap_or_default();
    ["/api/", "/ws", "/uploads/", "/.well-known/", "/rustspace/"]
        .iter()
        .all(|prefix| !path.starts_with(prefix))
        && path.matches('/').count() <= 2
        && name.contains('.')
}

/// Всё, что не нашло маршрута: файлы веб-клиента и его страница для
/// остальных адресов; для API — ответ «не найдено».
pub async fn fallback(State(state): State<AppState>, request: axum::extract::Request) -> Response {
    // Тело разбирается и для ненайденного маршрута: JSON с ошибкой даёт 400.
    let request = match crate::extract::parse_stage(request, false).await {
        Ok(request) => request,
        Err(refusal) => return refusal,
    };
    let method = request.method().clone();
    let uri = request.uri().clone();
    let path = uri.path();
    let api = path.starts_with("/api/") || path.starts_with("/ws");
    if (method == Method::GET || method == Method::HEAD)
        && let Some(asset) = brand_asset(path)
    {
        return asset;
    }
    // Без веб-клиента главная страница объясняет, чем подключаться.
    if state.config.web_dist.is_none()
        && path == "/"
        && (method == Method::GET || method == Method::HEAD)
    {
        let accept = request
            .headers()
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok());
        return landing(&state.config, pick_lang(uri.query(), accept));
    }
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
        && let Some(response) =
            client_file(&file, path.starts_with("/assets/"), &state.config).await
    {
        return response;
    }
    match client_file(&root.join("index.html"), false, &state.config).await {
        Some(page) => page,
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Not found", "statusCode": 404})),
        )
            .into_response(),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    #[test]
    fn web_client_files_skip_the_global_limit() {
        use axum::http::Method;
        assert!(is_static_file(&Method::GET, "/assets/index-abc.js"));
        assert!(is_static_file(&Method::GET, "/sw.js"));
        assert!(is_static_file(
            &Method::HEAD,
            "/fonts/DMSans-Variable.woff2"
        ));
        assert!(!is_static_file(&Method::POST, "/assets/x.js"));
        assert!(!is_static_file(&Method::GET, "/api/instance/info"));
        assert!(!is_static_file(&Method::GET, "/uploads/a/b.png"));
        assert!(!is_static_file(&Method::GET, "/ws"));
        assert!(!is_static_file(&Method::GET, "/"));
        assert!(!is_static_file(&Method::GET, "/rustspace/passkey.js"));
        assert!(!is_static_file(&Method::GET, "/a/b/c/d.js"));
    }

    #[test]
    fn landing_speaks_the_visitors_language() {
        assert_eq!(pick_lang(None, Some("ru-RU,ru;q=0.9,en;q=0.8")), Lang::Ru);
        assert_eq!(pick_lang(None, Some("en-US,en;q=0.9,ru;q=0.8")), Lang::En);
        assert_eq!(pick_lang(None, Some("de;q=0.5, ru;q=0.7")), Lang::Ru);
        assert_eq!(pick_lang(None, Some("uk-UA,uk;q=0.9")), Lang::En);
        assert_eq!(pick_lang(None, None), Lang::En);
        assert_eq!(pick_lang(Some("lang=ru"), Some("en")), Lang::Ru);
        assert_eq!(pick_lang(Some("x=1&lang=en"), Some("ru")), Lang::En);
        let plain = std::iter::once(("JWT_SECRET".to_owned(), "s".repeat(40))).collect();
        let config = crate::config::Config::from_map(&plain).expect("конфигурация");
        let page = landing_page(&config, Lang::En);
        assert!(page.contains(r#"<html lang="en">"#));
        assert!(page.contains("There is no web app here") && page.contains("source and builds"));
        assert!(!page.contains("Здесь нет веб-версии") && !page.contains("<!--"));
        assert!(page.contains(r#"href="?lang=ru""#));
    }

    #[test]
    fn landing_names_the_instance_and_escapes() {
        let vars = [
            ("JWT_SECRET", "s".repeat(40)),
            ("DOMAIN", "chat.example".to_owned()),
            (
                "BACKSPACE_SOURCE_URL",
                "https://example.org/src?a=1&b=2".to_owned(),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let config = crate::config::Config::from_map(&vars).expect("конфигурация");
        let page = landing_page(&config, Lang::Ru);
        assert!(!page.contains("{{"), "все подстановки сделаны");
        assert!(!page.contains("No web app") && !page.contains("<!--"));
        assert!(page.contains("https://chat.example"));
        assert!(page.contains(r#"<a href="https://example.org/src?a=1&amp;b=2" rel="noopener">исходный код и сборки</a>"#));
        // По умолчанию ссылка ведёт в репозиторий Rustspace.
        let plain = std::iter::once(("JWT_SECRET".to_owned(), "s".repeat(40))).collect();
        let page = landing_page(
            &crate::config::Config::from_map(&plain).expect("конфигурация"),
            Lang::Ru,
        );
        assert!(page.contains(crate::config::SOURCE_URL));
        assert!(page.contains("<svg"));
        // Ссылка на оригинал клиента на Rust не даёт — её не показываем.
        let upstream = [
            ("JWT_SECRET", "s".repeat(40)),
            (
                "BACKSPACE_SOURCE_URL",
                crate::config::UPSTREAM_SOURCE_URL.to_owned(),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let page = landing_page(
            &crate::config::Config::from_map(&upstream).expect("конфигурация"),
            Lang::Ru,
        );
        assert!(page.contains("ссылку на сборки даст администратор"));
        assert!(!page.contains("исходный код и сборки"));
        assert_eq!(
            escape_html(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    use super::*;

    #[test]
    fn web_client_is_marked_as_rustspace() {
        let plain = std::iter::once(("JWT_SECRET".to_owned(), "s".repeat(40))).collect();
        let config = crate::config::Config::from_map(&plain).expect("конфигурация");
        let page = r#"<html><head><meta name="apple-mobile-web-app-title" content="Backspace" /><link rel="icon" type="image/png" href="/icons/favicon-32.png" /><link rel="apple-touch-icon" href="/icons/apple-touch-icon.png" /><title>Backspace</title></head><body class="x"><div id="root"></div></body></html>"#;
        let branded = brand_index(page, &config);
        assert!(branded.contains("<title>Rustspace</title>"));
        assert!(branded.contains(r#"content="Rustspace""#));
        assert!(!branded.contains(r#"rel="icon" type="image/png""#));
        assert!(branded.contains(
            r#"<link rel="icon" type="image/svg+xml" href="/rustspace/brand/icon.svg" />"#
        ));
        assert!(branded.contains(r#"<body class="x"><a id="rustspace-badge""#));
        assert!(branded.contains(&escape_html(&config.source_code_url)));
        let manifest =
            brand_manifest(r#"{"name":"Backspace","short_name":"Backspace","start_url":"/"}"#);
        let value: serde_json::Value = serde_json::from_str(&manifest).expect("JSON");
        assert_eq!(value["name"], "Rustspace");
        assert_eq!(value["start_url"], "/");
        assert_eq!(value["icons"][0]["src"], "/rustspace/brand/icon-512.png");
        assert!(brand_asset("/rustspace/brand/web.css").is_some());
        assert!(brand_asset("/rustspace/brand/../x").is_none());
    }

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
