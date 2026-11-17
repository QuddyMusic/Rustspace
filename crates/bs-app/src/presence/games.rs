//! Словарь игр и поиск запущенных: порт `activityDetector.ts` из PR #482
//! оригинального Backspace (QuddyMusic, cirzear). Словарь — тот же
//! `games.json`; свежая версия берётся из репозитория оригинала.

use std::collections::HashSet;
use std::path::PathBuf;

use super::procs::{COMM_MAX, ProcessInfo};
use super::stores::{self, StoreGame};

/// Словарь, вшитый в клиент.
const SEED: &str = include_str!("../../assets/games.json");
/// Свежий словарь (раз в сутки, с `If-None-Match`).
pub(crate) const REMOTE_URL: &str = "https://raw.githubusercontent.com/TheZwiss/backspace/main/packages/desktop/resources/games.json";
/// Больше игр сразу сервер не принимает.
pub(crate) const MAX_GAMES: usize = 5;

/// Запись словаря.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GameEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    /// Имена исполняемых файлов (без учёта регистра).
    processes: Vec<String>,
    /// Подстроки командной строки (Minecraft — это `java`).
    patterns: Vec<String>,
    /// `playing`, `listening`, `watching`, `streaming`.
    pub(crate) kind: String,
}

/// Словарь с версией.
#[derive(Debug, Clone, Default)]
pub(crate) struct Dictionary {
    pub(crate) version: u64,
    pub(crate) entries: Vec<GameEntry>,
}

fn strings(value: Option<&serde_json::Value>) -> Option<Vec<String>> {
    value?
        .as_array()?
        .iter()
        .map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
        .collect()
}

fn parse_entry(value: &serde_json::Value) -> Option<GameEntry> {
    let id = value.get("id")?.as_str().filter(|s| !s.is_empty())?;
    let name = value.get("name")?.as_str().filter(|s| !s.is_empty())?;
    let processes = strings(value.get("processes"))?;
    let patterns = strings(value.get("cmdlinePatterns")).unwrap_or_default();
    if processes.is_empty() && patterns.is_empty() {
        return None;
    }
    let kind = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .filter(|t| matches!(*t, "playing" | "listening" | "watching" | "streaming"))
        .unwrap_or("playing");
    Some(GameEntry {
        id: id.to_owned(),
        name: name.to_owned(),
        processes,
        patterns,
        kind: kind.to_owned(),
    })
}

/// Разбор словаря: `{version, games}` или просто массив.
pub(crate) fn parse_dictionary(text: &str) -> Option<Dictionary> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let (version, list) = match &value {
        serde_json::Value::Array(list) => (0, list),
        serde_json::Value::Object(map) => {
            (map.get("version")?.as_u64()?, map.get("games")?.as_array()?)
        }
        _ => return None,
    };
    let entries: Vec<GameEntry> = list.iter().filter_map(parse_entry).collect();
    (!entries.is_empty()).then_some(Dictionary { version, entries })
}

fn cache_path() -> Option<PathBuf> {
    crate::saved_session::config_dir().map(|dir| dir.join("games-cache.json"))
}

/// Лучший из вшитого и сохранённого словаря (по версии).
pub(crate) fn load() -> Dictionary {
    let seed = parse_dictionary(SEED).unwrap_or_default();
    let cached = cache_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| parse_dictionary(&t));
    match cached {
        Some(cached) if cached.version >= seed.version => cached,
        _ => seed,
    }
}

/// Скачать свежий словарь, если он новее. `Some` — новый словарь.
pub(crate) async fn sync(http: &reqwest::Client, current: u64) -> Option<Dictionary> {
    let path = cache_path()?;
    let etag_path = path.with_file_name("games-cache-etag.txt");
    let etag = std::fs::read_to_string(&etag_path)
        .ok()
        .filter(|e| !e.trim().is_empty());
    let mut request = http
        .get(REMOTE_URL)
        .timeout(std::time::Duration::from_secs(10));
    if let Some(etag) = etag.as_deref() {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag.trim());
    }
    let response = request.send().await.ok()?;
    if response.status() != reqwest::StatusCode::OK {
        return None;
    }
    let new_etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = response.text().await.ok()?;
    let parsed = parse_dictionary(&body)?;
    if parsed.version < current {
        return None;
    }
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, &body).is_ok() {
        let _ = std::fs::rename(&temporary, &path);
    }
    if let Some(etag) = new_etag {
        let _ = std::fs::write(&etag_path, etag);
    }
    (parsed.version > current).then_some(parsed)
}

