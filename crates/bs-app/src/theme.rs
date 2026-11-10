//! Тема оформления: тёмная палитра в духе Discord и общие виджеты.
//!
//! Здесь нет бизнес-логики: только цвета, стили `egui` и мелкие
//! отрисовщики (аватар-заглушка, круглая кнопка, плашка статуса). Чистые
//! функции (цвет по имени, инициалы, время) покрыты тестами.

use bs_types::UserStatus;
use egui::{Align, Color32, CornerRadius, FontId, Layout, Margin, Sense, Stroke, TextStyle};

/// Цвета интерфейса: одна тема оформления.
///
/// Встроенные темы — [`Preset`]; пользователь может взять любую за основу и
/// поменять отдельные цвета, а также сохранить или загрузить тему файлом
/// JSON (`#rrggbb` для каждого поля). Текущая палитра хранится глобально:
/// интерфейс читает её через функции-аксессоры (`bg_chat()`, `accent()`…),
/// поэтому смена темы применяется со следующего кадра без перезапуска.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Palette {
    /// Тёмная тема (база стилей `egui` — тёмная или светлая).
    pub(crate) dark: bool,
    /// Фон приложения и колонки пространств (`--bg-base`).
    #[serde(with = "hex")]
    pub(crate) bg_rail: Color32,
    /// Колонка каналов и панель участников (`--bg-channel`).
    #[serde(with = "hex")]
    pub(crate) bg_sidebar: Color32,
    /// Лента сообщений (`--bg-chat`).
    #[serde(with = "hex")]
    pub(crate) bg_chat: Color32,
    /// Утопленные поля ввода (`--bg-input`).
    #[serde(with = "hex")]
    pub(crate) bg_input: Color32,
    /// Плавающие элементы: поле ввода, окна, меню.
    #[serde(with = "hex")]
    pub(crate) bg_bubble: Color32,
    /// Наведение на элемент списка.
    #[serde(with = "hex")]
    pub(crate) bg_hover: Color32,
    /// Выбранный элемент списка (`--bg-elevated`).
    #[serde(with = "hex")]
    pub(crate) bg_active: Color32,
    /// Наведение на кнопку.
    #[serde(with = "hex")]
    pub(crate) widget_hover: Color32,
    /// Тонкие границы и разделители.
    #[serde(with = "hex")]
    pub(crate) border: Color32,
    /// Основной текст.
    #[serde(with = "hex")]
    pub(crate) text: Color32,
    /// Приглушённый текст.
    #[serde(with = "hex")]
    pub(crate) text_muted: Color32,
    /// Третичный текст: подсказки, категории.
    #[serde(with = "hex")]
    pub(crate) text_faint: Color32,
    /// Яркий текст имён и заголовков.
    #[serde(with = "hex")]
    pub(crate) text_bright: Color32,
    /// Подпись канала в покое.
    #[serde(with = "hex")]
    pub(crate) channel_text: Color32,
    /// Брендовый акцент.
    #[serde(with = "hex")]
    pub(crate) accent: Color32,
    /// Текст на акцентной заливке.
    #[serde(with = "hex")]
    pub(crate) on_accent: Color32,
    /// Позитивные действия.
    #[serde(with = "hex")]
    pub(crate) mint: Color32,
    /// Подложка под мятный текст (кнопка «Добавить»).
    #[serde(with = "hex")]
    pub(crate) mint_bg: Color32,
    /// Непрочитанное и ошибки.
    #[serde(with = "hex")]
    pub(crate) rose: Color32,
    /// Предупреждения, «нет на месте».
    #[serde(with = "hex")]
    pub(crate) amber: Color32,
    /// Онлайн, говорит.
    #[serde(with = "hex")]
    pub(crate) green: Color32,
    /// Ссылки и информационные акценты.
    #[serde(with = "hex")]
    pub(crate) sky: Color32,
    /// «Не беспокоить», опасные действия.
    #[serde(with = "hex")]
    pub(crate) red: Color32,
    /// Офлайн.
    #[serde(with = "hex")]
    pub(crate) gray: Color32,
}

