//! Настройки клиента, сохраняемые на диск: масштаб, оформление, голос,
//! звуки, уведомления, горячие клавиши, язык.
//!
//! Файл `prefs.json` лежит в каталоге настроек приложения рядом с
//! `session.json`. Пишется атомарно (временный файл + rename) и не чаще раза
//! в секунду: изменения копятся в памяти, [`flush`] сохраняет их, когда
//! прошла пауза, и при выходе. Неизвестные или битые поля заменяются
//! значениями по умолчанию — старый файл никогда не мешает запуску.
//!
//! Хранилище глобальное: настройки читаются из многих экранов, а egui-кадр
//! однопоточный, так что блокировка не спорная.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::i18n::tr;
use crate::theme::Appearance;
use crate::voice_media::VoicePrefs;

/// Имя файла в каталоге настроек.
const FILE_NAME: &str = "prefs.json";
/// Пауза перед записью после последнего изменения.
const SAVE_DELAY: Duration = Duration::from_secs(1);

/// Звуки интерфейса (как `soundEffectVolume`/`messageSoundAllChannels` оригинала).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SoundPrefs {
    /// Громкость звуков, 0–200 (100 — как записано).
    pub(crate) volume: u16,
    /// Звук сообщения во всех каналах, а не только в ЛС и упоминаниях.
    pub(crate) message_all_channels: bool,
}

impl Default for SoundPrefs {
    fn default() -> Self {
        Self {
            volume: 100,
            message_all_channels: false,
        }
    }
}

/// Системные уведомления рабочего стола.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "независимые переключатели настроек"
)]
pub(crate) struct DesktopPrefs {
    /// Показывать системные уведомления о новых сообщениях.
    pub(crate) notifications: bool,
    /// Показывать текст сообщения в уведомлении.
    pub(crate) previews: bool,
    /// Только когда окно не в фокусе.
    pub(crate) only_unfocused: bool,
    /// Горячие клавиши работают и вне окна (если сборка и система позволяют).
    pub(crate) global_hotkeys: bool,
    /// Закрытие окна прячет его в трей (как десктоп оригинала).
    pub(crate) close_to_tray: bool,
}

impl Default for DesktopPrefs {
    fn default() -> Self {
        Self {
            notifications: true,
            previews: true,
            only_unfocused: true,
            global_hotkeys: true,
            close_to_tray: true,
        }
    }
}

/// Действия, которым можно назначить сочетание клавиш (как
/// `BINDABLE_ACTIONS` оригинала).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Action {
    /// Переключить микрофон.
    ToggleMute,
    /// Переключить звук.
    ToggleDeafen,
    /// Говорить, пока клавиша зажата.
    PushToTalk,
    /// Переключить камеру.
    ToggleCamera,
    /// Начать или остановить показ экрана.
    ToggleScreenShare,
    /// Выйти из голосового канала.
    Disconnect,
}

impl Action {
    /// Все действия в порядке показа.
    pub(crate) const ALL: [Self; 6] = [
        Self::ToggleMute,
        Self::ToggleDeafen,
        Self::PushToTalk,
        Self::ToggleCamera,
        Self::ToggleScreenShare,
        Self::Disconnect,
    ];

    /// Подпись в настройках.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::ToggleMute => tr("Вкл/выкл микрофон"),
            Self::ToggleDeafen => tr("Вкл/выкл звук"),
            Self::PushToTalk => tr("Рация (говорить, пока зажато)"),
            Self::ToggleCamera => tr("Вкл/выкл камеру"),
            Self::ToggleScreenShare => tr("Показ экрана"),
            Self::Disconnect => tr("Выйти из голосового канала"),
        }
    }
}

/// Сочетание клавиш: модификаторы и клавиша `egui`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Keybind {
    /// Ctrl (Cmd на macOS).
    #[serde(default)]
    pub(crate) command: bool,
    /// Shift.
    #[serde(default)]
    pub(crate) shift: bool,
    /// Alt.
    #[serde(default)]
    pub(crate) alt: bool,
    /// Клавиша.
    pub(crate) key: egui::Key,
}

impl Keybind {
    /// Совпадают ли модификаторы с зажатыми сейчас.
    pub(crate) fn modifiers_match(self, modifiers: egui::Modifiers) -> bool {
        modifiers.command == self.command
            && modifiers.shift == self.shift
            && modifiers.alt == self.alt
    }

    /// Человеческая подпись: «Ctrl+Shift+M».
    pub(crate) fn label(self) -> String {
        let mut out = String::new();
        if self.command {
            out.push_str(if cfg!(target_os = "macos") {
                "⌘+"
            } else {
                "Ctrl+"
            });
        }
        if self.shift {
            out.push_str("Shift+");
        }
        if self.alt {
            out.push_str("Alt+");
        }
        out.push_str(self.key.name());
        out
    }
}

