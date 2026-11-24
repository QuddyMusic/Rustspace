//! Сцена звонка: камеры и демонстрации экрана над чатом, выбор источника.
//!
//! Как `VoiceGrid`/`StreamTile` оригинала: чужую трансляцию сначала
//! предлагают посмотреть (и только тогда слышен её звук), камеры видны
//! сразу; щелчок по плитке разворачивает её на всю сцену.

use std::collections::{BTreeSet, HashMap};

use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Vec2};

use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::share_quality::{self, ShareCodec, ShareMode, ShareQuality};
use crate::theme;
use crate::video_media::{Feed, ShareKind, ShareSource, VideoControl};
use crate::voice::VoiceState;
use crate::voice_media::AudioSource;
use bs_api::StreamingLimits;

/// Ключ плитки: identity и «это экран».
type TileKey = (String, bool);

/// Выбор источника для демонстрации.
#[derive(Debug)]
pub(crate) struct Picker {
    sources: Result<Vec<ShareSource>, String>,
    selected: Option<usize>,
    quality: ShareQuality,
    /// Ограничения инстанса, на котором идёт звонок.
    limits: StreamingLimits,
}

/// Где показана сцена.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum StageMode {
    /// Полосой над чатом.
    #[default]
    Docked,
    /// Во весь экран (Esc — назад).
    Fullscreen,
    /// В отдельном окне.
    Window,
}

/// Состояние сцены между кадрами.
#[derive(Default)]
pub(crate) struct StageState {
    /// Где показана сцена.
    pub(crate) mode: StageMode,
    textures: HashMap<TileKey, (u64, egui::TextureHandle)>,
    /// Открытый выбор источника.
    pub(crate) picker: Option<Picker>,
    focused: Option<TileKey>,
    /// Чьи трансляции смотрим (identity ведущих).
    pub(crate) watching: BTreeSet<String>,
    /// Какие чужие трансляции были на прошлом кадре.
    pub(crate) known_streams: BTreeSet<String>,
    /// Найденные камеры (ищутся лениво — это небыстро).
    pub(crate) cameras: Option<Result<Vec<crate::video_media::CameraDevice>, String>>,
}

impl std::fmt::Debug for StageState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageState")
            .field("tiles", &self.textures.len())
            .field("watching", &self.watching)
            .finish_non_exhaustive()
    }
}

impl StageState {
    /// Открывает выбор источника.
    pub(crate) fn open_picker(&mut self, quality: ShareQuality, limits: StreamingLimits) {
        let sources = VideoControl::screen_sources();
        // Единственный вариант (системный выбор под Wayland) — сразу отмечен.
        let selected = sources
            .as_ref()
            .ok()
            .filter(|list| list.len() == 1)
            .map(|_| 0);
        self.picker = Some(Picker {
            sources,
            selected,
            quality,
            limits,
        });
    }

    /// Звонок закончился.
    pub(crate) fn clear(&mut self) {
        self.textures.clear();
        self.picker = None;
        self.focused = None;
        self.mode = StageMode::Docked;
        self.watching.clear();
        self.known_streams.clear();
    }
}

/// Имя из identity `userId:имя`.
fn display_name(identity: &str) -> &str {
    identity.split_once(':').map_or(identity, |(_, name)| name)
}

/// Что сделать после кадра сцены.
enum StageAction {
    Watch(String, bool),
    StopScreen,
    StopCamera,
    LocalAudio(crate::voice_media::LocalAudio),
    Mode(StageMode),
}

