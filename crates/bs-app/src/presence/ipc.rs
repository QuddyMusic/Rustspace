//! Локальный сервер Rich Presence: порт `richPresenceIpc.ts` из PR #482
//! оригинального Backspace (QuddyMusic, cirzear).
//!
//! Игры с поддержкой Rich Presence говорят по локальному IPC через сокеты
//! `discord-ipc-0` … `discord-ipc-9` (на Windows — именованные каналы). Если
//! отвечать на этом протоколе самим, можно принять их активность (osu!lazer:
//! карта, режим, время, обложка) и показать её как активность Rustspace.
//!
//! Кадры: `[i32 LE opcode][i32 LE длина][JSON]`; коды: 0 HANDSHAKE, 1 FRAME,
//! 2 CLOSE, 3 PING, 4 PONG. Рукопожатие `{v, client_id}`; некоторые
//! библиотеки шлют его голым JSON без заголовка — принимаются оба вида.
//! Команды едут в FRAME: `{cmd, args, nonce}`.
//!
//! Занятые живым сервером сокеты (обычно настоящим клиентом Discord) не
//! трогаются; оставшиеся после падения — забираются.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bs_rt::sync::watch;
use serde_json::{Value, json};
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const OP_HANDSHAKE: i32 = 0;
const OP_FRAME: i32 = 1;
const OP_CLOSE: i32 = 2;
const OP_PING: i32 = 3;
const OP_PONG: i32 = 4;
const SLOTS: u8 = 10;
const MAX_FRAME: usize = 1024 * 1024;
const DISCONNECT_GRACE: Duration = Duration::from_millis(2500);
const MAX_TEXT: usize = 128;
const MAX_URL: usize = 512;

/// Активность из пуша, уже очищенная.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct IpcActivity {
    pub(crate) kind: String,
    pub(crate) name: Option<String>,
    pub(crate) details: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) start: Option<i64>,
    pub(crate) end: Option<i64>,
    pub(crate) large_image: Option<String>,
    pub(crate) large_text: Option<String>,
    pub(crate) small_image: Option<String>,
    pub(crate) small_text: Option<String>,
}

/// Пуш одной игры.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IpcPush {
    pub(crate) client_id: String,
    pub(crate) pid: Option<u32>,
    pub(crate) activity: IpcActivity,
}

// ─── Кадры ──────────────────────────────────────────────────────────────────

/// Кадр протокола.
pub(crate) fn encode_frame(op: i32, payload: &Value) -> Vec<u8> {
    let json = payload.to_string().into_bytes();
    let mut out = Vec::with_capacity(8 + json.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&i32::try_from(json.len()).unwrap_or(i32::MAX).to_le_bytes());
    out.extend_from_slice(&json);
    out
}

/// Разобранное сообщение из начала буфера.
#[derive(Debug, PartialEq)]
pub(crate) enum Decoded {
    /// Кадр: код, JSON (или `Null`), сколько байт занял.
    Frame(i32, Value, usize),
    /// Голый JSON без заголовка.
    Raw(Value, usize),
    /// Ждём ещё байт.
    More,
    /// Мусор: соединение закрыть.
    Bad,
}

/// Голый JSON-объект в начале буфера (по парности скобок: голое
/// рукопожатие, склеенное с кадрами, режется правильно).
fn raw_json(buf: &[u8]) -> Option<Result<(Value, usize), ()>> {
    if buf.first() != Some(&b'{') {
        return None;
    }
    let (mut depth, mut in_string, mut escaped) = (0i32, false, false);
    for (i, c) in buf.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *c == b'\\' {
                escaped = true;
            } else if *c == b'"' {
                in_string = false;
            }
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(
                        serde_json::from_slice(&buf[..=i])
                            .map(|v| (v, i + 1))
                            .map_err(|_| ()),
                    );
                }
            }
            _ => {}
        }
    }
    if buf.len() > MAX_FRAME {
        Some(Err(()))
    } else {
        Some(Ok((Value::Null, 0)))
    }
}

