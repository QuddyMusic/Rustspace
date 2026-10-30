//! Тема оформления: тёмная палитра в духе Discord и общие виджеты.
//!
//! Здесь нет бизнес-логики: только цвета, стили `egui` и мелкие
//! отрисовщики (аватар-заглушка, круглая кнопка, плашка статуса). Чистые
//! функции (цвет по имени, инициалы, время) покрыты тестами.

use bs_types::UserStatus;
use egui::{Align, Color32, CornerRadius, FontId, Layout, Margin, Sense, Stroke, TextStyle};

/// Фон приложения и колонки пространств (`--bg-base`, чуть светлее).
pub(crate) const BG_RAIL: Color32 = Color32::from_rgb(0x0e, 0x0e, 0x14);
/// Колонка каналов и панель участников (`--bg-channel`).
pub(crate) const BG_SIDEBAR: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x23);
/// Лента сообщений (`--bg-chat`).
pub(crate) const BG_CHAT: Color32 = Color32::from_rgb(0x13, 0x13, 0x1a);
/// Утопленные поля ввода (`--bg-input`).
pub(crate) const BG_INPUT: Color32 = Color32::from_rgb(0x11, 0x11, 0x18);
/// Плавающее поле ввода сообщения: «стеклянный пузырь».
pub(crate) const BG_BUBBLE: Color32 = Color32::from_rgb(0x1d, 0x1d, 0x27);
/// Наведение на элемент списка.
pub(crate) const BG_HOVER: Color32 = Color32::from_rgb(0x21, 0x21, 0x2c);
/// Выбранный элемент списка (`--bg-elevated`).
pub(crate) const BG_ACTIVE: Color32 = Color32::from_rgb(0x25, 0x25, 0x30);
/// Тонкая граница: белый с ~7% непрозрачности поверх тёмного.
pub(crate) const BORDER: Color32 = Color32::from_rgb(0x28, 0x28, 0x32);
/// Основной текст (`--txt`).
pub(crate) const TEXT: Color32 = Color32::from_rgb(0xe4, 0xe4, 0xea);
/// Приглушённый текст (`--txt2`).
pub(crate) const TEXT_MUTED: Color32 = Color32::from_rgb(0xa0, 0xa0, 0xaa);
/// Третичный текст: подсказки, категории (`--txt3`).
pub(crate) const TEXT_FAINT: Color32 = Color32::from_rgb(0x6d, 0x6d, 0x7c);
/// Яркий текст имён и заголовков (`--txt`).
pub(crate) const TEXT_BRIGHT: Color32 = Color32::from_rgb(0xef, 0xef, 0xef);
/// Брендовый акцент «лаванда» (`--accent-primary`).
pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x7c, 0x6c, 0xf6);
/// Мятный: позитивные действия (`--accent-mint`).
pub(crate) const MINT: Color32 = Color32::from_rgb(0x86, 0xef, 0xac);
/// Тёмная мятная подложка под мятный текст (кнопка «Добавить»).
pub(crate) const MINT_BG: Color32 = Color32::from_rgb(0x17, 0x3b, 0x21);
/// Розовый: непрочитанное и ошибки (`--accent-rose`).
pub(crate) const ROSE: Color32 = Color32::from_rgb(0xfd, 0xa4, 0xaf);
/// Янтарный: неактивен, предупреждения (`--accent-amber`).
pub(crate) const AMBER: Color32 = Color32::from_rgb(0xfc, 0xd3, 0x4d);
/// Онлайн.
pub(crate) const GREEN: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);
/// Не беспокоить.
pub(crate) const RED: Color32 = Color32::from_rgb(0xf8, 0x71, 0x71);
/// Офлайн.
pub(crate) const GRAY: Color32 = Color32::from_rgb(0x6d, 0x6d, 0x7c);