/// Рисует сцену, если в звонке есть видео.
pub(crate) fn show(ui: &mut egui::Ui, voice: &mut VoiceState, bridge: &Bridge) {
    let feeds = voice
        .media
        .as_ref()
        .map(VideoControl::feeds)
        .unwrap_or_default();
    let watchers = voice
        .media
        .as_ref()
        .map(VideoControl::watchers)
        .unwrap_or_default();
    // Как у оригинала: пока идёт звонок, сцена показывает всех участников —
    // с видео или аватаркой.
    let people = call_people(voice, bridge);
    if feeds.is_empty() && people.is_empty() {
        voice.stage.textures.clear();
        voice.stage.focused = None;
        return;
    }
    // Кадры приходят из другого потока: перерисовываемся с частотой видео.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    let mut actions = Vec::new();
    let ctx = ui.ctx().clone();
    match voice.stage.mode {
        StageMode::Docked => {
            egui::Panel::top("call_stage")
                .resizable(true)
                .default_size(300.0)
                .size_range(160.0..=900.0)
                .frame(theme::module(theme::bg_rail()).inner_margin(egui::Margin::same(8)))
                .show(ui, |ui| {
                    stage_body(ui, voice, &feeds, &people, &watchers, &mut actions);
                });
        }
        StageMode::Fullscreen => {
            let screen = ctx.content_rect();
            egui::Area::new(egui::Id::unique("call_stage_fullscreen"))
                .order(egui::Order::Foreground)
                .fixed_pos(screen.min)
                .show(&ctx, |ui| {
                    ui.painter().rect_filled(screen, 0.0, theme::bg_rail());
                    let mut child =
                        ui.new_child(egui::UiBuilder::new().max_rect(screen.shrink(8.0)));
                    stage_body(&mut child, voice, &feeds, &people, &watchers, &mut actions);
                    ui.allocate_rect(screen, Sense::hover());
                });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                actions.push(StageAction::Mode(StageMode::Docked));
            }
        }
        StageMode::Window => {
            let builder = egui::ViewportBuilder::default()
                .with_title(tr("Звонок — Rustspace"))
                .with_inner_size([960.0, 600.0])
                .with_min_inner_size([320.0, 200.0]);
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("rustspace_call_window"),
                builder,
                |ctx, _class| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new().fill(theme::bg_rail()).inner_margin(8))
                        .show(ctx, |ui| {
                            stage_body(ui, voice, &feeds, &people, &watchers, &mut actions);
                        });
                    if ctx.input(|i| i.viewport().close_requested()) {
                        actions.push(StageAction::Mode(StageMode::Docked));
                    }
                },
            );
        }
    }
    for action in actions {
        match action {
            StageAction::Watch(identity, watching) => {
                voice.set_watching(bridge, &identity, watching);
            }
            StageAction::StopScreen => {
                if let Some(media) = voice.media.as_ref() {
                    voice.video.stop_screen(media);
                }
            }
            StageAction::StopCamera => {
                if let Some(media) = voice.media.as_ref() {
                    voice.video.stop_camera(media);
                }
            }
            StageAction::LocalAudio(audio) => voice.set_local_audio(bridge, &audio),
            StageAction::Mode(mode) => set_mode(&ctx, &mut voice.stage, mode),
        }
    }
}

/// Меняет режим сцены; во весь экран разворачивается и само окно.
fn set_mode(ctx: &egui::Context, stage: &mut StageState, mode: StageMode) {
    let was_fullscreen = stage.mode == StageMode::Fullscreen;
    let fullscreen = mode == StageMode::Fullscreen;
    if was_fullscreen != fullscreen {
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Fullscreen(fullscreen),
        );
    }
    stage.mode = mode;
}

