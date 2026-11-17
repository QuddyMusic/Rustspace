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
use crate::i18n::tr;
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
    /// Открыта вкладка «Избранное».
    favorites_only: bool,
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
    favorites: crate::gif_favorites::GifFavorites,
    stickers: Stickers,
    emoji_category: usize,
    reading: Option<(PickMode, Receiver<Vec<Picked>>)>,
    /// Инстанс, к которому относится накопленное состояние.
    origin: Option<usize>,
    note: Option<String>,
}

/// Ключ аккаунта для избранных GIF: инстанс + пользователь.
fn favorite_key(bridge: &Bridge, origin: Option<usize>) -> Option<String> {
    let base = bridge.base_at(origin.unwrap_or(0))?;
    let user = bridge.cache_at(origin.unwrap_or(0))?.me().id.clone();
    Some(crate::gif_favorites::account_key(base, &user))
}

/// Звёздочка: контур или залитая.
fn paint_star(painter: &egui::Painter, center: Pos2, radius: f32, filled: bool) {
    let points: Vec<Pos2> = (0..10)
        .map(|i| {
            let angle = std::f32::consts::PI * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
            let r = if i % 2 == 0 { radius } else { radius * 0.42 };
            center + Vec2::new(angle.cos(), angle.sin()) * r
        })
        .collect();
    let gold = Color32::from_rgb(250, 204, 21);
    if filled {
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(center, gold);
        for point in &points {
            mesh.colored_vertex(*point, gold);
        }
        for i in 0..10_u32 {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % 10);
        }
        painter.add(egui::Shape::mesh(mesh));
    }
    painter.add(egui::Shape::closed_line(
        points,
        egui::Stroke::new(1.5, if filled { gold } else { Color32::WHITE }),
    ));
}

/// Вкладки окна выбора; в режиме правки — только эмодзи.
fn picker_tabs(ui: &mut egui::Ui, tab: Picker, editing: bool) -> Picker {
    let mut next = tab;
    ui.horizontal(|ui| {
        if widgets::pill_tab(ui, tr("Эмодзи"), tab == Picker::Emoji).clicked() {
            next = Picker::Emoji;
        }
        if !editing {
            if widgets::pill_tab(ui, "GIF", tab == Picker::Gif).clicked() {
                next = Picker::Gif;
            }
            if widgets::pill_tab(ui, tr("Стикеры"), tab == Picker::Sticker).clicked() {
                next = Picker::Sticker;
            }
        }
    });
    next
}

/// Что сделали в сетке GIF.
#[derive(Default)]
struct GifGrid {
    /// Выбранный GIF (адрес).
    chosen: Option<String>,
    /// GIF, у которого нажали звёздочку.
    toggled: Option<GifItem>,
    /// Долистали до конца — пора дозагрузить.
    want_more: bool,
}

/// Что сделали с одной плиткой.
#[derive(Default)]
struct GifTile {
    chosen: bool,
    toggled: bool,
}

/// Сетка GIF в две колонки «кирпичиками» и подписи под ней.
fn gif_grid(
    ui: &mut egui::Ui,
    list: &[GifItem],
    favorites: &crate::gif_favorites::GifFavorites,
    gifs: &Gifs,
) -> GifGrid {
    let mut result = GifGrid::default();
    let favorites_only = gifs.favorites_only;
    let output = egui::ScrollArea::vertical()
        .id_salt("gif_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
        let gap = 6.0;
        let width = ui.available_width();
        let column = (width - gap) / 2.0;
        let mut heights = [0.0_f32; 2];
        let mut placed = Vec::with_capacity(list.len());
        for item in list {
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
        for (item, (side, top, height)) in list.iter().zip(placed) {
            let x = area.left() + side as f32 * (column + gap);
            let rect = Rect::from_min_size(
                Pos2::new(x, area.top() + top),
                Vec2::new(column, height),
            );
            if !rect.intersects(clip) {
                continue;
            }
                let tile = gif_tile(ui, rect, item, favorites.contains(&item.id));
                if tile.chosen {
                    result.chosen = Some(item.url.clone());
                }
                if tile.toggled {
                    result.toggled = Some(item.clone());
                }
            }
        if favorites_only {
            if list.is_empty() {
                theme::empty_state(
                    ui,
                    tr("Здесь пока пусто. Наведите на GIF в поиске и нажмите звёздочку — он появится тут."),
                );
            }
        } else if gifs.loading {
            ui.label(theme::muted(tr("Загрузка…")));
        } else if gifs.items.is_empty() && gifs.asked.is_some() {
            theme::empty_state(
                ui,
                tr("Ничего не нашлось. Если пусто даже без запроса — проверьте ключ Klipy в настройках инстанса."),
            );
        }
        });
    let visible_bottom = output.state.clamped_offset().y + output.inner_rect.height();
    result.want_more = visible_bottom + 120.0 >= output.content_size.y;
    result
}