/// Одно сообщение из начала буфера.
pub(crate) fn decode(buf: &[u8]) -> Decoded {
    if buf.is_empty() {
        return Decoded::More;
    }
    match raw_json(buf) {
        Some(Ok((_, 0))) => return Decoded::More,
        Some(Ok((value, used))) => return Decoded::Raw(value, used),
        Some(Err(())) => return Decoded::Bad,
        None => {}
    }
    if buf.len() < 8 {
        return Decoded::More;
    }
    let op = i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let len = i32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    let Ok(len) = usize::try_from(len) else {
        return Decoded::Bad;
    };
    if !(OP_HANDSHAKE..=OP_PONG).contains(&op) || len > MAX_FRAME {
        return Decoded::Bad;
    }
    if buf.len() < 8 + len {
        return Decoded::More;
    }
    let payload = serde_json::from_slice(&buf[8..8 + len]).unwrap_or(Value::Null);
    Decoded::Frame(op, payload, 8 + len)
}

// ─── Активность ─────────────────────────────────────────────────────────────

fn clamp(value: Option<&Value>, max: usize) -> Option<String> {
    let text: String = value?.as_str()?.chars().take(max).collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// Ссылка на картинку: только http(s). Ключи ассетов Discord разрешаются
/// лишь через его CDN, куда независимому инстансу ходить незачем; иконки
/// игр берутся с машины игрока и раздаются своим инстансом.
pub(crate) fn asset_url(value: Option<&Value>) -> Option<String> {
    let url = value?.as_str()?.trim();
    let lower = url.to_lowercase();
    ((lower.starts_with("https://") || lower.starts_with("http://")) && url.len() <= MAX_URL)
        .then(|| url.to_owned())
}

/// Секунды Discord RPC → миллисекунды (меньше 1e12 — это секунды).
fn epoch_ms(value: Option<&Value>) -> Option<i64> {
    let n = value?.as_f64().filter(|n| n.is_finite())?;
    #[allow(clippy::cast_possible_truncation, reason = "метки времени")]
    let ms = if n > 0.0 && n < 1e12 { n * 1000.0 } else { n } as i64;
    Some(ms)
}

/// Пуш `SET_ACTIVITY` → своя очищенная форма. Кнопки, party и secrets не
/// читаются и никуда не передаются.
pub(crate) fn normalize(raw: &Value) -> IpcActivity {
    let assets = raw.get("assets").cloned().unwrap_or(Value::Null);
    let pick = |a: &str, b: &str| assets.get(a).or_else(|| assets.get(b));
    let kind = match raw.get("type") {
        Some(Value::String(s))
            if matches!(
                s.as_str(),
                "playing" | "listening" | "watching" | "streaming"
            ) =>
        {
            s.clone()
        }
        Some(Value::Number(n)) => match n.as_u64() {
            Some(1) => "streaming",
            Some(2) => "listening",
            Some(3) => "watching",
            _ => "playing",
        }
        .to_owned(),
        _ => "playing".to_owned(),
    };
    let stamps = raw.get("timestamps");
    IpcActivity {
        kind,
        name: clamp(raw.get("name"), MAX_TEXT),
        details: clamp(raw.get("details"), MAX_TEXT),
        state: clamp(raw.get("state"), MAX_TEXT),
        start: epoch_ms(stamps.and_then(|t| t.get("start"))),
        end: epoch_ms(stamps.and_then(|t| t.get("end"))),
        large_image: asset_url(pick("large_image", "largeImage")),
        large_text: clamp(pick("large_text", "largeText"), MAX_TEXT),
        small_image: asset_url(pick("small_image", "smallImage")),
        small_text: clamp(pick("small_text", "smallText"), MAX_TEXT),
    }
}

fn valid_client_id(id: Option<&Value>) -> Option<String> {
    let id = id?.as_str()?;
    (!id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_digit()))
        .then(|| id.to_owned())
}

// ─── Состояние сервера ──────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct Shared {
    next_conn: u64,
    seq: u64,
    /// Соединение → (номер пуша, пуш).
    pushes: std::collections::HashMap<u64, (u64, IpcPush)>,
}

/// Работающий сервер; при сбросе останавливается и убирает сокеты.
pub(crate) struct Server {
    tasks: Vec<bs_rt::JoinHandle<()>>,
    #[cfg(unix)]
    paths: Vec<std::path::PathBuf>,
    /// Сколько сокетов (каналов) слушаем.
    pub(crate) sockets: usize,
}

impl Drop for Server {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        #[cfg(unix)]
        for path in &self.paths {
            let _ = crate::files::remove_file(path);
        }
    }
}

