//! Настройки → «Внешний вид»: масштаб, темы, свои цвета, скругления,
//! плотность ленты, импорт и экспорт темы.
//!
//! Любое изменение сразу применяется ([`theme::apply`]) и уходит в
//! [`crate::prefs`], откуда сохраняется на диск.

use egui::{Color32, CornerRadius, RichText, Sense, Stroke};

use super::dialogs::{card, page_title};
use super::settings_extra::setting_row;
use crate::i18n::tr;
use crate::theme::{self, Appearance, Palette, Preset};
use crate::widgets;

/// Быстрый выбор акцента.
#[allow(clippy::unreadable_literal, reason = "цвета записаны как #rrggbb")]
const ACCENTS: [u32; 10] = [
    0xe8743b, 0xf0a050, 0x7c6cf6, 0x5865f2, 0x3ba55d, 0x14b8a6, 0xf59e0b, 0xf97316, 0xec4899,
    0xef4444,
];

/// Готовые пары цветов свечения: (название, сверху, снизу).
#[allow(clippy::unreadable_literal, reason = "цвета записаны как #rrggbb")]
const GLOWS: [(&str, u32, u32); 5] = [
    ("Оксид", 0x7c6cf6, 0xe8743b),
    ("Море", 0x38bdf8, 0x14b8a6),
    ("Закат", 0xec4899, 0xf59e0b),
    ("Лес", 0x3ba55d, 0x14b8a6),
    ("Без цвета", 0x8a8399, 0x5b5468),
];

/// Состояние страницы между кадрами.
#[derive(Debug, Default)]
pub(crate) struct AppearanceState {
    /// Масштаб на ползунке (применяется после отпускания).
    scale: Option<f32>,
    /// Последнее сообщение об импорте/экспорте.
    notice: Option<(bool, String)>,
    /// Ждём файл темы из выбора файла.
    importing: Option<crate::sys::PickInbox>,
}

/// Рисует страницу.
pub(crate) fn page(ui: &mut egui::Ui, state: &mut AppearanceState) {
    page_title(ui, tr("Внешний вид"));
    let mut appearance = crate::prefs::get().appearance;
    let before = appearance.clone();

    scale_card(ui, state);
    ui.add_space(10.0);
    card(ui, |ui| {
        widgets::section_label(ui, tr("Тема"));
        ui.add_space(6.0);
        presets(ui, &mut appearance);
    });
    ui.add_space(10.0);
    card(ui, |ui| {
        widgets::section_label(ui, tr("Акцентный цвет"));
        ui.add_space(6.0);
        accent_row(ui, &mut appearance);
    });
    ui.add_space(10.0);
    card(ui, |ui| glass_card(ui, &mut appearance));
    ui.add_space(10.0);
    card(ui, |ui| {
        widgets::section_label(ui, tr("Форма и плотность"));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(tr("Скругление углов"));
            ui.spacing_mut().slider_width = 200.0;
            ui.add(egui::Slider::new(&mut appearance.radius, 0..=16).suffix(" px"));
        });
        ui.add_space(8.0);
        setting_row(
            ui,
            tr("Компактная лента"),
            tr("Меньше отступов между сообщениями — больше сообщений на экране."),
            &mut appearance.compact,
        );
        ui.add_space(4.0);
        setting_row(
            ui,
            tr("Чёткий текст"),
            tr(
                "Буквы прижаты к пиксельной сетке и плотнее. Без этого текст мягче, как в браузере.",
            ),
            &mut appearance.crisp_text,
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(tr("Кнопка «к последним сообщениям»"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::pill_tab(
                    ui,
                    tr("Справа"),
                    appearance.jump_place == theme::JumpPlace::Right,
                )
                .clicked()
                {
                    appearance.jump_place = theme::JumpPlace::Right;
                }
                if widgets::pill_tab(
                    ui,
                    tr("По центру"),
                    appearance.jump_place == theme::JumpPlace::Center,
                )
                .clicked()
                {
                    appearance.jump_place = theme::JumpPlace::Center;
                }
            });
        });
    });
    ui.add_space(10.0);
    card(ui, |ui| custom_colors(ui, &mut appearance));
    ui.add_space(10.0);
    card(ui, |ui| import_export(ui, state, &mut appearance));

    if appearance != before {
        theme::apply(ui.ctx(), &appearance);
        crate::prefs::update(|prefs| prefs.appearance = appearance);
    }
}