/// Плитка GIF: превью, подсказка и звёздочка «в избранное».
fn gif_tile(ui: &egui::Ui, rect: Rect, item: &GifItem, starred: bool) -> GifTile {
    let mut tile = GifTile::default();
    let response = ui.interact(rect, ui.scope_id().with(("gif", &item.id)), Sense::click());
    let mut painted = images::paint_value(
        ui.painter(),
        rect,
        &item.preview_url,
        crate::theme::rounded(8),
        true,
        Color32::WHITE,
    );
    let mut failure = None;
    if !painted {
        (painted, failure) = paint_gif_fallback(ui, rect, item);
    }
    if !painted {
        ui.painter()
            .rect_filled(rect, crate::theme::rounded(8), theme::bg_hover());
    }
    if response.hovered() {
        ui.painter().rect_stroke(
            rect,
            crate::theme::rounded(8),
            egui::Stroke::new(2.0, theme::mint()),
            egui::StrokeKind::Inside,
        );
    }
    let hint = match (&failure, painted) {
        (Some(message), false) => crate::tf!(
            "{a0}\nНе загрузилось: {message}",
            a0 = item.title,
            message = message
        ),
        _ => item.title.clone(),
    };
    let star = Rect::from_min_size(rect.right_top() + Vec2::new(-30.0, 4.0), Vec2::splat(26.0));
    let tile_hovered = response.hovered();
    if response.on_hover_text(hint).clicked() {
        tile.chosen = true;
    }
    if tile_hovered || starred || ui.rect_contains_pointer(rect) {
        let button = ui.interact(
            star,
            ui.scope_id().with(("gif-star", &item.id)),
            Sense::click(),
        );
        ui.painter().circle_filled(
            star.center(),
            13.0,
            Color32::from_black_alpha(if button.hovered() { 190 } else { 130 }),
        );
        paint_star(ui.painter(), star.center(), 8.0, starred);
        if button
            .on_hover_text(if starred {
                tr("Убрать из избранного")
            } else {
                tr("В избранное")
            })
            .clicked()
        {
            tile.toggled = true;
        }
    }
    tile
}

/// Что можно сделать в поле в этом кадре.
struct RowState {
    can_send: bool,
    ready_ids: Vec<String>,
    enabled: bool,
    editing: bool,
}

impl RowState {
    /// Отправка текущего черновика с готовыми вложениями.
    fn outgoing(&self, draft: &str) -> Outgoing {
        Outgoing {
            content: draft.trim().to_owned(),
            attachments: self.ready_ids.clone(),
            keep_draft: false,
        }
    }
}

