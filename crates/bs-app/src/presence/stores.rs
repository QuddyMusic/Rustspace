//! Установленные игры магазинов и лаунчеров: Steam, Flatpak, GOG, Heroic,
//! Bottles, Lutris. Порт `steamGames.ts`, `flatpakGames.ts`, `gogGames.ts`
//! и `launcherGames.ts` из PR #482 оригинального Backspace (QuddyMusic,
//! cirzear). Индексы перечитываются не чаще раза в минуту.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use web_time::Instant;

use super::procs::ProcessInfo;

const INDEX_TTL: Duration = Duration::from_secs(60);
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_GAMES_PER_SOURCE: usize = 2000;

/// Игра, найденная по процессу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreGame {
    /// `steam-570`, `flatpak-…`, `gog-…`, `heroic-…`, `bottles-…`, `lutris-…`.
    pub(crate) id: String,
    pub(crate) name: String,
    /// Каталог установки (для поиска иконки).
    pub(crate) dir: Option<PathBuf>,
    /// Готовая иконка, если лаунчер её хранит.
    pub(crate) icon: Option<PathBuf>,
    /// Имена `.exe` игры (для Wine).
    exe_names: Vec<String>,
}

/// Все установленные игры, с кэшем на минуту.
#[derive(Debug, Default)]
pub(crate) struct Stores {
    cached: Mutex<Option<(Instant, std::sync::Arc<Vec<StoreGame>>)>>,
}

impl Stores {
    /// Текущий индекс (перечитывается раз в минуту).
    pub(crate) fn games(&self) -> std::sync::Arc<Vec<StoreGame>> {
        let mut cached = self
            .cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, games)) = cached.as_ref()
            && at.elapsed() < INDEX_TTL
        {
            return games.clone();
        }
        let games = std::sync::Arc::new(load(&home()));
        *cached = Some((Instant::now(), games.clone()));
        games
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn load(home: &Path) -> Vec<StoreGame> {
    let mut games = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut push = |game: StoreGame| {
        if seen.insert(game.id.clone()) {
            games.push(game);
        }
    };
    for game in steam_games(home) {
        push(game);
    }
    for game in flatpak_games(home) {
        push(game);
    }
    for game in gog_games(home) {
        push(game);
    }
    for game in heroic_games(home)
        .into_iter()
        .chain(bottles_games(home))
        .chain(lutris_games(home))
    {
        push(game);
    }
    games
}

fn read_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return None;
    }
    crate::files::read_to_string(path).ok()
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&read_text(path)?).ok()
}

fn read_dir(path: &Path) -> Vec<std::fs::DirEntry> {
    std::fs::read_dir(path)
        .map(|d| d.flatten().collect())
        .unwrap_or_default()
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
        .to_lowercase()
        .trim_end_matches('/')
        .to_owned()
}

/// `target` лежит внутри `dir` (без учёта регистра, также по реальным путям).
fn is_under(target: &str, dir: &Path) -> bool {
    let d = normalize(&dir.to_string_lossy());
    let t = normalize(target);
    if !d.is_empty() && (t == d || t.starts_with(&format!("{d}/"))) {
        return true;
    }
    match (std::fs::canonicalize(target), std::fs::canonicalize(dir)) {
        (Ok(t), Ok(d)) => t.starts_with(d),
        _ => false,
    }
}

