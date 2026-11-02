//! Поле ввода сообщения: вложения, эмодзи, GIF и стикеры.
//!
//! Файлы выбираются системным диалогом или перетаскиванием и грузятся на
//! инстанс, которому принадлежит канал. Пока загрузка идёт, отправить
//! сообщение нельзя; готовые вложения уходят вместе с текстом.

#![allow(
    clippy::cast_precision_loss,
    clippy::suboptimal_flops,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "раскладка сеток и полосы прогресса: небольшие числа"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use bs_api::{GifItem, PersonalSticker};
use egui::{
    Align2, Color32, CornerRadius, Id, Key, Margin, Modifiers, Pos2, Rect, RichText, Sense, Vec2,
};

use crate::bridge::{BridgeRequest, ComposeEvent};
use crate::emoji_data::{CATEGORIES, Entry};
use crate::fleet::Bridge;
use crate::icons::Icon;
use crate::images;
use crate::media::human_size;
use crate::theme;
use crate::widgets;

/// Самый большой файл, который клиент согласен читать в память.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
/// Задержка поиска GIF после последнего символа, секунды.
const GIF_DEBOUNCE: f64 = 0.3;
/// Ширина и высота окна выбора.
const PICKER_SIZE: Vec2 = Vec2::new(392.0, 430.0);

/// Что пользователь отправляет.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outgoing {
    /// Текст (или адрес GIF, или токен стикера).
    pub(crate) content: String,
    /// Готовые вложения.
    pub(crate) attachments: Vec<String>,
    /// Не трогать черновик: ушёл GIF или стикер.
    pub(crate) keep_draft: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Picker {
    Emoji,
    Gif,
    Sticker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Open {
    #[default]
    None,
    Plus,
    Picker(Picker),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickMode {
    Files,
    Images,
    Sticker,
}

#[derive(Debug)]
enum StagedState {
    Uploading { sent: u64, total: u64 },
    Done(String),
    Failed(String),
}

#[derive(Debug)]
struct Staged {
    id: u64,
    name: String,
    size: u64,
    origin: usize,
    state: StagedState,
}

#[derive(Debug)]
struct Picked {
    name: String,
    bytes: Result<Vec<u8>, String>,
}

#[derive(Debug, Default)]
struct Gifs {
    enabled: Option<bool>,
    enabled_asked: bool,
    enabled_error: Option<String>,
    checked_at: f64,
    query: String,
    asked: Option<String>,
    typed_at: f64,
    items: Vec<GifItem>,
    next: String,
    loading: bool,
    error: Option<String>,
}

#[derive(Debug, Default)]
struct Stickers {
    items: Option<Vec<PersonalSticker>>,
    asked: bool,
    error: Option<String>,
}

/// Состояние поля ввода.
#[derive(Debug, Default)]
pub(crate) struct Composer {
    next_id: u64,
    /// Время последнего кадра: нужно разбору событий.
    now: f64,
    staged: Vec<Staged>,
    open: Open,
    gifs: Gifs,
    stickers: Stickers,
    emoji_category: usize,
    reading: Option<(PickMode, Receiver<Vec<Picked>>)>,
    /// Инстанс, к которому относится накопленное состояние.
    origin: Option<usize>,
    note: Option<String>,
}

/// Идентификатор поля ввода: нужен, чтобы вернуть в него фокус.
pub(crate) fn edit_id() -> Id {
    Id::unique("composer_edit")
}

/// Тип содержимого по расширению: сервер всё равно определяет его сам.
pub(crate) fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn read_paths(paths: Vec<PathBuf>) -> Vec<Picked> {
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .map_or_else(|| "файл".to_owned(), |n| n.to_string_lossy().into_owned());
            let bytes = match std::fs::metadata(&path) {
                Ok(meta) if meta.len() == 0 => Err("Файл пустой".to_owned()),
                Ok(meta) if meta.len() > MAX_FILE_BYTES => Err("Файл слишком большой".to_owned()),
                Ok(_) => std::fs::read(&path).map_err(|err| err.to_string()),
                Err(err) => Err(err.to_string()),
            };
            Picked { name, bytes }
        })
        .collect()
}