impl Default for Palette {
    fn default() -> Self {
        Self::AETHER
    }
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[allow(clippy::unreadable_literal, reason = "цвета записаны как #rrggbb")]
impl Palette {
    /// «Aether Drift» — тёплая тёмная тема оригинального Backspace.
    pub(crate) const AETHER: Self = Self {
        dark: true,
        bg_rail: rgb(0x0e0e14),
        bg_sidebar: rgb(0x1a1a23),
        bg_chat: rgb(0x13131a),
        bg_input: rgb(0x111118),
        bg_bubble: rgb(0x1d1d27),
        bg_hover: rgb(0x21212c),
        bg_active: rgb(0x252530),
        widget_hover: rgb(0x2e2e3b),
        border: rgb(0x282832),
        text: rgb(0xe4e4ea),
        text_muted: rgb(0xa0a0aa),
        text_faint: rgb(0x6d6d7c),
        text_bright: rgb(0xefefef),
        channel_text: rgb(0x8a8a98),
        accent: rgb(0x7c6cf6),
        on_accent: rgb(0xffffff),
        mint: rgb(0x86efac),
        mint_bg: rgb(0x173b21),
        rose: rgb(0xfda4af),
        amber: rgb(0xfcd34d),
        green: rgb(0x4ade80),
        sky: rgb(0x7dd3fc),
        red: rgb(0xf87171),
        gray: rgb(0x6d6d7c),
    };

    /// «Полночь» — глубокий чёрный для OLED-экранов.
    pub(crate) const MIDNIGHT: Self = Self {
        bg_rail: rgb(0x000000),
        bg_sidebar: rgb(0x0a0a0c),
        bg_chat: rgb(0x050506),
        bg_input: rgb(0x111113),
        bg_bubble: rgb(0x141417),
        bg_hover: rgb(0x16161a),
        bg_active: rgb(0x1c1c21),
        widget_hover: rgb(0x26262c),
        border: rgb(0x1f1f24),
        channel_text: rgb(0x7f7f8a),
        accent: rgb(0x8b7cff),
        ..Self::AETHER
    };

    /// «Nord» — холодная сине-серая палитра.
    pub(crate) const NORD: Self = Self {
        dark: true,
        bg_rail: rgb(0x242933),
        bg_sidebar: rgb(0x2e3440),
        bg_chat: rgb(0x2b303b),
        bg_input: rgb(0x252a34),
        bg_bubble: rgb(0x343b48),
        bg_hover: rgb(0x3b4252),
        bg_active: rgb(0x434c5e),
        widget_hover: rgb(0x4c566a),
        border: rgb(0x3b4252),
        text: rgb(0xe5e9f0),
        text_muted: rgb(0xb4bccc),
        text_faint: rgb(0x7b8394),
        text_bright: rgb(0xeceff4),
        channel_text: rgb(0x9aa3b5),
        accent: rgb(0x88c0d0),
        on_accent: rgb(0x1f232b),
        mint: rgb(0xa3be8c),
        mint_bg: rgb(0x2f3b2c),
        rose: rgb(0xd08770),
        amber: rgb(0xebcb8b),
        green: rgb(0xa3be8c),
        sky: rgb(0x81a1c1),
        red: rgb(0xbf616a),
        gray: rgb(0x7b8394),
    };