fn scale_card(ui: &mut egui::Ui, state: &mut AppearanceState) {
    card(ui, |ui| {
        widgets::section_label(ui, tr("Масштаб интерфейса"));
        ui.add_space(6.0);
        ui.spacing_mut().slider_width = 280.0;
        let scale = state.scale.get_or_insert_with(|| ui.ctx().zoom_factor());
        let response = ui.add(
            egui::Slider::new(scale, 0.8..=1.5)
                .step_by(0.05)
                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
        );
        if response.drag_stopped() || (response.changed() && !response.dragged()) {
            let value = *scale;
            ui.ctx().set_zoom_factor(value);
            crate::prefs::update(|prefs| prefs.scale = value);
        }
        ui.add_space(6.0);
        ui.label(theme::muted(tr("Применяется после того, как вы отпустите ползунок.")).size(12.5));
    });
}

/// Карточки встроенных тем с миниатюрой палитры.
fn presets(ui: &mut egui::Ui, appearance: &mut Appearance) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
        for preset in Preset::ALL {
            let selected = appearance.custom.is_none() && appearance.preset == preset;
            if preset_card(ui, preset, selected).clicked() {
                appearance.preset = preset;
                appearance.custom = None;
            }
        }
    });
    if appearance.custom.is_some() {
        ui.add_space(6.0);
        ui.label(
            theme::muted(crate::tf!(
                "Сейчас своя тема на основе «{a0}».",
                a0 = appearance.preset.title()
            ))
            .size(12.5),
        );
    }
}

fn preset_card(ui: &mut egui::Ui, preset: Preset, selected: bool) -> egui::Response {
    let p = preset.palette();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(150.0, 104.0), Sense::click());
    let painter = ui.painter_at(rect);
    let radius = crate::theme::rounded(10);
    // Стеклянная тема: подложка со свечением, панели полупрозрачные.
    let panel = |color: Color32| {
        if p.glass {
            theme::with_alpha(color, p.glass_opacity)
        } else {
            color
        }
    };
    if p.glass {
        painter.rect_filled(rect, radius, p.bg_rail);
        theme::paint_glows(&painter, rect, &p);
        painter.rect_filled(rect, radius, panel(p.bg_chat));
    } else {
        painter.rect_filled(rect, radius, p.bg_chat);
    }
    // Мини-раскладка: рейл, боковая панель, «сообщения», акцентная кнопка.
    let inner = rect.shrink(8.0);
    let rail = egui::Rect::from_min_size(inner.min, egui::vec2(14.0, 56.0));
    painter.rect_filled(rail, CornerRadius::same(4), panel(p.bg_rail));
    painter.circle_filled(rail.center_top() + egui::vec2(0.0, 9.0), 5.0, p.accent);
    let side = egui::Rect::from_min_size(
        rail.right_top() + egui::vec2(4.0, 0.0),
        egui::vec2(34.0, 56.0),
    );
    painter.rect_filled(side, CornerRadius::same(4), panel(p.bg_sidebar));
    for i in 0..3_u8 {
        let y = f32::from(i).mul_add(12.0, side.top() + 10.0);
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(side.left() + 5.0, y), egui::vec2(22.0, 4.0)),
            CornerRadius::same(2),
            if i == 0 {
                p.text_bright
            } else {
                p.channel_text
            },
        );
    }
    let feed_left = side.right() + 8.0;
    for (i, width) in [62.0, 44.0, 54.0].into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss, reason = "индекс 0..3")]
        let y = (i as f32).mul_add(16.0, inner.top() + 6.0);
        painter.circle_filled(egui::pos2(feed_left + 4.0, y + 3.0), 4.0, p.mint);
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(feed_left + 12.0, y), egui::vec2(width, 5.0)),
            CornerRadius::same(2),
            p.text,
        );
    }
    painter.text(
        egui::pos2(inner.left(), inner.bottom()),
        egui::Align2::LEFT_BOTTOM,
        preset.title(),
        egui::FontId::new(13.0, theme::bold_family()),
        p.text_bright,
    );
    let stroke = if selected {
        Stroke::new(2.0, theme::accent())
    } else if response.hovered() {
        Stroke::new(1.0, theme::text_faint())
    } else {
        Stroke::new(1.0, theme::border())
    };
    painter.rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(preset.title())
}