impl Composer {
    /// Забирает события моста и результаты выбора файлов.
    pub(crate) fn poll(&mut self, ctx: &egui::Context, bridge: &mut Bridge) {
        self.now = ctx.input(|i| i.time);
        for event in bridge.take_compose_events() {
            match event {
                ComposeEvent::Progress { id, sent, total } => {
                    if let Some(item) = self.staged.iter_mut().find(|s| s.id == id) {
                        item.state = StagedState::Uploading { sent, total };
                    }
                }
                ComposeEvent::Uploaded { id, result } => {
                    if let Some(item) = self.staged.iter_mut().find(|s| s.id == id) {
                        item.state = match result {
                            Ok(att) => StagedState::Done(att.id),
                            Err(message) => StagedState::Failed(message),
                        };
                    }
                }
                ComposeEvent::GifEnabled(result) => {
                    self.gifs.enabled_asked = false;
                    self.gifs.checked_at = self.now;
                    match result {
                        Ok(enabled) => {
                            self.gifs.enabled = Some(enabled);
                            self.gifs.enabled_error = None;
                        }
                        Err(message) => self.gifs.enabled_error = Some(message),
                    }
                }
                ComposeEvent::Gifs { query, pos, result } => {
                    if self.gifs.asked.as_deref() != Some(query.as_str()) {
                        continue;
                    }
                    self.gifs.loading = false;
                    match result {
                        Ok(page) => {
                            if pos.is_empty() {
                                self.gifs.items = page.results;
                            } else {
                                self.gifs.items.extend(page.results);
                            }
                            self.gifs.next = page.next;
                            self.gifs.error = None;
                        }
                        Err(message) => self.gifs.error = Some(message),
                    }
                }
                ComposeEvent::Stickers(result) => match result {
                    Ok(items) => {
                        self.stickers.items = Some(items);
                        self.stickers.error = None;
                    }
                    Err(message) => {
                        self.stickers.items.get_or_insert_with(Vec::new);
                        self.stickers.error = Some(message);
                    }
                },
                ComposeEvent::StickerChanged(result) => match result {
                    Ok(()) => {
                        self.stickers.error = None;
                        bridge.request(BridgeRequest::Stickers);
                    }
                    Err(message) => self.stickers.error = Some(message),
                },
                ComposeEvent::SendFailed(message) => self.note = Some(message),
            }
        }
        let ready = self
            .reading
            .as_ref()
            .and_then(|(mode, rx)| rx.try_recv().ok().map(|files| (*mode, files)));
        if let Some((mode, files)) = ready {
            self.reading = None;
            for file in files {
                self.accept_file(bridge, mode, file);
            }
            ctx.request_repaint();
        }
    }

    fn accept_file(&mut self, bridge: &Bridge, mode: PickMode, file: Picked) {
        let origin = bridge.focus_index();
        match (file.bytes, mode) {
            (Err(message), _) => self.note = Some(format!("{}: {message}", file.name)),
            (Ok(bytes), PickMode::Sticker) => {
                let name: String = file
                    .name
                    .rsplit_once('.')
                    .map_or(file.name.as_str(), |(stem, _)| stem)
                    .chars()
                    .take(32)
                    .collect();
                bridge.request(BridgeRequest::UploadSticker {
                    name,
                    bytes: Arc::new(bytes),
                });
            }
            (Ok(bytes), _) => {
                self.next_id += 1;
                let id = self.next_id;
                self.staged.push(Staged {
                    id,
                    name: file.name.clone(),
                    size: bytes.len() as u64,
                    origin,
                    state: StagedState::Uploading {
                        sent: 0,
                        total: bytes.len() as u64,
                    },
                });
                let mime = mime_for(&file.name).to_owned();
                if !bridge.request(BridgeRequest::Upload {
                    id,
                    name: file.name,
                    mime,
                    bytes: Arc::new(bytes),
                }) && let Some(item) = self.staged.last_mut()
                {
                    item.state = StagedState::Failed("Нет соединения".to_owned());
                }
            }
        }
    }