    /// «Сумерки» — фиолетовая тёмная тема с розовыми акцентами.
    pub(crate) const DUSK: Self = Self {
        dark: true,
        bg_rail: rgb(0x191622),
        bg_sidebar: rgb(0x221e2e),
        bg_chat: rgb(0x1e1a29),
        bg_input: rgb(0x18151f),
        bg_bubble: rgb(0x2a2538),
        bg_hover: rgb(0x2c2739),
        bg_active: rgb(0x352e46),
        widget_hover: rgb(0x3d3550),
        border: rgb(0x332c42),
        text: rgb(0xf0ecf8),
        text_muted: rgb(0xb7aecb),
        text_faint: rgb(0x7d7392),
        text_bright: rgb(0xf8f8f2),
        channel_text: rgb(0x9a90b0),
        accent: rgb(0xff79c6),
        on_accent: rgb(0x1e1a29),
        mint: rgb(0x50fa7b),
        mint_bg: rgb(0x1d3a27),
        rose: rgb(0xff92df),
        amber: rgb(0xf1fa8c),
        green: rgb(0x50fa7b),
        sky: rgb(0x8be9fd),
        red: rgb(0xff5555),
        gray: rgb(0x7d7392),
    };

    /// «Дневная» — светлая тема.
    pub(crate) const DAYLIGHT: Self = Self {
        dark: false,
        bg_rail: rgb(0xe3e5ea),
        bg_sidebar: rgb(0xf0f1f5),
        bg_chat: rgb(0xfbfbfd),
        bg_input: rgb(0xebedf2),
        bg_bubble: rgb(0xffffff),
        bg_hover: rgb(0xe6e8ef),
        bg_active: rgb(0xdcdfe8),
        widget_hover: rgb(0xd3d7e2),
        border: rgb(0xd8dbe3),
        text: rgb(0x2b2d36),
        text_muted: rgb(0x5d606e),
        text_faint: rgb(0x8a8d9a),
        text_bright: rgb(0x14151b),
        channel_text: rgb(0x60636f),
        accent: rgb(0x6555e8),
        on_accent: rgb(0xffffff),
        mint: rgb(0x15803d),
        mint_bg: rgb(0xd1f2dc),
        rose: rgb(0xd6336c),
        amber: rgb(0xb45309),
        green: rgb(0x16a34a),
        sky: rgb(0x0369a1),
        red: rgb(0xdc2626),
        gray: rgb(0x9ca3af),
    };

    /// «Высокий контраст» — максимальная читаемость.
    pub(crate) const CONTRAST: Self = Self {
        dark: true,
        bg_rail: rgb(0x000000),
        bg_sidebar: rgb(0x000000),
        bg_chat: rgb(0x000000),
        bg_input: rgb(0x101010),
        bg_bubble: rgb(0x141414),
        bg_hover: rgb(0x262626),
        bg_active: rgb(0x333333),
        widget_hover: rgb(0x404040),
        border: rgb(0x8a8a8a),
        text: rgb(0xffffff),
        text_muted: rgb(0xe0e0e0),
        text_faint: rgb(0xbdbdbd),
        text_bright: rgb(0xffffff),
        channel_text: rgb(0xe0e0e0),
        accent: rgb(0xffd400),
        on_accent: rgb(0x000000),
        mint: rgb(0x00ff7f),
        mint_bg: rgb(0x003a1c),
        rose: rgb(0xff6b8b),
        amber: rgb(0xffd400),
        green: rgb(0x00ff7f),
        sky: rgb(0x4dd2ff),
        red: rgb(0xff4040),
        gray: rgb(0xbdbdbd),
    };