fn accent_row(ui: &mut egui::Ui, appearance: &mut Appearance) {
    let mut palette = appearance.palette();
    let before = palette.accent;
    ui.horizontal_wrapped(|ui| {
        for hex in ACCENTS {
            let color = Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
            let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::click());
            ui.painter().circle_filled(rect.center(), 12.0, color);
            if palette.accent == color {
                ui.painter().circle_stroke(
                    rect.center(),
                    13.5,
                    Stroke::new(2.0, theme::text_bright()),
                );
            }
            if response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                palette.accent = color;
            }
        }
        ui.add_space(8.0);
        ui.label(theme::muted(tr("Свой:")));
        egui::color_picker::color_edit_button_srgba(
            ui,
            &mut palette.accent,
            egui::color_picker::Alpha::Opaque,
        );
    });
    if palette.accent != before {
        palette.on_accent = readable_on(palette.accent);
        appearance.custom = Some(palette);
    }
}

/// «Стекло»: полупрозрачные панели и свечение за ними.
fn glass_card(ui: &mut egui::Ui, appearance: &mut Appearance) {
    widgets::section_label(ui, tr("Стекло"));
    ui.add_space(6.0);
    let mut palette = appearance.palette();
    let before = palette;
    setting_row(
        ui,
        tr("Полупрозрачные панели"),
        tr("Панели окна пропускают мягкое свечение фона, окна и меню — тоже."),
        &mut palette.glass,
    );
    ui.add_enabled_ui(palette.glass, |ui| {
        ui.add_space(8.0);
        egui::Grid::new("appearance_glass")
            .num_columns(2)
            .spacing(egui::vec2(16.0, 10.0))
            .show(ui, |ui| {
                ui.spacing_mut().slider_width = 220.0;
                ui.label(tr("Непрозрачность панелей"));
                ui.add(egui::Slider::new(&mut palette.glass_opacity, 40..=100).suffix(" %"));
                ui.end_row();
                ui.label(tr("Свечение"));
                ui.add(egui::Slider::new(&mut palette.glow, 0..=40).suffix(" %"));
                ui.end_row();
                ui.label(tr("Цвета свечения"));
                ui.horizontal(|ui| {
                    egui::color_picker::color_edit_button_srgba(
                        ui,
                        &mut palette.glow_a,
                        egui::color_picker::Alpha::Opaque,
                    );
                    egui::color_picker::color_edit_button_srgba(
                        ui,
                        &mut palette.glow_b,
                        egui::color_picker::Alpha::Opaque,
                    );
                });
                ui.end_row();
            });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            for (name, top, bottom) in GLOWS {
                let (top, bottom) = (hex_color(top), hex_color(bottom));
                let chosen = palette.glow_a == top && palette.glow_b == bottom;
                if glow_chip(ui, tr(name), top, bottom, chosen).clicked() {
                    palette.glow_a = top;
                    palette.glow_b = bottom;
                    if palette.glow == 0 {
                        palette.glow = 16;
                    }
                }
            }
        });
    });
    if palette != before {
        appearance.custom = Some(palette);
    }
}