/// Все сохраняемые настройки.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Prefs {
    /// Масштаб интерфейса (zoom factor `egui`).
    pub(crate) scale: f32,
    /// Оформление.
    pub(crate) appearance: Appearance,
    /// Голос: устройства и обработка звука.
    pub(crate) voice: VoicePrefs,
    /// Звуки интерфейса.
    pub(crate) sounds: SoundPrefs,
    /// Системные уведомления.
    pub(crate) desktop: DesktopPrefs,
    /// Горячие клавиши.
    pub(crate) keybinds: std::collections::BTreeMap<Action, Keybind>,
    /// Язык интерфейса (код каталога: `ru`, `en`, …).
    pub(crate) language: String,
    /// Обновления клиента.
    pub(crate) updates: crate::updates::UpdatePrefs,
    /// Какую версию домашнего инстанса видел пользователь на странице
    /// «Backspace»: ключ — `id@адрес`.
    pub(crate) hub_seen: std::collections::BTreeMap<String, String>,
    /// Инстансы, в которые входили, — сначала последний.
    pub(crate) recent_instances: Vec<RecentInstance>,
    /// Личные переписки, где закрыли предупреждение «не зашифровано».
    pub(crate) plaintext_dismissed: std::collections::BTreeSet<String>,
    /// Каналы с деликатным контентом, в которые согласились войти.
    #[serde(default)]
    pub(crate) nsfw_allowed: std::collections::BTreeSet<String>,
    /// Перевод сообщений.
    #[serde(default)]
    pub(crate) translate: crate::translate::TranslatePrefs,
    /// Активность: игры, музыка, Rich Presence.
    #[serde(default)]
    pub(crate) activity: crate::presence::ActivityPrefs,
    /// Ширина левой и правой колонок.
    #[serde(default)]
    pub(crate) columns: crate::columns::Widths,
    /// Картинки превью ссылок грузятся с сайтов напрямую (сайт видит IP).
    pub(crate) preview_images: bool,
}

/// Инстанс, в который входили с этого устройства.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecentInstance {
    /// Адрес.
    pub(crate) base: String,
    /// Вход шёл по открытому `http://` с разрешения пользователя.
    #[serde(default)]
    pub(crate) insecure: bool,
}

/// Сколько недавних инстансов помнить.
pub(crate) const RECENT_INSTANCES: usize = 6;

/// Ставит инстанс первым в списке недавних (без повторов, не длиннее
/// [`RECENT_INSTANCES`]).
pub(crate) fn push_recent(list: &mut Vec<RecentInstance>, base: &str, insecure: bool) {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return;
    }
    list.retain(|item| !item.base.eq_ignore_ascii_case(base));
    list.insert(
        0,
        RecentInstance {
            base: base.to_owned(),
            insecure,
        },
    );
    list.truncate(RECENT_INSTANCES);
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            scale: 1.0,
            appearance: Appearance::default(),
            voice: VoicePrefs::default(),
            sounds: SoundPrefs::default(),
            desktop: DesktopPrefs::default(),
            keybinds: std::collections::BTreeMap::new(),
            language: "ru".to_owned(),
            updates: crate::updates::UpdatePrefs::default(),
            hub_seen: std::collections::BTreeMap::new(),
            plaintext_dismissed: std::collections::BTreeSet::new(),
            nsfw_allowed: std::collections::BTreeSet::new(),
            translate: crate::translate::TranslatePrefs::default(),
            activity: crate::presence::ActivityPrefs::default(),
            columns: crate::columns::Widths::default(),
            preview_images: true,
            recent_instances: Vec::new(),
        }
    }
}

impl Prefs {
    /// Приводит значения к допустимым диапазонам.
    fn sanitize(mut self) -> Self {
        if !self.scale.is_finite() {
            self.scale = 1.0;
        }
        self.scale = self.scale.clamp(0.5, 3.0);
        self.sounds.volume = self.sounds.volume.min(200);
        self.appearance.radius = self.appearance.radius.min(20);
        self.voice.sanitize();
        self.recent_instances.truncate(RECENT_INSTANCES);
        // Пустой репозиторий остался от версий, где умолчания не было.
        if self.updates.repo.trim().is_empty() {
            self.updates.repo = crate::updates::default_repo();
        }
        self
    }
}

#[derive(Debug)]
struct Store {
    prefs: Prefs,
    path: Option<PathBuf>,
    /// Время последнего несохранённого изменения.
    dirty_since: Option<Instant>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    f(STORE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(|| Store {
            prefs: Prefs::default(),
            path: None,
            dirty_since: None,
        }))
}

