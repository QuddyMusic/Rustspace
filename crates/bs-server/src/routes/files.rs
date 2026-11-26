//! Возобновляемые загрузки по протоколу tus 1.0 (`routes/files.ts`).
//!
//! Оригинал отдаёт протокол библиотеке `@tus/server` с файловым хранилищем;
//! здесь те же ответы собраны вручную: создание, дозапись, состояние,
//! отмена, срок жизни. Незавершённая загрузка лежит в `<uploads>/.tus/<id>`
//! рядом с описанием `<id>.json` того же вида, что у оригинала.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, LOCATION, TRANSFER_ENCODING,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use tokio::io::AsyncWriteExt;

use crate::auth::{AuthUser, verify_token};
use crate::error::error_text;
use crate::state::{AppState, now_ms};
use crate::{files, media, params};

const TUS_VERSION: &str = "1.0.0";
const EXTENSIONS: &str =
    "creation,creation-with-upload,creation-defer-length,termination,expiration";
const OFFSET_STREAM: &str = "application/offset+octet-stream";
/// Сколько загрузок пользователь может начать за минуту.
const CREATE_LIMIT: u32 = 30;
/// Сколько порций пользователь может прислать за минуту.
const PATCH_LIMIT: u32 = 1000;
const WINDOW: Duration = Duration::from_mins(1);

const NOT_FOUND: &str = "The file for this url was not found\n";
const LENGTH_REQUIRED: &str = "Upload-Length or Upload-Defer-Length header required\n";

/// Одна загрузка обрабатывается одним запросом за раз.
static LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Mutex::default);

fn lock_for(id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = LOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    locks.entry(id.to_owned()).or_default().clone()
}

/// Отказ: статус и текст, как у `@tus/server`.
type Refusal = (StatusCode, String);

fn refuse(status: StatusCode, text: &str) -> Refusal {
    (status, text.to_owned())
}

fn tus_name(name: &'static str) -> HeaderName {
    HeaderName::from_static(name)
}