/// Побеждает последний пуш среди живых соединений (и тех, что в отсрочке
/// после разрыва): новая игра перекрывает старую.
fn recompute(shared: &Mutex<Shared>, out: &watch::Sender<Option<IpcPush>>) {
    let best = shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pushes
        .values()
        .max_by_key(|(seq, _)| *seq)
        .map(|(_, push)| push.clone());
    out.send_if_modified(|current| {
        if *current == best {
            false
        } else {
            current.clone_from(&best);
            true
        }
    });
}

fn ready_payload() -> Value {
    let config = json!({"cdn_host": "cdn.discordapp.com", "api_endpoint": "//discord.com/api", "environment": "production"});
    json!({
        "cmd": "DISPATCH",
        "evt": "READY",
        "data": {
            "v": 1,
            "cfg": config,
            "config": config,
            "user": {"id": "123456789012345678", "username": "rustspace", "discriminator": "0", "avatar": null, "bot": false, "flags": 0}
        }
    })
}

/// Соединение одной игры: её состояние разбора.
#[cfg(not(target_arch = "wasm32"))]
struct Conn<'a> {
    id: u64,
    client_id: Option<String>,
    buf: Vec<u8>,
    shared: &'a Mutex<Shared>,
    out: &'a watch::Sender<Option<IpcPush>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Conn<'_> {
    /// Разобрать накопленное: кадры для ответа и `true`, если пора закрыть.
    fn feed(&mut self, data: &[u8]) -> (Vec<Vec<u8>>, bool) {
        self.buf.extend_from_slice(data);
        let mut replies = Vec::new();
        let close = |replies: &mut Vec<Vec<u8>>, code: i32, message: &str| {
            replies.push(encode_frame(
                OP_CLOSE,
                &json!({"code": code, "message": message}),
            ));
            true
        };
        loop {
            let (op, payload, used) = match decode(&self.buf) {
                Decoded::More => return (replies, false),
                Decoded::Bad => {
                    let done = close(&mut replies, 4000, "malformed payload");
                    return (replies, done);
                }
                Decoded::Raw(value, used) => (
                    if self.client_id.is_none() {
                        OP_HANDSHAKE
                    } else {
                        OP_FRAME
                    },
                    value,
                    used,
                ),
                Decoded::Frame(op, value, used) => (op, value, used),
            };
            self.buf.drain(..used);
            match op {
                OP_HANDSHAKE => {
                    let Some(id) = valid_client_id(payload.get("client_id")) else {
                        let done = close(&mut replies, 4004, "invalid client id");
                        return (replies, done);
                    };
                    tracing::debug!(client = %id, "Rich Presence: рукопожатие");
                    self.client_id = Some(id);
                    replies.push(encode_frame(OP_FRAME, &ready_payload()));
                }
                _ if self.client_id.is_none() => {
                    let done = close(&mut replies, 4001, "expected handshake");
                    return (replies, done);
                }
                OP_FRAME => {
                    let client = self.client_id.clone().unwrap_or_default();
                    if let Some(answer) = command(&payload, self.id, &client, self.shared, self.out)
                    {
                        replies.push(encode_frame(OP_FRAME, &answer));
                    }
                }
                OP_PING => {
                    let payload = if payload.is_null() {
                        json!({})
                    } else {
                        payload
                    };
                    replies.push(encode_frame(OP_PONG, &payload));
                }
                _ => return (replies, true),
            }
        }
    }
}

/// Одно соединение игры.
#[cfg(not(target_arch = "wasm32"))]
async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    shared: Arc<Mutex<Shared>>,
    out: watch::Sender<Option<IpcPush>>,
) {
    let id = {
        let mut guard = shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.next_conn += 1;
        guard.next_conn
    };
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut conn = Conn {
        id,
        client_id: None,
        buf: Vec::new(),
        shared: &shared,
        out: &out,
    };
    let mut chunk = [0u8; 8192];
    while let Ok(read) = reader.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        let (replies, close) = conn.feed(&chunk[..read]);
        let mut failed = false;
        for frame in replies {
            if writer.write_all(&frame).await.is_err() {
                failed = true;
                break;
            }
        }
        if close || failed {
            break;
        }
    }
    // Отсрочка: переподключающаяся игра не мигает активностью.
    bs_rt::time::sleep(DISCONNECT_GRACE).await;
    shared
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pushes
        .remove(&id);
    recompute(&shared, &out);
}