/// Плитки и переключатель режима над ними.
fn stage_body(
    ui: &mut egui::Ui,
    voice: &mut VoiceState,
    feeds: &[Feed],
    people: &[Person],
    watchers: &[String],
    actions: &mut Vec<StageAction>,
) {
    update_textures(ui.ctx(), &mut voice.stage, feeds);
    let mode = voice.stage.mode;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (label, target) = match mode {
                StageMode::Fullscreen => (tr("Выйти из полноэкранного (Esc)"), StageMode::Docked),
                _ => (tr("Во весь экран"), StageMode::Fullscreen),
            };
            if ui.small_button(label).clicked() {
                actions.push(StageAction::Mode(target));
            }
            let (label, target) = match mode {
                StageMode::Window => (tr("Вернуть в окно чата"), StageMode::Docked),
                _ => (tr("В отдельном окне"), StageMode::Window),
            };
            if ui.small_button(label).clicked() {
                actions.push(StageAction::Mode(target));
            }
        });
    });
    ui.add_space(4.0);
    let shown: Vec<&Feed> = match &voice.stage.focused {
        Some(key)
            if feeds
                .iter()
                .any(|f| (&f.identity, f.screen) == (&key.0, key.1)) =>
        {
            feeds
                .iter()
                .filter(|f| (&f.identity, f.screen) == (&key.0, key.1))
                .collect()
        }
        _ => feeds.iter().collect(),
    };
    // Участники без камеры — плитками с аватаркой (в режиме «одна плитка»
    // не показываются).
    let faces: Vec<&Person> = if voice.stage.focused.is_some() && shown.len() < feeds.len() {
        Vec::new()
    } else {
        people
            .iter()
            .filter(|person| {
                !feeds
                    .iter()
                    .any(|f| !f.screen && bs_user_id(&f.identity) == person.id)
            })
            .collect()
    };
    let area = ui.available_rect_before_wrap();
    let mut rects = grid(area, shown.len() + faces.len(), 8.0).into_iter();
    for (feed, rect) in shown.iter().zip(rects.by_ref()) {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        tile(&mut child, feed, voice, watchers, actions);
    }
    let speaking = voice.speaking.clone();
    for (person, rect) in faces.into_iter().zip(rects) {
        face_tile(ui, rect, person, speaking.contains(&person.id));
    }
    ui.allocate_rect(area, Sense::hover());
}

/// Участник звонка для плитки без видео.
#[derive(Debug, Clone)]
struct Person {
    id: String,
    name: String,
    avatar: Option<String>,
}

/// Id пользователя из identity LiveKit `userId:имя`.
fn bs_user_id(identity: &str) -> &str {
    identity.split_once(':').map_or(identity, |(id, _)| id)
}

/// Участники текущего звонка: я и остальные (из комнаты LiveKit), с
/// аватарками из кэша инстанса звонка.
fn call_people(voice: &VoiceState, bridge: &Bridge) -> Vec<Person> {
    let origin = match (&voice.call, &voice.dm) {
        (Some(call), _) => call.origin,
        (None, Some(dm)) => dm.origin,
        (None, None) => return Vec::new(),
    };
    let cache = bridge.cache_at(origin);
    let avatar = |id: &str| -> Option<String> {
        let cache = cache?;
        let in_space = voice.call.as_ref().and_then(|call| {
            cache
                .space(&call.space_id)?
                .members
                .iter()
                .find(|m| m.user.id == id)
                .and_then(|m| m.user.avatar.clone())
        });
        in_space.or_else(|| {
            cache
                .dm_channels()
                .flat_map(|c| c.members.iter())
                .find(|u| u.id == id)
                .and_then(|u| u.avatar.clone())
        })
    };
    let mut people = Vec::new();
    if let Some(cache) = cache {
        let me = cache.me();
        people.push(Person {
            id: me.id.clone(),
            name: me
                .display_name
                .clone()
                .unwrap_or_else(|| me.username.clone()),
            avatar: me.avatar.clone(),
        });
    }
    let members = voice
        .media
        .as_ref()
        .map(crate::voice_media::MediaEngine::members)
        .unwrap_or_default();
    for (id, name) in members {
        let avatar = avatar(&id);
        people.push(Person { id, name, avatar });
    }
    people
}