/// Круглая кнопка «Отправить»; `true` — нажата.
fn send_button(ui: &mut egui::Ui) -> bool {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    let fill = if response.hovered() {
        theme::mint()
    } else {
        theme::accent()
    };
    ui.painter()
        .rect_filled(rect, crate::theme::rounded(10), fill);
    crate::icons::paint(
        ui.painter(),
        rect.center(),
        18.0,
        Icon::Send,
        Color32::WHITE,
    );
    response.on_hover_text(tr("Отправить")).clicked()
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
            let name = path.file_name().map_or_else(
                || tr("файл").to_owned(),
                |n| n.to_string_lossy().into_owned(),
            );
            let bytes = match std::fs::metadata(&path) {
                Ok(meta) if meta.len() == 0 => Err(tr("Файл пустой").to_owned()),
                Ok(meta) if meta.len() > MAX_FILE_BYTES => {
                    Err(tr("Файл слишком большой").to_owned())
                }
                Ok(_) => std::fs::read(&path).map_err(|err| err.to_string()),
                Err(err) => Err(err.to_string()),
            };
            Picked { name, bytes }
        })
        .collect()
}

/// Картинка из буфера обмена как PNG-файл.
fn png_of(image: &egui::ColorImage) -> Picked {
    let rgba: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(egui::Color32::to_srgba_unmultiplied)
        .collect();
    let width = u32::try_from(image.size[0]).unwrap_or(0);
    let height = u32::try_from(image.size[1]).unwrap_or(0);
    Picked {
        name: "image.png".to_owned(),
        bytes: bs_media::encode_png(&rgba, width, height).map_err(|err| err.to_string()),
    }
}