fn respond(status: StatusCode, headers: &[(&'static str, String)], body: String) -> Response {
    let mut response = (status, body).into_response();
    let map = response.headers_mut();
    if status == StatusCode::NO_CONTENT {
        map.remove(CONTENT_TYPE);
    }
    map.insert(
        tus_name("tus-resumable"),
        HeaderValue::from_static(TUS_VERSION),
    );
    for (name, value) in headers {
        if let Ok(value) = HeaderValue::from_str(value) {
            map.insert(tus_name(name), value);
        }
    }
    response
}

/// Пары `Upload-Metadata` в порядке заголовка: ключ и значение в base64.
type Metadata = Vec<(String, Option<String>)>;

/// Разбор по правилам tus; `None` — заголовок некорректен.
fn parse_metadata(raw: &str) -> Option<Metadata> {
    let mut out: Metadata = Vec::new();
    for pair in raw.split(',') {
        let mut parts = pair.trim().split(' ');
        let key = parts.next().filter(|key| !key.is_empty())?;
        let value = parts.next();
        if parts.next().is_some()
            || !key.bytes().all(|b| b.is_ascii_graphic())
            || out.iter().any(|(k, _)| k == key)
        {
            return None;
        }
        let value = match value {
            None => None,
            Some(text) => Some(String::from_utf8_lossy(&STANDARD.decode(text).ok()?).into_owned()),
        };
        out.push((key.to_owned(), value));
    }
    Some(out)
}

fn serialize_metadata(metadata: &Metadata) -> String {
    metadata
        .iter()
        .map(|(key, value)| match value {
            Some(value) => format!("{key} {}", STANDARD.encode(value)),
            None => key.clone(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Значение поля, как его видит `parseUploadMetadata` оригинала: только
/// корректный UTF-8 (иначе поле считается отсутствующим).
fn strict_field(raw: &str, name: &str) -> Option<String> {
    raw.split(',').find_map(|pair| {
        let mut parts = pair.trim().split(' ');
        let (key, value) = (parts.next()?, parts.next()?);
        (key == name)
            .then(|| {
                STANDARD
                    .decode(value)
                    .ok()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
            })
            .flatten()
    })
}

/// Расширение имени в нижнем регистре (`extractExtension`).
pub(crate) fn extension(original: &str) -> String {
    let base = original
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default();
    let ext = match base.rfind('.') {
        Some(0) | None => "",
        Some(dot) => &base[dot..],
    };
    // Расширение попадает в имя файла на диске: управляющие символы и
    // обратная косая черта там не нужны ни одной системе.
    if ext.chars().any(|c| c.is_control() || c == '\\') {
        String::new()
    } else {
        ext.to_lowercase()
    }
}

/// Тип содержимого по расширению (`EXT_MIMETYPES`).
#[must_use]
pub fn mimetype_for(name: &str) -> Option<&'static str> {
    Some(match extension(name).as_str() {
        ".webp" => "image/webp",
        ".jpg" | ".jpeg" => "image/jpeg",
        ".png" => "image/png",
        ".gif" => "image/gif",
        ".svg" => "image/svg+xml",
        ".avif" => "image/avif",
        ".tiff" => "image/tiff",
        ".bmp" => "image/bmp",
        ".ico" => "image/x-icon",
        ".mp4" => "video/mp4",
        ".webm" => "video/webm",
        ".mov" => "video/quicktime",
        ".mp3" => "audio/mpeg",
        ".ogg" => "audio/ogg",
        ".wav" => "audio/wav",
        ".flac" => "audio/flac",
        ".aac" => "audio/aac",
        ".opus" => "audio/opus",
        ".pdf" => "application/pdf",
        _ => return None,
    })
}

/// Незавершённая загрузка.
#[derive(Debug, Clone)]
struct Upload {
    id: String,
    size: Option<u64>,
    metadata: Metadata,
    created_ms: i64,
}

impl Upload {
    fn field(&self, name: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(key, _)| key == name)
            .and_then(|(_, value)| value.as_deref())
    }
}

fn staging_dir(state: &AppState) -> PathBuf {
    state.config.upload_dir.join(".tus")
}

/// Срок жизни незавершённой загрузки (`TUS_EXPIRATION_HOURS`).
fn expiration_ms() -> i64 {
    static HOURS: LazyLock<i64> = LazyLock::new(|| {
        std::env::var("TUS_EXPIRATION_HOURS")
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .filter(|h| *h > 0)
            .unwrap_or(24)
    });
    *HOURS * 3_600_000
}

fn expires_header(upload: &Upload) -> String {
    let at = u64::try_from(upload.created_ms + expiration_ms()).unwrap_or_default();
    httpdate::fmt_http_date(std::time::UNIX_EPOCH + Duration::from_millis(at))
}

/// Дата вида `2026-11-11T21:07:36.779Z`.
fn iso_date(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let rest = ms.rem_euclid(86_400_000);
    // Алгоритм обратного перевода дней в дату (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    )
}

async fn save_description(dir: &Path, upload: &Upload) -> std::io::Result<()> {
    let mut metadata = Map::new();
    for (key, value) in &upload.metadata {
        metadata.insert(
            key.clone(),
            value.clone().map_or(Value::Null, Value::String),
        );
    }
    let mut body = json!({
        "id": upload.id,
        "offset": 0,
        "metadata": metadata,
        "metadata_order": upload.metadata.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>(),
        "creation_date": iso_date(upload.created_ms),
        "created_ms": upload.created_ms,
    });
    if let Some(size) = upload.size {
        body["size"] = size.into();
    }
    tokio::fs::write(dir.join(format!("{}.json", upload.id)), body.to_string()).await
}

async fn load(dir: &Path, id: &str) -> Option<(Upload, u64)> {
    let text = tokio::fs::read_to_string(dir.join(format!("{id}.json")))
        .await
        .ok()?;
    let offset = tokio::fs::metadata(dir.join(id)).await.ok()?.len();
    let body: Value = serde_json::from_str(&text).ok()?;
    let fields = body["metadata"].as_object().cloned().unwrap_or_default();
    let mut order: Vec<String> = body["metadata_order"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|key| key.as_str().map(str::to_owned))
        .collect();
    for key in fields.keys() {
        if !order.contains(key) {
            order.push(key.clone());
        }
    }
    let metadata = order
        .into_iter()
        .filter_map(|key| {
            fields
                .get(&key)
                .map(|value| (key.clone(), value.as_str().map(str::to_owned)))
        })
        .collect();
    // Описание, оставленное оригинальным сервером, несёт только дату строкой.
    let created_ms = body["created_ms"]
        .as_i64()
        .or_else(|| {
            body["creation_date"]
                .as_str()
                .and_then(crate::routes::search::parse_date)
        })
        .unwrap_or_else(now_ms);
    Some((
        Upload {
            id: id.to_owned(),
            size: body["size"].as_u64(),
            metadata,
            created_ms,
        },
        offset,
    ))
}

async fn discard(dir: &Path, id: &str) {
    let _ = tokio::fs::remove_file(dir.join(id)).await;
    let _ = tokio::fs::remove_file(dir.join(format!("{id}.json"))).await;
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn non_negative(text: &str) -> Option<u64> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// Проверка заголовков протокола (`validateHeader`).
fn validate(method: &Method, headers: &HeaderMap) -> Result<(), Refusal> {
    let invalid = |name: &str| refuse(StatusCode::BAD_REQUEST, &format!("Invalid {name}\n"));
    for name in ["upload-offset", "upload-length"] {
        if headers.contains_key(name) && header(headers, name).and_then(non_negative).is_none() {
            return Err(invalid(name));
        }
    }
    if headers.contains_key("upload-defer-length")
        && header(headers, "upload-defer-length") != Some("1")
    {
        return Err(invalid("upload-defer-length"));
    }
    if headers.contains_key("upload-metadata")
        && header(headers, "upload-metadata")
            .and_then(parse_metadata)
            .is_none()
    {
        return Err(invalid("upload-metadata"));
    }
    if headers.contains_key("tus-resumable")
        && header(headers, "tus-resumable") != Some(TUS_VERSION)
    {
        return Err(invalid("tus-resumable"));
    }
    if method == Method::PATCH
        && headers.contains_key(CONTENT_TYPE)
        && header(headers, "content-type") != Some(OFFSET_STREAM)
    {
        return Err(invalid("content-type"));
    }
    Ok(())
}

async fn authorize(state: &AppState, headers: &HeaderMap) -> Result<AuthUser, Refusal> {
    let Some(token) =
        header(headers, AUTHORIZATION.as_str()).and_then(|value| value.strip_prefix("Bearer "))
    else {
        return Err(refuse(
            StatusCode::UNAUTHORIZED,
            "Missing or invalid authorization header",
        ));
    };
    let (token, inner) = (token.to_owned(), state.clone());
    state
        .db
        .transaction(move |tx| verify_token(&inner, tx, &token))
        .await
        .map_err(|error| {
            let text = error
                .custom_text()
                .map_or_else(|| error_text(error.code(), None), str::to_owned);
            (error.status(), text)
        })
}

async fn max_size(state: &AppState) -> u64 {
    let configured = state.config.max_upload_size;
    state
        .db
        .transaction(move |tx| {
            let row = tx.query_one(
                "SELECT max_upload_size_bytes FROM instance_settings WHERE id = 1",
                params![],
            )?;
            Ok::<_, crate::error::ApiError>(
                row.map(|row| row.opt_int("max_upload_size_bytes"))
                    .transpose()?
                    .flatten(),
            )
        })
        .await
        .ok()
        .flatten()
        .and_then(|bytes| u64::try_from(bytes).ok())
        .unwrap_or(configured)
}

/// Дописывает тело запроса в файл, не выходя за `limit` байт сверх `offset`.
/// При превышении файл возвращается к прежней длине.
async fn append(path: &Path, offset: u64, limit: u64, body: Body) -> Result<u64, Refusal> {
    let failed = || {
        refuse(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong receiving the file\n",
        )
    };
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .await
        .map_err(|_| failed())?;
    let mut written = 0u64;
    let mut stream = body.into_data_stream();
    let mut overflow = false;
    while let Some(chunk) = stream.next().await {
        // Обрыв соединения: принятое остаётся, клиент продолжит с нового смещения.
        let Ok(chunk) = chunk else { break };
        if written + chunk.len() as u64 > limit {
            overflow = true;
            break;
        }
        file.write_all(&chunk).await.map_err(|_| failed())?;
        written += chunk.len() as u64;
    }
    file.flush().await.map_err(|_| failed())?;
    if overflow {
        file.set_len(offset).await.map_err(|_| failed())?;
        return Err(refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            "upload's size exceeded\n",
        ));
    }
    Ok(offset + written)
}

/// Завершение (`onUploadFinish`): обработка, перенос в каталог загрузок,
/// запись о вложении. Ответ — вложение в JSON.
async fn finish(
    state: &AppState,
    user: &AuthUser,
    upload: &Upload,
    size: u64,
) -> Result<String, Refusal> {
    let failed = || refuse(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error");
    let (Some(snowflake), Some(owner)) = (upload.field("snowflakeId"), upload.field("userId"))
    else {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "Forbidden: ownership mismatch",
        ));
    };
    if owner != user.id || !snowflake.bytes().all(|b| b.is_ascii_digit()) || snowflake.is_empty() {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "Forbidden: ownership mismatch",
        ));
    }
    let original = upload.field("originalName").unwrap_or("upload").to_owned();
    let filename = format!("{snowflake}{}", extension(&original));
    let source = staging_dir(state).join(&upload.id);
    let Some(target) = files::upload_path(&state.config, &filename) else {
        return Err(failed());
    };
    let mimetype = mimetype_for(&original).unwrap_or("application/octet-stream");
    let thumb_name = files::thumb_filename(&filename);
    let done = media::process(&source, mimetype, &state.config.upload_dir, &thumb_name).await;
    let drop_thumb = async || {
        if done.thumbnail.is_some() {
            let _ = tokio::fs::remove_file(state.config.upload_dir.join(&thumb_name)).await;
        }
    };
    if tokio::fs::rename(&source, &target).await.is_err() {
        drop_thumb().await;
        return Err(failed());
    }
    let _ = tokio::fs::remove_file(staging_dir(state).join(format!("{}.json", upload.id))).await;
    let now = now_ms();
    let row = (
        snowflake.to_owned(),
        owner.to_owned(),
        filename.clone(),
        original.clone(),
        done.clone(),
    );
    let size_int = i64::try_from(size).unwrap_or(i64::MAX);
    let inserted = state
        .db
        .transaction(move |tx| {
            tx.execute(
                "INSERT INTO attachments (id, uploader_id, message_id, dm_message_id, filename, original_name, mimetype, \
                 size, thumbnail_filename, width, height, duration, playable, created_at) \
                 VALUES (?, ?, NULL, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    &row.0,
                    &row.1,
                    &row.2,
                    &row.3,
                    mimetype,
                    size_int,
                    &row.4.thumbnail,
                    row.4.width,
                    row.4.height,
                    row.4.duration,
                    row.4.playable.map(i64::from),
                    now
                ],
            )?;
            Ok::<_, crate::error::ApiError>(())
        })
        .await;
    if inserted.is_err() {
        let _ = tokio::fs::remove_file(&target).await;
        drop_thumb().await;
        return Err(failed());
    }
    let mut body = json!({
        "id": snowflake,
        "messageId": "",
        "filename": filename,
        "originalName": original,
        "mimetype": mimetype,
        "size": size,
        "playable": done.playable,
        "createdAt": now,
    });
    for (name, value) in [
        ("thumbnailFilename", done.thumbnail.map(Value::from)),
        ("width", done.width.map(Value::from)),
        ("height", done.height.map(Value::from)),
        ("duration", done.duration.map(Value::from)),
    ] {
        if let Some(value) = value {
            body[name] = value;
        }
    }
    Ok(body.to_string())
}

