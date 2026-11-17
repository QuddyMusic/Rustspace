//! Активность (Rich Presence): во что играет и что слушает пользователь.
//!
//! Порт PR #482 оригинального Backspace — локальный IPC-сервер Rich
//! Presence, распознавание игр и медиа. Авторы исходной реализации:
//! QuddyMusic и cirzear (часть кода — Z.ai). Здесь всё на Rust: обход
//! `/proc`, магазины, D-Bus (MPRIS), разбор `.exe`, IPC на сокетах.
//!
//! Слои: найденные игры (опрос раз в 15 секунд) → пуш игры по IPC (богаче:
//! карта, режим, обложка) подменяет свою игру → играющий трек → выбор
//! игрока (свои игры, скрытые, порядок, переименования) → иконки.

pub(crate) mod art;
pub(crate) mod games;
pub(crate) mod ipc;
pub(crate) mod media;
pub(crate) mod procs;
pub(crate) mod stores;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use bs_types::{Activity, ActivityAssets, ActivityTimestamps, ActivityType};
use serde::{Deserialize, Serialize};

use games::{Binding, Detected, Dictionary};

/// Опрос процессов.
const SCAN_INTERVAL: Duration = Duration::from_secs(15);
/// Опрос плеера.
const MEDIA_INTERVAL: Duration = Duration::from_secs(5);
/// Задержка отправки после смены игр и после смены трека.
const GAME_DELAY: Duration = Duration::from_secs(5);
const MEDIA_DELAY: Duration = Duration::from_millis(500);
/// Сервер принимает не чаще раза в 3 секунды (снятие — в любой момент).
const SERVER_INTERVAL: Duration = Duration::from_millis(3100);
/// Загрузка иконки обновляется раз в 30 дней: сервер хранит год.
const ICON_RENEW_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const ICON_RETRY: Duration = Duration::from_secs(60);
/// Своих игр и запомненных выборов не больше этого.
const MAX_MANUAL: usize = 30;
const MAX_LIST: usize = 200;

/// Игра, добавленная вручную: с программой — видна, пока она запущена;
/// без программы — видна всегда.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ManualGame {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) process: Option<String>,
}

/// Настройки активности (только на этом устройстве).
#[allow(clippy::struct_excessive_bools, reason = "независимые переключатели")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ActivityPrefs {
    /// Искать запущенные игры.
    pub(crate) games: bool,
    /// Показывать играющую музыку и видео.
    pub(crate) media: bool,
    /// Принимать Rich Presence от игр (локальный IPC).
    pub(crate) rich_presence: bool,
    /// Отправлять иконки игр на свой инстанс.
    pub(crate) icons: bool,
    pub(crate) manual: Vec<ManualGame>,
    /// Ключи (`trim().to_lowercase()`) скрытых игр.
    pub(crate) hidden: Vec<String>,
    /// Ключи игр в порядке важности.
    pub(crate) order: Vec<String>,
    /// Ключ найденного имени → своё имя.
    pub(crate) renames: BTreeMap<String, String>,
}

impl Default for ActivityPrefs {
    fn default() -> Self {
        Self {
            games: true,
            media: true,
            rich_presence: true,
            icons: true,
            manual: Vec::new(),
            hidden: Vec::new(),
            order: Vec::new(),
            renames: BTreeMap::new(),
        }
    }
}