/// Применяет тему к контексту `egui`.
pub(crate) fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG_CHAT;
    visuals.window_fill = BG_BUBBLE;
    visuals.extreme_bg_color = BG_INPUT;
    visuals.faint_bg_color = BG_SIDEBAR;
    visuals.override_text_color = Some(TEXT);
    visuals.hyperlink_color = Color32::from_rgb(0x7d, 0xd3, 0xfc);
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0, TEXT_BRIGHT);
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(10);

    let radius = CornerRadius::same(8);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = BG_CHAT;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.noninteractive.corner_radius = radius;
    widgets.inactive.bg_fill = BG_ACTIVE;
    widgets.inactive.weak_bg_fill = BG_ACTIVE;
    widgets.inactive.bg_stroke = Stroke::NONE;
    widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    widgets.inactive.corner_radius = radius;
    widgets.hovered.bg_fill = Color32::from_rgb(0x2e, 0x2e, 0x3b);
    widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2e, 0x2e, 0x3b);
    widgets.hovered.bg_stroke = Stroke::NONE;
    widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_BRIGHT);
    widgets.hovered.corner_radius = radius;
    widgets.active.bg_fill = ACCENT;
    widgets.active.weak_bg_fill = ACCENT;
    widgets.active.bg_stroke = Stroke::NONE;
    widgets.active.fg_stroke = Stroke::new(1.0, TEXT_BRIGHT);
    widgets.active.corner_radius = radius;
    widgets.open.bg_fill = BG_ACTIVE;
    widgets.open.weak_bg_fill = BG_ACTIVE;
    widgets.open.corner_radius = radius;
    ctx.set_visuals(visuals);

    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.window_margin = Margin::same(12);
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(15.0));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::proportional(14.5));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::proportional(20.0));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::proportional(12.0));
    });
}

/// Заливка основной кнопки действия.
pub(crate) fn primary_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(text.into())
            .color(Color32::WHITE)
            .strong(),
    )
    .fill(ACCENT)
    .corner_radius(CornerRadius::same(8))
}

/// Приглушённая подпись.
#[must_use]
pub(crate) fn muted(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).color(TEXT_MUTED)
}

/// Цвет индикатора присутствия.
#[must_use]
pub(crate) const fn status_color(status: UserStatus) -> Color32 {
    match status {
        UserStatus::Online => GREEN,
        UserStatus::Idle => AMBER,
        UserStatus::DoNotDisturb => RED,
        UserStatus::Offline => GRAY,
    }
}

/// Палитра аватаров-заглушек: насыщенные тона, на которых читается белая буква.
const AVATAR_PALETTE: [Color32; 8] = [
    Color32::from_rgb(0x8f, 0x7c, 0xf7),
    Color32::from_rgb(0xe5, 0x55, 0x7f),
    Color32::from_rgb(0xf0, 0x8a, 0x3c),
    Color32::from_rgb(0x3d, 0xbf, 0x86),
    Color32::from_rgb(0x4a, 0xa8, 0xe8),
    Color32::from_rgb(0xd9, 0xa2, 0x3b),
    Color32::from_rgb(0xc2, 0x64, 0xd6),
    Color32::from_rgb(0x5b, 0x7b, 0xe0),
];

/// Устойчивый цвет по строке (FNV-1a): одно имя — один цвет.
#[must_use]
pub(crate) fn color_for(seed: &str) -> Color32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in seed.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    AVATAR_PALETTE[(hash as usize) % AVATAR_PALETTE.len()]
}

/// До двух заглавных букв из имени для аватара-заглушки.
#[must_use]
pub(crate) fn initials(name: &str) -> String {
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '-' || c == '_' || c == '.')
        .filter(|w| !w.is_empty())
        .collect();
    let mut out = String::new();
    match words.as_slice() {
        [] => out.push('?'),
        [one] => out.extend(one.chars().take(2).flat_map(char::to_uppercase)),
        [first, second, ..] => {
            out.extend(first.chars().take(1).flat_map(char::to_uppercase));
            out.extend(second.chars().take(1).flat_map(char::to_uppercase));
        }
    }
    out
}

/// Рисует круглый аватар-заглушку (инициалы на цветном фоне) и, если
/// задан статус, индикатор присутствия в правом нижнем углу.
pub(crate) fn avatar(
    ui: &mut egui::Ui,
    name: &str,
    size: f32,
    status: Option<UserStatus>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    paint_avatar(ui.painter(), rect, name, status, false);
    response
}