/// Кнопка готовой пары свечения: два кружка и название.
fn glow_chip(
    ui: &mut egui::Ui,
    name: &str,
    top: Color32,
    bottom: Color32,
    chosen: bool,
) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        name.to_owned(),
        egui::FontId::proportional(13.0),
        theme::text(),
    );
    let size = egui::vec2(galley.size().x + 52.0, 32.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    let fill = if chosen || response.hovered() {
        theme::bg_active()
    } else {
        theme::bg_hover()
    };
    painter.rect_filled(rect, crate::theme::rounded(16), fill);
    if chosen {
        painter.rect_stroke(
            rect,
            crate::theme::rounded(16),
            Stroke::new(1.5, theme::accent()),
            egui::StrokeKind::Inside,
        );
    }
    let left = rect.left_center() + egui::vec2(16.0, 0.0);
    painter.circle_filled(left, 7.0, top);
    painter.circle_filled(left + egui::vec2(10.0, 0.0), 7.0, bottom);
    painter.galley(
        egui::pos2(left.x + 22.0, rect.center().y - galley.size().y / 2.0),
        galley,
        theme::text(),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[allow(clippy::cast_possible_truncation, reason = "байты цвета из #rrggbb")]
const fn hex_color(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Белый или чёрный текст — что читается на фоне лучше.
fn readable_on(color: Color32) -> Color32 {
    let luma =
        2126 * u32::from(color.r()) + 7152 * u32::from(color.g()) + 722 * u32::from(color.b());
    if luma > 160 * 10_000 {
        Color32::from_rgb(0x14, 0x15, 0x1b)
    } else {
        Color32::WHITE
    }
}

fn custom_colors(ui: &mut egui::Ui, appearance: &mut Appearance) {
    egui::CollapsingHeader::new(RichText::new(tr("Свои цвета")).family(theme::bold_family()))
        .id_salt("appearance_custom_colors")
        .show(ui, |ui| {
            ui.label(
                theme::muted(tr(
                    "Любой цвет темы можно поменять. Правка создаёт свою тему на основе выбранной.",
                ))
                .size(12.5),
            );
            ui.add_space(6.0);
            let mut palette = appearance.palette();
            let before = palette;
            egui::Grid::new("appearance_colors")
                .num_columns(2)
                .spacing(egui::vec2(16.0, 6.0))
                .striped(false)
                .show(ui, |ui| {
                    for (label, color) in palette.fields_mut() {
                        ui.label(label);
                        egui::color_picker::color_edit_button_srgba(
                            ui,
                            color,
                            egui::color_picker::Alpha::Opaque,
                        );
                        ui.end_row();
                    }
                });
            ui.add_space(4.0);
            ui.checkbox(
                &mut palette.dark,
                tr("Тёмная основа (стили полос прокрутки, полей)"),
            );
            if palette != before {
                appearance.custom = Some(palette);
            }
            ui.add_space(6.0);
            if ui
                .add_enabled(
                    appearance.custom.is_some(),
                    egui::Button::new(tr("Сбросить к теме")),
                )
                .clicked()
            {
                appearance.custom = None;
            }
        });
}

fn import_export(ui: &mut egui::Ui, state: &mut AppearanceState, appearance: &mut Appearance) {
    widgets::section_label(ui, tr("Файл темы"));
    ui.add_space(4.0);
    ui.label(
        theme::muted(tr(
            "Тема сохраняется в JSON: им можно поделиться с друзьями.",
        ))
        .size(12.5),
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button(tr("Экспортировать…")).clicked() {
            state.notice = Some(export(&appearance.palette()));
        }
        if ui.button(tr("Импортировать…")).clicked() {
            state.importing = Some(crate::sys::pick_inbox(
                ui.ctx(),
                crate::sys::PickSpec {
                    filter: Some(("Тема (JSON)", &["json"])),
                    many: false,
                    limit: 64 * 1024,
                },
            ));
        }
        if let Some(file) =
            crate::sys::take_picked(&mut state.importing).and_then(|files| files.into_iter().next())
        {
            state.notice = Some(match file.bytes.and_then(|bytes| parse_theme(&bytes)) {
                Ok(palette) => {
                    appearance.custom = Some(palette);
                    (true, tr("Тема загружена").to_owned())
                }
                Err(err) => (false, err),
            });
        }
    });
    if let Some((ok, text)) = &state.notice {
        ui.add_space(4.0);
        let color = if *ok { theme::mint() } else { theme::rose() };
        ui.label(RichText::new(text).size(12.5).color(color));
    }
}

fn export(palette: &Palette) -> (bool, String) {
    // В браузере тема уходит в его загрузки.
    #[cfg(target_arch = "wasm32")]
    {
        let result = serde_json::to_vec_pretty(palette)
            .map_err(|err| err.to_string())
            .and_then(|json| crate::sys::offer_download("rustspace-theme.json", &json));
        match result {
            Ok(()) => (true, tr("Тема сохранена в загрузки браузера").to_owned()),
            Err(err) => (false, crate::tf!("Не удалось сохранить: {err}", err = err)),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let Some(path) = crate::sys::FileDialog::new()
            .set_file_name("rustspace-theme.json")
            .add_filter(tr("Тема (JSON)"), &["json"])
            .save_file()
        else {
            return (true, String::new());
        };
        let result = serde_json::to_vec_pretty(palette)
            .map_err(std::io::Error::other)
            .and_then(|json| crate::files::write(&path, json));
        match result {
            Ok(()) => (true, crate::tf!("Сохранено: {a0}", a0 = path.display())),
            Err(err) => (false, crate::tf!("Не удалось сохранить: {err}", err = err)),
        }
    }
}

/// Разбирает файл темы. Недостающие поля берутся из темы по умолчанию.
pub(crate) fn parse_theme(bytes: &[u8]) -> Result<Palette, String> {
    const LIMIT: usize = 64 * 1024;
    if bytes.len() > LIMIT {
        return Err(tr("Файл темы слишком большой").to_owned());
    }
    serde_json::from_slice::<Palette>(bytes)
        .map_err(|err| crate::tf!("Файл темы не подходит: {err}", err = err))
}

#[cfg(test)]
mod tests {
    use super::{parse_theme, readable_on};
    use crate::theme::{Palette, Preset};

    #[test]
    fn every_preset_survives_export_and_import() {
        for preset in Preset::ALL {
            let json = serde_json::to_vec(&preset.palette()).unwrap();
            assert_eq!(parse_theme(&json).unwrap(), preset.palette());
        }
    }

    #[test]
    fn partial_theme_fills_missing_colors() {
        let palette = parse_theme(br##"{"accent": "#ff0000"}"##).unwrap();
        assert_eq!(palette.accent, egui::Color32::from_rgb(255, 0, 0));
        assert_eq!(palette.bg_chat, Palette::AETHER.bg_chat);
        // Темы, сохранённые до стекла, остаются без него.
        assert!(!palette.glass);
    }

    #[test]
    fn glass_settings_travel_with_the_theme() {
        let json = serde_json::to_string(&Palette::OXIDE).unwrap();
        assert!(json.contains("\"glass\":true") && json.contains("\"glowA\":\"#7c6cf6\""));
        let back = parse_theme(json.as_bytes()).unwrap();
        assert_eq!((back.glass_opacity, back.glow), (62, 16));
    }

    #[test]
    fn bad_theme_is_rejected() {
        assert!(parse_theme(br#"{"accent": "red"}"#).is_err());
        assert!(parse_theme(b"not json").is_err());
        assert!(parse_theme(&vec![b' '; 70 * 1024]).is_err());
    }

    #[test]
    fn text_on_accent_stays_readable() {
        assert_eq!(
            readable_on(egui::Color32::from_rgb(0xff, 0xd4, 0x00)).r(),
            0x14
        );
        assert_eq!(
            readable_on(egui::Color32::from_rgb(0x7c, 0x6c, 0xf6)),
            egui::Color32::WHITE
        );
    }
}
