//! Раздача загруженных файлов (`routes/uploads.ts`).

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{
    ACCEPT_RANGES, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_SECURITY_POLICY, CONTENT_TYPE, RANGE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::error::ApiError;
use crate::state::AppState;
use crate::{files, params};

/// `encodeURIComponent`.
#[must_use]
pub fn encode_uri_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Диапазон из заголовка `Range` для файла длиной `size`: первый и последний
/// байт. `Err` — диапазон вне файла.
fn parse_range(value: &str, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return Ok(None);
    };
    // Несколько диапазонов разом не отдаются: берётся первый, как у оригинала.
    let spec = spec.split(',').next().unwrap_or_default().trim();
    let Some((first, last)) = spec.split_once('-') else {
        return Ok(None);
    };
    let (first, last) = (first.trim(), last.trim());
    if size == 0 {
        return Err(());
    }
    let (start, end) = if first.is_empty() {
        let suffix: u64 = last.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = first.parse().map_err(|_| ())?;
        let end = if last.is_empty() {
            size - 1
        } else {
            last.parse::<u64>().map_err(|_| ())?.min(size - 1)
        };
        (start, end)
    };
    if start > end || start >= size {
        Err(())
    } else {
        Ok(Some((start, end)))
    }
}

fn file_body(file: tokio::fs::File, length: u64) -> Body {
    let stream = futures_util::stream::unfold((file, length), |(mut file, left)| async move {
        if left == 0 {
            return None;
        }
        let mut chunk = vec![0u8; usize::try_from(left.min(64 * 1024)).unwrap_or(64 * 1024)];
        match file.read(&mut chunk).await {
            Ok(0) => None,
            Ok(read) => {
                chunk.truncate(read);
                Some((
                    Ok::<_, std::io::Error>(axum::body::Bytes::from(chunk)),
                    (file, left - read as u64),
                ))
            }
            Err(error) => Some((Err(error), (file, 0))),
        }
    });
    Body::from_stream(stream)
}

/// `GET /api/uploads/:filename`.
pub async fn get(
    State(state): State<AppState>,
    Path(filename): Path<String>,
    headers: HeaderMap,
) -> Response {
    let missing = || ApiError::not_found("file_not_found").into_response();
    let Some(path) = files::upload_path(&state.config, &filename) else {
        return missing();
    };
    let safe = files::basename(&filename).to_owned();
    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return missing();
    };
    let Ok(meta) = file.metadata().await else {
        return missing();
    };
    if !meta.is_file() {
        return missing();
    }
    let size = meta.len();
    let lookup = safe.clone();
    let known = state
        .db
        .transaction(move |tx| {
            let row = tx.query_one(
                "SELECT original_name, mimetype FROM attachments WHERE filename = ?",
                params![&lookup],
            )?;
            Ok::<_, ApiError>(match row {
                Some(row) => Some((row.text("original_name")?, row.text("mimetype")?)),
                None => None,
            })
        })
        .await
        .ok()
        .flatten();
    let (original, mimetype) = match known {
        Some((original, mimetype)) => (original, mimetype),
        None => (
            safe.clone(),
            crate::routes::files::mimetype_for(&safe)
                .unwrap_or("application/octet-stream")
                .to_owned(),
        ),
    };

    let mut out = HeaderMap::new();
    let fixed = HeaderValue::from_static;
    out.insert(CACHE_CONTROL, fixed("public, max-age=31536000, immutable"));
    out.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&mimetype).unwrap_or_else(|_| fixed("application/octet-stream")),
    );
    out.insert(
        CONTENT_SECURITY_POLICY,
        fixed("default-src 'none'; style-src 'unsafe-inline'; img-src 'self'"),
    );
    out.insert(ACCEPT_RANGES, fixed("bytes"));
    // Всё, что браузер мог бы исполнить или отрисовать как страницу, отдаётся
    // только на скачивание.
    let inline = mimetype != "image/svg+xml"
        && (mimetype.starts_with("image/")
            || mimetype.starts_with("video/")
            || mimetype.starts_with("audio/"));
    if !inline
        && let Ok(value) = HeaderValue::from_str(&format!(
            "attachment; filename=\"{}\"",
            encode_uri_component(&original)
        ))
    {
        out.insert(CONTENT_DISPOSITION, value);
    }

    let range = headers
        .get(RANGE)
        .and_then(|value| value.to_str().ok())
        .map(|value| parse_range(value, size));
    match range {
        Some(Err(())) => {
            out.insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{size}"))
                    .unwrap_or_else(|_| fixed("bytes */0")),
            );
            (StatusCode::RANGE_NOT_SATISFIABLE, out).into_response()
        }
        Some(Ok(Some((start, end)))) => {
            if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
                return missing();
            }
            let length = end - start + 1;
            if let Ok(value) = HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")) {
                out.insert(CONTENT_RANGE, value);
            }
            out.insert(CONTENT_LENGTH, HeaderValue::from(length));
            (StatusCode::PARTIAL_CONTENT, out, file_body(file, length)).into_response()
        }
        _ => {
            out.insert(CONTENT_LENGTH, HeaderValue::from(size));
            (StatusCode::OK, out, file_body(file, size)).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-4", 10), Ok(Some((0, 4))));
        assert_eq!(parse_range("bytes=5-", 10), Ok(Some((5, 9))));
        assert_eq!(parse_range("bytes=-3", 10), Ok(Some((7, 9))));
        assert_eq!(parse_range("bytes=2-999", 10), Ok(Some((2, 9))));
        assert_eq!(parse_range("bytes=10-", 10), Err(()));
        assert_eq!(parse_range("bytes=5-2", 10), Err(()));
        assert_eq!(parse_range("bytes=x-2", 10), Err(()));
        assert_eq!(parse_range("items=0-1", 10), Ok(None));
        assert_eq!(parse_range("bytes=0-0", 0), Err(()));
    }

    #[test]
    fn names_in_headers_are_percent_encoded() {
        assert_eq!(
            encode_uri_component("отчёт (1).pdf"),
            "%D0%BE%D1%82%D1%87%D1%91%D1%82%20(1).pdf"
        );
        assert_eq!(encode_uri_component("a\"b\r\n.txt"), "a%22b%0D%0A.txt");
    }
}