/// Отрисовка аватара в заданный прямоугольник.
pub(crate) fn paint_avatar(
    painter: &egui::Painter,
    rect: egui::Rect,
    name: &str,
    status: Option<UserStatus>,
    dim: bool,
) {
    let size = rect.width();
    let fill = if dim {
        color_for(name).gamma_multiply(0.45)
    } else {
        color_for(name)
    };
    painter.circle_filled(rect.center(), size / 2.0, fill);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initials(name),
        FontId::proportional(size * 0.42),
        if dim { TEXT_MUTED } else { Color32::WHITE },
    );
    if let Some(status) = status {
        let dot = (size * 0.17).max(4.0);
        let center = rect.right_bottom() - egui::vec2(size * 0.12, size * 0.12);
        painter.circle_filled(center, dot + 2.0, BG_SIDEBAR);
        painter.circle_filled(center, dot, status_color(status));
    }
}

/// Кнопка позитивного действия: тёмно-мятная подложка и мятный текст.
pub(crate) fn positive_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new(text.into()).color(MINT).strong())
        .fill(MINT_BG)
        .corner_radius(CornerRadius::same(8))
}

/// Рисует знак бренда — клавишу Backspace со стрелкой и крестиком.
fn paint_brand_mark(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    fill: Color32,
    cut: Color32,
) {
    let u = size / 24.0;
    let p = |x: f32, y: f32| center + egui::vec2((x - 12.0) * u, (y - 12.0) * u);
    painter.add(egui::Shape::convex_polygon(
        vec![
            p(2.0, 12.0),
            p(8.5, 5.0),
            p(21.5, 5.0),
            p(21.5, 19.0),
            p(8.5, 19.0),
        ],
        fill,
        Stroke::NONE,
    ));
    let stroke = Stroke::new((2.0 * u).max(1.5), cut);
    painter.line_segment([p(11.5, 9.0), p(16.5, 15.0)], stroke);
    painter.line_segment([p(16.5, 9.0), p(11.5, 15.0)], stroke);
}

/// Плитка «домой» (личные сообщения) в колонке пространств: лавандовый
/// квадрат со знаком бренда.
pub(crate) fn rail_home(ui: &mut egui::Ui, selected: bool, unread: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 50.0), Sense::click());
    let icon = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(46.0));
    let radius: u8 = if selected || response.hovered() {
        14
    } else {
        16
    };
    let painter = ui.painter();
    painter.rect_filled(icon, CornerRadius::same(radius), ACCENT);
    paint_brand_mark(painter, icon.center(), 26.0, Color32::WHITE, ACCENT);
    paint_rail_pill(painter, rect, selected, response.hovered(), unread);
    response
}

/// Индикатор слева от иконки: выбрана, наведена или есть непрочитанное.
fn paint_rail_pill(
    painter: &egui::Painter,
    rect: egui::Rect,
    selected: bool,
    hovered: bool,
    unread: bool,
) {
    let height = if selected {
        32.0
    } else if hovered {
        18.0
    } else if unread {
        8.0
    } else {
        0.0
    };
    if height > 0.0 {
        let pill = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 2.0, rect.center().y),
            egui::vec2(5.0, height),
        );
        painter.rect_filled(pill, CornerRadius::same(3), TEXT_BRIGHT);
    }
}

/// Кнопка пространства: круг с инициалами, при выборе или наведении —
/// скруглённый квадрат; «пилюля» слева показывает выбор и непрочитанное.
pub(crate) fn rail_button(
    ui: &mut egui::Ui,
    text: &str,
    color: Color32,
    selected: bool,
    unread: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 50.0), Sense::click());
    let hovered = response.hovered();
    let icon = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(46.0));
    let radius: u8 = if selected || hovered { 14 } else { 23 };
    let painter = ui.painter();
    painter.rect_filled(icon, CornerRadius::same(radius), color);
    painter.text(
        icon.center(),
        egui::Align2::CENTER_CENTER,
        text,
        FontId::proportional(16.0),
        Color32::WHITE,
    );
    paint_rail_pill(painter, rect, selected, hovered, unread);
    response
}