    /// Открывает системный диалог выбора файлов (в отдельном потоке).
    fn start_pick(&mut self, ctx: &egui::Context, mode: PickMode) {
        if self.reading.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let dialog = rfd::FileDialog::new();
            let paths = match mode {
                PickMode::Files => dialog.pick_files(),
                PickMode::Images => dialog
                    .add_filter(
                        "Изображения",
                        &["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp"],
                    )
                    .pick_files(),
                PickMode::Sticker => dialog
                    .add_filter("Изображения", &["png", "jpg", "jpeg", "gif", "webp"])
                    .pick_file()
                    .map(|p| vec![p]),
            };
            let files = paths.map(read_paths).unwrap_or_default();
            let _ = tx.send(files);
            ctx.request_repaint();
        });
        self.reading = Some((mode, rx));
    }

    /// Файлы, брошенные на окно.
    fn take_dropped(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() || self.reading.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        let paths: Vec<PathBuf> = dropped.iter().map(|f| f.path().to_path_buf()).collect();
        std::thread::spawn(move || {
            let files = read_paths(paths);
            let _ = tx.send(files);
            repaint.request_repaint();
        });
        self.reading = Some((PickMode::Files, rx));
    }

    fn reset_for(&mut self, origin: usize) {
        if self.origin != Some(origin) {
            self.staged.retain(|s| s.origin == origin);
            self.gifs = Gifs::default();
            self.stickers = Stickers::default();
            self.origin = Some(origin);
        }
    }

    /// Рисует поле и возвращает то, что нужно отправить.
    #[allow(clippy::too_many_lines, reason = "один экран: разметка поля целиком")]
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        bridge: &mut Bridge,
        draft: &mut String,
        hint: &str,
        enabled: bool,
        editing: bool,
    ) -> Option<Outgoing> {
        let ctx = ui.ctx().clone();
        self.reset_for(bridge.focus_index());
        self.poll(&ctx, bridge);
        if enabled && !editing {
            self.take_dropped(&ctx);
        }
        let dragging = ctx.input(|i| !i.raw.hovered_files.is_empty());

        let busy = self
            .staged
            .iter()
            .any(|s| matches!(s.state, StagedState::Uploading { .. }));
        let ready_ids: Vec<String> = self
            .staged
            .iter()
            .filter_map(|s| match &s.state {
                StagedState::Done(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        let has_text = !draft.trim().is_empty();
        let can_send = enabled && !busy && (has_text || (!editing && !ready_ids.is_empty()));

        let mut outgoing = None;
        let mut plus_rect = None;
        let mut emoji_rect = None;
        let edit_id = edit_id();

        egui::Frame::new()
            .fill(theme::BG_BUBBLE)
            .stroke(egui::Stroke::new(
                1.0,
                if dragging { theme::MINT } else { theme::BORDER },
            ))
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                if !editing {
                    self.show_staged(ui);
                }
                if let Some(note) = self.note.clone() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&note).color(theme::ROSE).size(12.5));
                        if ui.small_button("Скрыть").clicked() {
                            self.note = None;
                        }
                    });
                }
                if dragging {
                    ui.label(theme::muted("Отпустите, чтобы прикрепить файлы"));
                }
                ui.horizontal(|ui| {
                    if !editing {
                        let response = ui.add_enabled(
                            enabled,
                            widgets::flat_icon(Icon::Plus, 22.0, theme::TEXT_MUTED),
                        );
                        if response.on_hover_text("Добавить вложение").clicked() {
                            self.open = if self.open == Open::Plus {
                                Open::None
                            } else {
                                Open::Plus
                            };
                        }
                        plus_rect = Some(ui.min_rect());
                    }
                    let tools = if editing { 40.0 } else { 150.0 };
                    let send = if can_send { 40.0 } else { 0.0 };
                    let width = (ui.available_width() - tools - send).max(80.0);
                    let send_pressed = ui.input(|i| {
                        i.key_pressed(Key::Enter) && !i.modifiers.shift && !i.modifiers.command
                    });
                    if send_pressed && ctx.memory(|m| m.has_focus(edit_id)) {
                        ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                        if can_send {
                            outgoing = Some(Outgoing {
                                content: draft.trim().to_owned(),
                                attachments: ready_ids.clone(),
                                keep_draft: false,
                            });
                        }
                    }
                    let edit = ui.add_enabled(
                        enabled,
                        egui::TextEdit::multiline(draft)
                            .id(edit_id)
                            .hint_text(hint)
                            .frame(egui::Frame::NONE)
                            .desired_rows(1)
                            .desired_width(width),
                    );
                    let _ = edit;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if can_send {
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
                            let fill = if response.hovered() {
                                theme::MINT
                            } else {
                                theme::ACCENT
                            };
                            ui.painter().rect_filled(rect, CornerRadius::same(10), fill);
                            crate::icons::paint(
                                ui.painter(),
                                rect.center(),
                                18.0,
                                Icon::Send,
                                Color32::WHITE,
                            );
                            if response.on_hover_text("Отправить").clicked() {
                                outgoing = Some(Outgoing {
                                    content: draft.trim().to_owned(),
                                    attachments: ready_ids.clone(),
                                    keep_draft: false,
                                });
                            }
                        }
                        let smile = ui.add_enabled(
                            enabled,
                            widgets::flat_icon(Icon::Smile, 22.0, theme::TEXT_MUTED),
                        );
                        emoji_rect = Some(smile.rect);
                        if smile.on_hover_text("Эмодзи").clicked() {
                            self.toggle_picker(Picker::Emoji);
                        }
                        if !editing {
                            let sticker = ui.add_enabled(
                                enabled,
                                widgets::flat_icon(Icon::Sticker, 22.0, theme::TEXT_MUTED),
                            );
                            if sticker.on_hover_text("Стикеры").clicked() {
                                self.toggle_picker(Picker::Sticker);
                            }
                            let gif = ui.add_enabled(enabled, gif_button());
                            if gif.on_hover_text("GIF").clicked() {
                                self.toggle_picker(Picker::Gif);
                            }
                        }
                    });
                });
            });

        let ctx2 = ctx.clone();
        match self.open {
            Open::Plus => {
                if let Some(anchor) = plus_rect {
                    self.show_plus_menu(&ctx2, anchor);
                }
            }
            Open::Picker(tab) => {
                if let Some(anchor) = emoji_rect {
                    if let Some(out) = self.show_picker(&ctx2, bridge, anchor, tab, draft, editing)
                    {
                        outgoing = Some(out);
                    }
                }
            }
            Open::None => {}
        }
        if outgoing.as_ref().is_some_and(|o| !o.keep_draft) {
            self.staged
                .retain(|s| !matches!(s.state, StagedState::Done(_)));
        }
        outgoing
    }

    fn toggle_picker(&mut self, tab: Picker) {
        self.open = if self.open == Open::Picker(tab) {
            Open::None
        } else {
            Open::Picker(tab)
        };
    }

    fn show_staged(&mut self, ui: &mut egui::Ui) {
        if self.staged.is_empty() {
            return;
        }
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for item in &self.staged {
                egui::Frame::new()
                    .fill(theme::BG_HOVER)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.set_width(190.0);
                        ui.horizontal(|ui| {
                            let (rect, _) =
                                ui.allocate_exact_size(Vec2::splat(20.0), Sense::hover());
                            let icon = if mime_for(&item.name).starts_with("image/") {
                                Icon::Image
                            } else {
                                Icon::File
                            };
                            crate::icons::paint(
                                ui.painter(),
                                rect.center(),
                                18.0,
                                icon,
                                theme::SKY,
                            );
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(theme::truncate_chars(&item.name, 22))
                                        .color(theme::TEXT)
                                        .size(13.0),
                                );
                                match &item.state {
                                    StagedState::Uploading { sent, total } => {
                                        let fraction = if *total == 0 {
                                            0.0
                                        } else {
                                            (*sent as f64 / *total as f64) as f32
                                        };
                                        ui.add(
                                            egui::ProgressBar::new(fraction)
                                                .desired_width(110.0)
                                                .desired_height(6.0)
                                                .fill(theme::ACCENT),
                                        );
                                    }
                                    StagedState::Done(_) => {
                                        ui.label(
                                            theme::muted(human_size(
                                                i64::try_from(item.size).unwrap_or(i64::MAX),
                                            ))
                                            .size(11.5),
                                        );
                                    }
                                    StagedState::Failed(message) => {
                                        ui.label(
                                            RichText::new(theme::truncate_chars(message, 30))
                                                .color(theme::ROSE)
                                                .size(11.5),
                                        )
                                        .on_hover_text(message);
                                    }
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add(widgets::flat_icon(
                                            Icon::Close,
                                            12.0,
                                            theme::TEXT_FAINT,
                                        ))
                                        .on_hover_text("Убрать")
                                        .clicked()
                                    {
                                        remove = Some(item.id);
                                    }
                                },
                            );
                        });
                    });
            }
        });
        if let Some(id) = remove {
            self.staged.retain(|s| s.id != id);
        }
    }

    fn show_plus_menu(&mut self, ctx: &egui::Context, anchor: Rect) {
        let mut chosen = None;
        let area = egui::Area::new(Id::unique("composer_plus"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor.left_top() - Vec2::new(0.0, 8.0))
            .pivot(Align2::LEFT_BOTTOM)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::BG_SIDEBAR)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(220.0);
                        for (icon, label, mode) in [
                            (Icon::File, "Загрузить файл", PickMode::Files),
                            (Icon::Image, "Загрузить изображение", PickMode::Images),
                        ] {
                            if menu_row(ui, icon, label).clicked() {
                                chosen = Some(mode);
                            }
                        }
                    });
            });
        if let Some(mode) = chosen {
            self.open = Open::None;
            self.start_pick(ctx, mode);
        } else {
            self.close_when_outside(ctx, area.response.rect, anchor);
        }
    }

    fn close_when_outside(&mut self, ctx: &egui::Context, popup: Rect, anchor: Rect) {
        let (pressed, pos, escape) = ctx.input(|i| {
            (
                i.pointer.any_pressed(),
                i.pointer.interact_pos(),
                i.key_pressed(Key::Escape),
            )
        });
        let outside = pressed
            && pos.is_some_and(|p| {
                !popup.contains(p) && !anchor.expand2(Vec2::new(0.0, 6.0)).contains(p)
            });
        if outside || escape {
            self.open = Open::None;
        }
    }

    #[allow(clippy::too_many_lines, reason = "окно выбора: вкладки и сетки")]
    fn show_picker(
        &mut self,
        ctx: &egui::Context,
        bridge: &Bridge,
        anchor: Rect,
        tab: Picker,
        draft: &mut String,
        editing: bool,
    ) -> Option<Outgoing> {
        let mut out = None;
        let mut next_tab = tab;
        // Окно выровнено по правому краю поля, над ним.
        let pos = Pos2::new(anchor.right() + 120.0, anchor.top() - 12.0);
        let area = egui::Area::new(Id::unique("composer_picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .pivot(Align2::RIGHT_BOTTOM)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::BG_SIDEBAR)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_min_size(PICKER_SIZE);
                        ui.set_max_size(PICKER_SIZE);
                        ui.horizontal(|ui| {
                            if widgets::pill_tab(ui, "Эмодзи", tab == Picker::Emoji).clicked()
                            {
                                next_tab = Picker::Emoji;
                            }
                            if !editing {
                                if widgets::pill_tab(ui, "GIF", tab == Picker::Gif).clicked() {
                                    next_tab = Picker::Gif;
                                }
                                if widgets::pill_tab(ui, "Стикеры", tab == Picker::Sticker)
                                    .clicked()
                                {
                                    next_tab = Picker::Sticker;
                                }
                            }
                        });
                        ui.add_space(6.0);
                        match tab {
                            Picker::Emoji => self.emoji_tab(ui, draft),
                            Picker::Gif => out = self.gif_tab(ui, bridge),
                            Picker::Sticker => out = self.sticker_tab(ui, bridge),
                        }
                    });
            });
        self.open = Open::Picker(next_tab);
        if out.is_some() {
            self.open = Open::None;
        } else {
            let wide = anchor.expand2(Vec2::new(200.0, 6.0));
            self.close_when_outside(ctx, area.response.rect, wide);
        }
        out
    }

    fn emoji_tab(&mut self, ui: &mut egui::Ui, draft: &mut String) {
        if let Some(glyph) = emoji_picker(ui, &mut self.emoji_category) {
            draft.push_str(glyph);
            ui.ctx().memory_mut(|m| m.request_focus(edit_id()));
        }
    }

    #[allow(clippy::too_many_lines, reason = "вкладка: поиск, сетка и дозагрузка")]
    fn gif_tab(&mut self, ui: &mut egui::Ui, bridge: &Bridge) -> Option<Outgoing> {
        let now = ui.input(|i| i.time);
        self.now = now;
        // Пока GIF не подтверждены, спрашиваем снова раз в несколько секунд:
        // ключ администратор мог добавить позже, а ошибка сети — пройти.
        if self.gifs.enabled != Some(true)
            && !self.gifs.enabled_asked
            && (self.gifs.checked_at == 0.0 || now - self.gifs.checked_at >= 4.0)
        {
            self.gifs.enabled_asked = bridge.request(BridgeRequest::GifEnabled);
            if !self.gifs.enabled_asked {
                self.gifs.checked_at = now;
            }
        }
        if self.gifs.enabled != Some(true) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs(1));
        }
        match self.gifs.enabled {
            None => {
                if let Some(error) = &self.gifs.enabled_error {
                    theme::empty_state(ui, &format!("Не удалось проверить GIF: {error}"));
                } else {
                    theme::empty_state(ui, "Загрузка…");
                }
                return None;
            }
            Some(false) => {
                theme::empty_state(
                    ui,
                    "Поиск GIF не включён на этом инстансе: администратору нужно указать ключ Klipy в настройках инстанса (вкладка «Инстанс»).",
                );
                return None;
            }
            Some(true) => {}
        }
        let before = self.gifs.query.clone();
        widgets::search_box(ui, &mut self.gifs.query, "Поиск GIF в Klipy");
        if self.gifs.query != before {
            self.gifs.typed_at = now;
        }
        let asked = self.gifs.asked.clone();
        let settled = now - self.gifs.typed_at >= GIF_DEBOUNCE;
        if asked.as_deref() != Some(self.gifs.query.trim()) {
            if settled {
                let query = self.gifs.query.trim().to_owned();
                if bridge.request(BridgeRequest::Gifs {
                    query: query.clone(),
                    pos: String::new(),
                }) {
                    self.gifs.asked = Some(query);
                    self.gifs.loading = true;
                    self.gifs.items.clear();
                    self.gifs.next.clear();
                    self.gifs.error = None;
                }
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(120));
            }
        }
        ui.add_space(6.0);
        if let Some(error) = &self.gifs.error {
            ui.label(RichText::new(error).color(theme::ROSE).size(12.5));
        }
        let mut chosen = None;
        let mut want_more = false;
        let output = egui::ScrollArea::vertical()
            .id_salt("gif_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let gap = 6.0;
                let width = ui.available_width();
                let column = (width - gap) / 2.0;
                let mut heights = [0.0_f32; 2];
                let mut placed = Vec::with_capacity(self.gifs.items.len());
                for item in &self.gifs.items {
                    let ratio = if item.width == 0 || item.height == 0 {
                        0.75
                    } else {
                        item.height as f32 / item.width as f32
                    };
                    let height = (column * ratio).clamp(60.0, 240.0);
                    let side = usize::from(heights[1] < heights[0]);
                    placed.push((side, heights[side], height));
                    heights[side] += height + gap;
                }
                let total = heights[0].max(heights[1]);
                let (area, _) = ui.allocate_exact_size(Vec2::new(width, total), Sense::hover());
                let clip = ui.clip_rect();
                for (item, (side, top, height)) in self.gifs.items.iter().zip(placed) {
                    let x = area.left() + side as f32 * (column + gap);
                    let rect = Rect::from_min_size(
                        Pos2::new(x, area.top() + top),
                        Vec2::new(column, height),
                    );
                    if !rect.intersects(clip) {
                        continue;
                    }
                    let response =
                        ui.interact(rect, ui.scope_id().with(("gif", &item.id)), Sense::click());
                    let painted = images::paint_value(
                        ui.painter(),
                        rect,
                        &item.preview_url,
                        CornerRadius::same(8),
                        true,
                        Color32::WHITE,
                    );
                    if !painted {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(8), theme::BG_HOVER);
                    }
                    if response.hovered() {
                        ui.painter().rect_stroke(
                            rect,
                            CornerRadius::same(8),
                            egui::Stroke::new(2.0, theme::MINT),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if response.on_hover_text(&item.title).clicked() {
                        chosen = Some(item.url.clone());
                    }
                }
                if self.gifs.loading {
                    ui.label(theme::muted("Загрузка…"));
                } else if self.gifs.items.is_empty() && self.gifs.asked.is_some() {
                    theme::empty_state(
                        ui,
                        "Ничего не нашлось. Если пусто даже без запроса — проверьте ключ Klipy в настройках инстанса.",
                    );
                }
            });
        let visible_bottom = output.state.clamped_offset().y + output.inner_rect.height();
        if visible_bottom + 120.0 >= output.content_size.y {
            want_more = true;
        }
        if want_more
            && !self.gifs.loading
            && !self.gifs.next.is_empty()
            && let Some(query) = self.gifs.asked.clone()
        {
            let pos = std::mem::take(&mut self.gifs.next);
            if bridge.request(BridgeRequest::Gifs { query, pos }) {
                self.gifs.loading = true;
            }
        }
        chosen.map(|url| Outgoing {
            content: url,
            attachments: Vec::new(),
            keep_draft: true,
        })
    }

    fn sticker_tab(&mut self, ui: &mut egui::Ui, bridge: &Bridge) -> Option<Outgoing> {
        if !self.stickers.asked {
            self.stickers.asked = bridge.request(BridgeRequest::Stickers);
        }
        let mut chosen = None;
        let mut delete = None;
        let mut add = false;
        ui.horizontal(|ui| {
            ui.label(theme::section_title("Мои стикеры"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(theme::primary_button("Добавить")).clicked() {
                    add = true;
                }
            });
        });
        if let Some(error) = &self.stickers.error {
            ui.label(RichText::new(error).color(theme::ROSE).size(12.5));
        }
        ui.add_space(4.0);
        match &self.stickers.items {
            None => theme::empty_state(ui, "Загрузка…"),
            Some(items) if items.is_empty() => {
                theme::empty_state(
                    ui,
                    "Стикеров пока нет. Добавьте картинку — она станет стикером.",
                );
            }
            Some(items) => {
                egui::ScrollArea::vertical()
                    .id_salt("sticker_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let cell = 88.0;
                        let columns = ((ui.available_width() / cell).floor() as usize).max(1);
                        for row in items.chunks(columns) {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = Vec2::ZERO;
                                for sticker in row {
                                    let (rect, response) =
                                        ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                                    if response.hovered() {
                                        ui.painter().rect_filled(
                                            rect,
                                            CornerRadius::same(10),
                                            theme::BG_ACTIVE,
                                        );
                                    }
                                    images::paint_value(
                                        ui.painter(),
                                        rect.shrink(6.0),
                                        &sticker.token,
                                        CornerRadius::ZERO,
                                        true,
                                        Color32::WHITE,
                                    );
                                    let response = response.on_hover_text(&sticker.name);
                                    if response.clicked() {
                                        chosen = Some(sticker.token.clone());
                                    }
                                    response.context_menu(|ui| {
                                        if ui.button("Удалить стикер").clicked() {
                                            delete = Some(sticker.id.clone());
                                            ui.close();
                                        }
                                    });
                                }
                            });
                        }
                    });
            }
        }
        if add {
            self.start_pick(ui.ctx(), PickMode::Sticker);
        }
        if let Some(id) = delete {
            bridge.request(BridgeRequest::DeleteSticker(id));
        }
        chosen.map(|token| Outgoing {
            content: token,
            attachments: Vec::new(),
            keep_draft: true,
        })
    }
}