/// Плитка участника без видео: аватар по центру, имя, зелёное кольцо, когда
/// говорит.
fn face_tile(ui: &egui::Ui, rect: Rect, person: &Person, speaking: bool) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme::rounded(10), theme::bg_input());
    let size = (rect.height() * 0.42).clamp(40.0, 112.0);
    let center = rect.center() - Vec2::new(0.0, 10.0);
    let avatar = Rect::from_center_size(center, Vec2::splat(size));
    theme::paint_avatar(
        &painter,
        avatar,
        person.name.as_str(),
        None,
        false,
        person.avatar.as_deref(),
    );
    if speaking {
        painter.circle_stroke(
            center,
            size / 2.0 + 3.0,
            egui::Stroke::new(3.0, theme::green()),
        );
        painter.rect_stroke(
            rect.shrink(1.0),
            theme::rounded(10),
            egui::Stroke::new(2.0, theme::green()),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        egui::pos2(rect.center().x, avatar.bottom() + 14.0),
        Align2::CENTER_CENTER,
        &person.name,
        FontId::proportional(14.0),
        theme::text_bright(),
    );
}

fn update_textures(ctx: &egui::Context, stage: &mut StageState, feeds: &[Feed]) {
    stage.textures.retain(|key, _| {
        feeds
            .iter()
            .any(|f| (&f.identity, f.screen) == (&key.0, key.1))
    });
    for feed in feeds {
        let Some(image) = &feed.image else {
            continue;
        };
        let key = (feed.identity.clone(), feed.screen);
        if stage
            .textures
            .get(&key)
            .is_some_and(|(generation, _)| *generation == feed.generation)
        {
            continue;
        }
        let pixels = image.rgba();
        // Пустой кадр (бывает в начале захвата) wgpu не переживает: текстура
        // нулевой ширины — фатальная ошибка валидации.
        if image.size[0] == 0
            || image.size[1] == 0
            || pixels.len() != image.size[0] * image.size[1] * 4
        {
            continue;
        }
        let color = egui::ColorImage::from_rgba_unmultiplied(image.size, pixels);
        if let Some((generation, texture)) = stage.textures.get_mut(&key) {
            texture.set(color, egui::TextureOptions::LINEAR);
            *generation = feed.generation;
        } else {
            let name = format!("stage-{}-{}", feed.identity, feed.screen);
            let texture = ctx.load_texture(name, color, egui::TextureOptions::LINEAR);
            stage.textures.insert(key, (feed.generation, texture));
        }
    }
}

/// Размер ячейки: `count` ячеек с зазорами `gap` в длине `total`.
fn cell(total: f32, gap: f32, count: f32) -> f32 {
    gap.mul_add(-(count - 1.0), total) / count
}

/// Раскладывает `count` плиток 16:9 по области так, чтобы они были крупнее.
fn grid(area: Rect, count: usize, gap: f32) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let mut best = (1, 0.0_f32);
    for columns in 1..=count {
        let rows = count.div_ceil(columns);
        #[allow(clippy::cast_precision_loss, reason = "единицы плиток")]
        let (cols_f, rows_f) = (columns as f32, rows as f32);
        let cell_w = cell(area.width(), gap, cols_f);
        let cell_h = cell(area.height(), gap, rows_f);
        let width = cell_w.min(cell_h * 16.0 / 9.0);
        if width > best.1 {
            best = (columns, width);
        }
    }
    let columns = best.0;
    let rows = count.div_ceil(columns);
    #[allow(clippy::cast_precision_loss, reason = "единицы плиток")]
    let (cols_f, rows_f) = (columns as f32, rows as f32);
    let cell = Vec2::new(
        cell(area.width(), gap, cols_f),
        cell(area.height(), gap, rows_f),
    );
    (0..count)
        .map(|index| {
            #[allow(clippy::cast_precision_loss, reason = "номера плиток")]
            let (col, row) = ((index % columns) as f32, (index / columns) as f32);
            Rect::from_min_size(
                area.min + Vec2::new(col * (cell.x + gap), row * (cell.y + gap)),
                cell,
            )
        })
        .collect()
}