fn finished(body: String, offset: u64) -> Response {
    let mut response = respond(
        StatusCode::OK,
        &[("upload-offset", offset.to_string())],
        body,
    );
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

async fn create(
    state: &AppState,
    user: &AuthUser,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, Refusal> {
    let length = header(headers, "upload-length").and_then(non_negative);
    let deferred = headers.contains_key("upload-defer-length");
    if length.is_some() == deferred {
        return Err(refuse(StatusCode::BAD_REQUEST, LENGTH_REQUIRED));
    }
    if !state
        .hub
        .allow(&format!("upload-create:{}", user.id), CREATE_LIMIT, WINDOW)
    {
        return Err(refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "Upload creation rate limit exceeded",
        ));
    }
    let max = max_size(state).await;
    if length.is_some_and(|length| length > max) {
        return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE, "File too large"));
    }
    let raw = header(headers, "upload-metadata").unwrap_or_default();
    let original = strict_field(raw, "originalName").or_else(|| strict_field(raw, "filename"));
    let Some(original) = original.filter(|name| !crate::util::js_trim(name).is_empty()) else {
        return Err(refuse(
            StatusCode::BAD_REQUEST,
            "Upload-Metadata missing required field: filename",
        ));
    };
    let mut metadata = parse_metadata(raw).unwrap_or_default();
    for (key, value) in [
        ("snowflakeId", state.ids.next()),
        ("userId", user.id.clone()),
        ("originalName", original),
    ] {
        match metadata.iter_mut().find(|(name, _)| name == key) {
            Some(entry) => entry.1 = Some(value),
            None => metadata.push((key.to_owned(), Some(value))),
        }
    }
    let upload = Upload {
        id: hex::encode(rand::random::<[u8; 16]>()),
        size: length,
        metadata,
        created_ms: now_ms(),
    };
    let dir = staging_dir(state);
    let failed = || refuse(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|_| failed())?;
    tokio::fs::write(dir.join(&upload.id), b"")
        .await
        .map_err(|_| failed())?;
    save_description(&dir, &upload)
        .await
        .map_err(|_| failed())?;

    let mut offset = 0;
    let with_data = header(headers, "content-type") == Some(OFFSET_STREAM);
    if with_data {
        offset = append(&dir.join(&upload.id), 0, length.unwrap_or(max), body).await?;
    }
    // Оригинал в этом случае запускает завершение и затем отвечает 404, теряя
    // уже созданное вложение; здесь загрузка в один запрос просто завершается.
    if length == Some(offset) {
        return Ok(finished(
            finish(state, user, &upload, offset).await?,
            offset,
        ));
    }
    let mut out = vec![
        ("location", format!("/api/files/{}", upload.id)),
        ("upload-expires", expires_header(&upload)),
    ];
    if with_data {
        out.push(("upload-offset", offset.to_string()));
    }
    Ok(respond(StatusCode::CREATED, &out, String::new()))
}