/// Сетка эмодзи с вкладками категорий; возвращает выбранный эмодзи.
pub(crate) fn emoji_picker(ui: &mut egui::Ui, category: &mut usize) -> Option<&'static str> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (index, (name, tab_glyph, _)) in CATEGORIES.iter().enumerate() {
            let selected = index == *category;
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
            if selected {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(8), theme::BG_ACTIVE);
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(8), theme::BG_HOVER);
            }
            paint_emoji(ui, rect.shrink(6.0), tab_glyph, CATEGORIES.get(index));
            if response.on_hover_text(*name).clicked() {
                *category = index;
            }
        }
    });
    ui.add_space(4.0);
    let (name, _, entries) = CATEGORIES.get(*category)?;
    ui.label(theme::section_title(name));
    egui::ScrollArea::vertical()
        .id_salt("emoji_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let cell = 38.0;
            let columns = ((ui.available_width() / cell).floor() as usize).max(1);
            for row in entries.chunks(columns) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    for entry in row {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        if response.hovered() {
                            ui.painter()
                                .rect_filled(rect, CornerRadius::same(8), theme::BG_ACTIVE);
                        }
                        paint_texture(ui, rect.shrink(5.0), entry);
                        if response.clicked() {
                            chosen = Some(entry.glyph);
                        }
                    }
                });
            }
        });
    chosen
}

