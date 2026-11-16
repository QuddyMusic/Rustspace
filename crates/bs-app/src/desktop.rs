//! Настольное окружение: значок в трее, сворачивание в него и глобальные
//! горячие клавиши (как `createTray` и `keybindManager` десктопа оригинала).
//!
//! Оба механизма — за флагами сборки (`tray`, `global-hotkeys`); без них
//! [`Desktop`] ничего не делает. События приходят из чужих потоков (GTK,
//! перехватчик клавиш), складываются в очередь и будят окно, а разбираются
//! в обычном кадре интерфейса.
//!
//! Спрятанное окно eframe не рисует и не вызывает `App::ui`, а вся работа
//! клиента (WebSocket, звонки) идёт именно там. Поэтому, пока окно в трее,
//! [`Desktop::logic`] каждый проход просит отрисовку и в скрытом состоянии.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::prefs::{Action, Keybind};
use crate::voice::DockAction;

/// Что пришло извне окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(all(feature = "tray", feature = "global-hotkeys")),
    allow(
        dead_code,
        reason = "часть команд шлёт только трей или перехватчик клавиш"
    )
)]
pub(crate) enum Command {
    /// Показать окно.
    Show,
    /// Спрятать окно в трей.
    Hide,
    /// Переключить микрофон.
    ToggleMute,
    /// Переключить звук.
    ToggleDeafen,
    /// Выйти из приложения.
    Quit,
    /// Глобальная клавиша: действие и «нажата» (`false` — отпущена).
    Key(Action, bool),
    /// То же от портала Wayland: сочетание назначила система.
    PortalKey(Action, bool),
}

type Queue = Arc<Mutex<VecDeque<Command>>>;

/// Поднялись ли трей и глобальные клавиши (для страниц настроек).
static TRAY_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static KEYS_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Есть ли значок в трее.
pub(crate) fn tray_available() -> bool {
    TRAY_UP.load(std::sync::atomic::Ordering::Relaxed)
}

/// Окно живёт в сеансе Wayland (не XWayland).
pub(crate) fn is_wayland() -> bool {
    cfg!(target_os = "linux")
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind.eq_ignore_ascii_case("wayland"))
}

/// Работает ли глобальный перехват клавиш.
pub(crate) fn global_keys_available() -> bool {
    KEYS_UP.load(std::sync::atomic::Ordering::Relaxed) || portal_ready()
}

/// Состояние портала Wayland (`Off` вне Wayland и без перехвата в сборке).
pub(crate) fn portal_state() -> PortalView {
    #[cfg(all(feature = "global-hotkeys", target_os = "linux"))]
    {
        use crate::portal_keys::PortalState;
        match crate::portal_keys::status() {
            PortalState::Off => PortalView::Off,
            PortalState::Pending => PortalView::Pending,
            PortalState::Ready(map) => PortalView::Ready(map),
            PortalState::Failed(err) => PortalView::Failed(err),
        }
    }
    #[cfg(not(all(feature = "global-hotkeys", target_os = "linux")))]
    {
        PortalView::Off
    }
}

fn portal_ready() -> bool {
    matches!(portal_state(), PortalView::Ready(_))
}

/// Что показать о портале в настройках.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    not(all(feature = "global-hotkeys", target_os = "linux")),
    allow(dead_code, reason = "портал есть только на Linux с перехватом клавиш")
)]
pub(crate) enum PortalView {
    /// Портал не используется.
    Off,
    /// Ждём систему или пользователя.
    Pending,
    /// Сочетания по действиям (пустая строка — не назначено).
    Ready(BTreeMap<Action, String>),
    /// Портал отказал.
    Failed(String),
}

fn lock(queue: &Queue) -> MutexGuard<'_, VecDeque<Command>> {
    queue.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Что окну сделать в этом кадре.
#[derive(Debug, Default)]
pub(crate) struct DesktopInput {
    /// Действия звонка из меню трея.
    pub(crate) actions: Vec<DockAction>,
    /// Глобальные клавиши: действие и «нажата».
    pub(crate) keys: Vec<(Action, bool)>,
}