/// Ключ имени для выборов игрока.
pub(crate) fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Имя без управляющих знаков, не длиннее 128.
pub(crate) fn clean_name(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned: String = cleaned.chars().take(128).collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

impl ActivityPrefs {
    /// Привязки своих игр к программам.
    fn bindings(&self) -> Vec<Binding> {
        self.manual
            .iter()
            .filter_map(|g| {
                Some(Binding {
                    name: g.name.clone(),
                    process: g.process.clone()?,
                })
            })
            .collect()
    }

    /// Добавить свою игру (заменяя одноимённую).
    pub(crate) fn add_manual(&mut self, name: &str, process: Option<&str>) {
        let Some(name) = clean_name(name) else { return };
        let k = key(&name);
        self.manual.retain(|g| key(&g.name) != k);
        let process = process
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| p.chars().take(256).collect());
        self.manual.push(ManualGame { name, process });
        let extra = self.manual.len().saturating_sub(MAX_MANUAL);
        self.manual.drain(..extra);
    }

    /// Скрыть или вернуть игру.
    pub(crate) fn set_hidden(&mut self, name: &str, hide: bool) {
        let k = key(name);
        self.hidden.retain(|h| *h != k);
        if hide && !k.is_empty() {
            self.hidden.push(k);
            let extra = self.hidden.len().saturating_sub(MAX_LIST);
            self.hidden.drain(..extra);
        }
    }

    /// Своё имя для найденной игры (`None` — вернуть её собственное).
    pub(crate) fn set_rename(&mut self, detected: &str, chosen: Option<&str>) {
        let k = key(detected);
        self.renames.remove(&k);
        if let Some(name) = chosen.and_then(clean_name)
            && name != detected.trim()
            && !k.is_empty()
        {
            self.renames.insert(k, name);
        }
        while self.renames.len() > MAX_LIST {
            let Some(first) = self.renames.keys().next().cloned() else {
                break;
            };
            self.renames.remove(&first);
        }
    }

    /// Применить выбор игрока к найденному: свои игры без программы,
    /// без скрытых, в своём порядке, со своими именами.
    pub(crate) fn apply(&self, list: Vec<Activity>) -> Vec<Activity> {
        let playing = |a: &Activity| a.activity_type == ActivityType::Playing;
        let present: Vec<String> = list
            .iter()
            .filter(|a| playing(a))
            .map(|a| key(&a.name))
            .collect();
        let mut out: Vec<Activity> = self
            .manual
            .iter()
            .filter(|g| g.process.is_none() && !present.contains(&key(&g.name)))
            .map(|g| plain(ActivityType::Playing, &g.name, None))
            .collect();
        out.extend(list);
        out.retain(|a| {
            !playing(a)
                || !(self.hidden.contains(&key(&a.name))
                    || self
                        .renames
                        .get(&key(&a.name))
                        .is_some_and(|r| self.hidden.contains(&key(r))))
        });
        if !self.order.is_empty() {
            let rank = |a: &Activity| {
                let shown = self
                    .renames
                    .get(&key(&a.name))
                    .map_or_else(|| key(&a.name), |r| key(r));
                self.order
                    .iter()
                    .position(|k| *k == key(&a.name) || *k == shown)
                    .unwrap_or(usize::MAX)
            };
            let mut games: Vec<Activity> = out.iter().filter(|a| playing(a)).cloned().collect();
            games.sort_by_key(rank);
            let mut games = games.into_iter();
            for slot in &mut out {
                if playing(slot)
                    && let Some(next) = games.next()
                {
                    *slot = next;
                }
            }
        }
        for activity in &mut out {
            if playing(activity)
                && let Some(chosen) = self.renames.get(&key(&activity.name))
            {
                activity.name.clone_from(chosen);
            }
        }
        out.truncate(games::MAX_GAMES);
        out
    }
}

fn plain(kind: ActivityType, name: &str, start: Option<i64>) -> Activity {
    Activity {
        activity_type: kind,
        name: name.to_owned(),
        details: None,
        state: None,
        timestamps: start.map(|start| ActivityTimestamps {
            start: Some(start),
            end: None,
        }),
        playback: None,
        assets: None,
        url: None,
    }
}