    /// Цвета для редактора темы: подпись и ссылка на поле.
    pub(crate) fn fields_mut(&mut self) -> [(&'static str, &mut Color32); 24] {
        [
            ("Фон рейла пространств", &mut self.bg_rail),
            ("Боковые панели", &mut self.bg_sidebar),
            ("Лента сообщений", &mut self.bg_chat),
            ("Поля ввода", &mut self.bg_input),
            ("Окна и меню", &mut self.bg_bubble),
            ("Наведение", &mut self.bg_hover),
            ("Выбранный элемент", &mut self.bg_active),
            ("Наведение на кнопку", &mut self.widget_hover),
            ("Границы", &mut self.border),
            ("Текст", &mut self.text),
            ("Приглушённый текст", &mut self.text_muted),
            ("Подсказки", &mut self.text_faint),
            ("Заголовки и имена", &mut self.text_bright),
            ("Названия каналов", &mut self.channel_text),
            ("Акцент", &mut self.accent),
            ("Текст на акценте", &mut self.on_accent),
            ("Позитивное действие", &mut self.mint),
            ("Подложка позитивного", &mut self.mint_bg),
            ("Непрочитанное, ошибки", &mut self.rose),
            ("Предупреждения", &mut self.amber),
            ("В сети", &mut self.green),
            ("Ссылки", &mut self.sky),
            ("Опасные действия", &mut self.red),
            ("Не в сети", &mut self.gray),
        ]
    }
}

/// Встроенные темы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Preset {
    /// Тема оригинального клиента.
    Aether,
    /// Чёрная для OLED.
    Midnight,
    /// Nord.
    Nord,
    /// Фиолетовая.
    Dusk,
    /// Светлая.
    Daylight,
    /// Высокий контраст.
    Contrast,
}

impl Preset {
    /// Все встроенные темы в порядке показа.
    pub(crate) const ALL: [Self; 6] = [
        Self::Aether,
        Self::Midnight,
        Self::Nord,
        Self::Dusk,
        Self::Daylight,
        Self::Contrast,
    ];

    /// Название для интерфейса.
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Aether => "Aether Drift",
            Self::Midnight => "Полночь",
            Self::Nord => "Nord",
            Self::Dusk => "Сумерки",
            Self::Daylight => "Дневная",
            Self::Contrast => "Высокий контраст",
        }
    }

    /// Палитра темы.
    pub(crate) const fn palette(self) -> Palette {
        match self {
            Self::Aether => Palette::AETHER,
            Self::Midnight => Palette::MIDNIGHT,
            Self::Nord => Palette::NORD,
            Self::Dusk => Palette::DUSK,
            Self::Daylight => Palette::DAYLIGHT,
            Self::Contrast => Palette::CONTRAST,
        }
    }
}

/// Сохраняемые настройки оформления.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Appearance {
    /// Встроенная тема, взятая за основу.
    pub(crate) preset: Preset,
    /// Своя палитра поверх темы (после правки цветов или загрузки файла).
    pub(crate) custom: Option<Palette>,
    /// Скругление углов карточек и кнопок, px.
    pub(crate) radius: u8,
    /// Компактная лента: меньше отступов между сообщениями.
    pub(crate) compact: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            preset: Preset::Aether,
            custom: None,
            radius: 8,
            compact: false,
        }
    }
}

impl Appearance {
    /// Палитра, которую нужно показать.
    pub(crate) fn palette(&self) -> Palette {
        self.custom.unwrap_or_else(|| self.preset.palette())
    }
}

/// Текущая палитра.
static CURRENT: std::sync::RwLock<Palette> = std::sync::RwLock::new(Palette::AETHER);
/// Скругление и плотность ленты.
static SHAPE: std::sync::RwLock<(u8, bool)> = std::sync::RwLock::new((8, false));

