//! Составные виджеты в стиле Backspace: строки списков, поиск, кнопки-иконки.
//!
//! Цвета и шрифты берутся из [`crate::theme`]; здесь только раскладка.

use bs_types::UserStatus;
use egui::{Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, Sense, Stroke};

use crate::icons::Icon;
use crate::theme::{
    accent, avatar, bg_active, bg_hover, bg_input, bold_family, border, mint, muted, paint_avatar,
    rose, text, text_bright, text_faint, text_muted, truncate_chars,
};

/// Поле поиска: скруглённая «утопленная» подложка без рамки.
pub(crate) fn search_box(ui: &mut egui::Ui, text: &mut String, hint: &str) -> egui::Response {
    egui::Frame::new()
        .fill(bg_input())
        .corner_radius(crate::theme::rounded(8))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .hint_text(egui::RichText::new(hint).color(text_faint()))
                    .frame(egui::Frame::NONE)
                    .desired_width(ui.available_width()),
            )
        })
        .inner
}

/// Квадратная кнопка с символом (написать, удалить, настройки…).
pub(crate) fn icon_button(ui: &mut egui::Ui, icon: Icon, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), Sense::click());
    let hovered = response.hovered();
    let fill = if hovered { bg_active() } else { bg_hover() };
    ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
    crate::icons::paint(
        ui.painter(),
        rect.center(),
        size * 0.5,
        icon,
        if hovered { text_bright() } else { text_muted() },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Значок без подложки как виджет: занимает квадрат и рисуется на месте.
pub(crate) fn flat_icon(icon: Icon, size: f32, color: Color32) -> impl egui::Widget {
    move |ui: &mut egui::Ui| {
        let (rect, response) =
            ui.allocate_exact_size(egui::Vec2::splat(size + 6.0), Sense::click());
        let tint = if response.hovered() {
            text_bright()
        } else {
            color
        };
        crate::icons::paint(ui.painter(), rect.center(), size, icon, tint);
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// Вкладка-«таблетка» в шапке: выбранная получает подложку.
pub(crate) fn pill_tab(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::proportional(14.5),
        if selected {
            text_bright()
        } else {
            text_muted()
        },
    );
    let size = galley.size() + egui::vec2(18.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if selected {
        ui.painter()
            .rect_filled(rect, crate::theme::rounded(7), bg_active());
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, crate::theme::rounded(7), bg_hover());
    }
    let pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(pos, galley, text_muted());
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Яркая мятная кнопка («Добавить друга»): тёмный текст на мятном фоне.
pub(crate) fn mint_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let fill = if selected {
        mint()
    } else {
        mint().gamma_multiply(0.92)
    };
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::new(13.5, bold_family()),
        Color32::from_rgb(0x0b, 0x2a, 0x16),
    );
    let size = galley.size() + egui::vec2(18.0, 10.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    ui.painter()
        .rect_filled(rect, crate::theme::rounded(6), fill);
    let pos = rect.center() - galley.size() / 2.0;
    ui.painter().galley(pos, galley, text());
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Подпись секции строчными капителями («ОНЛАЙН — 3»).
pub(crate) fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(11.0)
            .family(bold_family())
            .color(text_faint()),
    );
}

/// Данные строки человека.
pub(crate) struct PersonRow<'a> {
    /// Имя для показа.
    pub(crate) name: &'a str,
    /// Серый «@handle» рядом с именем.
    pub(crate) handle: Option<&'a str>,
    /// Вторая строка: статус или пояснение.
    pub(crate) subtitle: Option<&'a str>,
    /// Статус присутствия для индикатора.
    pub(crate) status: Option<UserStatus>,
    /// Значение поля `avatar` (имя файла или путь).
    pub(crate) avatar: Option<&'a str>,
    /// Ключ цвета заглушки и `avatarColor`.
    pub(crate) face: (Option<&'a str>, Option<&'a str>),
}

