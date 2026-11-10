//! Вложения, стикеры и просмотр картинки на весь экран.

use bs_api::STICKER_PREFIX;
use bs_types::Attachment;
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Vec2};

use crate::download::{Click, Target};
use crate::i18n::tr;
use crate::icons::Icon;
use crate::images::{Download, ImageHub, ImageState, paint_cover};
use crate::theme;

/// Самая большая сторона вложения-картинки в ленте.
const MAX_BOX: Vec2 = Vec2::new(380.0, 300.0);
/// Сторона стикера.
const STICKER_SIDE: f32 = 160.0;
/// Вложения тяжелее не загружаем сами: показываем карточкой файла.
const MAX_AUTO_BYTES: i64 = 20 * 1024 * 1024;

/// То, что ленте нужно знать о вложении.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachmentView {
    /// Имя файла на сервере.
    pub(crate) filename: String,
    /// Имя превью, если сервер его сделал.
    pub(crate) thumbnail: Option<String>,
    /// Имя, которое увидит человек.
    pub(crate) name: String,
    /// Тип содержимого.
    pub(crate) mimetype: String,
    /// Размер в байтах.
    pub(crate) size: i64,
    /// Размеры по данным сервера.
    pub(crate) dims: Option<(u32, u32)>,
}

impl AttachmentView {
    /// Берёт нужное из ответа сервера.
    pub(crate) fn from_api(att: &Attachment) -> Self {
        let dims = match (att.width, att.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => {
                u32::try_from(w).ok().zip(u32::try_from(h).ok())
            }
            _ => None,
        };
        Self {
            filename: att.filename.clone(),
            thumbnail: att.thumbnail_filename.clone(),
            name: att.original_name.clone(),
            mimetype: att.mimetype.clone(),
            size: att.size,
            dims,
        }
    }

    /// Картинка, которую умеем показать (без SVG и прочего).
    pub(crate) fn is_image(&self) -> bool {
        matches!(
            self.mimetype.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        ) && self.size <= MAX_AUTO_BYTES
    }

    fn is_gif(&self) -> bool {
        self.mimetype == "image/gif"
    }
}

/// Число пикселей как `f32` (размеры вложений невелики).
#[allow(clippy::cast_precision_loss, reason = "размеры для раскладки")]
const fn f32_of(value: u32) -> f32 {
    value as f32
}

/// Если сообщение — стикер, возвращает ссылку на него.
#[must_use]
pub(crate) fn sticker_of(body: &str) -> Option<&str> {
    let value = body.trim();
    value.starts_with(STICKER_PREFIX).then_some(value)
}

/// Если сообщение — ссылка на GIF (так отправляет выбор из поиска), возвращает её.
#[must_use]
pub(crate) fn gif_of(body: &str) -> Option<&str> {
    let value = body.trim();
    (!value.contains(char::is_whitespace) && bs_api::AssetPath::external(value).is_some())
        .then_some(value)
}

/// Вписывает `size` в `max`, не увеличивая.
#[must_use]
pub(crate) fn fit(size: [f32; 2], max: Vec2) -> Vec2 {
    let (w, h) = (size[0].max(1.0), size[1].max(1.0));
    let scale = (max.x / w).min(max.y / h).min(1.0);
    Vec2::new((w * scale).max(1.0), (h * scale).max(1.0))
}

/// Размер в человеческом виде.
#[must_use]
pub(crate) fn human_size(bytes: i64) -> String {
    const KIB: f64 = 1024.0;
    #[allow(clippy::cast_precision_loss, reason = "размер нужен лишь для подписи")]
    let b = bytes.max(0) as f64;
    if b < KIB {
        crate::tf!("{bytes} Б", bytes = bytes)
    } else if b < KIB * KIB {
        crate::tf!("{a0} КБ", a0 = format!("{:.0}", b / KIB))
    } else {
        crate::tf!("{a0} МБ", a0 = format!("{:.1}", b / (KIB * KIB)))
    }
}

/// Картинка, которую можно открыть во весь экран: где она на экране и что открывать.
pub(crate) type Hit = (Rect, String);

/// Кнопка скачивания в ленте: где она на экране и что скачивать по клику.
pub(crate) type DownloadHit = (Rect, Click);

/// Размер кнопки скачивания.
const DL_BUTTON: f32 = 28.0;

/// Что вложение значит для скачивания.
fn target_of(att: &AttachmentView) -> Target {
    Target {
        value: att.filename.clone(),
        name: att.name.clone(),
    }
}