fn current() -> Palette {
    *CURRENT
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

macro_rules! palette_accessors {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {
        $(
            $(#[$doc])*
            #[must_use]
            pub(crate) fn $name() -> Color32 {
                current().$name
            }
        )*
    };
}

palette_accessors!(
    /// Фон рейла пространств.
    bg_rail,
    /// Боковые панели.
    bg_sidebar,
    /// Лента сообщений.
    bg_chat,
    /// Поля ввода.
    bg_input,
    /// Окна и плавающие элементы.
    bg_bubble,
    /// Наведение.
    bg_hover,
    /// Выбранный элемент.
    bg_active,
    /// Границы.
    border,
    /// Основной текст.
    text,
    /// Приглушённый текст.
    text_muted,
    /// Подсказки.
    text_faint,
    /// Заголовки и имена.
    text_bright,
    /// Названия каналов в покое.
    channel_text,
    /// Акцент.
    accent,
    /// Текст на акценте.
    on_accent,
    /// Позитивное действие.
    mint,
    /// Подложка позитивного.
    mint_bg,
    /// Непрочитанное и ошибки.
    rose,
    /// Предупреждения.
    amber,
    /// В сети.
    green,
    /// Ссылки.
    sky,
    /// Опасные действия.
    red,
    /// Не в сети.
    gray,
);

/// Скругление углов из настроек.
#[must_use]
pub(crate) fn radius() -> u8 {
    SHAPE.read().map_or(8, |shape| shape.0)
}

/// Включена ли компактная лента.
#[must_use]
pub(crate) fn compact() -> bool {
    SHAPE.read().is_ok_and(|shape| shape.1)
}

/// `#rrggbb` для полей палитры.
mod hex {
    use egui::Color32;
    use serde::{Deserialize as _, Deserializer, Serializer};

    #[allow(
        clippy::trivially_copy_pass_by_ref,
        reason = "сигнатура задана serde(with)"
    )]
    pub(super) fn serialize<S: Serializer>(color: &Color32, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&format!(
            "#{:02x}{:02x}{:02x}",
            color.r(),
            color.g(),
            color.b()
        ))
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Color32, D::Error> {
        let text = String::deserialize(de)?;
        super::parse_hex(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("ожидался цвет #rrggbb: {text}")))
    }
}

/// Разбирает `#rrggbb` или `rrggbb`.
#[must_use]
pub(crate) fn parse_hex(text: &str) -> Option<Color32> {
    let digits = text.trim().trim_start_matches('#');
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok().map(rgb)
}

/// Имя семейства полужирного начертания (Inter SemiBold).
const BOLD_FAMILY: &str = "inter-semibold";

/// Семейство для заголовков и имён: у `egui` нет синтетического жирного.
#[must_use]
pub(crate) fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name(BOLD_FAMILY.into())
}

/// Подключает Inter (латиница и кириллица), как у оригинального клиента
/// для русского языка. Встроенные шрифты egui остаются запасными — из них
/// берутся эмодзи и символы.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter-regular".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        BOLD_FAMILY.to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    let fallback = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut regular = vec!["inter-regular".to_owned()];
    regular.extend(fallback.iter().cloned());
    fonts
        .families
        .insert(egui::FontFamily::Proportional, regular);
    let mut bold = vec![BOLD_FAMILY.to_owned()];
    bold.extend(fallback);
    fonts.families.insert(bold_family(), bold);
    ctx.set_fonts(fonts);
}

/// Цвет из палитры аватаров Backspace (`mint`, `sky`, …); `None` для неизвестных.
#[must_use]
pub(crate) fn palette(name: &str) -> Option<Color32> {
    Some(match name {
        "mint" => Color32::from_rgb(0x10, 0xb9, 0x81),
        "sky" => Color32::from_rgb(0x5b, 0x7c, 0xf6),
        "lavender" => Color32::from_rgb(0xb4, 0x84, 0xfa),
        "coral" => Color32::from_rgb(0xf9, 0x6a, 0x2f),
        "rose" => Color32::from_rgb(0xf0, 0x4a, 0x8a),
        "teal" => Color32::from_rgb(0x14, 0xb8, 0xc4),
        "amber" => Color32::from_rgb(0xf5, 0xa5, 0x0b),
        _ => return None,
    })
}

/// Названия цветов палитры в порядке сервера.
pub(crate) const PALETTE_NAMES: [&str; 7] =
    ["mint", "sky", "lavender", "coral", "rose", "teal", "amber"];

/// Подключает шрифты; вызывается один раз при запуске.
pub(crate) fn install(ctx: &egui::Context) {
    install_fonts(ctx);
}