/// Строка списка людей: аватар, имя, «@handle», статус; справа — кнопки.
/// Сверху — тонкая линия, как у списка друзей в оригинале.
pub(crate) fn person_row(
    ui: &mut egui::Ui,
    person: &PersonRow<'_>,
    buttons: impl FnOnce(&mut egui::Ui),
) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 62.0), Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, border()));
    let avatar_rect = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(12.0, 15.0),
        egui::Vec2::splat(32.0),
    );
    paint_avatar(
        ui.painter(),
        avatar_rect,
        crate::theme::Face {
            name: person.name,
            key: person.face.0,
            color: person.face.1,
        },
        person.status,
        person.status == Some(UserStatus::Offline),
        person.avatar,
    );
    let mut text_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                egui::pos2(avatar_rect.right() + 12.0, rect.top() + 11.0),
                egui::pos2(rect.right() - 120.0, rect.bottom() - 6.0),
            ))
            .layout(Layout::top_down(Align::Min)),
    );
    text_ui.spacing_mut().item_spacing = egui::vec2(6.0, 1.0);
    text_ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(truncate_chars(person.name, 28))
                .size(15.0)
                .family(bold_family())
                .color(text_bright()),
        );
        if let Some(handle) = person.handle {
            ui.label(muted(truncate_chars(handle, 36)).size(13.0));
        }
    });
    if let Some(subtitle) = person.subtitle.filter(|s| !s.is_empty()) {
        text_ui.label(muted(truncate_chars(subtitle, 60)).size(12.0));
    }
    let mut button_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(12.0, 0.0)))
            .layout(Layout::right_to_left(Align::Center)),
    );
    button_ui.spacing_mut().item_spacing.x = 8.0;
    buttons(&mut button_ui);
}

/// Данные строки личной беседы.
pub(crate) struct DmEntry<'a> {
    /// Имя собеседника.
    pub(crate) name: &'a str,
    /// Ключ цвета заглушки и `avatarColor`.
    pub(crate) face: (Option<&'a str>, Option<&'a str>),
    /// Присутствие собеседника (для личных бесед).
    pub(crate) status: Option<UserStatus>,
    /// Начало последнего сообщения.
    pub(crate) preview: Option<&'a str>,
    /// Время последнего сообщения, уже отформатированное.
    pub(crate) stamp: Option<&'a str>,
    /// Выбрана ли беседа.
    pub(crate) selected: bool,
    /// Есть ли непрочитанное.
    pub(crate) unread: bool,
    /// Аватар собеседника (для бесед один на один).
    pub(crate) avatar: Option<&'a str>,
}