/// Идёт ли трансляция в OBS (по последнему журналу за сутки).
fn obs_streaming() -> bool {
    let home = PathBuf::from(
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .unwrap_or_default(),
    );
    let mut dirs = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let appdata = PathBuf::from(appdata);
        dirs.push(appdata.join("obs-studio/logs"));
        dirs.push(appdata.join("slobs-client/node-obs/logs"));
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
        dirs.push(PathBuf::from(config).join("obs-studio/logs"));
    }
    dirs.push(home.join(".config/obs-studio/logs"));
    dirs.push(home.join(".var/app/com.obsproject.Studio/config/obs-studio/logs"));
    dirs.push(home.join("Library/Application Support/obs-studio/logs"));
    for dir in dirs {
        let latest = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("txt") || x.eq_ignore_ascii_case("log"))
            })
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .max_by_key(|(at, _)| *at);
        let Some((modified, file)) = latest else {
            continue;
        };
        if modified
            .elapsed()
            .is_ok_and(|age| age.as_secs() > 24 * 60 * 60)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        let tail = &bytes[bytes.len().saturating_sub(1024 * 1024)..];
        let content = String::from_utf8_lossy(tail);
        let start = ["==== Streaming Start =", "Stream output started"]
            .iter()
            .filter_map(|m| content.rfind(m))
            .max();
        let stop = ["==== Streaming Stop =", "Stream output stopped"]
            .iter()
            .filter_map(|m| content.rfind(m))
            .max();
        if start.is_some_and(|start| stop.is_none_or(|stop| start > stop)) {
            return true;
        }
    }
    false
}

impl GameEntry {
    /// Подходит ли процесс: имя (или его обрезка до 15 знаков), Windows-
    /// программы процесса Wine или подстрока командной строки.
    pub(crate) fn matches(&self, info: &ProcessInfo) -> bool {
        let candidates = std::iter::once(&info.name).chain(info.win_exes.iter());
        for candidate in candidates {
            let name = candidate.to_lowercase();
            for process in &self.processes {
                let process = process.to_lowercase();
                if name == process
                    || (process.chars().count() > COMM_MAX
                        && name.chars().count() == COMM_MAX
                        && process.starts_with(&name))
                {
                    return true;
                }
            }
        }
        if !info.cmdline.is_empty() {
            let cmdline = info.cmdline.to_lowercase();
            return self
                .patterns
                .iter()
                .any(|p| cmdline.contains(&p.to_lowercase()));
        }
        false
    }
}

/// Игра, которую добавил сам игрок и привязал к программе.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) name: String,
    pub(crate) process: String,
}

/// Найденная игра и процесс, по которому её нашли.
#[derive(Debug, Clone)]
pub(crate) struct Detected {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) info: Option<ProcessInfo>,
    /// Игра из магазина: каталог и иконка оттуда.
    pub(crate) store: Option<StoreGame>,
}

/// Ключ имени: только буквы и цифры, в нижнем регистре.
pub(crate) fn name_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_launcher(id: &str, name: &str) -> bool {
    id.to_lowercase().contains("launcher") || name.to_lowercase().contains("launcher")
}

#[derive(Default)]
struct Found {
    list: Vec<Detected>,
    ids: HashSet<String>,
    names: HashSet<String>,
    claimed: HashSet<u32>,
}

impl Found {
    fn add(&mut self, game: Detected) {
        self.ids.insert(game.id.clone());
        self.names
            .insert(format!("{}:{}", game.kind, name_key(&game.name)));
        if let Some(info) = &game.info
            && info.pid > 0
        {
            self.claimed.insert(info.pid);
        }
        self.list.push(game);
    }

    fn claims(&self, info: &ProcessInfo) -> bool {
        info.pid > 0 && self.claimed.contains(&info.pid)
    }
}