async fn patch(
    state: &AppState,
    user: &AuthUser,
    mut upload: Upload,
    offset: u64,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, Refusal> {
    let Some(claimed) = header(headers, "upload-offset").and_then(non_negative) else {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "Upload-Offset header required\n",
        ));
    };
    if !headers.contains_key(CONTENT_TYPE) {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "Content-Type header required\n",
        ));
    }
    if claimed != offset {
        return Err(refuse(StatusCode::CONFLICT, "Upload-Offset conflict\n"));
    }
    let dir = staging_dir(state);
    let max = max_size(state).await;
    if let Some(declared) = header(headers, "upload-length").and_then(non_negative) {
        if upload.size.is_some() || declared < offset {
            return Err(refuse(StatusCode::BAD_REQUEST, LENGTH_REQUIRED));
        }
        // У оригинала отложенная длина обходит предел размера: он проверяется
        // только при создании.
        if declared > max {
            return Err(refuse(StatusCode::PAYLOAD_TOO_LARGE, "File too large"));
        }
        upload.size = Some(declared);
        save_description(&dir, &upload)
            .await
            .map_err(|_| refuse(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error"))?;
    }
    let limit = upload.size.unwrap_or(max).saturating_sub(offset);
    let new_offset = append(&dir.join(&upload.id), offset, limit, body).await?;
    if upload.size == Some(new_offset) {
        return Ok(finished(
            finish(state, user, &upload, new_offset).await?,
            new_offset,
        ));
    }
    Ok(respond(
        StatusCode::NO_CONTENT,
        &[
            ("upload-offset", new_offset.to_string()),
            ("upload-expires", expires_header(&upload)),
        ],
        String::new(),
    ))
}

fn upload_id(path: &str) -> Option<&str> {
    let id = path.strip_prefix("/api/files/")?.trim_end_matches('/');
    (!id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then_some(id)
}

async fn serve(state: AppState, request: Request) -> Result<Response, Refusal> {
    let (parts, body) = request.into_parts();
    let (method, headers, path) = (parts.method, parts.headers, parts.uri.path().to_owned());
    if method == Method::OPTIONS {
        return Ok(respond(
            StatusCode::NO_CONTENT,
            &[
                ("tus-version", TUS_VERSION.to_owned()),
                ("tus-extension", EXTENSIONS.to_owned()),
            ],
            String::new(),
        ));
    }
    if method != Method::GET && !headers.contains_key("tus-resumable") {
        return Err(refuse(
            StatusCode::PRECONDITION_FAILED,
            "Tus-Resumable Required\n",
        ));
    }
    validate(&method, &headers)?;
    let user = authorize(&state, &headers).await?;
    if method == Method::POST {
        return create(&state, &user, &headers, body).await;
    }
    if !matches!(
        method,
        Method::PATCH | Method::HEAD | Method::DELETE | Method::GET
    ) {
        return Err(refuse(StatusCode::NOT_FOUND, "Not found\n"));
    }
    let id = upload_id(&path).ok_or_else(|| refuse(StatusCode::NOT_FOUND, NOT_FOUND))?;
    if method == Method::PATCH
        && !state
            .hub
            .allow(&format!("upload-patch:{}", user.id), PATCH_LIMIT, WINDOW)
    {
        return Err(refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "PATCH rate limit exceeded",
        ));
    }
    let lock = lock_for(id);
    let _guard = lock.lock().await;
    let dir = staging_dir(&state);
    let (upload, offset) = load(&dir, id)
        .await
        .ok_or_else(|| refuse(StatusCode::NOT_FOUND, NOT_FOUND))?;
    if upload.field("userId") != Some(user.id.as_str()) {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "Forbidden: you do not own this upload",
        ));
    }
    if upload.created_ms + expiration_ms() < now_ms() {
        discard(&dir, id).await;
        return Err(refuse(
            StatusCode::GONE,
            "The file for this url no longer exists\n",
        ));
    }
    match method {
        Method::PATCH => patch(&state, &user, upload, offset, &headers, body).await,
        Method::HEAD => {
            let mut out = vec![("upload-offset", offset.to_string())];
            match upload.size {
                Some(size) => out.push(("upload-length", size.to_string())),
                None => out.push(("upload-defer-length", "1".to_owned())),
            }
            if !upload.metadata.is_empty() {
                out.push(("upload-metadata", serialize_metadata(&upload.metadata)));
            }
            let mut response = respond(StatusCode::OK, &out, String::new());
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            Ok(response)
        }
        Method::DELETE => {
            discard(&dir, id).await;
            Ok(respond(StatusCode::NO_CONTENT, &[], String::new()))
        }
        // Незавершённые загрузки наружу не отдаются.
        _ => Err(refuse(StatusCode::NOT_FOUND, NOT_FOUND)),
    }
}