fn exe_names_of<'a>(paths: impl IntoIterator<Item = Option<&'a str>>) -> Vec<String> {
    let mut names = Vec::new();
    for path in paths.into_iter().flatten() {
        let name = path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path)
            .to_lowercase();
        if Path::new(&name).extension().is_some_and(|e| e == "exe") && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn text(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

// ─── Steam ──────────────────────────────────────────────────────────────────

/// Корни Steam (родной, Flatpak, Snap, Windows), без повторов.
pub(crate) fn steam_roots(home: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![
        home.join(".steam/steam"),
        home.join(".local/share/Steam"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
        home.join("snap/steam/common/.local/share/Steam"),
        home.join("Library/Application Support/Steam"),
    ];
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(dir) = std::env::var_os(var) {
            candidates.push(PathBuf::from(dir).join("Steam"));
        }
    }
    let mut roots = Vec::new();
    for candidate in candidates {
        if let Ok(real) = std::fs::canonicalize(&candidate)
            && !roots.contains(&real)
        {
            roots.push(real);
        }
    }
    roots
}

fn unescape_vdf(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Значение строкового ключа VDF/ACF (`"key"  "value"`), первое по тексту.
fn vdf_values<'a>(text: &'a str, key: &'a str) -> impl Iterator<Item = String> + 'a {
    let needle = format!("\"{key}\"");
    let mut from = 0;
    std::iter::from_fn(move || {
        loop {
            let at = text[from..].find(&needle)? + from;
            from = at + needle.len();
            let rest = text[from..].trim_start_matches([' ', '\t']);
            if let Some(body) = rest.strip_prefix('"') {
                let mut end = None;
                let mut escaped = false;
                for (i, c) in body.char_indices() {
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == '"' {
                        end = Some(i);
                        break;
                    }
                }
                if let Some(end) = end {
                    return Some(unescape_vdf(&body[..end]));
                }
            }
        }
    })
}

/// Манифест приложения Steam: номер, имя, каталог и «установлено целиком».
pub(crate) fn parse_app_manifest(text: &str) -> Option<(u64, String, String, bool)> {
    let first = |key| vdf_values(text, key).next();
    let app_id: u64 = first("appid")?.parse().ok().filter(|id| *id > 0)?;
    let name = first("name").filter(|n| !n.is_empty())?;
    let dir = first("installdir").filter(|d| !d.is_empty())?;
    let flags: u64 = first("StateFlags")?.parse().ok()?;
    Some((app_id, name, dir, flags & 4 != 0))
}

/// Среды выполнения и Proton — не игры.
fn is_steam_tool(name: &str, dir: &str) -> bool {
    let name = name.trim().to_lowercase();
    let dir = dir.trim().to_lowercase();
    name.starts_with("steam linux runtime")
        || name.starts_with("proton")
        || name.starts_with("steamworks common redistributables")
        || dir.starts_with("steamlinuxruntime")
        || dir.starts_with("proton")
        || dir == "steamworks shared"
}

fn steam_games(home: &Path) -> Vec<StoreGame> {
    let mut games = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in steam_roots(home) {
        let mut libraries: Vec<PathBuf> = read_text(&root.join("steamapps/libraryfolders.vdf"))
            .map(|text| vdf_values(&text, "path").map(PathBuf::from).collect())
            .unwrap_or_default();
        libraries.push(root.clone());
        for library in libraries {
            let apps = library.join("steamapps");
            for entry in read_dir(&apps) {
                let file = entry.file_name().to_string_lossy().into_owned();
                if !file.starts_with("appmanifest_")
                    || entry
                        .path()
                        .extension()
                        .is_none_or(|e| !e.eq_ignore_ascii_case("acf"))
                {
                    continue;
                }
                let Some((id, name, dir, installed)) = read_text(&entry.path())
                    .as_deref()
                    .and_then(parse_app_manifest)
                else {
                    continue;
                };
                if !installed || is_steam_tool(&name, &dir) || !seen.insert(id) {
                    continue;
                }
                games.push(StoreGame {
                    id: format!("steam-{id}"),
                    name,
                    dir: Some(apps.join("common").join(dir)),
                    icon: None,
                    exe_names: Vec::new(),
                });
            }
        }
    }
    games
}

/// Номер приложения из `SteamLaunch AppId=…` в командной строке `reaper`.
fn steam_launch_id(cmdline: &str) -> Option<String> {
    let mut words = cmdline.split_whitespace();
    while let Some(word) = words.next() {
        if word == "SteamLaunch" {
            return words
                .next()?
                .strip_prefix("AppId=")
                .filter(|id| id.bytes().all(|b| b.is_ascii_digit()))
                .map(|id| format!("steam-{id}"));
        }
    }
    None
}

// ─── Flatpak ────────────────────────────────────────────────────────────────

const FLATPAK_LAUNCHERS: &[&str] = &["steam", "lutris", "bottles", "heroic", "itch", "prism"];

pub(crate) fn flatpak_dirs(home: &Path) -> Vec<PathBuf> {
    [
        home.join(".local/share/flatpak"),
        PathBuf::from("/var/lib/flatpak"),
    ]
    .into_iter()
    .filter(|dir| dir.exists())
    .collect()
}

/// Раздел `[Desktop Entry]`: значение ключа.
pub(crate) fn desktop_value(text: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
            continue;
        }
        if inside
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
            && !v.trim().is_empty()
        {
            return Some(v.trim().to_owned());
        }
    }
    None
}

fn flatpak_games(home: &Path) -> Vec<StoreGame> {
    let mut games = Vec::new();
    for install in flatpak_dirs(home) {
        for app in read_dir(&install.join("app")) {
            let id = app.file_name().to_string_lossy().into_owned();
            let lower = id.to_lowercase();
            if FLATPAK_LAUNCHERS.iter().any(|m| lower.contains(m)) {
                continue;
            }
            let Some(desktop) =
                read_text(&install.join(format!("exports/share/applications/{id}.desktop")))
            else {
                continue;
            };
            let is_game = desktop_value(&desktop, "Categories")
                .is_some_and(|c| c.split(';').any(|c| c.trim().eq_ignore_ascii_case("game")));
            let Some(name) = desktop_value(&desktop, "Name").filter(|_| is_game) else {
                continue;
            };
            let files = read_dir(&app.path())
                .into_iter()
                .flat_map(|arch| read_dir(&arch.path()))
                .map(|branch| branch.path().join("active/files"))
                .find(|files| files.exists())
                .and_then(|files| std::fs::canonicalize(files).ok());
            let Some(files) = files else { continue };
            let icon = ["512x512", "256x256", "128x128", "64x64"]
                .iter()
                .map(|size| {
                    files.join(format!(
                        "../export/share/icons/hicolor/{size}/apps/{id}.png"
                    ))
                })
                .find(|p| p.exists());
            games.push(StoreGame {
                id: format!("flatpak-{id}"),
                name,
                dir: Some(files),
                icon,
                exe_names: Vec::new(),
            });
        }
    }
    games
}

// ─── GOG (родной установщик) ────────────────────────────────────────────────

fn gog_games(home: &Path) -> Vec<StoreGame> {
    let mut games = Vec::new();
    for entry in read_dir(&home.join("GOG Games")) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let folder = entry.file_name().to_string_lossy().into_owned();
        let info = read_dir(&dir).into_iter().find_map(|f| {
            let name = f.file_name().to_string_lossy().into_owned();
            (name.starts_with("goggame-")
                && f.path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("info")))
            .then(|| read_json(&f.path()))
            .flatten()
        });
        let info = info.map(|v| v.as_array().and_then(|a| a.first()).cloned().unwrap_or(v));
        let name = info
            .as_ref()
            .and_then(|i| text(i, "name"))
            .unwrap_or_else(|| folder.clone());
        let id = info
            .as_ref()
            .and_then(|i| text(i, "gameId"))
            .unwrap_or(folder);
        let icon = [
            "support/icon.png",
            "support/icon_256.png",
            "support/icon_512.png",
        ]
        .iter()
        .map(|p| dir.join(p))
        .find(|p| p.exists());
        games.push(StoreGame {
            id: format!("gog-{id}"),
            name,
            dir: Some(dir),
            icon,
            exe_names: Vec::new(),
        });
    }
    games
}