/// Применяет оформление: палитру, скругления и стили `egui`.
pub(crate) fn apply(ctx: &egui::Context, appearance: &Appearance) {
    let palette = appearance.palette();
    if let Ok(mut current) = CURRENT.write() {
        *current = palette;
    }
    if let Ok(mut shape) = SHAPE.write() {
        *shape = (appearance.radius, appearance.compact);
    }
    let p = palette;
    let mut visuals = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.panel_fill = p.bg_chat;
    visuals.window_fill = p.bg_bubble;
    visuals.extreme_bg_color = p.bg_input;
    visuals.faint_bg_color = p.bg_sidebar;
    visuals.code_bg_color = p.bg_input;
    visuals.override_text_color = Some(p.text);
    visuals.hyperlink_color = p.sky;
    visuals.warn_fg_color = p.amber;
    visuals.error_fg_color = p.red;
    visuals.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.85 } else { 0.35 });
    visuals.selection.stroke = Stroke::new(1.0, p.text_bright);
    visuals.window_stroke = Stroke::new(1.0, p.border);
    let big = appearance.radius.saturating_add(4);
    visuals.window_corner_radius = CornerRadius::same(big);
    visuals.menu_corner_radius = CornerRadius::same(appearance.radius.saturating_add(2));
    visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }),
    };
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 4],
        blur: 14,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 90 } else { 30 }),
    };

    let radius = CornerRadius::same(appearance.radius);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = p.bg_chat;
    widgets.noninteractive.weak_bg_fill = p.bg_chat;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    widgets.noninteractive.corner_radius = radius;
    widgets.inactive.bg_fill = p.bg_active;
    widgets.inactive.weak_bg_fill = p.bg_active;
    widgets.inactive.bg_stroke = Stroke::NONE;
    widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    widgets.inactive.corner_radius = radius;
    widgets.hovered.bg_fill = p.widget_hover;
    widgets.hovered.weak_bg_fill = p.widget_hover;
    widgets.hovered.bg_stroke = Stroke::NONE;
    widgets.hovered.fg_stroke = Stroke::new(1.0, p.text_bright);
    widgets.hovered.corner_radius = radius;
    widgets.active.bg_fill = p.accent;
    widgets.active.weak_bg_fill = p.accent;
    widgets.active.bg_stroke = Stroke::NONE;
    widgets.active.fg_stroke = Stroke::new(1.0, p.on_accent);
    widgets.active.corner_radius = radius;
    widgets.open.bg_fill = p.bg_active;
    widgets.open.weak_bg_fill = p.bg_active;
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
            .color(on_accent())
            .family(bold_family()),
    )
    .fill(accent())
    .corner_radius(CornerRadius::same(radius()))
}

/// Приглушённая подпись.
#[must_use]
pub(crate) fn muted(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text.into()).color(text_muted())
}

/// Цвет индикатора присутствия.
#[must_use]
pub(crate) fn status_color(status: UserStatus) -> Color32 {
    match status {
        UserStatus::Online => green(),
        UserStatus::Idle => amber(),
        UserStatus::DoNotDisturb => red(),
        UserStatus::Offline => gray(),
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
    image: Option<&str>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    paint_avatar(ui.painter(), rect, name, status, false, image);
    response
}

/// Отрисовка аватара в заданный прямоугольник.
pub(crate) fn paint_avatar(
    painter: &egui::Painter,
    rect: egui::Rect,
    name: &str,
    status: Option<UserStatus>,
    dim: bool,
    image: Option<&str>,
) {
    let size = rect.width();
    let tint = if dim {
        Color32::WHITE.gamma_multiply(0.45)
    } else {
        Color32::WHITE
    };
    let drawn = image.is_some_and(|value| {
        crate::images::paint_value(
            painter,
            rect,
            value,
            CornerRadius::same(u8::try_from(size.round() as u32 / 2 + 1).unwrap_or(u8::MAX)),
            false,
            tint,
        )
    });
    if !drawn {
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
            if dim { text_muted() } else { Color32::WHITE },
        );
    }
    if let Some(status) = status {
        let dot = (size * 0.17).max(4.0);
        let center = rect.right_bottom() - egui::vec2(size * 0.12, size * 0.12);
        painter.circle_filled(center, dot + 2.0, bg_sidebar());
        painter.circle_filled(center, dot, status_color(status));
    }
}