/// Все методы `/api/files` и `/api/files/*`.
pub async fn tus(State(state): State<AppState>, request: Request) -> Response {
    // Fastify отвергает тело без типа раньше, чем запрос дойдёт до tus.
    let has_body = request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v != "0")
        || request.headers().contains_key(TRANSFER_ENCODING);
    if has_body && !request.headers().contains_key(CONTENT_TYPE) && request.method() != Method::GET
    {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            axum::Json(json!({
                "statusCode": 415,
                "code": "FST_ERR_CTP_INVALID_MEDIA_TYPE",
                "error": "Unsupported Media Type",
                "message": "Unsupported Media Type: undefined",
            })),
        )
            .into_response();
    }
    let head = request.method() == Method::HEAD;
    match serve(state, request).await {
        Ok(response) => response,
        Err((status, text)) => {
            let mut response =
                respond(status, &[], if head { String::new() } else { text.clone() });
            if head && let Ok(length) = HeaderValue::from_str(&text.len().to_string()) {
                response.headers_mut().insert(CONTENT_LENGTH, length);
            }
            response.headers_mut().remove(LOCATION);
            response
        }
    }
}

/// Убирает незавершённые загрузки, срок которых истёк.
pub async fn sweep_expired(state: &AppState) {
    let dir = staging_dir(state);
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return;
    };
    let limit = expiration_ms();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        let lock = lock_for(id);
        let _guard = lock.lock().await;
        let expired = match load(&dir, id).await {
            Some((upload, _)) => upload.created_ms + limit < now_ms(),
            None => true,
        };
        if expired {
            discard(&dir, id).await;
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "в тестах сбой подготовки — это провал теста"
)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trip() {
        let parsed =
            parse_metadata("filename 0L/RgNC4INCy0LXRgi5UWFQ=,flag,filetype dGV4dC9wbGFpbg==")
                .expect("разбор");
        assert_eq!(
            parsed[0],
            ("filename".to_owned(), Some("при вет.TXT".to_owned()))
        );
        assert_eq!(parsed[1], ("flag".to_owned(), None));
        assert_eq!(
            serialize_metadata(&parsed),
            "filename 0L/RgNC4INCy0LXRgi5UWFQ=,flag,filetype dGV4dC9wbGFpbg=="
        );
        assert!(parse_metadata("flag,bad !!!").is_none());
        assert!(parse_metadata("a b c").is_none());
        assert!(parse_metadata("a YQ==,a YQ==").is_none());
        assert_eq!(strict_field("filename /w==", "filename"), None);
        assert_eq!(
            strict_field("x YQ==,filename YQ==", "filename").as_deref(),
            Some("a")
        );
    }

    #[test]
    fn extensions_and_types() {
        assert_eq!(extension("../../x.tar.GZ"), ".gz");
        assert_eq!(extension(".bashrc"), "");
        assert_eq!(extension("noext"), "");
        assert_eq!(extension("a.b/c"), "");
        assert_eq!(extension("a.p\\x"), "");
        assert_eq!(mimetype_for("Фото.JPG"), Some("image/jpeg"));
        assert_eq!(mimetype_for("archive.zip"), None);
    }

    #[test]
    fn upload_ids_cannot_leave_the_staging_directory() {
        assert_eq!(upload_id("/api/files/abc123"), Some("abc123"));
        assert_eq!(upload_id("/api/files/../x"), None);
        assert_eq!(upload_id("/api/files/a/b"), None);
        assert_eq!(upload_id("/api/files/a.json"), None);
        assert_eq!(upload_id("/api/files"), None);
    }

    #[test]
    fn dates() {
        assert_eq!(iso_date(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_date(1_794_431_256_779), "2026-11-11T21:07:36.779Z");
    }
}