/// Строка в списке личных сообщений: аватар, имя, время, превью.
pub(crate) fn dm_entry(ui: &mut egui::Ui, dm: &DmEntry<'_>) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), Sense::click());
    let row = rect.shrink2(egui::vec2(8.0, 1.0));
    if dm.selected {
        ui.painter()
            .rect_filled(row, crate::theme::rounded(8), bg_active());
    } else if response.hovered() {
        ui.painter()
            .rect_filled(row, crate::theme::rounded(8), bg_hover());
    }
    let avatar_rect = egui::Rect::from_min_size(
        row.left_top() + egui::vec2(8.0, 7.0),
        egui::Vec2::splat(32.0),
    );
    paint_avatar(
        ui.painter(),
        avatar_rect,
        crate::theme::Face {
            name: dm.name,
            key: dm.face.0,
            color: dm.face.1,
        },
        dm.status,
        false,
        dm.avatar,
    );
    let text_left = avatar_rect.right() + 10.0;
    let name_color = if dm.selected || dm.unread {
        text_bright()
    } else {
        text_muted()
    };
    let name_font = if dm.unread {
        FontId::new(14.5, bold_family())
    } else {
        FontId::proportional(14.5)
    };
    let stamp_width = dm.stamp.map_or(0.0, |s| {
        ui.painter()
            .layout_no_wrap(s.to_owned(), FontId::proportional(11.0), text_faint())
            .size()
            .x
            + 8.0
    });
    let name_clip = egui::Rect::from_min_max(
        egui::pos2(text_left, row.top()),
        egui::pos2(row.right() - 8.0 - stamp_width, row.top() + 24.0),
    );
    ui.painter().with_clip_rect(name_clip).text(
        egui::pos2(text_left, row.top() + 14.0),
        Align2::LEFT_CENTER,
        dm.name,
        name_font,
        name_color,
    );
    if let Some(stamp) = dm.stamp {
        ui.painter().text(
            egui::pos2(row.right() - 8.0, row.top() + 14.0),
            Align2::RIGHT_CENTER,
            stamp,
            FontId::proportional(11.0),
            text_faint(),
        );
    }
    if let Some(preview) = dm.preview.filter(|p| !p.is_empty()) {
        let clip = egui::Rect::from_min_max(
            egui::pos2(text_left, row.top() + 24.0),
            egui::pos2(row.right() - 22.0, row.bottom()),
        );
        ui.painter().with_clip_rect(clip).text(
            egui::pos2(text_left, row.top() + 33.0),
            Align2::LEFT_CENTER,
            preview.replace('\n', " "),
            FontId::proportional(12.0),
            text_faint(),
        );
    }
    if dm.unread {
        ui.painter().circle_filled(
            egui::pos2(row.right() - 12.0, row.top() + 33.0),
            4.0,
            rose(),
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Пункт навигации в левой колонке («Друзья», «Обзор»).
pub(crate) fn nav_row(
    ui: &mut egui::Ui,
    glyph: Icon,
    label: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 42.0), Sense::click());
    let row = rect.shrink2(egui::vec2(8.0, 1.0));
    if selected {
        ui.painter()
            .rect_filled(row, crate::theme::rounded(8), bg_active());
    } else if response.hovered() {
        ui.painter()
            .rect_filled(row, crate::theme::rounded(8), bg_hover());
    }
    let color = if selected {
        text_bright()
    } else {
        text_muted()
    };
    crate::icons::paint(
        ui.painter(),
        egui::pos2(row.left() + 22.0, row.center().y),
        19.0,
        glyph,
        color,
    );
    ui.painter().text(
        egui::pos2(row.left() + 44.0, row.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(15.0),
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Компактная строка для панели «Сейчас онлайн».
pub(crate) fn compact_person<'a>(
    ui: &mut egui::Ui,
    face: impl Into<crate::theme::Face<'a>>,
    subtitle: Option<&str>,
    status: UserStatus,
    image: Option<&str>,
) -> egui::Response {
    let height = if subtitle.is_some() { 48.0 } else { 44.0 };
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(
            rect.shrink2(egui::vec2(0.0, 1.0)),
            crate::theme::rounded(8),
            bg_hover(),
        );
    }
    let face = face.into();
    let name = face.name;
    let offline = status == UserStatus::Offline;
    let avatar_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 4.0, rect.center().y - 16.0),
        egui::Vec2::splat(32.0),
    );
    paint_avatar(
        ui.painter(),
        avatar_rect,
        face,
        Some(status),
        offline,
        image,
    );
    let x = avatar_rect.right() + 10.0;
    let color = if offline { text_faint() } else { text() };
    let clip = egui::Rect::from_min_max(
        egui::pos2(x, rect.top()),
        egui::pos2(rect.right() - 4.0, rect.bottom()),
    );
    let painter = ui.painter().with_clip_rect(clip);
    if let Some(subtitle) = subtitle.filter(|s| !s.is_empty()) {
        painter.text(
            egui::pos2(x, rect.center().y - 8.0),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(14.0),
            color,
        );
        painter.text(
            egui::pos2(x, rect.center().y + 9.0),
            Align2::LEFT_CENTER,
            subtitle,
            FontId::proportional(11.0),
            text_faint(),
        );
    } else {
        painter.text(
            egui::pos2(x, rect.center().y),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(14.0),
            color,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Круглая кнопка в колонке пространств («создать», «обзор»).
pub(crate) fn rail_action(
    ui: &mut egui::Ui,
    glyph: Icon,
    color: Color32,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 50.0), Sense::click());
    let icon = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(44.0));
    let hovered = response.hovered();
    let radius: u8 = if selected || hovered { 14 } else { 22 };
    let fill = if selected || hovered {
        color.gamma_multiply(0.30)
    } else {
        bg_active()
    };
    ui.painter()
        .rect_filled(icon, CornerRadius::same(radius), fill);
    crate::icons::paint(ui.painter(), icon.center(), 22.0, glyph, color);
    if selected {
        let pill = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 2.0, rect.center().y),
            egui::vec2(5.0, 32.0),
        );
        ui.painter()
            .rect_filled(pill, CornerRadius::same(3), text_bright());
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Круглый выбор цвета палитры. Возвращает ответ; выбранный обведён.
pub(crate) fn color_swatch(
    ui: &mut egui::Ui,
    color: Color32,
    selected: bool,
    size: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), Sense::click());
    ui.painter()
        .circle_filled(rect.center(), size / 2.0 - 3.0, color);
    if selected {
        ui.painter().circle_stroke(
            rect.center(),
            size / 2.0 - 0.5,
            Stroke::new(2.0, text_bright()),
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Подпись поля формы («ИМЯ ПРОСТРАНСТВА»).
pub(crate) fn field_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(10.0);
    section_label(ui, text);
    ui.add_space(4.0);
}

/// Однострочное поле формы на «утопленной» подложке.
pub(crate) fn text_field(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    max_chars: usize,
) -> egui::Response {
    egui::Frame::new()
        .fill(bg_input())
        .corner_radius(crate::theme::rounded(8))
        .inner_margin(Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .hint_text(egui::RichText::new(hint).color(text_faint()))
                    .frame(egui::Frame::NONE)
                    .char_limit(max_chars)
                    .desired_width(ui.available_width()),
            )
        })
        .inner
}