/// Тонкий разделитель в колонке пространств.
pub(crate) fn rail_separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 2.0), Sense::hover());
    let line = egui::Rect::from_center_size(rect.center(), egui::vec2(28.0, 2.0));
    ui.painter()
        .rect_filled(line, CornerRadius::same(1), BORDER);
}

/// Цвет подписи канала в покое.
const CHANNEL_TEXT: Color32 = Color32::from_rgb(0x8a, 0x8a, 0x98);

/// Строка в списках каналов/бесед: мягкая подложка при выборе, акцентная
/// черта слева, розовая точка у непрочитанных. `leading` рисуется перед подписью.
fn list_row_with(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    unread: bool,
    leading: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), Sense::click());
    let row = rect.shrink2(egui::vec2(8.0, 1.0));
    let bg = if selected {
        Some(BG_ACTIVE)
    } else if response.hovered() {
        Some(BG_HOVER)
    } else {
        None
    };
    if let Some(bg) = bg {
        ui.painter().rect_filled(row, CornerRadius::same(8), bg);
    }
    if selected {
        let bar = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 4.0, rect.center().y),
            egui::vec2(3.0, 18.0),
        );
        ui.painter().rect_filled(bar, CornerRadius::same(2), ACCENT);
    }
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(row.shrink2(egui::vec2(10.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    leading(&mut inner);
    let color = if selected || unread {
        TEXT_BRIGHT
    } else {
        CHANNEL_TEXT
    };
    let mut text = egui::RichText::new(label).color(color);
    if unread {
        text = text.strong();
    }
    inner.add(egui::Label::new(text).truncate().selectable(false));
    if unread {
        inner.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (dot, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, ROSE);
        });
    }
    response
}

/// Строка канала пространства.
pub(crate) fn list_row(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    unread: bool,
) -> egui::Response {
    list_row_with(ui, label, selected, unread, |_| {})
}

/// Строка личной беседы с аватаром.
pub(crate) fn dm_row(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    unread: bool,
) -> egui::Response {
    list_row_with(ui, label, selected, unread, |ui| {
        avatar(ui, label, 28.0, None);
        ui.add_space(4.0);
    })
}

/// Недоступная строка (например, голосовой канал): приглушена, без отклика.
pub(crate) fn disabled_row(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left() + 20.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(15.0),
        TEXT_FAINT,
    );
    response
}

/// Заголовок категории каналов («ТЕКСТОВЫЕ КАНАЛЫ»).
pub(crate) fn category_header(ui: &mut egui::Ui, name: &str) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        ui.label(
            egui::RichText::new(name.to_uppercase())
                .size(11.0)
                .strong()
                .color(TEXT_FAINT),
        );
    });
    ui.add_space(2.0);
}

/// Заголовок секции в правой панели («В СЕТИ — 3»).
#[must_use]
pub(crate) fn section_title(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase())
        .size(11.0)
        .strong()
        .color(TEXT_FAINT)
}

/// Участник в правой панели: аватар со статусом и имя; офлайн — приглушён.
pub(crate) fn member_row(ui: &mut egui::Ui, name: &str, status: UserStatus) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), Sense::hover());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), BG_HOVER);
    }
    let offline = status == UserStatus::Offline;
    let avatar_rect = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(8.0, 6.0),
        egui::Vec2::splat(32.0),
    );
    paint_avatar(ui.painter(), avatar_rect, name, Some(status), offline);
    ui.painter().text(
        egui::pos2(avatar_rect.right() + 12.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        truncate_chars(name, 22),
        FontId::proportional(15.0),
        if offline { TEXT_FAINT } else { TEXT },
    );
}

/// Обрезает строку до `max` символов с многоточием.
#[must_use]
pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Разделитель дней в ленте: дата по центру мелким приглушённым шрифтом.
pub(crate) fn day_divider(ui: &mut egui::Ui, label: &str) {
    ui.add_space(14.0);
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(11.5)
                .strong()
                .color(TEXT_FAINT),
        );
    });
    ui.add_space(6.0);
}