/// Трей, окно и глобальные клавиши.
#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "независимые флаги: трей, окно спрятано, выход, глобальные клавиши"
)]
pub(crate) struct Desktop {
    queue: Queue,
    tray: bool,
    hidden: bool,
    quitting: bool,
    /// Включены ли глобальные клавиши в настройках (на прошлом кадре).
    global_enabled: bool,
    #[cfg(feature = "global-hotkeys")]
    keys: Option<global::Keys>,
    #[cfg(all(feature = "tray", not(target_os = "linux")))]
    _icon: Option<tray_icon::TrayIcon>,
}

impl std::fmt::Debug for Desktop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Desktop")
            .field("tray", &self.tray)
            .field("hidden", &self.hidden)
            .finish_non_exhaustive()
    }
}

impl Desktop {
    /// Поднимает трей и перехват клавиш (что есть в сборке и в системе).
    pub(crate) fn start(ctx: &egui::Context) -> Self {
        #[allow(unused_mut, reason = "без настольных флагов менять нечего")]
        let mut desktop = Self::default();
        #[cfg(feature = "tray")]
        desktop.start_tray(ctx);
        #[cfg(all(feature = "global-hotkeys", target_os = "linux"))]
        if crate::portal_keys::wanted() {
            // Под Wayland перехват X11 видит только окна XWayland: нужен портал.
            let queue = Arc::clone(&desktop.queue);
            let wake = ctx.clone();
            crate::portal_keys::start(Box::new(move |action, pressed| {
                lock(&queue).push_back(Command::PortalKey(action, pressed));
                wake.request_repaint();
            }));
        } else {
            desktop.keys = global::Keys::new(&desktop.queue, ctx);
        }
        #[cfg(all(feature = "global-hotkeys", not(target_os = "linux")))]
        {
            desktop.keys = global::Keys::new(&desktop.queue, ctx);
        }
        let _ = ctx;
        TRAY_UP.store(desktop.tray, std::sync::atomic::Ordering::Relaxed);
        KEYS_UP.store(
            desktop.has_global_keys(),
            std::sync::atomic::Ordering::Relaxed,
        );
        desktop
    }