/// Команда из кадра; ответ, если нужен.
fn command(
    payload: &Value,
    conn: u64,
    client_id: &str,
    shared: &Mutex<Shared>,
    out: &watch::Sender<Option<IpcPush>>,
) -> Option<Value> {
    let cmd = payload
        .get("cmd")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let nonce = payload
        .get("nonce")
        .filter(|n| n.is_string() || n.is_number())
        .cloned()
        .unwrap_or(Value::Null);
    let args = payload.get("args").cloned().unwrap_or(Value::Null);
    match cmd {
        "SET_ACTIVITY" => {
            let pid = args
                .get("pid")
                .and_then(Value::as_u64)
                .and_then(|p| u32::try_from(p).ok());
            let activity = args.get("activity").filter(|a| a.is_object());
            {
                let mut guard = shared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(activity) = activity {
                    guard.seq += 1;
                    let seq = guard.seq;
                    let push = IpcPush {
                        client_id: client_id.to_owned(),
                        pid,
                        activity: normalize(activity),
                    };
                    guard.pushes.insert(conn, (seq, push));
                } else {
                    guard.pushes.remove(&conn);
                }
            }
            recompute(shared, out);
            Some(
                json!({"cmd": "SET_ACTIVITY", "data": activity.cloned().unwrap_or(Value::Null), "evt": null, "nonce": nonce}),
            )
        }
        "SUBSCRIBE" | "UNSUBSCRIBE" => Some(json!({
            "cmd": cmd,
            "data": {"evt": args.get("evt").and_then(Value::as_str)},
            "evt": null,
            "nonce": nonce
        })),
        "DISPATCH" => None,
        _ => Some(json!({
            "cmd": if cmd.is_empty() { "UNKNOWN" } else { cmd },
            "data": {"code": -1, "message": "unsupported command"},
            "evt": "ERROR",
            "nonce": nonce
        })),
    }
}

// ─── Запуск ─────────────────────────────────────────────────────────────────

/// Каталоги, где игры ищут сокеты: XDG_RUNTIME_DIR, затем TMPDIR/TMP/TEMP//tmp.
#[cfg(unix)]
fn socket_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        dirs.push(std::path::PathBuf::from(xdg));
    }
    let tmp = ["TMPDIR", "TMP", "TEMP"]
        .iter()
        .find_map(std::env::var_os)
        .map_or_else(
            || std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from,
        );
    if !dirs.contains(&tmp) {
        dirs.push(tmp);
    }
    dirs
}

/// Запустить сервер на всех свободных местах. `None` — свободных нет.
#[cfg(unix)]
pub(crate) async fn start(out: watch::Sender<Option<IpcPush>>) -> Option<Server> {
    use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
    let shared = Arc::new(Mutex::new(Shared::default()));
    let mut server = Server {
        tasks: Vec::new(),
        paths: Vec::new(),
        sockets: 0,
    };
    for dir in socket_dirs() {
        for slot in 0..SLOTS {
            let path = dir.join(format!("discord-ipc-{slot}"));
            if let Ok(meta) = std::fs::symlink_metadata(&path) {
                if !meta.file_type().is_socket() {
                    continue; // Чужой файл не удаляем никогда.
                }
                let probe = bs_rt::time::timeout(
                    Duration::from_millis(400),
                    tokio::net::UnixStream::connect(&path),
                )
                .await;
                match probe {
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                        if crate::files::remove_file(&path).is_err() {
                            continue;
                        }
                    }
                    _ => continue, // Живой сервер (или медленный) — не наш.
                }
            }
            let Ok(listener) = tokio::net::UnixListener::bind(&path) else {
                continue;
            };
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            let (shared, out) = (shared.clone(), out.clone());
            server.tasks.push(bs_rt::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    bs_rt::spawn(serve(stream, shared.clone(), out.clone()));
                }
            }));
            server.paths.push(path);
            server.sockets += 1;
        }
    }
    (server.sockets > 0).then_some(server)
}