/// Кнопка позитивного действия: тёмно-мятная подложка и мятный текст.
pub(crate) fn positive_button(text: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new(
        egui::RichText::new(text.into())
            .color(mint())
            .family(bold_family()),
    )
    .fill(mint_bg())
    .corner_radius(CornerRadius::same(radius()))
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
    painter.rect_filled(icon, CornerRadius::same(radius), accent());
    crate::brand::paint(painter, icon.center(), 26.0, on_accent());
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
        painter.rect_filled(pill, CornerRadius::same(3), text_bright());
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
    image: Option<&str>,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 50.0), Sense::click());
    let hovered = response.hovered();
    let icon = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(46.0));
    let radius: u8 = if selected || hovered { 14 } else { 23 };
    let painter = ui.painter();
    let drawn = image.is_some_and(|value| {
        crate::images::paint_value(
            painter,
            icon,
            value,
            CornerRadius::same(radius),
            hovered,
            Color32::WHITE,
        )
    });
    if !drawn {
        painter.rect_filled(icon, CornerRadius::same(radius), color);
        painter.text(
            icon.center(),
            egui::Align2::CENTER_CENTER,
            text,
            FontId::proportional(16.0),
            Color32::WHITE,
        );
    }
    paint_rail_pill(painter, rect, selected, hovered, unread);
    response
}

/// Тонкий разделитель в колонке пространств.
pub(crate) fn rail_separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 2.0), Sense::hover());
    let line = egui::Rect::from_center_size(rect.center(), egui::vec2(28.0, 2.0));
    ui.painter()
        .rect_filled(line, CornerRadius::same(1), border());
}

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
        Some(bg_active())
    } else if response.hovered() {
        Some(bg_hover())
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
        ui.painter()
            .rect_filled(bar, CornerRadius::same(2), accent());
    }
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(row.shrink2(egui::vec2(10.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    leading(&mut inner);
    let color = if selected || unread {
        text_bright()
    } else {
        channel_text()
    };
    let mut text = egui::RichText::new(label).color(color);
    if unread {
        text = text.family(bold_family());
    }
    inner.add(egui::Label::new(text).truncate().selectable(false));
    if unread {
        inner.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (dot, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, rose());
        });
    }
    response
}

/// Значок «заглушён» (перечёркнутый кружок) у правого края строки.
pub(crate) fn muted_mark(ui: &egui::Ui, row: &egui::Rect) {
    let center = egui::pos2(row.right() - 18.0, row.center().y);
    let stroke = egui::Stroke::new(1.4, text_faint());
    ui.painter().circle_stroke(center, 5.5, stroke);
    ui.painter().line_segment(
        [
            center + egui::vec2(-3.8, 3.8),
            center + egui::vec2(3.8, -3.8),
        ],
        stroke,
    );
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

/// Строка голосового канала; `active` — мы сейчас в нём.
pub(crate) fn voice_row(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(
            rect.shrink2(egui::vec2(8.0, 1.0)),
            CornerRadius::same(8),
            bg_hover(),
        );
    }
    let color = if active { green() } else { channel_text() };
    crate::icons::paint(
        ui.painter(),
        egui::pos2(rect.left() + 28.0, rect.center().y),
        16.0,
        crate::icons::Icon::Speaker,
        color,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 46.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        truncate_chars(label, 22),
        FontId::proportional(15.0),
        color,
    );
    response
}

