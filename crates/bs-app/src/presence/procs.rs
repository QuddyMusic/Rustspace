//! Список процессов: что запущено, с путём, командной строкой и (для Wine)
//! именами Windows-программ. Порт `activityDetector.ts` из PR #482
//! оригинального Backspace (QuddyMusic, cirzear).

use std::path::Path;

/// Ядро Linux хранит `comm` в 16 байтах с нулём на конце: длинные имена
/// обрезаны до 15 знаков («minecraft-launcher» → «minecraft-launc»).
pub(crate) const COMM_MAX: usize = 15;

/// Процесс, как его видит детектор.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProcessInfo {
    pub(crate) pid: u32,
    /// Имя: базовое имя исполняемого файла, иначе первого слова командной
    /// строки, иначе `comm`.
    pub(crate) name: String,
    /// Командная строка через пробелы (пусто, если недоступна).
    pub(crate) cmdline: String,
    /// Полный путь к исполняемому файлу.
    pub(crate) exe: Option<String>,
    /// Windows-программы, которые может исполнять процесс Wine/Proton,
    /// самые точные — первыми. Только у процессов Wine.
    pub(crate) win_exes: Vec<String>,
    /// Рабочий каталог процесса Wine: там обычно лежит сама игра.
    pub(crate) cwd: Option<String>,
    /// Процесс текущего пользователя.
    pub(crate) mine: bool,
}

fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

#[cfg(any(target_os = "linux", test))]
fn is_windows_path(token: &str) -> bool {
    let t = token.trim_start_matches('"').as_bytes();
    t.len() >= 3 && t[0].is_ascii_alphabetic() && t[1] == b':' && (t[2] == b'\\' || t[2] == b'/')
}

/// Самое говорящее имя: исполняемый файл (полное и с регистром), первое
/// слово командной строки или `comm` (может быть обрезан).
pub(crate) fn pick_name(
    exe: Option<&str>,
    tokens: &[String],
    comm: Option<&str>,
) -> Option<String> {
    if let Some(base) = exe.map(base_name).filter(|b| !b.is_empty()) {
        return Some(base.to_owned());
    }
    if let Some(first) = tokens.first() {
        let base = base_name(first).trim_end_matches('"');
        if !base.is_empty() {
            return Some(base.to_owned());
        }
    }
    comm.filter(|c| !c.is_empty()).map(str::to_owned)
}

#[cfg(any(target_os = "linux", test))]
/// Программы, которые запускает любой префикс Wine, и обработчики сбоев
/// игр: это никогда не игра. Прокладка Proton `steam.exe` — исключение,
/// когда больше ничего не названо.
const WINE_PLUMBING: &[&str] = &[
    "start.exe",
    "explorer.exe",
    "services.exe",
    "svchost.exe",
    "winedevice.exe",
    "plugplay.exe",
    "rpcss.exe",
    "tabtip.exe",
    "conhost.exe",
    "wineboot.exe",
    "winemenubuilder.exe",
    "rundll32.exe",
    "cmd.exe",
    "unitycrashhandler32.exe",
    "unitycrashhandler64.exe",
    "microsoftedgeupdate.exe",
];

#[cfg(any(target_os = "linux", test))]
fn is_wine_plumbing(name: &str) -> bool {
    let lower = name.to_lowercase();
    WINE_PLUMBING
        .iter()
        .any(|p| *p == lower || (lower.chars().count() == COMM_MAX && p.starts_with(&lower)))
}

#[cfg(any(target_os = "linux", test))]
/// Все «<имя>.exe» в командной строке, каким бы разделителем ни шло перед ними.
fn exe_names(cmdline: &str) -> Vec<String> {
    let lower = cmdline.to_lowercase();
    let mut found = Vec::new();
    let (mut from, mut taken) = (0, 0);
    while let Some(at) = lower[from..].find(".exe") {
        let end = from + at + 4;
        let next = cmdline[end..].chars().next();
        if next.is_none_or(|c| c == '"' || c.is_whitespace()) {
            // Как ленивое `([^\\/"]+?\.exe)` оригинала: от разделителя пути
            // или от конца прошлого совпадения.
            let start = cmdline[..from + at]
                .rfind(['\\', '/', '"'])
                .map_or(0, |i| i + 1)
                .max(taken);
            found.push(cmdline[start..end].trim_start().to_owned());
            taken = end;
        }
        from = end;
    }
    found
}

#[cfg(any(target_os = "linux", test))]
/// Windows-программы процесса Wine (самые точные — первыми) или пусто для
/// любого другого. Процесс Wine узнаётся по загрузчику (`wine`,
/// `wine64-preloader`, сборки Proton и Bottles) или по командной строке в
/// духе Windows; родная программа, у которой среди аргументов «foo.exe»
/// (редактор открыл файл), сюда не попадает.
pub(crate) fn windows_exes(
    exe: Option<&str>,
    tokens: &[String],
    comm: Option<&str>,
) -> Vec<String> {
    let base = exe.map(base_name).unwrap_or_default().to_lowercase();
    let loader = base.starts_with("wine") && !base.starts_with("wineserver");
    let windows_style = tokens.first().is_some_and(|t| is_windows_path(t));
    if !loader && !windows_style {
        return Vec::new();
    }
    let mut names: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        let name = name.trim();
        if !name.is_empty() && !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            names.push(name.to_owned());
        }
    };
    for captured in exe_names(&tokens.join(" ")) {
        let captured = captured.trim();
        if is_wine_plumbing(captured) {
            continue;
        }
        // Аргументы до первого разделителя пути («-k Foo bar.exe») попадают в захват.
        let words: Vec<&str> = captured.split(' ').collect();
        for from in 0..words.len() {
            add(&words[from..].join(" "));
        }
    }
    // Wine называет процесс по программе, обрезая до 15 знаков.
    if loader
        && let Some(comm) = comm
        && !comm.to_lowercase().starts_with("wine")
        && !is_wine_plumbing(comm)
    {
        add(comm);
    }
    let real: Vec<String> = names
        .iter()
        .filter(|n| !n.eq_ignore_ascii_case("steam.exe"))
        .cloned()
        .collect();
    if real.is_empty() { names } else { real }
}