/// Запустить сервер на всех свободных именованных каналах.
#[cfg(windows)]
#[allow(
    clippy::unused_async,
    reason = "та же подпись, что у версии для Unix, где ожидание есть"
)]
pub(crate) async fn start(out: watch::Sender<Option<IpcPush>>) -> Option<Server> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let shared = Arc::new(Mutex::new(Shared::default()));
    let mut server = Server {
        tasks: Vec::new(),
        sockets: 0,
    };
    for slot in 0..SLOTS {
        let name = format!(r"\\.\pipe\discord-ipc-{slot}");
        // Первый экземпляр не создаётся, если канал уже держит другой сервер.
        let Ok(mut pipe) = ServerOptions::new().first_pipe_instance(true).create(&name) else {
            continue;
        };
        let (shared, out) = (shared.clone(), out.clone());
        server.tasks.push(bs_rt::spawn(async move {
            loop {
                if pipe.connect().await.is_err() {
                    break;
                }
                let Ok(next) = ServerOptions::new().create(&name) else {
                    break;
                };
                let connected = std::mem::replace(&mut pipe, next);
                bs_rt::spawn(serve(connected, shared.clone(), out.clone()));
            }
        }));
        server.sockets += 1;
    }
    (server.sockets > 0).then_some(server)
}

/// В браузере программы не достучатся до страницы: IPC нет.
#[cfg(target_arch = "wasm32")]
#[allow(clippy::unused_async, reason = "как у настоящего сервера")]
pub(crate) async fn start(_out: watch::Sender<Option<IpcPush>>) -> Option<Server> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_and_raw_handshakes() {
        let frame = encode_frame(OP_FRAME, &json!({"cmd": "X"}));
        assert_eq!(
            decode(&frame),
            Decoded::Frame(OP_FRAME, json!({"cmd": "X"}), frame.len())
        );
        assert_eq!(decode(&frame[..5]), Decoded::More);
        let mut glued = br#"{"v":1,"client_id":"42"}"#.to_vec();
        let raw_len = glued.len();
        glued.extend_from_slice(&frame);
        assert_eq!(
            decode(&glued),
            Decoded::Raw(json!({"v": 1, "client_id": "42"}), raw_len)
        );
        assert_eq!(decode(br#"{"v":1,"#), Decoded::More);
        assert_eq!(decode(&[9, 0, 0, 0, 0, 0, 0, 0]), Decoded::Bad);
    }

    #[test]
    fn activities_are_cleaned() {
        let activity = normalize(&json!({
            "type": 2,
            "details": "  Map [Hard]  ",
            "timestamps": {"start": 1_700_000_000},
            "assets": {"large_image": "mp:key", "small_image": "https://x/y.png", "large_text": "a"},
            "buttons": [{"label": "x", "url": "https://evil"}]
        }));
        assert_eq!(activity.kind, "listening");
        assert_eq!(activity.details.as_deref(), Some("Map [Hard]"));
        assert_eq!(activity.start, Some(1_700_000_000_000));
        assert_eq!(activity.large_image, None);
        assert_eq!(activity.small_image.as_deref(), Some("https://x/y.png"));
        assert_eq!(valid_client_id(Some(&json!("12ab"))), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_game_pushes_activity_over_the_socket() {
        let (out, mut seen) = watch::channel(None);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (client, server) = tokio::io::duplex(4096);
        bs_rt::spawn(serve(server, shared, out));
        let (mut read, mut write) = tokio::io::split(client);
        write
            .write_all(&encode_frame(
                OP_HANDSHAKE,
                &json!({"v": 1, "client_id": "1216669957799018608"}),
            ))
            .await
            .unwrap_or_default();
        let mut head = [0u8; 8];
        read.read_exact(&mut head).await.unwrap_or_default();
        assert_eq!(
            i32::from_le_bytes([head[0], head[1], head[2], head[3]]),
            OP_FRAME
        );
        let len =
            usize::try_from(i32::from_le_bytes([head[4], head[5], head[6], head[7]])).unwrap_or(0);
        let mut body = vec![0u8; len];
        read.read_exact(&mut body).await.unwrap_or_default();
        assert!(String::from_utf8_lossy(&body).contains("READY"));
        let set = json!({"cmd": "SET_ACTIVITY", "nonce": "1", "args": {"pid": 77, "activity": {"details": "Playing"}}});
        write
            .write_all(&encode_frame(OP_FRAME, &set))
            .await
            .unwrap_or_default();
        let changed = bs_rt::time::timeout(Duration::from_secs(2), seen.changed()).await;
        assert!(changed.is_ok());
        let push = seen.borrow().clone();
        assert_eq!(push.as_ref().and_then(|p| p.pid), Some(77));
        assert_eq!(
            push.and_then(|p| p.activity.details).as_deref(),
            Some("Playing")
        );
    }
}