// ─── Heroic, Bottles, Lutris ────────────────────────────────────────────────

fn heroic_games(home: &Path) -> Vec<StoreGame> {
    let mut games = Vec::new();
    let mut add =
        |runner: &str, app: &str, name: String, dir: Option<String>, exe: Option<String>| {
            games.push(StoreGame {
                id: format!("heroic-{runner}-{app}"),
                name,
                dir: dir.map(PathBuf::from).filter(|d| d.is_absolute()),
                icon: None,
                exe_names: exe_names_of([exe.as_deref()]),
            });
        };
    for config in [
        home.join(".config/heroic"),
        home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic"),
    ] {
        if let Some(serde_json::Value::Object(epic)) =
            read_json(&config.join("legendaryConfig/legendary/installed.json"))
        {
            for (app, entry) in &epic {
                if let Some(name) = text(entry, "title") {
                    add(
                        "epic",
                        app,
                        name,
                        text(entry, "install_path"),
                        text(entry, "executable"),
                    );
                }
            }
        }
        let titles: std::collections::HashMap<String, String> =
            read_json(&config.join("gog_store/library.json"))
                .and_then(|v| v.get("games").and_then(|g| g.as_array()).cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|g| Some((text(g, "app_name")?, text(g, "title")?)))
                .collect();
        let installed = read_json(&config.join("gog_store/installed.json"))
            .and_then(|v| v.get("installed").and_then(|i| i.as_array()).cloned())
            .unwrap_or_default();
        for entry in installed {
            let Some(app) = text(&entry, "appName") else {
                continue;
            };
            let path = text(&entry, "install_path");
            let info = path
                .as_ref()
                .and_then(|p| read_json(&Path::new(p).join(format!("goggame-{app}.info"))));
            let exe = info.as_ref().and_then(|info| {
                let tasks = info.get("playTasks")?.as_array()?;
                let primary = tasks
                    .iter()
                    .find(|t| t.get("isPrimary").and_then(serde_json::Value::as_bool) == Some(true))
                    .or_else(|| tasks.first())?;
                text(primary, "path")
            });
            let name = titles
                .get(&app)
                .cloned()
                .or_else(|| info.as_ref().and_then(|i| text(i, "name")));
            if let Some(name) = name {
                add("gog", &app, name, path, exe);
            }
        }
        if let Some(sideload) = read_json(&config.join("sideload_apps/library.json"))
            && let Some(list) = sideload.get("games").and_then(|g| g.as_array())
        {
            for entry in list {
                let (Some(app), Some(name)) = (text(entry, "app_name"), text(entry, "title"))
                else {
                    continue;
                };
                let exe = entry.get("install").and_then(|i| text(i, "executable"));
                let dir = exe
                    .as_ref()
                    .map(Path::new)
                    .filter(|p| p.is_absolute())
                    .and_then(Path::parent)
                    .map(|p| p.to_string_lossy().into_owned());
                add("sideload", &app, name, dir, exe);
            }
        }
    }
    games.truncate(MAX_GAMES_PER_SOURCE);
    games
}