/// Поле для пароля: символы скрыты.
pub(crate) fn secret_field(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    max_chars: usize,
) -> egui::Response {
    egui::Frame::new()
        .fill(bg_input())
        .corner_radius(crate::theme::rounded(8))
        .inner_margin(Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(text)
                    .password(true)
                    .hint_text(egui::RichText::new(hint).color(text_faint()))
                    .frame(egui::Frame::NONE)
                    .char_limit(max_chars)
                    .desired_width(ui.available_width()),
            )
        })
        .inner
}

/// Многострочное поле формы со счётчиком символов.
pub(crate) fn text_area(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    max_chars: usize,
    rows: usize,
) -> egui::Response {
    let response = egui::Frame::new()
        .fill(bg_input())
        .corner_radius(crate::theme::rounded(8))
        .inner_margin(Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(text)
                    .hint_text(egui::RichText::new(hint).color(text_faint()))
                    .frame(egui::Frame::NONE)
                    .char_limit(max_chars)
                    .desired_rows(rows)
                    .desired_width(ui.available_width()),
            )
        })
        .inner;
    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        ui.label(muted(format!("{}/{max_chars}", text.chars().count())).size(11.0));
    });
    response
}

/// Радио-строка с пояснением (видимость пространства).
pub(crate) fn radio_card(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), Sense::click());
    if selected {
        ui.painter()
            .rect_filled(rect, crate::theme::rounded(8), bg_active());
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, crate::theme::rounded(8), bg_hover());
    }
    let center = egui::pos2(rect.left() + 20.0, rect.center().y);
    ui.painter().circle_stroke(
        center,
        8.0,
        Stroke::new(1.5, if selected { accent() } else { text_faint() }),
    );
    if selected {
        ui.painter().circle_filled(center, 4.5, accent());
    }
    ui.painter().text(
        egui::pos2(rect.left() + 40.0, rect.center().y - 8.0),
        Align2::LEFT_CENTER,
        title,
        FontId::new(14.5, bold_family()),
        text_bright(),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 40.0, rect.center().y + 10.0),
        Align2::LEFT_CENTER,
        subtitle,
        FontId::proportional(12.0),
        text_faint(),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Аватар-кружок крупного размера для карточек (без статуса).
pub(crate) fn big_avatar<'a>(
    ui: &mut egui::Ui,
    face: impl Into<crate::theme::Face<'a>>,
    size: f32,
    image: Option<&str>,
) {
    avatar(ui, face, size, None, image);
}

/// Выключатель «таблеткой». Возвращает `true`, если значение изменилось.
pub(crate) fn toggle(ui: &mut egui::Ui, value: &mut bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(40.0, 22.0), Sense::click());
    let fill = if *value { accent() } else { bg_active() };
    ui.painter().rect_filled(rect, CornerRadius::same(11), fill);
    let x = if *value {
        rect.right() - 11.0
    } else {
        rect.left() + 11.0
    };
    ui.painter()
        .circle_filled(egui::pos2(x, rect.center().y), 8.0, text_bright());
    if response.clicked() {
        *value = !*value;
        return true;
    }
    false
}

/// Метка «чужого инстанса» на значке пространства: кружок с буквой хоста.
pub(crate) fn origin_badge(ui: &egui::Ui, rect: egui::Rect, host: &str) {
    let center = rect.center() + egui::vec2(17.0, 17.0);
    let painter = ui.painter();
    painter.circle_filled(center, 9.0, crate::theme::bg_rail());
    painter.circle_filled(center, 7.5, crate::theme::sky());
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        host.chars()
            .next()
            .map_or_else(String::new, |c| c.to_uppercase().to_string()),
        egui::FontId::proportional(10.0),
        egui::Color32::BLACK,
    );
}