/// Все запущенные игры: сначала привязанные игроком, затем словарь (его
/// порядок — приоритет), затем магазины. Одна игра — один раз; лаунчер
/// уходит, как только запущена настоящая игра.
pub(crate) fn detect(
    infos: &[ProcessInfo],
    dictionary: &Dictionary,
    store_games: &[StoreGame],
    bindings: &[Binding],
) -> Vec<Detected> {
    let mut acc = Found::default();
    for binding in bindings {
        let id = format!("manual:{}", binding.process.to_lowercase());
        if acc.ids.contains(&id) {
            continue;
        }
        let entry = GameEntry {
            id: id.clone(),
            name: binding.name.clone(),
            processes: vec![binding.process.clone()],
            patterns: Vec::new(),
            kind: "playing".into(),
        };
        let running: Vec<&ProcessInfo> = infos.iter().filter(|i| entry.matches(i)).collect();
        if let Some(first) = running.first() {
            acc.add(Detected {
                id,
                name: binding.name.clone(),
                kind: "playing".into(),
                info: Some((*first).clone()),
                store: None,
            });
            acc.claimed
                .extend(running.iter().map(|i| i.pid).filter(|p| *p > 0));
        }
    }

    let mut obs: Option<bool> = None;
    for entry in &dictionary.entries {
        if acc.ids.contains(&entry.id) {
            continue;
        }
        let Some(info) = infos.iter().find(|i| entry.matches(i)) else {
            continue;
        };
        if acc.claims(info) {
            continue;
        }
        if (entry.id == "obs" || entry.id == "streamlabs")
            && !*obs.get_or_insert_with(obs_streaming)
        {
            continue;
        }
        acc.add(Detected {
            id: entry.id.clone(),
            name: entry.name.clone(),
            kind: entry.kind.clone(),
            info: Some(info.clone()),
            store: None,
        });
    }

    for info in infos {
        if acc.claims(info) {
            continue;
        }
        let Some(game) = stores::game_for(info, store_games) else {
            continue;
        };
        if acc.ids.contains(&game.id)
            || acc
                .names
                .contains(&format!("playing:{}", name_key(&game.name)))
        {
            continue;
        }
        acc.add(Detected {
            id: game.id.clone(),
            name: game.name.clone(),
            kind: "playing".into(),
            info: Some(info.clone()),
            store: Some(game),
        });
    }

    let mut found = acc.list;
    let real_game = found
        .iter()
        .any(|g| g.kind == "playing" && !is_launcher(&g.id, &g.name));
    if real_game {
        found.retain(|g| !(g.kind == "playing" && is_launcher(&g.id, &g.name)));
    }
    found.truncate(MAX_GAMES);
    found
}

/// Название игры по процессу (для пушей по IPC без имени).
pub(crate) fn name_for_pid(dictionary: &Dictionary, pid: u32) -> Option<String> {
    let info = super::procs::by_pid(pid)?;
    dictionary
        .entries
        .iter()
        .find(|e| e.matches(&info))
        .map(|e| e.name.clone())
}

/// Wine-программы, которые не принадлежат ни одной найденной игре: о них
/// можно спросить игрока («запущена программа, которую я не знаю»).
pub(crate) fn unknown_wine_programs(infos: &[ProcessInfo], games: &[Detected]) -> Vec<String> {
    let claimed_pids: HashSet<u32> = games
        .iter()
        .filter_map(|g| g.info.as_ref())
        .map(|i| i.pid)
        .collect();
    let claimed_dirs: Vec<&str> = games
        .iter()
        .filter_map(|g| g.info.as_ref()?.cwd.as_deref())
        .collect();
    let mut seen = Vec::new();
    for info in infos {
        let Some(program) = info
            .win_exes
            .first()
            .filter(|p| p.to_lowercase().ends_with(".exe"))
        else {
            continue;
        };
        if claimed_pids.contains(&info.pid) {
            continue;
        }
        if let Some(cwd) = info.cwd.as_deref()
            && claimed_dirs.iter().any(|dir| {
                cwd == *dir
                    || cwd.starts_with(&format!("{dir}/"))
                    || dir.starts_with(&format!("{cwd}/"))
            })
        {
            continue;
        }
        if !seen
            .iter()
            .any(|s: &String| s.eq_ignore_ascii_case(program))
        {
            seen.push(program.clone());
        }
    }
    seen.sort_by_key(|s| s.to_lowercase());
    seen.truncate(20);
    seen
}