/// Участник голосового канала под его строкой.
pub(crate) fn occupant_row(
    ui: &mut egui::Ui,
    name: &str,
    muted: bool,
    deafened: bool,
    speaking: bool,
    image: Option<&str>,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), Sense::click());
    let avatar_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 40.0, rect.center().y),
        egui::Vec2::splat(22.0),
    );
    paint_avatar(ui.painter(), avatar_rect, name, None, false, image);
    if speaking {
        ui.painter()
            .circle_stroke(avatar_rect.center(), 12.5, egui::Stroke::new(2.0, green()));
    }
    ui.painter().text(
        egui::pos2(rect.left() + 58.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        truncate_chars(name, 20),
        FontId::proportional(13.5),
        if speaking { green() } else { text_muted() },
    );
    let mut x = rect.right() - 22.0;
    if deafened {
        crate::icons::paint(
            ui.painter(),
            egui::pos2(x, rect.center().y),
            14.0,
            crate::icons::Icon::HeadphonesOff,
            rose(),
        );
        x -= 18.0;
    }
    if muted || deafened {
        crate::icons::paint(
            ui.painter(),
            egui::pos2(x, rect.center().y),
            14.0,
            crate::icons::Icon::MicOff,
            rose(),
        );
    }
    response
}

/// Отклики заголовка категории.
pub(crate) struct CategoryHeader {
    /// Название (ПКМ — настройки категории).
    pub(crate) label: egui::Response,
    /// Кнопка «+», если она показана.
    pub(crate) plus: Option<egui::Response>,
}

/// Заголовок категории каналов («ТЕКСТОВЫЕ КАНАЛЫ»).
///
/// С `add` справа появляется «+» (новый канал в категории).
pub(crate) fn category_header(ui: &mut egui::Ui, name: &str, add: bool) -> CategoryHeader {
    ui.add_space(10.0);
    let mut plus = None;
    let label = ui
        .horizontal(|ui| {
            ui.add_space(18.0);
            let label = ui.add(
                egui::Label::new(
                    egui::RichText::new(name.to_uppercase())
                        .size(11.0)
                        .family(bold_family())
                        .color(text_faint()),
                )
                .sense(egui::Sense::click()),
            );
            if add {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(10.0);
                    plus = Some(
                        ui.add(crate::widgets::flat_icon(
                            crate::icons::Icon::Plus,
                            16.0,
                            text_faint(),
                        ))
                        .on_hover_text("Создать канал"),
                    );
                });
            }
            label
        })
        .inner;
    ui.add_space(2.0);
    CategoryHeader { label, plus }
}

/// Заголовок секции в правой панели («В СЕТИ — 3»).
#[must_use]
pub(crate) fn section_title(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase())
        .size(11.0)
        .family(bold_family())
        .color(text_faint())
}

/// Участник в правой панели: аватар со статусом и имя; офлайн — приглушён.
pub(crate) fn member_row(
    ui: &mut egui::Ui,
    name: &str,
    status: UserStatus,
    image: Option<&str>,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), bg_hover());
    }
    let offline = status == UserStatus::Offline;
    let avatar_rect = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(8.0, 6.0),
        egui::Vec2::splat(32.0),
    );
    paint_avatar(
        ui.painter(),
        avatar_rect,
        name,
        Some(status),
        offline,
        image,
    );
    ui.painter().text(
        egui::pos2(avatar_rect.right() + 12.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        truncate_chars(name, 22),
        FontId::proportional(15.0),
        if offline { text_faint() } else { text() },
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
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
                .family(bold_family())
                .color(text_faint()),
        );
    });
    ui.add_space(6.0);
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

/// Короткая подпись для списка бесед: время, «вчера» или дата.
#[must_use]
pub(crate) fn short_stamp(ms: i64, now_ms: i64) -> String {
    use chrono::TimeZone as _;
    let (Some(at), Some(now)) = (
        chrono::Local.timestamp_millis_opt(ms).single(),
        chrono::Local.timestamp_millis_opt(now_ms).single(),
    ) else {
        return String::new();
    };
    match (now.date_naive() - at.date_naive()).num_days() {
        0 => at.format("%H:%M").to_string(),
        1 => "вчера".to_owned(),
        _ => at.format("%d.%m.%y").to_string(),
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