/// Картинка с сохранением пропорций по центру `rect`.
fn fit(rect: Rect, size: Vec2) -> Rect {
    if size.x <= 0.0 || size.y <= 0.0 {
        return rect;
    }
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    Rect::from_center_size(rect.center(), size * scale)
}

fn tile(
    ui: &mut egui::Ui,
    feed: &Feed,
    voice: &mut VoiceState,
    watchers: &[String],
    actions: &mut Vec<StageAction>,
) {
    let rect = ui.max_rect();
    let response = ui.interact(
        rect,
        ui.auto_id_with(("stage_tile", &feed.identity, feed.screen)),
        Sense::click(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 10.0, Color32::from_black_alpha(220));
    let key = (feed.identity.clone(), feed.screen);
    let remote_stream = feed.screen && !feed.local;
    let watching = !remote_stream || voice.stage.watching.contains(&feed.identity);
    let name = if feed.local {
        tr("Вы").to_owned()
    } else {
        display_name(&feed.identity).to_owned()
    };
    if watching {
        if let Some((_, texture)) = voice.stage.textures.get(&key) {
            let image_rect = fit(rect.shrink(2.0), texture.size_vec2());
            painter.image(
                texture.id(),
                image_rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                tr("Ждём видео…"),
                FontId::proportional(13.0),
                theme::text_muted(),
            );
        }
    } else {
        painter.text(
            rect.center() - Vec2::new(0.0, 18.0),
            Align2::CENTER_CENTER,
            crate::tf!("{name} показывает экран", name = name),
            FontId::proportional(14.0),
            theme::text_bright(),
        );
        let button =
            Rect::from_center_size(rect.center() + Vec2::new(0.0, 16.0), Vec2::new(220.0, 30.0));
        if ui
            .put(button, egui::Button::new(tr("Смотреть трансляцию")))
            .clicked()
        {
            actions.push(StageAction::Watch(feed.identity.clone(), true));
        }
    }
    let label = if feed.screen {
        crate::tf!("{name} · экран", name = name)
    } else {
        name
    };
    let label_pos = rect.left_bottom() + Vec2::new(10.0, -10.0);
    let galley = painter.layout_no_wrap(label, FontId::proportional(12.5), Color32::WHITE);
    let back = Rect::from_min_size(
        label_pos - Vec2::new(4.0, galley.size().y + 2.0),
        galley.size() + Vec2::new(8.0, 4.0),
    );
    painter.rect_filled(back, 6.0, Color32::from_black_alpha(160));
    painter.galley(
        label_pos - Vec2::new(0.0, galley.size().y),
        galley,
        Color32::WHITE,
    );
    if feed.local && feed.screen && !watchers.is_empty() {
        painter.text(
            rect.right_top() + Vec2::new(-10.0, 10.0),
            Align2::RIGHT_TOP,
            crate::tf!("Смотрят: {count}", count = watchers.len()),
            FontId::proportional(12.0),
            theme::text_bright(),
        );
    }
    if response.clicked() && watching {
        let focused = &mut voice.stage.focused;
        *focused = if focused.as_ref() == Some(&key) {
            None
        } else {
            Some(key)
        };
    }
    let prefs = voice.prefs.clone();
    response.context_menu(|ui| {
        crate::widgets::menu_spacing(ui);
        tile_menu(ui, feed, &prefs, watching, actions);
    });
}

/// Контекстное меню плитки.
fn tile_menu(
    ui: &mut egui::Ui,
    feed: &Feed,
    prefs: &crate::voice_media::VoicePrefs,
    watching: bool,
    actions: &mut Vec<StageAction>,
) {
    if feed.local {
        let (title, action) = if feed.screen {
            (tr("Остановить демонстрацию"), StageAction::StopScreen)
        } else {
            (tr("Выключить камеру"), StageAction::StopCamera)
        };
        if ui.button(title).clicked() {
            actions.push(action);
            ui.close();
        }
        return;
    }
    if feed.screen {
        if let Some(audio) =
            crate::voice::local_audio_menu(ui, prefs, &feed.user_id, AudioSource::Stream)
        {
            actions.push(StageAction::LocalAudio(audio));
        }
        ui.separator();
        if watching && ui.button(tr("Перестать смотреть")).clicked() {
            actions.push(StageAction::Watch(feed.identity.clone(), false));
            ui.close();
        }
    } else if let Some(audio) =
        crate::voice::local_audio_menu(ui, prefs, &feed.user_id, AudioSource::Voice)
    {
        actions.push(StageAction::LocalAudio(audio));
    }
}

/// Окно выбора источника для демонстрации экрана.
pub(crate) fn show_picker(ctx: &egui::Context, voice: &mut VoiceState) {
    let Some(picker) = voice.stage.picker.as_mut() else {
        return;
    };
    let mut open = true;
    let mut start = None;
    egui::Window::new(tr("Показать экран"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .default_width(420.0)
        .open(&mut open)
        .show(ctx, |ui| {
            match &picker.sources {
                Err(err) => {
                    ui.colored_label(theme::rose(), err);
                }
                Ok(sources) if sources.is_empty() => {
                    ui.label(tr(
                        "Нечего показать: система не отдала ни одного экрана или окна.",
                    ));
                }
                Ok(sources) => {
                    egui::ScrollArea::vertical()
                        .max_height(280.0)
                        .show(ui, |ui| {
                            for (index, source) in sources.iter().enumerate() {
                                let text = match source.kind {
                                    ShareKind::System => {
                                        tr("Выбрать экран или окно в системном окне").to_owned()
                                    }
                                    ShareKind::Screen => {
                                        crate::tf!("Экран {name}", name = source.name)
                                    }
                                    ShareKind::Window => {
                                        crate::tf!("Окно: {name}", name = source.name)
                                    }
                                };
                                if ui
                                    .selectable_label(picker.selected == Some(index), text)
                                    .clicked()
                                {
                                    picker.selected = Some(index);
                                }
                            }
                        });
                }
            }
            ui.add_space(8.0);
            quality_controls(ui, &mut picker.quality, &picker.limits);
            ui.add_space(8.0);
            let chosen = picker
                .selected
                .and_then(|index| picker.sources.as_ref().ok()?.get(index).cloned());
            ui.add_enabled_ui(chosen.is_some(), |ui| {
                if ui
                    .button(RichText::new(tr("Начать демонстрацию")).strong())
                    .clicked()
                {
                    start = chosen.clone().map(|source| (source, picker.quality));
                }
            });
        });
    if let Some((source, quality)) = start {
        voice.prefs.share_quality = quality;
        let prefs = voice.prefs.clone();
        crate::prefs::update(|saved| saved.voice = prefs);
        let settings = quality.settings(&voice.stream_limits());
        if let Some(media) = voice.media.as_ref() {
            voice.video.start_screen(media, &source, settings);
        }
        voice.stage.picker = None;
    } else if !open {
        voice.stage.picker = None;
    }
}

/// Ряд кнопок-переключателей; возвращает выбранное значение, если нажали.
fn pills<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    values: &[T],
    current: T,
    label: impl Fn(T) -> String,
) -> Option<T> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        for value in values {
            if ui
                .selectable_label(*value == current, label(*value))
                .clicked()
            {
                picked = Some(*value);
            }
        }
    });
    picked
}