/// Состояние скачивания файла.
fn download_of(ui: &egui::Ui, att: &AttachmentView) -> Option<Download> {
    ImageHub::of(ui.ctx()).and_then(|hub| hub.download(&att.filename))
}

/// Рисует круглую кнопку скачивания в `rect` и возвращает её для обработки
/// клика строкой сообщения. Показывает ход: проценты, галочку или ошибку.
fn download_badge(
    ui: &egui::Ui,
    rect: Rect,
    id: egui::Id,
    state: Option<&Download>,
    overlay: bool,
) {
    // Строка сообщения перекрывает виджеты внутри, поэтому наведение
    // определяем по положению указателя, а не по отклику виджета.
    let hover = ui.rect_contains_pointer(rect);
    let fill = if overlay {
        Color32::from_black_alpha(if hover { 200 } else { 150 })
    } else if hover {
        theme::bg_hover()
    } else {
        Color32::TRANSPARENT
    };
    ui.painter()
        .circle_filled(rect.center(), rect.width() / 2.0, fill);
    match state {
        Some(Download::Running { got, total }) => {
            let text =
                Download::percent(*got, *total).map_or_else(|| "…".to_owned(), |p| format!("{p}%"));
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                FontId::proportional(10.5),
                theme::text_bright(),
            );
            ui.ctx().request_repaint();
        }
        Some(Download::Done) => {
            crate::icons::paint(
                ui.painter(),
                rect.center(),
                16.0,
                Icon::Check,
                theme::green(),
            );
        }
        Some(Download::Failed(_)) => {
            crate::icons::paint(
                ui.painter(),
                rect.center(),
                16.0,
                Icon::Download,
                theme::red(),
            );
        }
        None => {
            let color = if hover {
                theme::text_bright()
            } else {
                theme::text_muted()
            };
            crate::icons::paint(ui.painter(), rect.center(), 16.0, Icon::Download, color);
        }
    }
    if hover {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        let tip = match state {
            Some(Download::Running { .. }) => tr("Скачивается…").to_owned(),
            Some(Download::Done) => tr("Сохранено. Нажмите, чтобы скачать ещё раз").to_owned(),
            Some(Download::Failed(why)) => {
                crate::tf!("Не удалось: {why}. Нажмите, чтобы повторить", why = why)
            }
            None => tr("Скачать").to_owned(),
        };
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            id,
            egui::PopupAnchor::Pointer,
        )
        .show(|ui| {
            ui.label(tip);
        });
    }
}