/// Путь к файлу настроек по умолчанию.
pub(crate) fn default_path() -> Option<PathBuf> {
    crate::saved_session::config_dir().map(|dir| dir.join(FILE_NAME))
}

/// Загружает настройки из файла (или берёт значения по умолчанию) и
/// запоминает путь для последующих сохранений.
pub(crate) fn load(path: Option<PathBuf>) -> Prefs {
    let prefs = path
        .as_deref()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| match serde_json::from_slice::<Prefs>(&bytes) {
            Ok(prefs) => Some(prefs),
            Err(err) => {
                tracing::warn!(%err, "prefs.json повреждён — используются настройки по умолчанию");
                None
            }
        })
        .unwrap_or_default()
        .sanitize();
    with_store(|store| {
        store.prefs = prefs.clone();
        store.path = path;
        store.dirty_since = None;
    });
    prefs
}

/// Копия текущих настроек.
pub(crate) fn get() -> Prefs {
    with_store(|store| store.prefs.clone())
}

/// Меняет настройки; сохранение произойдёт при следующем [`flush`].
pub(crate) fn update(change: impl FnOnce(&mut Prefs)) {
    with_store(|store| {
        let before = store.prefs.clone();
        change(&mut store.prefs);
        if store.prefs != before {
            store.dirty_since = Some(Instant::now());
        }
    });
}

/// Сохраняет изменения, если с последнего прошла пауза (или `force`).
pub(crate) fn flush(force: bool) {
    let pending = with_store(|store| {
        let due = store
            .dirty_since
            .is_some_and(|since| force || since.elapsed() >= SAVE_DELAY);
        if !due {
            return None;
        }
        store.dirty_since = None;
        store.path.clone().map(|path| (path, store.prefs.clone()))
    });
    if let Some((path, prefs)) = pending
        && let Err(err) = write(&path, &prefs)
    {
        tracing::warn!(%err, path = %path.display(), "не удалось сохранить настройки");
    }
}

/// Атомарная запись файла.
fn write(path: &std::path::Path, prefs: &Prefs) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(prefs).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::{Action, Keybind, Prefs, RECENT_INSTANCES, push_recent, write};
    use crate::theme::Preset;

    #[test]
    fn recent_instances_keep_order_without_repeats() {
        let mut list = Vec::new();
        push_recent(&mut list, "https://a.example/", false);
        push_recent(&mut list, "https://b.example", true);
        push_recent(&mut list, "https://A.example", false);
        push_recent(&mut list, "   ", false);
        let bases: Vec<&str> = list.iter().map(|item| item.base.as_str()).collect();
        assert_eq!(bases, ["https://A.example", "https://b.example"]);
        assert!(list[1].insecure);
        for n in 0..10 {
            push_recent(&mut list, &format!("https://{n}.example"), false);
        }
        assert_eq!(list.len(), RECENT_INSTANCES);
        assert_eq!(list[0].base, "https://9.example");
    }

    #[test]
    fn recent_instances_round_trip_and_old_files_load() {
        let mut prefs = Prefs::default();
        push_recent(&mut prefs.recent_instances, "https://chat.example", false);
        let text = serde_json::to_string(&prefs).unwrap();
        assert!(text.contains("recentInstances"));
        let back: Prefs = serde_json::from_str(&text).unwrap();
        assert_eq!(back.recent_instances, prefs.recent_instances);
        // Файл прежней версии, без списка.
        let old: Prefs = serde_json::from_str(r#"{"scale":1.0}"#).unwrap();
        assert!(old.recent_instances.is_empty());
    }

    #[test]
    fn roundtrip_keeps_every_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        let mut prefs = Prefs {
            scale: 1.25,
            ..Prefs::default()
        };
        prefs.appearance.preset = Preset::Nord;
        prefs.sounds.volume = 150;
        prefs.voice.noise_suppression = false;
        prefs.keybinds.insert(
            Action::ToggleMute,
            Keybind {
                command: true,
                shift: true,
                alt: false,
                key: egui::Key::M,
            },
        );
        write(&path, &prefs).unwrap();
        let back: Prefs = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back, prefs);
    }

    #[test]
    fn broken_values_fall_back_to_defaults() {
        let parsed: Prefs =
            serde_json::from_str(r#"{"scale": 99, "sounds": {"volume": 900}, "unknown": 1}"#)
                .unwrap();
        let fixed = parsed.sanitize();
        assert!((fixed.scale - 3.0).abs() < f32::EPSILON);
        assert_eq!(fixed.sounds.volume, 200);
        assert_eq!(fixed.appearance, crate::theme::Appearance::default());
    }

    #[test]
    fn keybind_label_is_readable() {
        let bind = Keybind {
            command: false,
            shift: true,
            alt: true,
            key: egui::Key::F5,
        };
        assert_eq!(bind.label(), "Shift+Alt+F5");
    }
}