/// Превью не пришло: рисуем сам GIF и возвращаем причину сбоя.
fn paint_gif_fallback(ui: &egui::Ui, rect: Rect, item: &GifItem) -> (bool, Option<String>) {
    let Some(hub) = images::ImageHub::of(ui.ctx()) else {
        return (false, None);
    };
    let images::ImageState::Failed(message) = hub.state(&item.preview_url) else {
        return (false, None);
    };
    let painted = images::paint_value(
        ui.painter(),
        rect,
        &item.url,
        crate::theme::rounded(8),
        true,
        Color32::WHITE,
    );
    (painted, Some(message))
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
                    item.state = StagedState::Failed(tr("Нет соединения").to_owned());
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
                        tr("Изображения"),
                        &["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp"],
                    )
                    .pick_files(),
                PickMode::Sticker => dialog
                    .add_filter(tr("Изображения"), &["png", "jpg", "jpeg", "gif", "webp"])
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

    /// Картинка или файлы из буфера обмена по Ctrl+V.
    ///
    /// Текстовая вставка обрабатывается самим полем. Картинку (скриншот,
    /// «копировать изображение») оболочка отдаёт готовым событием; файлы из
    /// проводника она не отдаёт, их читаем сами, если вставки текста не было.
    fn take_pasted(&mut self, ctx: &egui::Context) {
        let (images, command, text) = ctx.input(|i| {
            let mut images = Vec::new();
            let mut command = false;
            let mut text = false;
            for event in &i.events {
                match event {
                    egui::Event::PasteImage(image) => images.push(image.clone()),
                    egui::Event::Paste(value) => text |= !value.is_empty(),
                    egui::Event::Key {
                        key: egui::Key::V,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.command => command = true,
                    _ => {}
                }
            }
            (images, command, text)
        });
        let files = command && !text && images.is_empty();
        if (images.is_empty() && !files) || self.reading.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let mut picked: Vec<Picked> = images.iter().map(|image| png_of(image)).collect();
            if files && let Some(paths) = crate::clipboard::files() {
                picked.extend(read_paths(paths));
            }
            let _ = tx.send(picked);
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

    /// Этот GIF из ленты уже в избранном?
    pub(crate) fn is_favorite_gif(&mut self, bridge: &Bridge, url: &str) -> bool {
        if let Some(key) = favorite_key(bridge, self.origin) {
            self.favorites.select(&key);
        }
        self.favorites.contains_url(url)
    }

    /// Добавляет GIF из ленты в избранное или убирает; возвращает текст для
    /// уведомления.
    pub(crate) fn toggle_favorite_gif(
        &mut self,
        bridge: &Bridge,
        url: &str,
        size: Option<[u32; 2]>,
    ) -> String {
        let Some(key) = favorite_key(bridge, self.origin) else {
            return tr("Не удалось определить аккаунт для избранного").to_owned();
        };
        self.favorites.select(&key);
        match self.favorites.toggle_url(url, size) {
            Ok(true) => tr("GIF добавлен в избранное").to_owned(),
            Ok(false) => tr("GIF убран из избранного").to_owned(),
            Err(err) => crate::tf!("Избранное не сохранилось на диск: {err}", err = err),
        }
    }

    /// Рисует поле и возвращает то, что нужно отправить.
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
            self.take_pasted(&ctx);
        }
        let dragging = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let row = self.row_state(draft, enabled, editing);
        let mut outgoing = None;
        let mut anchors = (None, None);
        let frame = theme::floating(theme::bg_bubble(), 16);
        let frame = if dragging {
            frame.stroke(egui::Stroke::new(1.0, theme::mint()))
        } else {
            frame
        };
        frame
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                self.frame_notes(ui, dragging, editing);
                anchors = self.input_row(ui, draft, hint, &row, &mut outgoing);
            });
        let (plus_rect, emoji_rect) = anchors;
        match self.open {
            Open::Plus => {
                if let Some(anchor) = plus_rect {
                    self.show_plus_menu(&ctx, anchor);
                }
            }
            Open::Picker(tab) => {
                if let Some(anchor) = emoji_rect
                    && let Some(out) = self.show_picker(&ctx, bridge, anchor, tab, draft, editing)
                {
                    outgoing = Some(out);
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

    /// Можно ли отправлять и какие вложения уже загружены.
    fn row_state(&self, draft: &str, enabled: bool, editing: bool) -> RowState {
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
        RowState {
            can_send: enabled && !busy && (has_text || (!editing && !ready_ids.is_empty())),
            ready_ids,
            enabled,
            editing,
        }
    }

    /// Вложения, ошибка и подсказка про перетаскивание над полем.
    fn frame_notes(&mut self, ui: &mut egui::Ui, dragging: bool, editing: bool) {
        if !editing {
            self.show_staged(ui);
        }
        if let Some(note) = self.note.clone() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&note).color(theme::rose()).size(12.5));
                if ui.small_button(tr("Скрыть")).clicked() {
                    self.note = None;
                }
            });
        }
        if dragging {
            ui.label(theme::muted(tr("Отпустите, чтобы прикрепить файлы")));
        }
    }

    /// Строка поля: «+», текст и кнопки. Возвращает, где «+» и эмодзи
    /// (к ним крепятся всплывающие окна).
    fn input_row(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut String,
        hint: &str,
        row: &RowState,
        outgoing: &mut Option<Outgoing>,
    ) -> (Option<egui::Rect>, Option<egui::Rect>) {
        let edit_id = edit_id();
        let mut plus_rect = None;
        let mut emoji_rect = None;
        ui.horizontal(|ui| {
            if !row.editing {
                let response = ui.add_enabled(
                    row.enabled,
                    widgets::flat_icon(Icon::Plus, 22.0, theme::text_muted()),
                );
                if response.on_hover_text(tr("Добавить вложение")).clicked() {
                    self.open = if self.open == Open::Plus {
                        Open::None
                    } else {
                        Open::Plus
                    };
                }
                plus_rect = Some(ui.min_rect());
            }
            let tools = if row.editing { 40.0 } else { 150.0 };
            let send = if row.can_send { 40.0 } else { 0.0 };
            let width = (ui.available_width() - tools - send).max(80.0);
            let send_pressed = ui
                .input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift && !i.modifiers.command);
            if send_pressed && ui.ctx().memory(|m| m.has_focus(edit_id)) {
                ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
                if row.can_send {
                    *outgoing = Some(row.outgoing(draft));
                }
            }
            ui.add_enabled(
                row.enabled,
                egui::TextEdit::multiline(draft)
                    .id(edit_id)
                    .hint_text(hint)
                    .frame(egui::Frame::NONE)
                    .desired_rows(1)
                    .desired_width(width),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if row.can_send && send_button(ui) {
                    *outgoing = Some(row.outgoing(draft));
                }
                emoji_rect = Some(self.tool_buttons(ui, row));
            });
        });
        (plus_rect, emoji_rect)
    }

    /// Эмодзи, стикеры и GIF справа от поля; возвращает место кнопки эмодзи.
    fn tool_buttons(&mut self, ui: &mut egui::Ui, row: &RowState) -> egui::Rect {
        let smile = ui.add_enabled(
            row.enabled,
            widgets::flat_icon(Icon::Smile, 22.0, theme::text_muted()),
        );
        let rect = smile.rect;
        if smile.on_hover_text(tr("Эмодзи")).clicked() {
            self.toggle_picker(Picker::Emoji);
        }
        if !row.editing {
            let sticker = ui.add_enabled(
                row.enabled,
                widgets::flat_icon(Icon::Sticker, 22.0, theme::text_muted()),
            );
            if sticker.on_hover_text(tr("Стикеры")).clicked() {
                self.toggle_picker(Picker::Sticker);
            }
            let gif = ui.add_enabled(row.enabled, gif_button());
            if gif.on_hover_text("GIF").clicked() {
                self.toggle_picker(Picker::Gif);
            }
        }
        rect
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
                    .fill(theme::bg_hover())
                    .corner_radius(crate::theme::rounded(10))
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
                                theme::sky(),
                            );
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(theme::truncate_chars(&item.name, 22))
                                        .color(theme::text())
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
                                                .fill(theme::accent()),
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
                                                .color(theme::rose())
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
                                            theme::text_faint(),
                                        ))
                                        .on_hover_text(tr("Убрать"))
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
                theme::floating(theme::bg_sidebar(), 12)
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(220.0);
                        for (icon, label, mode) in [
                            (Icon::File, tr("Загрузить файл"), PickMode::Files),
                            (Icon::Image, tr("Загрузить изображение"), PickMode::Images),
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
                theme::floating(theme::bg_sidebar(), 14)
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_min_size(PICKER_SIZE);
                        ui.set_max_size(PICKER_SIZE);
                        next_tab = picker_tabs(ui, tab, editing);
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

    fn gif_tab(&mut self, ui: &mut egui::Ui, bridge: &Bridge) -> Option<Outgoing> {
        let now = ui.input(|i| i.time);
        self.now = now;
        if let Some(key) = favorite_key(bridge, self.origin) {
            self.favorites.select(&key);
        }
        ui.horizontal(|ui| {
            if widgets::pill_tab(ui, tr("Поиск"), !self.gifs.favorites_only).clicked() {
                self.gifs.favorites_only = false;
            }
            if widgets::pill_tab(ui, tr("Избранное"), self.gifs.favorites_only).clicked() {
                self.gifs.favorites_only = true;
            }
        });
        ui.add_space(4.0);
        if !self.gif_gate(ui, bridge, now) {
            return None;
        }
        let favorites_only = self.gifs.favorites_only;
        if !favorites_only {
            self.gif_search(ui, bridge, now);
        }
        ui.add_space(6.0);
        if let Some(error) = self.gifs.error.as_ref().filter(|_| !favorites_only) {
            ui.label(RichText::new(error).color(theme::rose()).size(12.5));
        }
        if let Some(note) = &self.note {
            ui.label(RichText::new(note).color(theme::rose()).size(12.5));
        }
        let list: &[GifItem] = if favorites_only {
            self.favorites.items()
        } else {
            &self.gifs.items
        };
        let grid = gif_grid(ui, list, &self.favorites, &self.gifs);
        if let Some(item) = grid.toggled {
            self.note = self
                .favorites
                .toggle(&item)
                .err()
                .map(|err| crate::tf!("Избранное не сохранилось на диск: {err}", err = err));
        }
        if grid.want_more
            && !favorites_only
            && !self.gifs.loading
            && !self.gifs.next.is_empty()
            && let Some(query) = self.gifs.asked.clone()
        {
            let pos = std::mem::take(&mut self.gifs.next);
            if bridge.request(BridgeRequest::Gifs { query, pos }) {
                self.gifs.loading = true;
            }
        }
        grid.chosen.map(|url| Outgoing {
            content: url,
            attachments: Vec::new(),
            keep_draft: true,
        })
    }

    /// Включён ли поиск GIF (избранному ключ не нужен); если нет —
    /// показывает почему и раз в несколько секунд спрашивает снова.
    fn gif_gate(&mut self, ui: &mut egui::Ui, bridge: &Bridge, now: f64) -> bool {
        // Ключ администратор мог добавить позже, а ошибка сети — пройти.
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
        let gate = if self.gifs.favorites_only {
            Some(true)
        } else {
            self.gifs.enabled
        };
        match gate {
            None => {
                if let Some(error) = &self.gifs.enabled_error {
                    theme::empty_state(
                        ui,
                        &crate::tf!("Не удалось проверить GIF: {error}", error = error),
                    );
                } else {
                    theme::empty_state(ui, tr("Загрузка…"));
                }
                false
            }
            Some(false) => {
                theme::empty_state(
                    ui,
                    tr(
                        "Поиск GIF не включён на этом инстансе: администратору нужно указать ключ Klipy в настройках инстанса (вкладка «Инстанс»).",
                    ),
                );
                false
            }
            Some(true) => true,
        }
    }

    /// Строка поиска; запрос уходит, когда пользователь перестал печатать.
    fn gif_search(&mut self, ui: &mut egui::Ui, bridge: &Bridge, now: f64) {
        let before = self.gifs.query.clone();
        widgets::search_box(ui, &mut self.gifs.query, tr("Поиск GIF в Klipy"));
        if self.gifs.query != before {
            self.gifs.typed_at = now;
        }
        let settled = now - self.gifs.typed_at >= GIF_DEBOUNCE;
        if self.gifs.asked.as_deref() == Some(self.gifs.query.trim()) {
            return;
        }
        if !settled {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(120));
            return;
        }
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
    }

    fn sticker_tab(&mut self, ui: &mut egui::Ui, bridge: &Bridge) -> Option<Outgoing> {
        if !self.stickers.asked {
            self.stickers.asked = bridge.request(BridgeRequest::Stickers);
        }
        let mut chosen = None;
        let mut delete = None;
        let mut add = false;
        ui.horizontal(|ui| {
            ui.label(theme::section_title(tr("Мои стикеры")));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(theme::primary_button(tr("Добавить"))).clicked() {
                    add = true;
                }
            });
        });
        if let Some(error) = &self.stickers.error {
            ui.label(RichText::new(error).color(theme::rose()).size(12.5));
        }
        ui.add_space(4.0);
        match &self.stickers.items {
            None => theme::empty_state(ui, tr("Загрузка…")),
            Some(items) if items.is_empty() => {
                theme::empty_state(
                    ui,
                    tr("Стикеров пока нет. Добавьте картинку — она станет стикером."),
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
                                            crate::theme::rounded(10),
                                            theme::bg_active(),
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
                                        if ui.button(tr("Удалить стикер")).clicked() {
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
                    .rect_filled(rect, crate::theme::rounded(8), theme::bg_active());
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(rect, crate::theme::rounded(8), theme::bg_hover());
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
                            ui.painter().rect_filled(
                                rect,
                                crate::theme::rounded(8),
                                theme::bg_active(),
                            );
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
            theme::text_bright()
        } else {
            theme::text_muted()
        };
        ui.painter().rect_stroke(
            rect,
            crate::theme::rounded(6),
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
            .rect_filled(rect, crate::theme::rounded(8), theme::bg_active());
    }
    crate::icons::paint(
        ui.painter(),
        Pos2::new(rect.left() + 18.0, rect.center().y),
        18.0,
        icon,
        theme::text_muted(),
    );
    ui.painter().text(
        Pos2::new(rect.left() + 38.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        theme::text(),
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