/// Свои программы, к которым игрок может привязать игру: без системных
/// (из /usr, /bin, …); программа Wine — по своему Windows-имени.
pub(crate) fn running_programs(infos: &[ProcessInfo]) -> Vec<String> {
    const SYSTEM: &[&str] = &[
        "/usr/", "/bin/", "/sbin/", "/lib/", "/lib32/", "/lib64/", "/etc/", "/snap/", "/run/",
        "/proc/", "/sys/", "/var/",
    ];
    let mut seen: Vec<String> = Vec::new();
    for info in infos.iter().filter(|i| i.mine) {
        let program = if let Some(windows) = info.win_exes.first() {
            windows.clone()
        } else {
            if info
                .exe
                .as_deref()
                .is_some_and(|exe| SYSTEM.iter().any(|s| exe.starts_with(s)))
            {
                continue;
            }
            if cfg!(windows)
                && info
                    .exe
                    .as_deref()
                    .is_some_and(|exe| exe.to_lowercase().contains("\\windows\\"))
            {
                continue;
            }
            info.name.clone()
        };
        if !seen.iter().any(|s| s.eq_ignore_ascii_case(&program)) {
            seen.push(program);
        }
    }
    seen.sort_by_key(|s| s.to_lowercase());
    seen.truncate(300);
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, cmdline: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: name.into(),
            cmdline: cmdline.into(),
            mine: true,
            ..ProcessInfo::default()
        }
    }

    #[test]
    fn seed_dictionary_parses() {
        let seed = parse_dictionary(SEED).unwrap_or_default();
        assert!(seed.version >= 3 && seed.entries.len() > 200);
    }

    #[test]
    fn detection_follows_the_original_rules() {
        let dictionary = parse_dictionary(
            r#"{"version":1,"games":[
                {"id":"mc","name":"Minecraft","processes":[],"cmdlinePatterns":["net.minecraft.client.main.main"]},
                {"id":"mcl","name":"Minecraft Launcher","processes":["minecraft-launcher"]},
                {"id":"cs2","name":"Counter-Strike 2","processes":["cs2"]}
            ]}"#,
        )
        .unwrap_or_default();
        // Лаунчер виден, пока нет игры; обрезанный comm узнаётся.
        let launcher = detect(&[proc(1, "minecraft-launc", "")], &dictionary, &[], &[]);
        assert_eq!(
            launcher.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            ["Minecraft Launcher"]
        );
        let both = detect(
            &[
                proc(1, "minecraft-launc", ""),
                proc(2, "java", "java net.minecraft.client.main.Main"),
                proc(3, "cs2", ""),
            ],
            &dictionary,
            &[],
            &[],
        );
        assert_eq!(
            both.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            ["Minecraft", "Counter-Strike 2"]
        );
        // Своя привязка забирает процесс у словаря.
        let manual = detect(
            &[proc(3, "cs2", "")],
            &dictionary,
            &[],
            &[Binding {
                name: "CS".into(),
                process: "CS2".into(),
            }],
        );
        assert_eq!(
            manual.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
            ["CS"]
        );
    }

    #[test]
    fn programs_and_unknown_wine() {
        let mut wine = proc(9, "wine64-preloader", "Z:\\g\\Odd.exe");
        wine.win_exes = vec!["Odd.exe".into()];
        let mut system = proc(4, "bash", "bash");
        system.exe = Some("/usr/bin/bash".into());
        let infos = vec![wine, system, proc(5, "mygame", "")];
        assert_eq!(
            running_programs(&infos),
            vec!["mygame".to_owned(), "Odd.exe".to_owned()]
        );
        assert_eq!(
            unknown_wine_programs(&infos, &[]),
            vec!["Odd.exe".to_owned()]
        );
    }
}