    /// Работают ли глобальные клавиши.
    #[cfg_attr(
        not(feature = "global-hotkeys"),
        allow(clippy::unused_self, reason = "без перехватчика")
    )]
    pub(crate) fn has_global_keys(&self) -> bool {
        #[cfg(feature = "global-hotkeys")]
        {
            self.keys.is_some() || portal_ready()
        }
        #[cfg(not(feature = "global-hotkeys"))]
        {
            false
        }
    }

    /// Вызывается из `App::logic`: пока окно спрятано, клиент продолжает работать.
    pub(crate) fn logic(&self, ctx: &egui::Context) {
        if self.hidden {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        }
    }

    /// Привязки, перехваченные глобально (окно их не обрабатывает).
    pub(crate) fn global_actions(&self) -> BTreeSet<Action> {
        if !self.global_enabled {
            return BTreeSet::new();
        }
        if let PortalView::Ready(map) = portal_state() {
            return map
                .into_iter()
                .filter(|(_, trigger)| !trigger.is_empty())
                .map(|(action, _)| action)
                .collect();
        }
        #[cfg(feature = "global-hotkeys")]
        if let Some(keys) = &self.keys {
            return keys.actions();
        }
        BTreeSet::new()
    }

    /// Разбирает очередь, следит за закрытием окна, обновляет глобальные клавиши.
    pub(crate) fn frame(
        &mut self,
        ctx: &egui::Context,
        binds: &BTreeMap<Action, Keybind>,
        global_enabled: bool,
        close_to_tray: bool,
    ) -> DesktopInput {
        #[cfg(feature = "global-hotkeys")]
        if let Some(keys) = self.keys.as_mut() {
            let none = BTreeMap::new();
            keys.sync(if global_enabled { binds } else { &none });
        }
        let _ = binds;
        self.global_enabled = global_enabled;
        let mut input = DesktopInput::default();
        let commands: Vec<Command> = lock(&self.queue).drain(..).collect();
        for command in commands {
            match command {
                Command::Show => self.show(ctx),
                Command::Hide => self.hide(ctx),
                Command::ToggleMute => input.actions.push(DockAction::ToggleMute),
                Command::ToggleDeafen => input.actions.push(DockAction::ToggleDeafen),
                Command::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Command::Key(action, pressed) => input.keys.push((action, pressed)),
                Command::PortalKey(action, pressed) => {
                    if global_enabled {
                        input.keys.push((action, pressed));
                    }
                }
            }
        }
        // Под Wayland свёрнутое окно возвращают с панели задач, не из трея.
        if self.hidden && is_wayland() && ctx.input(|i| i.viewport().focused == Some(true)) {
            self.hidden = false;
        }
        let close = ctx.input(|i| i.viewport().close_requested());
        if close && self.tray && close_to_tray && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide(ctx);
        }
        input
    }

    fn show(&mut self, ctx: &egui::Context) {
        self.hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn hide(&mut self, ctx: &egui::Context) {
        if !self.tray {
            return;
        }
        self.hidden = true;
        if is_wayland() {
            // Протокол Wayland не умеет прятать окно (winit игнорирует
            // Visible(false)), поэтому окно сворачивается, а Backspace
            // продолжает работать.
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }

    #[cfg(feature = "tray")]
    fn start_tray(&mut self, ctx: &egui::Context) {
        tray::install_menu_handler(&self.queue, ctx);
        #[cfg(target_os = "linux")]
        {
            // GTK живёт в своём потоке: winit на Linux не крутит цикл GTK.
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("bs-tray".into())
                .spawn(move || {
                    if let Err(err) = gtk::init() {
                        tracing::warn!(%err, "GTK не запустился, трея не будет");
                        let _ = ready_tx.send(false);
                        return;
                    }
                    match tray::build() {
                        Ok(icon) => {
                            let _ = ready_tx.send(true);
                            gtk::main();
                            drop(icon);
                        }
                        Err(err) => {
                            tracing::warn!(%err, "значок в трее не создан");
                            let _ = ready_tx.send(false);
                        }
                    }
                });
            self.tray = spawned.is_ok() && ready_rx.recv().unwrap_or(false);
        }
        #[cfg(not(target_os = "linux"))]
        match tray::build() {
            Ok(icon) => {
                self._icon = Some(icon);
                self.tray = true;
            }
            Err(err) => tracing::warn!(%err, "значок в трее не создан"),
        }
    }
}

#[cfg(feature = "tray")]
mod tray {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    use super::{Command, Queue, lock};
    use crate::i18n::tr;

    const SHOW: &str = "show";
    const HIDE: &str = "hide";
    const MUTE: &str = "mute";
    const DEAFEN: &str = "deafen";
    const QUIT: &str = "quit";

    /// Пункты меню трея → команды окну.
    pub(super) fn install_menu_handler(queue: &Queue, ctx: &egui::Context) {
        let queue = std::sync::Arc::clone(queue);
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let command = match event.id.as_ref() {
                SHOW => Command::Show,
                HIDE => Command::Hide,
                MUTE => Command::ToggleMute,
                DEAFEN => Command::ToggleDeafen,
                QUIT => Command::Quit,
                _ => return,
            };
            lock(&queue).push_back(command);
            ctx.request_repaint();
        }));
    }

    pub(super) fn build() -> Result<TrayIcon, String> {
        let png = include_bytes!("../../../packaging/linux/backspace-rs.png");
        let data = eframe::icon_data::from_png_bytes(png).map_err(|err| err.to_string())?;
        let icon =
            Icon::from_rgba(data.rgba, data.width, data.height).map_err(|err| err.to_string())?;
        let menu = Menu::new();
        let items = [
            MenuItem::with_id(SHOW, tr("Показать"), true, None),
            MenuItem::with_id(HIDE, tr("Скрыть"), true, None),
        ];
        let voice = [
            MenuItem::with_id(MUTE, tr("Микрофон вкл/выкл"), true, None),
            MenuItem::with_id(DEAFEN, tr("Звук вкл/выкл"), true, None),
        ];
        let quit = MenuItem::with_id(QUIT, tr("Выйти"), true, None);
        for item in &items {
            menu.append(item).map_err(|err| err.to_string())?;
        }
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|err| err.to_string())?;
        for item in &voice {
            menu.append(item).map_err(|err| err.to_string())?;
        }
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|err| err.to_string())?;
        menu.append(&quit).map_err(|err| err.to_string())?;
        TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(crate::brand::NAME)
            .with_icon(icon)
            .build()
            .map_err(|err| err.to_string())
    }
}

#[cfg(feature = "global-hotkeys")]
mod global {
    use std::collections::{BTreeMap, BTreeSet};

    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    use super::{Command, Queue, lock};
    use crate::prefs::{Action, Keybind};