/// Строка списка людей: плоская, с тонкой линией снизу, как в оригинале.
pub(crate) fn person_card(
    ui: &mut egui::Ui,
    name: &str,
    subtitle: Option<&str>,
    buttons: impl FnOnce(&mut egui::Ui),
) {
    let width = ui.available_width();
    let response = ui
        .allocate_ui_with_layout(
            egui::vec2(width, 62.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.add_space(8.0);
                avatar(ui, name, 40.0, None);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(
                        egui::RichText::new(name)
                            .size(15.5)
                            .strong()
                            .color(TEXT_BRIGHT),
                    );
                    if let Some(subtitle) = subtitle.filter(|s| !s.is_empty()) {
                        ui.label(muted(subtitle).size(12.5));
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(8.0);
                    buttons(ui);
                });
            },
        )
        .response;
    let y = response.rect.bottom();
    ui.painter()
        .hline(response.rect.x_range(), y, Stroke::new(1.0, BORDER));
}

/// Пустое состояние списка: приглушённая подпись по центру.
pub(crate) fn empty_state(ui: &mut egui::Ui, text: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(muted(text).size(15.0));
    });
}

/// Подпись времени сообщения: «Сегодня в 15:32», «Вчера в 09:10» или полная дата.
#[must_use]
pub(crate) fn stamp(ms: i64, now_ms: i64) -> String {
    use chrono::TimeZone as _;
    let (Some(at), Some(now)) = (
        chrono::Local.timestamp_millis_opt(ms).single(),
        chrono::Local.timestamp_millis_opt(now_ms).single(),
    ) else {
        return String::new();
    };
    let days = (now.date_naive() - at.date_naive()).num_days();
    match days {
        0 => format!("Сегодня в {}", at.format("%H:%M")),
        1 => format!("Вчера в {}", at.format("%H:%M")),
        _ => at.format("%d.%m.%Y %H:%M").to_string(),
    }
}

/// Номер календарного дня по местному времени (для разделителей дат).
#[must_use]
pub(crate) fn local_day(ms: i64) -> Option<chrono::NaiveDate> {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.date_naive())
}

/// Подпись даты для разделителя в ленте.
#[must_use]
pub(crate) fn day_label(ms: i64) -> String {
    use chrono::TimeZone as _;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map_or_else(String::new, |t| t.format("%d.%m.%Y").to_string())
}

#[cfg(test)]
mod tests {
    use super::{color_for, initials, stamp, truncate_chars};

    #[test]
    fn stamp_words_for_today_and_yesterday() {
        let now = chrono::Local::now().timestamp_millis();
        assert!(stamp(now, now).starts_with("Сегодня в "));
        let day = 24 * 60 * 60 * 1000;
        assert!(stamp(now - day, now).starts_with("Вчера в "));
        let old = stamp(now - 5 * day, now);
        assert!(
            !old.starts_with("Сегодня") && !old.starts_with("Вчера"),
            "{old}"
        );
        assert_eq!(old.len(), "01.01.2026 12:00".len());
    }

    #[test]
    fn truncate_keeps_short_and_cuts_long() {
        assert_eq!(truncate_chars("abc", 5), "abc");
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
        assert_eq!(truncate_chars("Привет, мир", 7), "Привет…");
    }

    #[test]
    fn initials_cover_common_shapes() {
        assert_eq!(initials("alice"), "AL");
        assert_eq!(initials("Alice Cooper"), "AC");
        assert_eq!(initials("mary-jane_doe"), "MJ");
        assert_eq!(initials("  "), "?");
        assert_eq!(initials(""), "?");
        assert_eq!(initials("Борис"), "БО");
        assert_eq!(initials("Иван Петров"), "ИП");
    }

    #[test]
    fn color_is_stable_per_name() {
        assert_eq!(color_for("alice"), color_for("alice"));
        // Разные имена обычно дают разные цвета (хотя бы среди нескольких).
        let colors: std::collections::HashSet<_> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|n| color_for(n))
            .collect();
        assert!(colors.len() > 1);
    }
}