/// Подпись раздела в окне выбора.
fn caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(text.to_uppercase())
            .size(11.0)
            .strong()
            .color(theme::text_faint()),
    );
}

/// Настройки качества (`StreamQualityControls` оригинала): показываются
/// только варианты, разрешённые инстансом, а отмеченным — то, что
/// трансляция реально получит.
fn quality_controls(ui: &mut egui::Ui, quality: &mut ShareQuality, limits: &StreamingLimits) {
    let effective = quality.effective(limits);
    let mut heights = share_quality::allowed_heights(limits);
    if heights.is_empty() {
        heights = share_quality::RESOLUTIONS.to_vec();
    }
    caption(ui, tr("Разрешение"));
    if let Some(height) = pills(ui, &heights, effective.height, ShareQuality::height_label) {
        quality.height = height;
    }
    let mut rates = share_quality::allowed_rates(limits);
    if rates.is_empty() {
        rates = share_quality::FRAMERATES.to_vec();
    }
    caption(ui, tr("Частота кадров"));
    if let Some(fps) = pills(ui, &rates, effective.fps, |fps| {
        crate::tf!("{fps} к/с", fps = fps)
    }) {
        quality.fps = fps;
    }
    caption(ui, tr("Содержимое"));
    if let Some(mode) = pills(
        ui,
        &[ShareMode::Gaming, ShareMode::Detail],
        quality.mode,
        |mode| match mode {
            ShareMode::Gaming => tr("Плавность (игры, видео)").to_owned(),
            ShareMode::Detail => tr("Чёткость (текст, код)").to_owned(),
        },
    ) {
        quality.mode = mode;
    }
    caption(ui, tr("Кодек"));
    if let Some(codec) = pills(
        ui,
        &[ShareCodec::Vp9, ShareCodec::H264],
        quality.codec,
        |codec| match codec {
            ShareCodec::Vp9 => "VP9".to_owned(),
            ShareCodec::H264 => "H.264".to_owned(),
        },
    ) {
        quality.codec = codec;
    }
    caption(ui, tr("Битрейт"));
    let auto_kbps = ShareQuality {
        custom_bitrate_kbps: None,
        ..*quality
    }
    .settings(limits)
    .bitrate
        / 1000;
    if limits.allow_custom_bitrate {
        let mut custom = effective.custom_bitrate_kbps.is_some();
        ui.horizontal(|ui| {
            if ui.radio_value(&mut custom, false, tr("Авто")).clicked() {
                quality.custom_bitrate_kbps = None;
            }
            if ui.radio_value(&mut custom, true, tr("Свой")).clicked() {
                quality.custom_bitrate_kbps = Some(u32::try_from(auto_kbps).unwrap_or(u32::MAX));
            }
        });
        if let Some(mut kbps) = effective.custom_bitrate_kbps {
            let low = limits.min_bitrate_kbps.max(100);
            let high = limits.max_bitrate_kbps.max(low);
            let step = f64::from(limits.bitrate_step_kbps.max(50));
            if ui
                .add(
                    egui::Slider::new(&mut kbps, low..=high)
                        .step_by(step)
                        .suffix(tr(" кбит/с")),
                )
                .changed()
            {
                quality.custom_bitrate_kbps = Some(kbps);
            }
        }
    }
    let kbps = quality.settings(limits).bitrate / 1000;
    ui.label(theme::muted(crate::tf!(
        "Итоговый битрейт: {mbps} Мбит/с",
        mbps = format!(
            "{:.1}",
            f64::from(u32::try_from(kbps).unwrap_or(u32::MAX)) / 1000.0
        )
    )));
    ui.add_space(4.0);
    ui.checkbox(&mut quality.share_audio, tr("Транслировать звук системы"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_prefers_bigger_tiles() {
        let area = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(1600.0, 450.0));
        let rects = grid(area, 2, 0.0);
        assert_eq!(rects.len(), 2);
        assert!((rects[1].min.x - 800.0).abs() < 0.5, "две плитки в ряд");
        assert!(grid(area, 0, 8.0).is_empty());
    }

    #[test]
    fn fit_keeps_aspect() {
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 400.0));
        let fitted = fit(rect, Vec2::new(1600.0, 900.0));
        assert!((fitted.width() - 400.0).abs() < 0.01);
        assert!((fitted.height() - 225.0).abs() < 0.01);
    }

    #[test]
    fn name_comes_from_identity() {
        assert_eq!(display_name("u1:Alice"), "Alice");
        assert_eq!(display_name("u1"), "u1");
    }
}