/// Простейший YAML (вложенные словари и скаляры) — для `bottle.yml` и
/// конфигов Lutris; списки пропускаются.
pub(crate) fn parse_simple_yaml(text: &str) -> serde_json::Value {
    let mut root = serde_json::Map::new();
    let mut path: Vec<(usize, String)> = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with('-')
            || raw.starts_with("---")
        {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let Some((key, rest)) = yaml_key(trimmed) else {
            continue;
        };
        while path.last().is_some_and(|(i, _)| *i >= indent) {
            path.pop();
        }
        let Some(map) = descend(&mut root, &path) else {
            continue;
        };
        let rest = rest.trim();
        if rest.is_empty() || rest.starts_with('#') {
            map.insert(
                key.clone(),
                serde_json::Value::Object(serde_json::Map::new()),
            );
            path.push((indent, key));
        } else {
            map.insert(key, yaml_scalar(rest));
        }
    }
    serde_json::Value::Object(root)
}

fn descend<'a>(
    map: &'a mut serde_json::Map<String, serde_json::Value>,
    path: &[(usize, String)],
) -> Option<&'a mut serde_json::Map<String, serde_json::Value>> {
    let Some(((_, first), rest)) = path.split_first() else {
        return Some(map);
    };
    match map.get_mut(first)? {
        serde_json::Value::Object(next) => descend(next, rest),
        _ => None,
    }
}

fn unquote(raw: &str, quote: char) -> (String, usize) {
    let mut out = String::new();
    let mut chars = raw.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        if quote == '\'' && c == '\'' && chars.peek().is_some_and(|(_, n)| *n == '\'') {
            out.push('\'');
            chars.next();
        } else if quote == '"' && c == '\\' {
            if let Some((_, n)) = chars.next() {
                out.push(n);
            }
        } else if c == quote {
            return (out, i + 1);
        } else {
            out.push(c);
        }
    }
    (out, raw.len())
}