/// Одна кнопка «Скачать все» под несколькими вложениями сообщения.
pub(crate) fn show_download_all(
    ui: &mut egui::Ui,
    attachments: &[AttachmentView],
    downloads: &mut Vec<DownloadHit>,
) {
    if attachments.len() < 2 {
        return;
    }
    let states: Vec<Option<Download>> = attachments.iter().map(|a| download_of(ui, a)).collect();
    let running = states
        .iter()
        .filter(|s| matches!(s, Some(Download::Running { .. })))
        .count();
    let done = states
        .iter()
        .filter(|s| matches!(s, Some(Download::Done)))
        .count();
    let total: i64 = attachments.iter().map(|a| a.size).sum();
    let label = if running > 0 {
        crate::tf!(
            "Скачивается… ({running} из {a0})",
            running = running,
            a0 = attachments.len()
        )
    } else if done == attachments.len() {
        crate::tf!("Все файлы сохранены ({a0})", a0 = attachments.len())
    } else {
        crate::tf!(
            "Скачать все ({a0} · {a1})",
            a0 = attachments.len(),
            a1 = human_size(total)
        )
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(250.0, 30.0), Sense::hover());
    let hover = ui.rect_contains_pointer(rect) && running == 0;
    ui.painter().rect(
        rect,
        crate::theme::rounded(8),
        if hover {
            theme::bg_hover()
        } else {
            theme::bg_bubble()
        },
        egui::Stroke::new(1.0, theme::border()),
        egui::StrokeKind::Inside,
    );
    crate::icons::paint(
        ui.painter(),
        egui::pos2(rect.min.x + 18.0, rect.center().y),
        16.0,
        if done == attachments.len() {
            Icon::Check
        } else {
            Icon::Download
        },
        if done == attachments.len() {
            theme::green()
        } else {
            theme::text_muted()
        },
    );
    ui.painter().text(
        egui::pos2(rect.min.x + 34.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        theme::text(),
    );
    if hover {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if running == 0 {
        downloads.push((
            rect,
            Click::All(attachments.iter().map(target_of).collect()),
        ));
    }
}

/// Вложение в ленте. Возвращает область готовой картинки: клик по ней
/// обрабатывает вызывающий (строка сообщения перекрывает виджеты внутри).
pub(crate) fn show_attachment(
    ui: &mut egui::Ui,
    att: &AttachmentView,
    downloads: &mut Vec<DownloadHit>,
) -> Option<Hit> {
    if !att.is_image() {
        file_card(ui, att, downloads);
        return None;
    }
    // Для статичных картинок берём превью, для GIF — сам файл (анимация).
    let shown = if att.is_gif() {
        att.filename.as_str()
    } else {
        att.thumbnail.as_deref().unwrap_or(&att.filename)
    };
    let hub = ImageHub::of(ui.ctx());
    let state = hub.as_ref().map_or(ImageState::Absent, |h| h.state(shown));
    let guess = att
        .dims
        .map_or([240.0, 160.0], |(w, h)| [f32_of(w), f32_of(h)]);
    let size = match &state {
        ImageState::Ready(image) if att.dims.is_none() || att.thumbnail.is_some() => {
            fit(image.size_f32(), MAX_BOX)
        }
        _ => fit(guess, MAX_BOX),
    };
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let radius = crate::theme::rounded(10);
    let mut failure: Option<String> = None;
    let mut ready = false;
    match state {
        ImageState::Ready(image) => {
            ready = true;
            paint_cover(ui.painter(), rect, &image, radius, true, Color32::WHITE);
        }
        ImageState::Failed(reason) => {
            placeholder(ui, rect, tr("Не удалось показать картинку"));
            failure = Some(reason);
        }
        ImageState::Loading => placeholder(ui, rect, tr("Загрузка…")),
        ImageState::Absent => placeholder(ui, rect, &att.name),
    }
    let response = response.on_hover_text(failure.unwrap_or_else(|| att.name.clone()));
    if ready && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // Кнопка скачивания в углу картинки: видна при наведении и во время скачивания.
    let state = download_of(ui, att);
    if ready && (ui.rect_contains_pointer(rect) || state.is_some()) {
        let button = Rect::from_min_size(
            egui::pos2(rect.max.x - DL_BUTTON - 8.0, rect.min.y + 8.0),
            Vec2::splat(DL_BUTTON),
        );
        download_badge(
            ui,
            button,
            ui.auto_id_with(("dl", &att.filename)),
            state.as_ref(),
            true,
        );
        downloads.push((button, Click::One(target_of(att))));
    }
    ready.then(|| (rect, att.filename.clone()))
}

/// Тип содержимого по расширению имени файла.
pub(crate) fn mime_for_name(name: &str) -> &'static str {
    crate::screens::composer::mime_for(name)
}

/// Стикер в ленте.
pub(crate) fn show_sticker(ui: &mut egui::Ui, value: &str) -> Option<Hit> {
    let state = ImageHub::of(ui.ctx()).map_or(ImageState::Absent, |h| h.state(value));
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(STICKER_SIDE), Sense::hover());
    let mut ready = false;
    match state {
        ImageState::Ready(image) => {
            ready = true;
            let target = fit(image.size_f32(), Vec2::splat(STICKER_SIDE));
            let target = Rect::from_center_size(rect.center(), target);
            paint_cover(
                ui.painter(),
                target,
                &image,
                CornerRadius::ZERO,
                true,
                Color32::WHITE,
            );
        }
        ImageState::Loading => placeholder(ui, rect, tr("Стикер…")),
        _ => placeholder(ui, rect, tr("Стикер недоступен")),
    }
    if ready && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ready.then(|| (rect, value.to_owned()))
}

/// GIF-сообщение в ленте.
pub(crate) fn show_gif(ui: &mut egui::Ui, url: &str) -> Option<Hit> {
    let state = ImageHub::of(ui.ctx()).map_or(ImageState::Absent, |h| h.state(url));
    let size = match &state {
        ImageState::Ready(image) => fit(image.size_f32(), MAX_BOX),
        _ => Vec2::new(240.0, 160.0),
    };
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let mut ready = false;
    match state {
        ImageState::Ready(image) => {
            ready = true;
            paint_cover(
                ui.painter(),
                rect,
                &image,
                crate::theme::rounded(10),
                true,
                Color32::WHITE,
            );
        }
        ImageState::Loading => placeholder(ui, rect, "GIF…"),
        _ => placeholder(ui, rect, tr("GIF недоступен")),
    }
    if ready && response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ready.then(|| (rect, url.to_owned()))
}