/// Рисует эмодзи картинкой, если она есть; `false` — нужен запасной текст.
pub(crate) fn paint_glyph(ui: &egui::Ui, rect: Rect, glyph: &str) -> bool {
    crate::emoji_data::find(glyph).is_some_and(|entry| {
        paint_texture(ui, rect, entry);
        true
    })
}

/// Текстура эмодзи; декодируется при первом показе и кэшируется.
fn emoji_texture(ctx: &egui::Context, entry: &Entry) -> Option<egui::TextureHandle> {
    let id = Id::unique(("bs_emoji", entry.glyph));
    if let Some(texture) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(texture);
    }
    let decoded = bs_media::decode(entry.png, &bs_media::Limits::default()).ok()?;
    let frame = decoded.frames.first()?;
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [decoded.width as usize, decoded.height as usize],
        &frame.rgba,
    );
    let texture = ctx.load_texture(
        format!("emoji-{}", entry.glyph),
        image,
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|d| d.insert_temp(id, texture.clone()));
    Some(texture)
}

fn paint_texture(ui: &egui::Ui, rect: Rect, entry: &Entry) {
    if let Some(texture) = emoji_texture(ui.ctx(), entry) {
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

/// Эмодзи без селектора вариантов: так сравниваются записи.
fn bare(glyph: &str) -> &str {
    glyph.trim_end_matches('\u{fe0f}')
}

/// Значок вкладки: первый эмодзи категории.
fn paint_emoji(ui: &egui::Ui, rect: Rect, glyph: &str, category: Option<&(&str, &str, &[Entry])>) {
    if let Some(entry) =
        category.and_then(|(_, _, entries)| entries.iter().find(|e| bare(e.glyph) == bare(glyph)))
    {
        paint_texture(ui, rect, entry);
    }
}

/// Кнопка «GIF»: рамка с подписью.
fn gif_button() -> impl egui::Widget {
    |ui: &mut egui::Ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(34.0, 22.0), Sense::click());
        let color = if response.hovered() {
            theme::TEXT_BRIGHT
        } else {
            theme::TEXT_MUTED
        };
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(6),
            egui::Stroke::new(1.5, color),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "GIF",
            egui::FontId::proportional(11.5),
            color,
        );
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

fn menu_row(ui: &mut egui::Ui, icon: Icon, label: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), theme::BG_ACTIVE);
    }
    crate::icons::paint(
        ui.painter(),
        Pos2::new(rect.left() + 18.0, rect.center().y),
        18.0,
        icon,
        theme::TEXT_MUTED,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 38.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        theme::TEXT,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::mime_for;

    #[test]
    fn mime_by_extension() {
        assert_eq!(mime_for("a.PNG"), "image/png");
        assert_eq!(mime_for("clip.mp4"), "video/mp4");
        assert_eq!(mime_for("noext"), "application/octet-stream");
        assert_eq!(mime_for("a.tar.gz"), "application/octet-stream");
    }
}
