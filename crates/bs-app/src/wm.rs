//! Как спрятать окно под тайлинговыми композиторами Wayland.
//!
//! Протокол Wayland не умеет прятать окно, а «свернуть» тайлинговые
//! композиторы (Hyprland, Sway) просто игнорируют. Зато у них есть IPC:
//! окно можно убрать на скрытый спецворкспейс Hyprland или в scratchpad Sway
//! и вернуть обратно на текущий рабочий стол. Окно ищем по PID процесса.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// Спецворкспейс Hyprland, куда прячется окно.
const SPECIAL: &str = "special:rustspace";

/// Hyprland с Lua-конфигом (0.55+): `dispatch` там принимает только
/// Lua-выражение, поэтому команды идут через `eval`. Узнаём по первому отказу.
static HYPR_LUA: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ответ Hyprland — «ok».
fn hypr_ok(reply: Option<&str>) -> bool {
    reply.is_some_and(|text| text.trim() == "ok")
}

/// Ответ Hyprland, по которому видно Lua-конфиг.
fn wants_lua(reply: Option<&str>) -> bool {
    reply.is_some_and(|text| text.contains("hl.dispatch") || text.contains("lua"))
}

/// Куда переносить окно.
#[derive(Debug, Clone, Copy)]
enum Target<'a> {
    /// Спецворкспейс (спрятать).
    Hidden,
    /// Текущий стол (показать); в классическом режиме — по номеру.
    Active(Option<&'a str>),
}

/// Lua для `eval`: перенести окна процесса `pid` на стол, не переключаясь
/// туда, и при `focus` дать им фокус. Текущий стол Lua берёт сам —
/// `hl.get_active_workspace()`; строка с номером («3») там была бы именем.
fn lua_move(pid: u32, target: Target<'_>, focus: bool) -> String {
    let workspace = match target {
        Target::Hidden => format!("\"{SPECIAL}\""),
        // Номер текущего стола: сам объект стола принимают не все версии.
        Target::Active(_) => "(function() local ws = hl.get_active_workspace() \
             if type(ws) == \"table\" or type(ws) == \"userdata\" then return ws.id or ws.name end \
             return ws end)()"
            .to_owned(),
    };
    let focus = if focus {
        " hl.dispatch(hl.dsp.focus({ window = w }))"
    } else {
        ""
    };
    format!(
        "for _, w in ipairs(hl.get_windows() or {{}}) do if w.pid == {pid} then \
         hl.dispatch(hl.dsp.window.move({{ workspace = {workspace}, follow = false, window = w }}))\
         {focus} end end"
    )
}

/// Перенести окна процесса на рабочий стол: классическим `dispatch`, а у
/// Lua-конфига — через `eval`.
fn hypr_move(
    socket: &PathBuf,
    pid: u32,
    target: Target<'_>,
    focus: bool,
) -> (bool, Option<String>) {
    use std::sync::atomic::Ordering;
    let classic = match target {
        Target::Hidden => Some(SPECIAL),
        Target::Active(number) => number,
    };
    if !HYPR_LUA.load(Ordering::Relaxed)
        && let Some(workspace) = classic
    {
        let reply = hypr(
            socket,
            &format!("dispatch movetoworkspacesilent {workspace},pid:{pid}"),
        );
        if hypr_ok(reply.as_deref()) {
            if focus {
                let _ = hypr(socket, &format!("dispatch focuswindow pid:{pid}"));
            }
            return (true, reply);
        }
        if !wants_lua(reply.as_deref()) {
            return (false, reply);
        }
        tracing::info!("Hyprland с Lua-конфигом: команды окна идут через eval");
        HYPR_LUA.store(true, Ordering::Relaxed);
    }
    let reply = hypr(socket, &format!("eval {}", lua_move(pid, target, focus)));
    (hypr_ok(reply.as_deref()), reply)
}

/// Композитор, которому можно приказать спрятать окно.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Wm {
    /// Hyprland: путь к командному сокету.
    Hyprland(PathBuf),
    /// Sway: путь к сокету i3-IPC.
    Sway(PathBuf),
}

impl Wm {
    /// Композитор текущего сеанса, если он из поддерживаемых.
    pub(crate) fn detect() -> Option<Self> {
        let env = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        if let Some(signature) = env("HYPRLAND_INSTANCE_SIGNATURE") {
            // Новые версии держат сокет в XDG_RUNTIME_DIR, старые — в /tmp.
            let mut candidates = Vec::new();
            if let Some(runtime) = env("XDG_RUNTIME_DIR") {
                candidates.push(PathBuf::from(runtime).join("hypr"));
            }
            candidates.push(PathBuf::from("/tmp/hypr"));
            let found = candidates
                .into_iter()
                .map(|dir| dir.join(&signature).join(".socket.sock"))
                .find(|path| path.exists());
            if let Some(path) = &found {
                tracing::info!(socket = %path.display(), "Hyprland: окно прячется через IPC");
            } else {
                tracing::warn!("Hyprland: сокет IPC не найден, окно спрятать не получится");
            }
            return found.map(Self::Hyprland);
        }
        let sway = env("SWAYSOCK").map(|path| Self::Sway(PathBuf::from(path)));
        if sway.is_some() {
            tracing::info!("Sway: окно прячется через IPC (scratchpad)");
        }
        sway
    }

    /// Спрятать окно этого процесса. `false` — не вышло.
    pub(crate) fn hide(&self) -> bool {
        let pid = std::process::id();
        let done = match self {
            Self::Hyprland(socket) => {
                let (ok, reply) = hypr_move(socket, pid, Target::Hidden, false);
                if !ok {
                    tracing::warn!(?reply, pid, "Hyprland не спрятал окно");
                }
                ok
            }
            Self::Sway(socket) => sway_ok(socket, &format!("[pid={pid}] move scratchpad")),
        };
        tracing::debug!(done, "окно спрятано через IPC композитора");
        done
    }