/// Процессы системы.
#[cfg(target_os = "linux")]
pub(crate) fn list() -> Vec<ProcessInfo> {
    list_in(Path::new("/proc"))
}

/// Обход `/proc`: путь к исполняемому файлу (с путями Wine), командная
/// строка через нули и `comm` как запасной вариант.
#[cfg(target_os = "linux")]
pub(crate) fn list_in(root: &Path) -> Vec<ProcessInfo> {
    use std::os::unix::fs::MetadataExt as _;
    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut infos = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if let Some(mut info) = read_pid(&entry.path(), pid) {
            info.mine = me.is_none_or(|me| entry.metadata().is_ok_and(|m| m.uid() == me));
            infos.push(info);
        }
    }
    infos
}

/// Один процесс по номеру.
#[cfg(target_os = "linux")]
pub(crate) fn by_pid(pid: u32) -> Option<ProcessInfo> {
    read_pid(&Path::new("/proc").join(pid.to_string()), pid)
}

#[cfg(target_os = "linux")]
fn read_pid(dir: &Path, pid: u32) -> Option<ProcessInfo> {
    let exe = std::fs::read_link(dir.join("exe"))
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .filter(|p| !p.ends_with(" (deleted)"));
    let tokens: Vec<String> = std::fs::read(dir.join("cmdline"))
        .map(|raw| {
            raw.split(|b| *b == 0)
                .filter(|t| !t.is_empty())
                .map(|t| String::from_utf8_lossy(t).into_owned())
                .collect()
        })
        .unwrap_or_default();
    let comm = std::fs::read_to_string(dir.join("comm"))
        .ok()
        .map(|c| c.trim().to_owned())
        .filter(|c| !c.is_empty());
    let name = pick_name(exe.as_deref(), &tokens, comm.as_deref())?;
    let win_exes = windows_exes(exe.as_deref(), &tokens, comm.as_deref());
    let cwd = if win_exes.is_empty() {
        None
    } else {
        std::fs::read_link(dir.join("cwd"))
            .ok()
            .and_then(|p| p.to_str().map(str::to_owned))
    };
    Some(ProcessInfo {
        pid,
        name,
        cmdline: tokens.join(" "),
        exe,
        win_exes,
        cwd,
        mine: true,
    })
}

/// Процессы системы (Windows, macOS).
#[cfg(not(target_os = "linux"))]
pub(crate) fn list() -> Vec<ProcessInfo> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );
    system
        .processes()
        .iter()
        .map(|(pid, process)| {
            let exe = process.exe().and_then(Path::to_str).map(str::to_owned);
            let tokens: Vec<String> = process
                .cmd()
                .iter()
                .map(|t| t.to_string_lossy().into_owned())
                .collect();
            let comm = process.name().to_string_lossy().into_owned();
            ProcessInfo {
                pid: pid.as_u32(),
                name: pick_name(exe.as_deref(), &tokens, Some(&comm)).unwrap_or(comm),
                cmdline: tokens.join(" "),
                exe,
                win_exes: Vec::new(),
                cwd: None,
                mine: true,
            }
        })
        .collect()
}

/// Один процесс по номеру (Windows, macOS).
#[cfg(not(target_os = "linux"))]
pub(crate) fn by_pid(pid: u32) -> Option<ProcessInfo> {
    list().into_iter().find(|info| info.pid == pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(line: &[&str]) -> Vec<String> {
        line.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn wine_programs_are_named_by_their_windows_exe() {
        let exes = windows_exes(
            Some("/home/u/.local/share/Steam/compatibilitytools.d/GE/files/bin/wine64-preloader"),
            &tokens(&["Z:\\games\\Star Rail\\StarRail.exe", "-popupwindow"]),
            Some("StarRail.exe"),
        );
        assert_eq!(exes.first().map(String::as_str), Some("StarRail.exe"));
        let shim = windows_exes(
            Some("/usr/bin/wine64-preloader"),
            &tokens(&["steam.exe", "Game.exe"]),
            None,
        );
        assert_eq!(shim, vec!["Game.exe".to_owned()]);
        let plumbing = windows_exes(
            Some("/usr/bin/wine"),
            &tokens(&["C:\\windows\\system32\\services.exe"]),
            None,
        );
        assert!(plumbing.is_empty());
        let editor = windows_exes(Some("/usr/bin/vim"), &tokens(&["vim", "setup.exe"]), None);
        assert!(editor.is_empty());
        let spaced = windows_exes(
            Some("/usr/bin/wine"),
            &tokens(&["C:\\GOG", "Games\\Stardew", "Valley\\Stardew", "Valley.exe"]),
            None,
        );
        assert!(
            spaced.iter().any(|n| n == "Stardew Valley.exe"),
            "{spaced:?}"
        );
    }

    #[test]
    fn names_prefer_the_executable() {
        assert_eq!(
            pick_name(Some("/opt/game/Game.x86_64"), &[], Some("Game.x86")).as_deref(),
            Some("Game.x86_64")
        );
        assert_eq!(
            pick_name(None, &tokens(&["\"C:\\a\\b.exe\""]), None).as_deref(),
            Some("b.exe")
        );
        assert_eq!(
            pick_name(None, &[], Some("kworker")).as_deref(),
            Some("kworker")
        );
    }
}
