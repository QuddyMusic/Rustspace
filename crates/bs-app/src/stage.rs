//! Сцена звонка: камеры и демонстрации экрана над чатом, выбор источника.
//!
//! Как `VoiceGrid`/`StreamTile` оригинала: чужую трансляцию сначала
//! предлагают посмотреть (и только тогда слышен её звук), камеры видны
//! сразу; щелчок по плитке разворачивает её на всю сцену.

use std::collections::{BTreeSet, HashMap};

use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Vec2};

use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;
use crate::video_media::{Feed, ShareQuality, ShareSource, VideoControl};
use crate::voice::VoiceState;
use crate::voice_media::AudioSource;

/// Ключ плитки: identity и «это экран».
type TileKey = (String, bool);

/// Выбор источника для демонстрации.
#[derive(Debug)]
pub(crate) struct Picker {
    sources: Result<Vec<ShareSource>, String>,
    selected: Option<usize>,
    quality: ShareQuality,
}

/// Состояние сцены между кадрами.
#[derive(Default)]
pub(crate) struct StageState {
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
    pub(crate) fn open_picker(&mut self, quality: ShareQuality) {
        self.picker = Some(Picker {
            sources: VideoControl::screen_sources(),
            selected: None,
            quality,
        });
    }

    /// Звонок закончился.
    pub(crate) fn clear(&mut self) {
        self.textures.clear();
        self.picker = None;
        self.focused = None;
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
    if feeds.is_empty() {
        voice.stage.textures.clear();
        voice.stage.focused = None;
        return;
    }
    // Кадры приходят из другого потока: перерисовываемся с частотой видео.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
    let mut actions = Vec::new();
    egui::Panel::top("call_stage")
        .resizable(true)
        .default_size(300.0)
        .size_range(160.0..=900.0)
        .frame(
            egui::Frame::new()
                .fill(theme::bg_rail())
                .inner_margin(egui::Margin::same(8)),
        )
        .show(ui, |ui| {
            update_textures(ui.ctx(), &mut voice.stage, &feeds);
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
            let area = ui.available_rect_before_wrap();
            let rects = grid(area, shown.len(), 8.0);
            for (feed, rect) in shown.iter().zip(rects) {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                tile(&mut child, feed, voice, &watchers, &mut actions);
            }
            ui.allocate_rect(area, Sense::hover());
        });
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
        }
    }
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
        if pixels.len() != image.size[0] * image.size[1] * 4 {
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
    response.context_menu(|ui| tile_menu(ui, feed, &prefs, watching, actions));
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
                                let kind = if source.window {
                                    tr("Окно")
                                } else {
                                    tr("Экран")
                                };
                                let text = format!(
                                    "{kind}: {} — {}×{}",
                                    source.name, source.width, source.height
                                );
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
            ui.horizontal(|ui| {
                ui.label(tr("Качество"));
                egui::ComboBox::from_id_salt("share_height")
                    .selected_text(format!("{}p", picker.quality.height))
                    .show_ui(ui, |ui| {
                        for height in ShareQuality::HEIGHTS {
                            ui.selectable_value(
                                &mut picker.quality.height,
                                height,
                                format!("{height}p"),
                            );
                        }
                    });
                egui::ComboBox::from_id_salt("share_fps")
                    .selected_text(crate::tf!("{fps} к/с", fps = picker.quality.fps))
                    .show_ui(ui, |ui| {
                        for fps in ShareQuality::RATES {
                            ui.selectable_value(
                                &mut picker.quality.fps,
                                fps,
                                crate::tf!("{fps} к/с", fps = fps),
                            );
                        }
                    });
            });
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
        if let Some(media) = voice.media.as_ref() {
            voice.video.start_screen(media, &source, quality);
        }
        voice.stage.picker = None;
    } else if !open {
        voice.stage.picker = None;
    }
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