    /// Вернуть окно на текущий рабочий стол и дать ему фокус.
    pub(crate) fn show(&self) -> bool {
        let pid = std::process::id();
        match self {
            Self::Hyprland(socket) => {
                use std::sync::atomic::Ordering;
                // Классическому `dispatch` нужен номер стола; Lua узнаёт его сам.
                let number = if HYPR_LUA.load(Ordering::Relaxed) {
                    None
                } else {
                    hypr(socket, "j/activeworkspace")
                        .and_then(|reply| serde_json::from_str::<serde_json::Value>(&reply).ok())
                        .and_then(|value| value.get("id").and_then(serde_json::Value::as_i64))
                        .map(|id| id.to_string())
                };
                let (moved, reply) =
                    hypr_move(socket, pid, Target::Active(number.as_deref()), true);
                if moved {
                    tracing::info!("Hyprland: окно возвращено");
                } else {
                    tracing::warn!(?reply, pid, "Hyprland не вернул окно");
                }
                moved
            }
            // `scratchpad show` у окна в scratchpad показывает его поверх
            // текущего стола; `floating disable` возвращает в раскладку.
            Self::Sway(socket) => sway_ok(
                socket,
                &format!("[pid={pid}] scratchpad show, floating disable, focus"),
            ),
        }
    }
}

/// Одна команда в сокет Hyprland: ответ целиком.
fn hypr(socket: &PathBuf, command: &str) -> Option<String> {
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    stream.write_all(command.as_bytes()).ok()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply).ok()?;
    Some(reply)
}

/// Тип сообщения i3-IPC «выполнить команду».
const I3_RUN_COMMAND: u32 = 0;
const I3_MAGIC: &[u8; 6] = b"i3-ipc";

/// Сообщение i3-IPC: магия, длина и тип (native-endian), тело.
fn i3_frame(kind: u32, body: &str) -> Vec<u8> {
    let mut frame = Vec::with_capacity(14 + body.len());
    frame.extend_from_slice(I3_MAGIC);
    frame.extend_from_slice(&u32::try_from(body.len()).unwrap_or(0).to_ne_bytes());
    frame.extend_from_slice(&kind.to_ne_bytes());
    frame.extend_from_slice(body.as_bytes());
    frame
}

/// Команда Sway; `true`, если все её части выполнились.
fn sway_ok(socket: &PathBuf, command: &str) -> bool {
    let reply = (|| {
        let mut stream = UnixStream::connect(socket).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
        stream.write_all(&i3_frame(I3_RUN_COMMAND, command)).ok()?;
        let mut header = [0_u8; 14];
        stream.read_exact(&mut header).ok()?;
        if &header[..6] != I3_MAGIC {
            return None;
        }
        let length = u32::from_ne_bytes(header[6..10].try_into().ok()?);
        let mut body = vec![0_u8; usize::try_from(length).ok()?];
        stream.read_exact(&mut body).ok()?;
        serde_json::from_slice::<Vec<serde_json::Value>>(&body).ok()
    })();
    reply.is_some_and(|results| {
        !results.is_empty()
            && results
                .iter()
                .all(|result| result.get("success") == Some(&serde_json::Value::Bool(true)))
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn i3_frame_layout() {
        let frame = i3_frame(0, "nop");
        assert_eq!(&frame[..6], b"i3-ipc");
        assert_eq!(u32::from_ne_bytes(frame[6..10].try_into().expect("len")), 3);
        assert_eq!(&frame[14..], b"nop");
    }

    /// Поддельный Hyprland: записывает команды и отвечает как настоящий.
    #[test]
    fn hyprland_hide_and_show() {
        let dir = std::env::temp_dir().join(format!("rs-wm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join(".socket.sock");
        let listener = UnixListener::bind(&path).expect("bind");
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().expect("accept");
                let mut buf = [0_u8; 256];
                let n = stream.read(&mut buf).expect("read");
                let command = String::from_utf8_lossy(&buf[..n]).into_owned();
                let reply = if command == "j/activeworkspace" {
                    r#"{"id": 3, "name": "3"}"#
                } else {
                    "ok"
                };
                stream.write_all(reply.as_bytes()).expect("write");
                seen.push(command);
            }
            seen
        });
        let wm = Wm::Hyprland(path);
        assert!(wm.hide());
        assert!(wm.show());
        let seen = server.join().expect("join");
        let pid = std::process::id();
        assert_eq!(
            seen,
            [
                format!("dispatch movetoworkspacesilent special:rustspace,pid:{pid}"),
                "j/activeworkspace".to_owned(),
                format!("dispatch movetoworkspacesilent 3,pid:{pid}"),
                format!("dispatch focuswindow pid:{pid}"),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hyprland_lua_command() {
        let lua = lua_move(42, Target::Hidden, false);
        assert!(lua.contains("w.pid == 42"), "{lua}");
        assert!(
            lua.contains(r#"hl.dsp.window.move({ workspace = "special:rustspace", follow = false, window = w })"#),
            "{lua}"
        );
        assert!(!lua.contains("focus("), "{lua}");
        let back = lua_move(42, Target::Active(Some("3")), true);
        assert!(back.contains("hl.dsp.focus({ window = w })"), "{back}");
        assert!(back.contains("hl.get_active_workspace()"), "{back}");
        assert!(back.contains("ws.id or ws.name"), "{back}");
        assert!(wants_lua(Some(
            "error: [string \"return hl.dispatch(movetoworkspacesilent spec...\"]:1: ')' expected"
        )));
        assert!(!wants_lua(Some("ok")));
    }
}