fn yaml_key(line: &str) -> Option<(String, &str)> {
    if let Some(quote) = line.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let (name, end) = unquote(line, quote);
        return line[end..].strip_prefix(':').map(|rest| (name, rest));
    }
    let colon = line.find(':')?;
    let after = &line[colon + 1..];
    if !(after.is_empty() || after.starts_with(char::is_whitespace)) || colon == 0 {
        return None;
    }
    Some((line[..colon].trim().to_owned(), after))
}

fn yaml_scalar(raw: &str) -> serde_json::Value {
    if raw == "~" || raw == "null" {
        return serde_json::Value::Null;
    }
    if let Some(quote) = raw.chars().next().filter(|c| *c == '"' || *c == '\'') {
        return serde_json::Value::String(unquote(raw, quote).0);
    }
    let value = raw.find(" #").map_or(raw, |i| &raw[..i]).trim();
    serde_json::Value::String(value.to_owned())
}

fn bottles_games(home: &Path) -> Vec<StoreGame> {
    let mut games = Vec::new();
    for root in [
        home.join(".local/share/bottles/bottles"),
        home.join(".var/app/com.usebottles.bottles/data/bottles/bottles"),
    ] {
        for bottle in read_dir(&root) {
            let dir = bottle.path();
            let Some(text) = read_text(&dir.join("bottle.yml")) else {
                continue;
            };
            let yaml = parse_simple_yaml(&text);
            let Some(programs) = yaml
                .get("External_Programs")
                .or_else(|| yaml.get("Programs"))
                .and_then(serde_json::Value::as_object)
            else {
                continue;
            };
            let bottle_name = bottle.file_name().to_string_lossy().into_owned();
            for (key, program) in programs {
                let name = text_or(program, "name").unwrap_or_else(|| key.clone());
                let executable = text_or(program, "executable");
                let program_path = text_or(program, "path");
                let exe_names = exe_names_of([executable.as_deref(), program_path.as_deref()]);
                if exe_names.is_empty() {
                    continue;
                }
                let icon = dir.join("icons").join(format!("{name}.png"));
                games.push(StoreGame {
                    id: format!("bottles-{bottle_name}-{key}"),
                    name,
                    dir: program_path
                        .as_deref()
                        .map(Path::new)
                        .filter(|p| p.is_absolute())
                        .and_then(Path::parent)
                        .map(Path::to_path_buf),
                    icon: icon.exists().then_some(icon),
                    exe_names,
                });
            }
        }
    }
    games.truncate(MAX_GAMES_PER_SOURCE);
    games
}

fn text_or(value: &serde_json::Value, key: &str) -> Option<String> {
    text(value, key)
}