    /// Зарегистрированные глобальные сочетания.
    pub(super) struct Keys {
        manager: GlobalHotKeyManager,
        registered: std::sync::Arc<std::sync::Mutex<Vec<(u32, Action)>>>,
        hotkeys: Vec<HotKey>,
        binds: BTreeMap<Action, Keybind>,
    }

    impl Keys {
        pub(super) fn new(queue: &Queue, ctx: &egui::Context) -> Option<Self> {
            let manager = match GlobalHotKeyManager::new() {
                Ok(manager) => manager,
                Err(err) => {
                    tracing::info!(%err, "глобальные клавиши недоступны, работают только в окне");
                    return None;
                }
            };
            let registered =
                std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, Action)>::new()));
            let lookup = std::sync::Arc::clone(&registered);
            let queue = std::sync::Arc::clone(queue);
            let ctx = ctx.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
                let action = lookup
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .find(|(id, _)| *id == event.id)
                    .map(|(_, action)| *action);
                if let Some(action) = action {
                    let pressed = event.state == HotKeyState::Pressed;
                    lock(&queue).push_back(Command::Key(action, pressed));
                    ctx.request_repaint();
                }
            }));
            Some(Self {
                manager,
                registered,
                hotkeys: Vec::new(),
                binds: BTreeMap::new(),
            })
        }

        /// Перерегистрирует сочетания, если привязки поменялись.
        pub(super) fn sync(&mut self, binds: &BTreeMap<Action, Keybind>) {
            if *binds == self.binds {
                return;
            }
            self.binds.clone_from(binds);
            if let Err(err) = self.manager.unregister_all(&self.hotkeys) {
                tracing::debug!(%err, "старые сочетания не сняты");
            }
            self.hotkeys.clear();
            let mut registered = Vec::new();
            for (action, bind) in binds {
                let Some(hotkey) = to_hotkey(*bind) else {
                    tracing::info!(key = ?bind.key, "клавишу нельзя перехватить глобально");
                    continue;
                };
                match self.manager.register(hotkey) {
                    Ok(()) => {
                        registered.push((hotkey.id(), *action));
                        self.hotkeys.push(hotkey);
                    }
                    Err(err) => tracing::warn!(%err, label = %bind.label(), "сочетание занято"),
                }
            }
            *self
                .registered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = registered;
        }

        pub(super) fn actions(&self) -> BTreeSet<Action> {
            self.registered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|(_, action)| *action)
                .collect()
        }
    }

    /// Привязка egui → сочетание перехватчика.
    pub(super) fn to_hotkey(bind: Keybind) -> Option<HotKey> {
        let mut mods = Modifiers::empty();
        if bind.command {
            mods |= if cfg!(target_os = "macos") {
                Modifiers::META
            } else {
                Modifiers::CONTROL
            };
        }
        if bind.shift {
            mods |= Modifiers::SHIFT;
        }
        if bind.alt {
            mods |= Modifiers::ALT;
        }
        let code = code(bind.key)?;
        Some(HotKey::new((!mods.is_empty()).then_some(mods), code))
    }

    /// Клавиши egui и коды перехватчика.
    const KEY_CODES: &[(egui::Key, Code)] = &[
        (egui::Key::A, Code::KeyA),
        (egui::Key::B, Code::KeyB),
        (egui::Key::C, Code::KeyC),
        (egui::Key::D, Code::KeyD),
        (egui::Key::E, Code::KeyE),
        (egui::Key::F, Code::KeyF),
        (egui::Key::G, Code::KeyG),
        (egui::Key::H, Code::KeyH),
        (egui::Key::I, Code::KeyI),
        (egui::Key::J, Code::KeyJ),
        (egui::Key::K, Code::KeyK),
        (egui::Key::L, Code::KeyL),
        (egui::Key::M, Code::KeyM),
        (egui::Key::N, Code::KeyN),
        (egui::Key::O, Code::KeyO),
        (egui::Key::P, Code::KeyP),
        (egui::Key::Q, Code::KeyQ),
        (egui::Key::R, Code::KeyR),
        (egui::Key::S, Code::KeyS),
        (egui::Key::T, Code::KeyT),
        (egui::Key::U, Code::KeyU),
        (egui::Key::V, Code::KeyV),
        (egui::Key::W, Code::KeyW),
        (egui::Key::X, Code::KeyX),
        (egui::Key::Y, Code::KeyY),
        (egui::Key::Z, Code::KeyZ),
        (egui::Key::Num0, Code::Digit0),
        (egui::Key::Num1, Code::Digit1),
        (egui::Key::Num2, Code::Digit2),
        (egui::Key::Num3, Code::Digit3),
        (egui::Key::Num4, Code::Digit4),
        (egui::Key::Num5, Code::Digit5),
        (egui::Key::Num6, Code::Digit6),
        (egui::Key::Num7, Code::Digit7),
        (egui::Key::Num8, Code::Digit8),
        (egui::Key::Num9, Code::Digit9),
        (egui::Key::F1, Code::F1),
        (egui::Key::F2, Code::F2),
        (egui::Key::F3, Code::F3),
        (egui::Key::F4, Code::F4),
        (egui::Key::F5, Code::F5),
        (egui::Key::F6, Code::F6),
        (egui::Key::F7, Code::F7),
        (egui::Key::F8, Code::F8),
        (egui::Key::F9, Code::F9),
        (egui::Key::F10, Code::F10),
        (egui::Key::F11, Code::F11),
        (egui::Key::F12, Code::F12),
        (egui::Key::F13, Code::F13),
        (egui::Key::F14, Code::F14),
        (egui::Key::F15, Code::F15),
        (egui::Key::F16, Code::F16),
        (egui::Key::F17, Code::F17),
        (egui::Key::F18, Code::F18),
        (egui::Key::F19, Code::F19),
        (egui::Key::F20, Code::F20),
        (egui::Key::Space, Code::Space),
        (egui::Key::Enter, Code::Enter),
        (egui::Key::Tab, Code::Tab),
        (egui::Key::Backspace, Code::Backspace),
        (egui::Key::Escape, Code::Escape),
        (egui::Key::Insert, Code::Insert),
        (egui::Key::Delete, Code::Delete),
        (egui::Key::Home, Code::Home),
        (egui::Key::End, Code::End),
        (egui::Key::PageUp, Code::PageUp),
        (egui::Key::PageDown, Code::PageDown),
        (egui::Key::ArrowUp, Code::ArrowUp),
        (egui::Key::ArrowDown, Code::ArrowDown),
        (egui::Key::ArrowLeft, Code::ArrowLeft),
        (egui::Key::ArrowRight, Code::ArrowRight),
        (egui::Key::Minus, Code::Minus),
        (egui::Key::Equals, Code::Equal),
        (egui::Key::Comma, Code::Comma),
        (egui::Key::Period, Code::Period),
        (egui::Key::Slash, Code::Slash),
        (egui::Key::Backslash, Code::Backslash),
        (egui::Key::Semicolon, Code::Semicolon),
        (egui::Key::Quote, Code::Quote),
        (egui::Key::Backtick, Code::Backquote),
        (egui::Key::OpenBracket, Code::BracketLeft),
        (egui::Key::CloseBracket, Code::BracketRight),
    ];

    fn code(key: egui::Key) -> Option<Code> {
        KEY_CODES
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, code)| *code)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn keybinds_map_to_hotkeys() {
            let bind = Keybind {
                command: true,
                shift: true,
                alt: false,
                key: egui::Key::M,
            };
            let hotkey = to_hotkey(bind).expect("сочетание");
            let ctrl = if cfg!(target_os = "macos") {
                Modifiers::META
            } else {
                Modifiers::CONTROL
            };
            assert!(hotkey.matches(ctrl | Modifiers::SHIFT, Code::KeyM));
            let plain = Keybind {
                command: false,
                shift: false,
                alt: false,
                key: egui::Key::F13,
            };
            assert!(to_hotkey(plain).is_some());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_without_tray_is_not_intercepted() {
        let ctx = egui::Context::default();
        let mut desktop = Desktop::default();
        lock(&desktop.queue).push_back(Command::ToggleMute);
        lock(&desktop.queue).push_back(Command::Key(Action::PushToTalk, true));
        let input = desktop.frame(&ctx, &BTreeMap::new(), true, true);
        assert_eq!(input.actions, vec![DockAction::ToggleMute]);
        assert_eq!(input.keys, vec![(Action::PushToTalk, true)]);
        desktop.hide(&ctx);
        assert!(
            !desktop.hidden,
            "без трея окно не прячется — его не вернуть"
        );
    }
}