fn placeholder(ui: &egui::Ui, rect: Rect, text: &str) {
    ui.painter()
        .rect_filled(rect, crate::theme::rounded(10), theme::bg_bubble());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        theme::truncate_chars(text, 28),
        FontId::proportional(12.5),
        theme::text_faint(),
    );
}

fn file_card(ui: &mut egui::Ui, att: &AttachmentView, downloads: &mut Vec<DownloadHit>) {
    let state = download_of(ui, att);
    egui::Frame::new()
        .fill(theme::bg_bubble())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(crate::theme::rounded(10))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(
                        egui::RichText::new(theme::truncate_chars(&att.name, 40))
                            .color(theme::sky()),
                    );
                    ui.label(theme::muted(human_size(att.size)).size(11.5));
                });
                ui.add_space(10.0);
                let (button, badge) =
                    ui.allocate_exact_size(Vec2::splat(DL_BUTTON), Sense::hover());
                download_badge(ui, button, badge.id, state.as_ref(), false);
                downloads.push((button, Click::One(target_of(att))));
            });
        });
}

/// Картинка на весь экран; закрывается кликом, Esc или крестиком.
/// Возвращает `true`, когда её надо закрыть.
pub(crate) fn viewer(ctx: &egui::Context, value: &str) -> bool {
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::unique("bs_media_viewer"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let (rect, backdrop) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(220));
            let state = ImageHub::of(ctx).map_or(ImageState::Absent, |h| h.state(value));
            match state {
                ImageState::Ready(image) => {
                    let avail = Vec2::new(rect.width() - 80.0, rect.height() - 80.0);
                    let natural = image.size_f32();
                    let size = fit(natural, avail.max(Vec2::splat(64.0)));
                    let target = Rect::from_center_size(rect.center(), size);
                    paint_cover(
                        ui.painter(),
                        target,
                        &image,
                        crate::theme::rounded(6),
                        true,
                        Color32::WHITE,
                    );
                }
                ImageState::Loading => {
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        tr("Загрузка…"),
                        FontId::proportional(16.0),
                        theme::text_muted(),
                    );
                }
                _ => {
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        tr("Не удалось показать картинку"),
                        FontId::proportional(16.0),
                        theme::text_muted(),
                    );
                }
            }
            let close_rect = Rect::from_min_size(
                egui::pos2(rect.right() - 52.0, rect.top() + 12.0),
                Vec2::splat(36.0),
            );
            let close_resp =
                ui.interact(close_rect, ui.auto_id_with("viewer_close"), Sense::click());
            let color = if close_resp.hovered() {
                Color32::WHITE
            } else {
                theme::text_muted()
            };
            crate::icons::paint(
                ui.painter(),
                close_rect.center(),
                24.0,
                crate::icons::Icon::Close,
                color,
            );
            if backdrop.clicked() || close_resp.clicked() {
                close = true;
            }
            backdrop.context_menu(|ui| {
                if ui.button(tr("Копировать картинку")).clicked() {
                    if let Some(hub) = ImageHub::of(ctx) {
                        hub.request_raw(value, crate::images::RawPurpose::Copy);
                    }
                    ui.close();
                }
            });
        });
    close
}

#[cfg(test)]
mod tests {
    use egui::Vec2;

    use super::{fit, human_size, sticker_of};

    #[test]
    fn sticker_detection() {
        assert_eq!(
            sticker_of("sticker:https://h/api/stickers/assets/a.webp"),
            Some("sticker:https://h/api/stickers/assets/a.webp")
        );
        assert_eq!(
            sticker_of("  sticker:/api/stickers/assets/a.webp "),
            Some("sticker:/api/stickers/assets/a.webp")
        );
        assert_eq!(sticker_of("привет sticker:x"), None);
    }

    #[test]
    fn fit_keeps_ratio_and_never_upscales() {
        let big = fit([1000.0, 500.0], Vec2::new(380.0, 300.0));
        assert!((big.x - 380.0).abs() < 0.01 && (big.y - 190.0).abs() < 0.01);
        let small = fit([40.0, 20.0], Vec2::new(380.0, 300.0));
        assert_eq!(small, Vec2::new(40.0, 20.0));
        let zero = fit([0.0, 0.0], Vec2::new(10.0, 10.0));
        assert!(zero.x >= 1.0 && zero.y >= 1.0);
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(human_size(10), "10 Б");
        assert_eq!(human_size(2048), "2 КБ");
        assert_eq!(human_size(3 * 1024 * 1024), "3.0 МБ");
        assert_eq!(human_size(-5), "-5 Б");
    }
}