fn kind_of(kind: &str) -> ActivityType {
    match kind {
        "listening" => ActivityType::Listening,
        "watching" => ActivityType::Watching,
        "streaming" => ActivityType::Streaming,
        _ => ActivityType::Playing,
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Иконка: ссылка на инстансе и когда её загружали.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct IconRecord {
    url: String,
    at: i64,
}

/// Состояние движка.
#[derive(Default)]
struct State {
    dictionary: Dictionary,
    games: Vec<Detected>,
    /// Когда каждая игра (по id) впервые замечена.
    starts: HashMap<String, i64>,
    ipc: Option<ipc::IpcPush>,
    ipc_sockets: usize,
    media: Option<(media::Track, media::Timeline)>,
    unknown_wine: Vec<String>,
    programs: Vec<String>,
    /// Найденные локально иконки по имени игры (PNG).
    local_icons: HashMap<String, Option<Vec<u8>>>,
    /// Загруженные иконки: адрес инстанса → имя → запись.
    uploaded: HashMap<String, HashMap<String, IconRecord>>,
    uploading: std::collections::HashSet<String>,
    icon_misses: HashMap<String, Instant>,
    /// Что и когда отправили; по номерам сессий.
    sent: HashMap<usize, (String, Instant)>,
    /// С какого момента ждёт отправки изменение.
    pending: Option<(String, Instant, bool)>,
}

struct Engine {
    state: Mutex<State>,
    rescan: tokio::sync::Notify,
    ipc_server: Mutex<Option<ipc::Server>>,
    ipc_starting: std::sync::atomic::AtomicBool,
}

static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

fn lock(engine: &Engine) -> MutexGuard<'_, State> {
    engine.state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn icons_path() -> Option<std::path::PathBuf> {
    crate::saved_session::config_dir().map(|d| d.join("game-icons.json"))
}

/// Запустить движок (один раз).
fn engine(runtime: &tokio::runtime::Handle, ctx: &egui::Context) -> Arc<Engine> {
    ENGINE
        .get_or_init(|| {
            let uploaded = icons_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
            let engine = Arc::new(Engine {
                state: Mutex::new(State {
                    dictionary: games::load(),
                    uploaded,
                    ..State::default()
                }),
                rescan: tokio::sync::Notify::new(),
                ipc_server: Mutex::new(None),
                ipc_starting: std::sync::atomic::AtomicBool::new(false),
            });
            spawn_scanner(runtime, ctx, &engine);
            spawn_media(ctx, &engine);
            engine
        })
        .clone()
}

fn spawn_scanner(runtime: &tokio::runtime::Handle, ctx: &egui::Context, engine: &Arc<Engine>) {
    let (engine, ctx) = (engine.clone(), ctx.clone());
    runtime.spawn(async move {
        let http = reqwest::Client::new();
        let stores = Arc::new(stores::Stores::default());
        let mut synced: Option<Instant> = None;
        loop {
            let prefs = crate::prefs::get().activity;
            if synced.is_none_or(|at| at.elapsed() > Duration::from_secs(24 * 60 * 60)) {
                synced = Some(Instant::now());
                let version = lock(&engine).dictionary.version;
                if let Some(fresh) = games::sync(&http, version).await {
                    lock(&engine).dictionary = fresh;
                }
            }
            let dictionary = lock(&engine).dictionary.clone();
            let scan_stores = stores.clone();
            let scanned = tokio::task::spawn_blocking(move || {
                let infos = procs::list();
                let found = if prefs.games {
                    games::detect(&infos, &dictionary, &scan_stores.games(), &prefs.bindings())
                } else {
                    Vec::new()
                };
                let unknown = games::unknown_wine_programs(&infos, &found);
                let programs = games::running_programs(&infos);
                (found, unknown, programs)
            })
            .await;
            if let Ok((found, unknown, programs)) = scanned {
                apply_scan(&engine, found, unknown, programs);
                fetch_local_icons(&engine, &ctx);
                ctx.request_repaint();
            }
            tokio::select! {
                () = tokio::time::sleep(SCAN_INTERVAL) => {}
                () = engine.rescan.notified() => {}
            }
        }
    });
}

fn apply_scan(engine: &Engine, found: Vec<Detected>, unknown: Vec<String>, programs: Vec<String>) {
    let now = now_ms();
    let mut state = lock(engine);
    state
        .starts
        .retain(|id, _| found.iter().any(|g| g.id == *id));
    for game in &found {
        state.starts.entry(game.id.clone()).or_insert(now);
    }
    state.games = found;
    state.unknown_wine = unknown;
    state.programs = programs;
}

/// Найти иконки новых игр на диске (в фоне).
fn fetch_local_icons(engine: &Arc<Engine>, ctx: &egui::Context) {
    let missing: Vec<Detected> = {
        let state = lock(engine);
        state
            .games
            .iter()
            .filter(|g| g.kind == "playing" && !state.local_icons.contains_key(&g.name))
            .cloned()
            .collect()
    };
    for game in missing {
        lock(engine).local_icons.insert(game.name.clone(), None);
        let (engine, ctx) = (engine.clone(), ctx.clone());
        std::thread::spawn(move || {
            let icon = art::icon_for(&game);
            if icon.is_some() {
                lock(&engine).local_icons.insert(game.name.clone(), icon);
                ctx.request_repaint();
            }
        });
    }
}

/// Опрос плеера в своём потоке (D-Bus блокирующий).
fn spawn_media(ctx: &egui::Context, engine: &Arc<Engine>) {
    let (engine, ctx) = (engine.clone(), ctx.clone());
    std::thread::Builder::new()
        .name("rustspace-media".into())
        .spawn(move || {
            #[cfg(all(target_os = "linux", feature = "presence"))]
            let mut connection: Option<zbus::blocking::Connection> = None;
            loop {
                let enabled = crate::prefs::get().activity.media;
                #[cfg(all(target_os = "linux", feature = "presence"))]
                let track = if enabled {
                    if connection.is_none() {
                        connection = zbus::blocking::Connection::session().ok();
                    }
                    connection.as_ref().and_then(media::mpris::poll)
                } else {
                    None
                };
                #[cfg(not(all(target_os = "linux", feature = "presence")))]
                let track: Option<media::Track> = {
                    let _ = enabled;
                    None
                };
                let now = now_ms();
                let mut state = lock(&engine);
                let next = track.map(|track| {
                    let timeline =
                        media::next_timeline(state.media.as_ref().map(|(_, t)| t), &track, now);
                    (track, timeline)
                });
                let changed = state
                    .media
                    .as_ref()
                    .map(|(t, l)| (&t.title, &t.artist, &t.player, l.start))
                    != next
                        .as_ref()
                        .map(|(t, l)| (&t.title, &t.artist, &t.player, l.start));
                state.media = next;
                drop(state);
                if changed {
                    ctx.request_repaint();
                }
                std::thread::sleep(MEDIA_INTERVAL);
            }
        })
        .ok();
}

/// Включить или выключить IPC-сервер по настройке.
fn ensure_ipc(
    engine: &Arc<Engine>,
    runtime: &tokio::runtime::Handle,
    ctx: &egui::Context,
    wanted: bool,
) {
    let running = engine
        .ipc_server
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some();
    if !wanted && running {
        engine
            .ipc_server
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let mut state = lock(engine);
        state.ipc = None;
        state.ipc_sockets = 0;
        drop(state);
        return;
    }
    if !wanted
        || running
        || engine
            .ipc_starting
            .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    let (engine, ctx) = (engine.clone(), ctx.clone());
    runtime.spawn(async move {
        let (out, mut pushes) = tokio::sync::watch::channel(None);
        let server = ipc::start(out).await;
        let sockets = server.as_ref().map_or(0, |s| s.sockets);
        if server.is_none() {
            tracing::info!("Rich Presence: свободных мест для IPC нет");
        }
        *engine
            .ipc_server
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = server;
        lock(&engine).ipc_sockets = sockets;
        engine
            .ipc_starting
            .store(false, std::sync::atomic::Ordering::SeqCst);
        ctx.request_repaint();
        while pushes.changed().await.is_ok() {
            let push = pushes.borrow_and_update().clone();
            let mut state = lock(&engine);
            state.ipc = push;
            drop(state);
            ctx.request_repaint();
        }
    });
}

/// Имена клиентов, которые шлют Rich Presence без своего имени.
fn known_client(id: &str) -> Option<&'static str> {
    match id {
        "1216669957799018608" => Some("osu!lazer"),
        _ => None,
    }
}

impl State {
    /// Имя игры из пуша: своё, по процессу, по известному клиенту.
    fn ipc_name(&self, push: &ipc::IpcPush) -> String {
        push.activity
            .name
            .clone()
            .or_else(|| {
                push.pid
                    .and_then(|pid| games::name_for_pid(&self.dictionary, pid))
            })
            .or_else(|| known_client(&push.client_id).map(str::to_owned))
            .unwrap_or_else(|| "Unknown game".to_owned())
    }

    /// Активности до выбора игрока.
    fn detected(&self, now: i64) -> Vec<Activity> {
        // Плееры из словаря — не активность: что играет, скажет сам плеер.
        let mut list: Vec<Activity> = self
            .games
            .iter()
            .filter(|g| g.kind != "listening" && g.kind != "watching")
            .map(|g| plain(kind_of(&g.kind), &g.name, self.starts.get(&g.id).copied()))
            .collect();
        if let Some(push) = &self.ipc {
            let a = &push.activity;
            let name = self.ipc_name(push);
            let assets = (a.large_image.is_some()
                || a.large_text.is_some()
                || a.small_image.is_some()
                || a.small_text.is_some())
            .then(|| ActivityAssets {
                large_image: a.large_image.clone(),
                large_text: a.large_text.clone(),
                small_image: a.small_image.clone(),
                small_text: a.small_text.clone(),
            });
            let kind = kind_of(&a.kind);
            let pushed = Activity {
                activity_type: kind,
                name: name.clone(),
                details: a.details.clone(),
                state: a.state.clone(),
                timestamps: (a.start.is_some() || a.end.is_some()).then_some(ActivityTimestamps {
                    start: a.start,
                    end: a.end,
                }),
                playback: None,
                assets,
                url: None,
            };
            let k = games::name_key(&name);
            match list
                .iter()
                .position(|x| x.activity_type == kind && games::name_key(&x.name) == k)
            {
                Some(at) => list[at] = pushed,
                None => list.insert(0, pushed),
            }
        }
        list.truncate(games::MAX_GAMES);
        if let Some((track, timeline)) = &self.media {
            list.push(media::to_activity(track, timeline.start, now));
        }
        list
    }
}

/// Строка для сравнения: без позиции трека, которая растёт сама.
fn fingerprint(list: &[Activity]) -> String {
    let mut list = list.to_vec();
    for activity in &mut list {
        if let Some(playback) = &mut activity.playback {
            playback.position_ms = 0;
        }
    }
    serde_json::to_string(&list).unwrap_or_default()
}

/// Сводка для страницы настроек.
#[derive(Debug, Clone, Default)]
pub(crate) struct Overview {
    /// Найденные игры: имя и откуда.
    pub(crate) games: Vec<(String, String)>,
    pub(crate) ipc_sockets: usize,
    pub(crate) ipc_game: Option<String>,
    pub(crate) unknown_wine: Vec<String>,
    pub(crate) programs: Vec<String>,
    /// Что видят другие сейчас.
    pub(crate) shown: Vec<Activity>,
}

/// Что сейчас найдено (для настроек).
pub(crate) fn overview() -> Overview {
    let Some(engine) = ENGINE.get() else {
        return Overview::default();
    };
    let prefs = crate::prefs::get().activity;
    let state = lock(engine);
    Overview {
        games: state
            .games
            .iter()
            .map(|g| {
                let source = g.store.as_ref().map_or_else(
                    || {
                        if g.id.starts_with("manual:") {
                            "manual"
                        } else {
                            "dictionary"
                        }
                    },
                    |s| s.id.split('-').next().unwrap_or("store"),
                );
                (g.name.clone(), source.to_owned())
            })
            .collect(),
        ipc_sockets: state.ipc_sockets,
        ipc_game: state.ipc.as_ref().map(|p| state.ipc_name(p)),
        unknown_wine: state.unknown_wine.clone(),
        programs: state.programs.clone(),
        shown: prefs.apply(state.detected(now_ms())),
    }
}

/// Пересканировать сейчас (после правки своих игр).
pub(crate) fn rescan() {
    if let Some(engine) = ENGINE.get() {
        engine.rescan.notify_one();
    }
}

/// Каждый кадр: запустить движок, собрать активность и разослать её
/// всем подключённым инстансам (с задержкой и с учётом ограничителя).
pub(crate) fn tick(ctx: &egui::Context, bridge: &mut crate::fleet::Bridge) {
    let runtime = bridge.runtime();
    let engine = engine(&runtime, ctx);
    take_uploads(&engine, bridge);
    let prefs = crate::prefs::get().activity;
    ensure_ipc(&engine, &runtime, ctx, prefs.rich_presence);
    let live = bridge.live_origins();
    let now = now_ms();
    let mut state = lock(&engine);
    let mut list = prefs.apply(state.detected(now));
    let home = bridge.home().map(|(base, _)| base);
    if let Some(home) = &home {
        attach_icons(&state, home, &mut list);
    }
    let print = fingerprint(&list);
    let media_changed = {
        let media = |l: &[Activity]| {
            fingerprint(
                &l.iter()
                    .filter(|a| a.activity_type != ActivityType::Playing)
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        };
        state.sent.values().next().is_none_or(|(sent, _)| {
            let old: Vec<Activity> = serde_json::from_str(sent).unwrap_or_default();
            media(&old) != media(&list)
        })
    };
    let mut next_wake: Option<Duration> = None;
    for index in live.clone() {
        let (sent, at) = state.sent.get(&index).cloned().unwrap_or_else(|| {
            (
                "\u{0}".to_owned(),
                Instant::now()
                    .checked_sub(SERVER_INTERVAL)
                    .unwrap_or_else(Instant::now),
            )
        });
        if sent == print {
            continue;
        }
        let first = sent == "\u{0}";
        let pending_since = match &state.pending {
            Some((p, since, _)) if *p == print => *since,
            _ => {
                state.pending = Some((print.clone(), Instant::now(), media_changed));
                Instant::now()
            }
        };
        let old: Vec<Activity> = serde_json::from_str(&sent).unwrap_or_default();
        let removing = list.len() < old.len();
        let delay = if first {
            Duration::ZERO
        } else if media_changed {
            MEDIA_DELAY
        } else {
            GAME_DELAY
        };
        let waited = pending_since.elapsed();
        let since_last = at.elapsed();
        if waited >= delay && (removing || since_last >= SERVER_INTERVAL) {
            if let Some(session) = bridge.session_at(index) {
                session.send(bs_types::ClientEvent::ActivityUpdate {
                    activities: list.clone(),
                });
            }
            state.sent.insert(index, (print.clone(), Instant::now()));
        } else {
            let left = delay.saturating_sub(waited).max(if removing {
                Duration::ZERO
            } else {
                SERVER_INTERVAL.saturating_sub(since_last)
            });
            next_wake = Some(next_wake.map_or(left, |w| w.min(left)));
        }
    }
    state.sent.retain(|index, _| live.contains(index));
    // Прогресс трека в настройках и пробуждение к следующей отправке.
    if let Some(wake) = next_wake {
        ctx.request_repaint_after(wake + Duration::from_millis(50));
    }
    let wanted = upload_candidates(&mut state, &prefs, home.as_deref(), &list);
    drop(state);
    for (name, bytes) in wanted {
        upload_icon(bridge, name, bytes);
    }
}

/// Прикрепить загруженные иконки к играм без своей картинки.
fn attach_icons(state: &State, home: &str, list: &mut [Activity]) {
    let Some(known) = state.uploaded.get(home) else {
        return;
    };
    for activity in list
        .iter_mut()
        .filter(|a| a.activity_type == ActivityType::Playing)
    {
        if activity
            .assets
            .as_ref()
            .is_some_and(|a| a.large_image.is_some())
        {
            continue;
        }
        if let Some(record) = known.get(&activity.name) {
            activity
                .assets
                .get_or_insert_with(ActivityAssets::default)
                .large_image = Some(record.url.clone());
        }
    }
}

/// Какие иконки пора загрузить (новые или старше 30 дней).
fn upload_candidates(
    state: &mut State,
    prefs: &ActivityPrefs,
    home: Option<&str>,
    list: &[Activity],
) -> Vec<(String, Vec<u8>)> {
    let Some(home) = home.filter(|_| prefs.icons) else {
        return Vec::new();
    };
    let now = now_ms();
    let mut out = Vec::new();
    for activity in list
        .iter()
        .filter(|a| a.activity_type == ActivityType::Playing)
    {
        let name = &activity.name;
        // Переименованная игра: иконка ищется по найденному имени.
        let detected = state
            .games
            .iter()
            .find(|g| prefs.renames.get(&key(&g.name)).is_some_and(|r| r == name))
            .map_or_else(|| name.clone(), |g| g.name.clone());
        let fresh = state
            .uploaded
            .get(home)
            .and_then(|m| m.get(name))
            .is_some_and(|r| now - r.at < ICON_RENEW_MS);
        if fresh
            || state.uploading.contains(name)
            || state
                .icon_misses
                .get(name)
                .is_some_and(|at| at.elapsed() < ICON_RETRY)
        {
            continue;
        }
        if activity
            .assets
            .as_ref()
            .is_some_and(|a| a.large_image.is_some())
            && !state
                .uploaded
                .get(home)
                .is_some_and(|m| m.contains_key(name))
        {
            continue;
        }
        if let Some(Some(bytes)) = state.local_icons.get(&detected) {
            state.uploading.insert(name.clone());
            out.push((name.clone(), bytes.clone()));
        }
    }
    out
}

/// Загрузка идёт через домашнюю сессию; итог забирается на следующем кадре.
fn upload_icon(bridge: &mut crate::fleet::Bridge, name: String, image: Vec<u8>) {
    if let Some(session) = bridge.session_at_mut(0) {
        session.request(crate::bridge::BridgeRequest::Extras(
            crate::bridge::extras::ExtrasOp::GameIcon { name, image },
        ));
    }
}

/// Итоги загрузок иконок с домашней сессии.
fn take_uploads(engine: &Engine, bridge: &mut crate::fleet::Bridge) {
    let Some((home, _)) = bridge.home() else {
        return;
    };
    let Some(session) = bridge.session_at_mut(0) else {
        return;
    };
    let done = std::mem::take(&mut session.extras_mut().game_icons);
    if done.is_empty() {
        return;
    }
    let mut state = lock(engine);
    for (name, result) in done {
        state.uploading.remove(&name);
        match result {
            Ok(url) => {
                state
                    .uploaded
                    .entry(home.clone())
                    .or_default()
                    .insert(name, IconRecord { url, at: now_ms() });
            }
            Err(error) => {
                tracing::debug!(%error, "иконка игры не загружена");
                state.icon_misses.insert(name, Instant::now());
            }
        }
    }
    if let Some(path) = icons_path()
        && let Ok(text) = serde_json::to_string(&state.uploaded)
    {
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_add_hide_order_and_rename() {
        let mut prefs = ActivityPrefs::default();
        prefs.add_manual("  My\tGame ", None);
        prefs.add_manual("Bound", Some("bound.exe"));
        prefs.set_hidden("Dota 2", true);
        prefs.set_rename("Counter-Strike 2", Some("CS"));
        prefs.order = vec![key("CS"), key("My Game")];
        let found = vec![
            plain(ActivityType::Playing, "Dota 2", Some(1)),
            plain(ActivityType::Listening, "Song", None),
            plain(ActivityType::Playing, "Counter-Strike 2", Some(2)),
        ];
        let shown: Vec<String> = prefs.apply(found).into_iter().map(|a| a.name).collect();
        assert_eq!(shown, ["CS", "Song", "My Game"]);
        prefs.set_rename("Counter-Strike 2", Some("Counter-Strike 2"));
        assert!(prefs.renames.is_empty());
    }

    #[test]
    fn ipc_pushes_replace_their_scanned_game() {
        let mut state = State::default();
        state.games.push(Detected {
            id: "osu".into(),
            name: "osu!".into(),
            kind: "playing".into(),
            info: None,
            store: None,
        });
        state.starts.insert("osu".into(), 5);
        state.ipc = Some(ipc::IpcPush {
            client_id: "1".into(),
            pid: None,
            activity: ipc::IpcActivity {
                kind: "playing".into(),
                name: Some("OSU!".into()),
                details: Some("Map".into()),
                ..ipc::IpcActivity::default()
            },
        });
        let list = state.detected(10);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].details.as_deref(), Some("Map"));
        assert_eq!(fingerprint(&list), fingerprint(&state.detected(99)));
    }
}