/// «my-cool_game» → «My Cool Game».
pub(crate) fn name_from_slug(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn lutris_games(home: &Path) -> Vec<StoreGame> {
    let icon_dirs: Vec<PathBuf> = [
        home.join(".local/share/icons/hicolor"),
        home.join(".var/app/net.lutris.Lutris/data/icons/hicolor"),
    ]
    .iter()
    .flat_map(|root| ["128x128", "256x256", "64x64", "48x48"].map(|s| root.join(s).join("apps")))
    .collect();
    let mut games = Vec::new();
    for config in [
        home.join(".config/lutris/games"),
        home.join(".var/app/net.lutris.Lutris/config/lutris/games"),
    ] {
        for entry in read_dir(&config) {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = file
                .strip_suffix(".yml")
                .or_else(|| file.strip_suffix(".yaml"))
            else {
                continue;
            };
            let Some((slug, number)) = stem.rsplit_once('-') else {
                continue;
            };
            if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Some(text) = read_text(&entry.path()) else {
                continue;
            };
            let yaml = parse_simple_yaml(&text);
            let Some(game) = yaml.get("game") else {
                continue;
            };
            let exe = text_or(game, "exe");
            let working = text_or(game, "working_dir").filter(|d| Path::new(d).is_absolute());
            let exe_names = exe_names_of([exe.as_deref()]);
            let dir = working.map(PathBuf::from).or_else(|| {
                exe.as_deref()
                    .map(Path::new)
                    .filter(|p| p.is_absolute())
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
            });
            if exe_names.is_empty() && dir.is_none() {
                continue;
            }
            let icon = icon_dirs
                .iter()
                .map(|d| d.join(format!("lutris_{slug}.png")))
                .find(|p| p.exists());
            games.push(StoreGame {
                id: format!("lutris-{slug}"),
                name: text_or(&yaml, "name").unwrap_or_else(|| name_from_slug(slug)),
                dir,
                icon,
                exe_names,
            });
        }
    }
    games.truncate(MAX_GAMES_PER_SOURCE);
    games
}

// ─── Сопоставление ──────────────────────────────────────────────────────────

/// Игра, которой принадлежит процесс: Steam — по команде запуска `reaper`,
/// затем по каталогу; остальные — по каталогу или, для Wine, по имени `.exe`
/// и рабочему каталогу.
pub(crate) fn game_for(info: &ProcessInfo, games: &[StoreGame]) -> Option<StoreGame> {
    if let Some(id) = steam_launch_id(&info.cmdline)
        && let Some(game) = games.iter().find(|g| g.id == id)
    {
        return Some(game.clone());
    }
    let cmdline = normalize(&info.cmdline);
    let wine: Vec<String> = info.win_exes.iter().map(|n| n.to_lowercase()).collect();
    for game in games {
        let launcher = game.id.starts_with("heroic-")
            || game.id.starts_with("bottles-")
            || game.id.starts_with("lutris-");
        if launcher && !wine.is_empty() {
            let named =
                game.exe_names.is_empty() || game.exe_names.iter().any(|n| wine.contains(n));
            if !named {
                continue;
            }
            match &game.dir {
                None if !game.exe_names.is_empty() => return Some(game.clone()),
                Some(dir)
                    if info.cwd.as_deref().is_some_and(|cwd| is_under(cwd, dir))
                        || cmdline.contains(&normalize(&dir.to_string_lossy())) =>
                {
                    return Some(game.clone());
                }
                _ => {}
            }
            continue;
        }
        let Some(dir) = &game.dir else { continue };
        if info.exe.as_deref().is_some_and(|exe| is_under(exe, dir)) {
            return Some(game.clone());
        }
        if (game.id.starts_with("gog-") || launcher)
            && cmdline.contains(&format!("{}/", normalize(&dir.to_string_lossy())))
        {
            return Some(game.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_manifests_and_launch_ids() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n\t\"StateFlags\"\t\t\"4\"\n\t\"installdir\"\t\t\"dota 2 beta\"\n}";
        assert_eq!(
            parse_app_manifest(acf),
            Some((570, "Dota 2".into(), "dota 2 beta".into(), true))
        );
        assert!(is_steam_tool("Proton 9.0", "Proton 9.0"));
        assert_eq!(
            steam_launch_id("/x/reaper SteamLaunch AppId=1091500 -- /x/proton waitforexitandrun")
                .as_deref(),
            Some("steam-1091500")
        );
        let lib = "\"libraryfolders\" { \"0\" { \"path\" \"/mnt/g\\\\ames\" } }";
        assert_eq!(
            vdf_values(lib, "path").collect::<Vec<_>>(),
            vec!["/mnt/g\\ames".to_owned()]
        );
    }

    #[test]
    fn yaml_and_launcher_matching() {
        let yaml = parse_simple_yaml(
            "name: My Game\ngame:\n  exe: /g/x/Game.exe\n  working_dir: '/g/x'\nsystem: {}\n",
        );
        assert_eq!(yaml["game"]["exe"], "/g/x/Game.exe");
        assert_eq!(yaml["game"]["working_dir"], "/g/x");
        assert_eq!(name_from_slug("hollow-knight_silk"), "Hollow Knight Silk");
        let games = vec![StoreGame {
            id: "lutris-game".into(),
            name: "Game".into(),
            dir: Some(PathBuf::from("/g/x")),
            icon: None,
            exe_names: vec!["game.exe".into()],
        }];
        let info = ProcessInfo {
            name: "wine64-preloader".into(),
            win_exes: vec!["Game.exe".into()],
            cwd: Some("/g/x".into()),
            ..ProcessInfo::default()
        };
        assert_eq!(
            game_for(&info, &games).map(|g| g.id),
            Some("lutris-game".into())
        );
        let other = ProcessInfo {
            win_exes: vec!["Other.exe".into()],
            ..info
        };
        assert!(game_for(&other, &games).is_none());
    }
}
