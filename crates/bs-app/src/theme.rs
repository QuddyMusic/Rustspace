//! Тема оформления: тёмная палитра в духе Discord и общие виджеты.
//!
//! Здесь нет бизнес-логики: только цвета, стили `egui` и мелкие
//! отрисовщики (аватар-заглушка, круглая кнопка, плашка статуса). Чистые
//! функции (цвет по имени, инициалы, время) покрыты тестами.

use crate::i18n::tr;
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
    /// Стекло: модули окна полупрозрачные, за ними — мягкое свечение.
    pub(crate) glass: bool,
    /// Непрозрачность стеклянных панелей, % (40–100).
    pub(crate) glass_opacity: u8,
    /// Сила свечения за панелями, % (0–40).
    pub(crate) glow: u8,
    /// Цвет свечения сверху справа.
    #[serde(with = "hex")]
    pub(crate) glow_a: Color32,
    /// Цвет свечения снизу слева.
    #[serde(with = "hex")]
    pub(crate) glow_b: Color32,
}

impl Default for Palette {
    // Недостающие поля старого файла темы берутся отсюда: у тем до
    // появления стекла его нет.
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
        glass: false,
        glass_opacity: 100,
        glow: 0,
        glow_a: rgb(0x7c6cf6),
        glow_b: rgb(0xe8743b),
    };

    /// «Оксид» — тёмное стекло Rustspace: чернильно-фиолетовая основа,
    /// ржавый акцент, полупрозрачные панели на мягком свечении.
    pub(crate) const OXIDE: Self = Self {
        dark: true,
        bg_rail: rgb(0x100d17),
        bg_sidebar: rgb(0x1e192a),
        bg_chat: rgb(0x181422),
        bg_input: rgb(0x120f1a),
        bg_bubble: rgb(0x241e32),
        bg_hover: rgb(0x2a2438),
        bg_active: rgb(0x322a44),
        widget_hover: rgb(0x3a3150),
        border: rgb(0x2e2740),
        text: rgb(0xdedae8),
        text_muted: rgb(0xb6aecb),
        text_faint: rgb(0x8d86a0),
        text_bright: rgb(0xf4f1f9),
        channel_text: rgb(0x9c95ad),
        accent: rgb(0xe8743b),
        on_accent: rgb(0xffffff),
        mint: rgb(0x86efac),
        mint_bg: rgb(0x173b21),
        rose: rgb(0xfda4af),
        amber: rgb(0xf5b94a),
        green: rgb(0x6fe0a6),
        sky: rgb(0x8ab4ff),
        red: rgb(0xf06a6f),
        gray: rgb(0x6f6884),
        glass: true,
        glass_opacity: 62,
        glow: 16,
        glow_a: rgb(0x7c6cf6),
        glow_b: rgb(0xe8743b),
    };

    /// «Оксид светлая» — то же стекло на тёплом светлом фоне.
    pub(crate) const OXIDE_LIGHT: Self = Self {
        dark: false,
        bg_rail: rgb(0xece7e0),
        bg_sidebar: rgb(0xfbf9f6),
        bg_chat: rgb(0xffffff),
        bg_input: rgb(0xf3eee8),
        bg_bubble: rgb(0xffffff),
        bg_hover: rgb(0xf0ebe4),
        bg_active: rgb(0xf0dccd),
        widget_hover: rgb(0xe9e2d8),
        border: rgb(0xddd5ca),
        text: rgb(0x221d2b),
        text_muted: rgb(0x4f4860),
        text_faint: rgb(0x6b6479),
        text_bright: rgb(0x1a1622),
        channel_text: rgb(0x655e72),
        accent: rgb(0xc75a22),
        on_accent: rgb(0xffffff),
        mint: rgb(0x23784a),
        mint_bg: rgb(0xdff3e6),
        rose: rgb(0xc2334d),
        amber: rgb(0xa86a12),
        green: rgb(0x2f9e62),
        sky: rgb(0x2b6cb0),
        red: rgb(0xc53030),
        gray: rgb(0x8a8298),
        glass: true,
        glass_opacity: 72,
        glow: 14,
        glow_a: rgb(0xb8a9ff),
        glow_b: rgb(0xf2a77a),
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
        glass: false,
        glass_opacity: 100,
        glow: 0,
        glow_a: rgb(0x7c6cf6),
        glow_b: rgb(0xe8743b),
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
        glass: false,
        glass_opacity: 100,
        glow: 0,
        glow_a: rgb(0x7c6cf6),
        glow_b: rgb(0xe8743b),
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
        glass: false,
        glass_opacity: 100,
        glow: 0,
        glow_a: rgb(0x7c6cf6),
        glow_b: rgb(0xe8743b),
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
        glass: false,
        glass_opacity: 100,
        glow: 0,
        glow_a: rgb(0x7c6cf6),
        glow_b: rgb(0xe8743b),
    };

    /// Цвета для редактора темы: подпись и ссылка на поле.
    pub(crate) fn fields_mut(&mut self) -> [(&'static str, &mut Color32); 26] {
        [
            (tr("Фон рейла пространств"), &mut self.bg_rail),
            (tr("Боковые панели"), &mut self.bg_sidebar),
            (tr("Лента сообщений"), &mut self.bg_chat),
            (tr("Поля ввода"), &mut self.bg_input),
            (tr("Окна и меню"), &mut self.bg_bubble),
            (tr("Наведение"), &mut self.bg_hover),
            (tr("Выбранный элемент"), &mut self.bg_active),
            (tr("Наведение на кнопку"), &mut self.widget_hover),
            (tr("Границы"), &mut self.border),
            (tr("Текст"), &mut self.text),
            (tr("Приглушённый текст"), &mut self.text_muted),
            (tr("Подсказки"), &mut self.text_faint),
            (tr("Заголовки и имена"), &mut self.text_bright),
            (tr("Названия каналов"), &mut self.channel_text),
            (tr("Акцент"), &mut self.accent),
            (tr("Текст на акценте"), &mut self.on_accent),
            (tr("Позитивное действие"), &mut self.mint),
            (tr("Подложка позитивного"), &mut self.mint_bg),
            (tr("Непрочитанное, ошибки"), &mut self.rose),
            (tr("Предупреждения"), &mut self.amber),
            (tr("В сети"), &mut self.green),
            (tr("Ссылки"), &mut self.sky),
            (tr("Опасные действия"), &mut self.red),
            (tr("Не в сети"), &mut self.gray),
            (tr("Свечение сверху"), &mut self.glow_a),
            (tr("Свечение снизу"), &mut self.glow_b),
        ]
    }
}

/// Встроенные темы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Preset {
    /// Тёмное стекло Rustspace.
    Oxide,
    /// Светлое стекло Rustspace.
    OxideLight,
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
    pub(crate) const ALL: [Self; 8] = [
        Self::Oxide,
        Self::OxideLight,
        Self::Aether,
        Self::Midnight,
        Self::Nord,
        Self::Dusk,
        Self::Daylight,
        Self::Contrast,
    ];

    /// Название для интерфейса.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Oxide => tr("Оксид"),
            Self::OxideLight => tr("Оксид светлая"),
            Self::Aether => "Aether Drift",
            Self::Midnight => tr("Полночь"),
            Self::Nord => "Nord",
            Self::Dusk => tr("Сумерки"),
            Self::Daylight => tr("Дневная"),
            Self::Contrast => tr("Высокий контраст"),
        }
    }

    /// Палитра темы.
    pub(crate) const fn palette(self) -> Palette {
        match self {
            Self::Oxide => Palette::OXIDE,
            Self::OxideLight => Palette::OXIDE_LIGHT,
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
    /// Чёткий текст, как у egui по умолчанию: хинтинг и усиленное
    /// сглаживание. Без него текст мягче и тоньше — как в браузере.
    pub(crate) crisp_text: bool,
    /// Где кнопка «к последним сообщениям».
    pub(crate) jump_place: JumpPlace,
}

/// Место кнопки «к последним сообщениям» над полем ввода.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum JumpPlace {
    /// По центру ленты.
    #[default]
    Center,
    /// У правого края.
    Right,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            preset: Preset::Oxide,
            custom: None,
            radius: 12,
            compact: false,
            crisp_text: false,
            jump_place: JumpPlace::Center,
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
static CURRENT: std::sync::RwLock<Palette> = std::sync::RwLock::new(Palette::OXIDE);
/// Скругление и плотность ленты.
static SHAPE: std::sync::RwLock<(u8, bool)> = std::sync::RwLock::new((8, false));

fn current() -> Palette {
    *CURRENT
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Тёмная ли текущая тема.
#[must_use]
pub(crate) fn is_dark() -> bool {
    current().dark
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

/// Скругление, рассчитанное на стандартные 8 px, под настройку пользователя.
///
/// Все рамки интерфейса берут радиус отсюда: при 0 углы прямые, при 16 —
/// вдвое круглее стандартных.
#[must_use]
pub(crate) fn rounded(base: u8) -> CornerRadius {
    let scaled = (u32::from(base) * u32::from(radius()) + 4) / 8;
    CornerRadius::same(u8::try_from(scaled).unwrap_or(u8::MAX))
}

/// Зазор между модулями окна: при прямых углах модули стоят вплотную.
#[must_use]
pub(crate) fn module_gap() -> i8 {
    match (radius(), glass()) {
        (0, _) => 0,
        (_, true) => 8,
        _ => 6,
    }
}

/// Включено ли стекло в текущей теме.
#[must_use]
pub(crate) fn glass() -> bool {
    current().glass
}

/// Заливка панели: при стекле — полупрозрачная, сквозь неё видно свечение.
#[must_use]
pub(crate) fn surface(fill: Color32) -> Color32 {
    let p = current();
    if !p.glass {
        return fill;
    }
    with_alpha(fill, p.glass_opacity.clamp(30, 100))
}

/// Цвет с непрозрачностью `percent` %.
#[must_use]
pub(crate) fn with_alpha(color: Color32, percent: u8) -> Color32 {
    let alpha = u8::try_from(u16::from(percent.min(100)) * 255 / 100).unwrap_or(u8::MAX);
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// Кромка стекла: светлая на тёмной теме, тёмная на светлой.
#[must_use]
pub(crate) fn glass_stroke() -> Stroke {
    glass_stroke_for(is_dark())
}

fn glass_stroke_for(dark: bool) -> Stroke {
    if dark {
        Stroke::new(1.0, Color32::from_white_alpha(22))
    } else {
        Stroke::new(1.0, Color32::from_black_alpha(20))
    }
}

/// Утопленная подложка внутри панели (плашка пользователя, вставки).
#[must_use]
pub(crate) fn inset() -> Color32 {
    match (glass(), is_dark()) {
        (true, true) => Color32::from_black_alpha(46),
        (true, false) => Color32::from_white_alpha(150),
        (false, _) => bg_rail(),
    }
}

/// Рамка плавающего элемента (поле ввода, меню, панели поверх ленты):
/// при стекле — полупрозрачная, с кромкой и мягкой тенью.
pub(crate) fn floating(fill: Color32, base_radius: u8) -> egui::Frame {
    let frame = egui::Frame::new().corner_radius(rounded(base_radius));
    if glass() {
        frame
            .fill(with_alpha(
                fill,
                current().glass_opacity.saturating_add(18).min(96),
            ))
            .stroke(glass_stroke())
            .shadow(egui::epaint::Shadow {
                offset: [0, 6],
                blur: 22,
                spread: 0,
                color: Color32::from_black_alpha(if is_dark() { 70 } else { 22 }),
            })
    } else {
        frame.fill(fill).stroke(Stroke::new(1.0, border()))
    }
}

/// Рамка модуля главного окна (боковая панель, чат, участники…):
/// своя подложка, скругление из настроек и зазор вокруг.
pub(crate) fn module(fill: Color32) -> egui::Frame {
    let gap = module_gap();
    let frame = egui::Frame::new()
        .fill(surface(fill))
        .corner_radius(rounded(12))
        .outer_margin(Margin {
            left: 0,
            right: gap,
            top: gap,
            bottom: gap,
        });
    if glass() {
        frame.stroke(glass_stroke())
    } else {
        frame
    }
}

/// Рамка колонки пространств: при стекле — такой же модуль, как остальные.
pub(crate) fn rail_frame() -> egui::Frame {
    if glass() {
        let gap = module_gap();
        module(bg_sidebar())
            .outer_margin(Margin {
                left: gap,
                right: gap,
                top: gap,
                bottom: gap,
            })
            .inner_margin(Margin::symmetric(0, 10))
    } else {
        egui::Frame::new()
            .fill(bg_rail())
            .inner_margin(Margin::symmetric(0, 10))
    }
}

/// Подложка под модулями окна — цвет зазоров между ними, а при стекле
/// ещё и мягкое свечение, которое просвечивает сквозь панели.
pub(crate) fn paint_backdrop(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, bg_rail());
    paint_glows(painter, rect, &current());
}

/// Два пятна свечения палитры `p` в прямоугольнике `rect` (для фона окна и
/// миниатюр тем). Без стекла или при нулевой силе ничего не рисует.
pub(crate) fn paint_glows(painter: &egui::Painter, rect: egui::Rect, p: &Palette) {
    if !p.glass || p.glow == 0 {
        return;
    }
    let strength = f32::from(p.glow.min(60)) / 100.0;
    let (w, h) = (rect.width(), rect.height());
    paint_glow(
        painter,
        rect.min + egui::vec2(w * 0.78, h * 0.08),
        egui::vec2(w * 0.47, h * 0.53),
        p.glow_a,
        strength,
    );
    paint_glow(
        painter,
        rect.min + egui::vec2(w * 0.30, h),
        egui::vec2(w * 0.55, h * 0.62),
        p.glow_b,
        strength,
    );
}

/// Мягкое эллиптическое пятно цвета `color`: к краю прозрачность плавно
/// растёт до нуля (как `radial-gradient(… , transparent 70%)`).
fn paint_glow(
    painter: &egui::Painter,
    center: egui::Pos2,
    radii: egui::Vec2,
    color: Color32,
    strength: f32,
) {
    const RINGS: u32 = 24;
    const SEGMENTS: u32 = 72;
    let [red, green, blue, _] = color.to_srgba_unmultiplied();
    let tint = |t: f32| {
        // Плавный спад: полная сила в центре, ноль на 70 % радиуса.
        let x = (t / 0.7).min(1.0);
        let smooth = x * x * 2.0f32.mul_add(-x, 3.0);
        let falloff = (1.0 - smooth).max(0.0);
        let alpha = (strength * falloff * 255.0).round().clamp(0.0, 255.0);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "alpha в пределах 0..=255"
        )]
        Color32::from_rgba_unmultiplied(red, green, blue, alpha as u8)
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(center, tint(0.0));
    for ring in 1..=RINGS {
        #[allow(clippy::cast_precision_loss, reason = "малые числа")]
        let t = ring as f32 / RINGS as f32;
        for segment in 0..SEGMENTS {
            #[allow(clippy::cast_precision_loss, reason = "малые числа")]
            let angle = segment as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            let offset = egui::vec2(angle.cos() * radii.x, angle.sin() * radii.y) * t;
            mesh.colored_vertex(center + offset, tint(t));
        }
    }
    let at = |ring: u32, segment: u32| {
        if ring == 0 {
            0
        } else {
            1 + (ring - 1) * SEGMENTS + segment % SEGMENTS
        }
    };
    for segment in 0..SEGMENTS {
        mesh.add_triangle(at(0, 0), at(1, segment), at(1, segment + 1));
    }
    for ring in 1..RINGS {
        for segment in 0..SEGMENTS {
            let (inner, inner_next) = (at(ring, segment), at(ring, segment + 1));
            let (outer, outer_next) = (at(ring + 1, segment), at(ring + 1, segment + 1));
            mesh.add_triangle(inner, outer, outer_next);
            mesh.add_triangle(inner, outer_next, inner_next);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
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
        super::parse_hex(&text).ok_or_else(|| {
            serde::de::Error::custom(crate::tf!("ожидался цвет #rrggbb: {text}", text = text))
        })
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

/// Без «чёткого текста» — мягкая отрисовка букв. По умолчанию egui на
/// тёмном фоне утолщает глифы (2c − c²) и прижимает их к пиксельной сетке —
/// текст выходит жирным и резким. Браузер рисует мягче: без жёсткого
/// хинтинга и с гаммой около 1.
fn soften_text(visuals: &mut egui::Visuals, crisp: bool, dark: bool) {
    if crisp {
        return;
    }
    visuals.text_options.font_hinting = false;
    if dark {
        visuals.text_options.color_transfer_function =
            egui::epaint::FontColorTransferFunction::Gamma(1.15);
    }
}

/// Палитра — для обеих тем egui, а тема закреплена за ней: иначе при
/// светлой системе Windows стандартные поля и кнопки становятся белыми.
fn set_visuals(ctx: &egui::Context, dark: bool, visuals: egui::Visuals) {
    ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
    ctx.set_visuals_of(egui::Theme::Light, visuals);
}

/// Применяет оформление: палитру, скругления и стили `egui`.
pub(crate) fn apply(ctx: &egui::Context, appearance: &Appearance) {
    let mut palette = appearance.palette();
    if palette.glass {
        // Наведение и выделение на стекле — тоже полупрозрачные.
        palette.bg_hover = with_alpha(palette.bg_hover, 62);
        palette.bg_active = with_alpha(palette.bg_active, 78);
        palette.widget_hover = with_alpha(palette.widget_hover, 82);
    }
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
    visuals.window_fill = if p.glass {
        with_alpha(p.bg_bubble, p.glass_opacity.saturating_add(26).min(96))
    } else {
        p.bg_bubble
    };
    visuals.extreme_bg_color = p.bg_input;
    visuals.faint_bg_color = p.bg_sidebar;
    visuals.code_bg_color = p.bg_input;
    visuals.override_text_color = Some(p.text);
    soften_text(&mut visuals, appearance.crisp_text, p.dark);
    visuals.hyperlink_color = p.sky;
    visuals.warn_fg_color = p.amber;
    visuals.error_fg_color = p.red;
    visuals.selection.bg_fill = p.accent.gamma_multiply(if p.dark { 0.85 } else { 0.35 });
    visuals.selection.stroke = Stroke::new(1.0, p.text_bright);
    visuals.window_stroke = if p.glass {
        glass_stroke_for(p.dark)
    } else {
        Stroke::new(1.0, p.border)
    };
    visuals.window_corner_radius = rounded(12);
    visuals.menu_corner_radius = rounded(10);
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
    set_visuals(ctx, p.dark, visuals);

    ctx.all_styles_mut(|style| {
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

/// Градиент заглушки: начало, конец (135°, как CSS-градиенты оригинала).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gradient {
    /// Цвет левого верхнего угла.
    pub(crate) from: Color32,
    /// Цвет правого нижнего угла.
    pub(crate) to: Color32,
}

const fn grad(from: u32, to: u32) -> Gradient {
    Gradient {
        from: rgb(from),
        to: rgb(to),
    }
}

/// `AVATAR_GRADIENTS` оригинала, в порядке `mint, sky, lavender, coral,
/// rose, teal, amber`.
#[allow(clippy::unreadable_literal, reason = "цвета записаны как #rrggbb")]
const AVATAR_GRADIENTS: [Gradient; 7] = [
    grad(0x059669, 0x10b981),
    grad(0x3b82f6, 0x6366f1),
    grad(0xa78bfa, 0xc084fc),
    grad(0xf97316, 0xef4444),
    grad(0xf43f5e, 0xec4899),
    grad(0x14b8a6, 0x06b6d4),
    grad(0xf59e0b, 0xeab308),
];

/// `SPACE_GRADIENTS` оригинала (порядок для хэша: red-orange, pink,
/// teal-cyan, amber-yellow, blue-indigo, lavender, mint).
#[allow(clippy::unreadable_literal, reason = "цвета записаны как #rrggbb")]
const SPACE_GRADIENTS: [Gradient; 7] = [
    grad(0xef4444, 0xf97316),
    grad(0xec4899, 0xf472b6),
    grad(0x14b8a6, 0x06b6d4),
    grad(0xf59e0b, 0xeab308),
    grad(0x3b82f6, 0x6366f1),
    grad(0xa78bfa, 0xc084fc),
    grad(0x059669, 0x10b981),
];

/// djb2 по UTF-16, как `hashString` оригинала: одинаковый цвет у одного
/// пользователя во всех клиентах.
fn djb2(text: &str) -> u32 {
    text.encode_utf16().fold(5381_u32, |hash, unit| {
        (hash << 5).wrapping_add(hash).wrapping_add(u32::from(unit))
    })
}

fn named_index(color: &str) -> Option<usize> {
    PALETTE_NAMES.iter().position(|name| *name == color)
}

/// Градиент аватара пользователя: сохранённый `avatarColor` или хэш id
/// (а без id — имени).
#[must_use]
pub(crate) fn avatar_gradient(key: &str, color: Option<&str>) -> Gradient {
    color.and_then(named_index).map_or_else(
        || AVATAR_GRADIENTS[djb2(key) as usize % AVATAR_GRADIENTS.len()],
        |index| AVATAR_GRADIENTS[index],
    )
}

/// Градиент иконки пространства.
#[must_use]
pub(crate) fn space_gradient(key: &str, color: Option<&str>) -> Gradient {
    // Порядок SPACE_GRADIENT_MAP оригинала по именам палитры.
    const BY_NAME: [usize; 7] = [6, 4, 5, 0, 1, 2, 3];
    color.and_then(named_index).map_or_else(
        || SPACE_GRADIENTS[djb2(key) as usize % SPACE_GRADIENTS.len()],
        |index| SPACE_GRADIENTS[BY_NAME[index]],
    )
}

/// Средний цвет градиента — для мест, где нужна одна заливка.
#[must_use]
pub(crate) fn color_for(seed: &str) -> Color32 {
    let g = avatar_gradient(seed, None);
    g.from.lerp_to_gamma(g.to, 0.5)
}

/// Заливает прямоугольник со скруглением диагональным градиентом.
pub(crate) fn fill_gradient(
    painter: &egui::Painter,
    rect: egui::Rect,
    rounding: f32,
    gradient: Gradient,
    alpha: f32,
) {
    let shape = egui::epaint::RectShape::filled(
        rect,
        CornerRadius::same(rounding.round().clamp(0.0, 255.0) as u8),
        Color32::WHITE,
    );
    let mut mesh = egui::epaint::Mesh::default();
    let mut tess = egui::epaint::Tessellator::new(
        painter.pixels_per_point(),
        egui::epaint::TessellationOptions::default(),
        [1, 1],
        Vec::new(),
    );
    tess.tessellate_rect(&shape, &mut mesh);
    let span = (rect.width() + rect.height()).max(1.0);
    for vertex in &mut mesh.vertices {
        let t = ((vertex.pos.x - rect.left()) + (vertex.pos.y - rect.top())) / span;
        let color = gradient.from.lerp_to_gamma(gradient.to, t.clamp(0.0, 1.0));
        vertex.color = color.gamma_multiply(alpha * f32::from(vertex.color.a()) / 255.0);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Кто изображён на аватаре: имя для буквы, ключ для цвета, `avatarColor`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Face<'a> {
    /// Отображаемое имя (первая буква — на заглушке).
    pub(crate) name: &'a str,
    /// id пользователя (а для федеративного — `homeUserId`).
    pub(crate) key: Option<&'a str>,
    /// Сохранённый цвет аватара.
    pub(crate) color: Option<&'a str>,
}

impl<'a> From<&'a str> for Face<'a> {
    fn from(name: &'a str) -> Self {
        Self {
            name,
            key: None,
            color: None,
        }
    }
}

impl<'a> From<&'a String> for Face<'a> {
    fn from(name: &'a String) -> Self {
        Self::from(name.as_str())
    }
}

/// Первая буква имени заглавной (`name.charAt(0).toUpperCase()` оригинала).
#[must_use]
pub(crate) fn first_letter(name: &str) -> String {
    name.trim()
        .chars()
        .next()
        .map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

/// Рисует круглый аватар-заглушку (инициалы на цветном фоне) и, если
/// задан статус, индикатор присутствия в правом нижнем углу.
pub(crate) fn avatar<'a>(
    ui: &mut egui::Ui,
    face: impl Into<Face<'a>>,
    size: f32,
    status: Option<UserStatus>,
    image: Option<&str>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    paint_avatar(ui.painter(), rect, face, status, false, image);
    response
}

/// Отрисовка аватара в заданный прямоугольник.
pub(crate) fn paint_avatar<'a>(
    painter: &egui::Painter,
    rect: egui::Rect,
    face: impl Into<Face<'a>>,
    status: Option<UserStatus>,
    dim: bool,
    image: Option<&str>,
) {
    let face = face.into();
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
        let gradient = avatar_gradient(face.key.unwrap_or(face.name), face.color);
        fill_gradient(
            painter,
            rect,
            size / 2.0,
            gradient,
            if dim { 0.45 } else { 1.0 },
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            first_letter(face.name),
            FontId::new((size * 0.4).round(), bold_family()),
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
    // Значок Rustspace: на наведении чуть крупнее, как у пространств.
    let icon = if selected || response.hovered() {
        icon.expand(1.0)
    } else {
        icon
    };
    if let Some(texture) = crate::brand::icon_texture(ui.ctx(), icon.width()) {
        painter.image(
            texture.id(),
            icon,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        painter.rect_filled(icon, CornerRadius::same(radius), accent());
        crate::brand::paint(painter, icon.center(), 26.0, on_accent());
    }
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
    gradient: Gradient,
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
        fill_gradient(painter, icon, f32::from(radius), gradient, 1.0);
        painter.text(
            icon.center(),
            egui::Align2::CENTER_CENTER,
            text,
            FontId::new(17.0, bold_family()),
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
        ui.painter().rect_filled(row, rounded(8), bg);
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
    // Справа — место под точку непрочитанного и значок «заглушён».
    let label_width = inner.available_width() - 22.0;
    let color = if selected || unread {
        text_bright()
    } else {
        channel_text()
    };
    let mut text = egui::RichText::new(label).color(color);
    if unread {
        text = text.family(bold_family());
    }
    inner.scope(|ui| {
        ui.set_max_width(label_width.max(1.0));
        ui.add(egui::Label::new(text).truncate().selectable(false));
    });
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
        ui.painter()
            .rect_filled(rect.shrink2(egui::vec2(8.0, 1.0)), rounded(8), bg_hover());
    }
    let color = if active { green() } else { channel_text() };
    crate::icons::paint(
        ui.painter(),
        egui::pos2(rect.left() + 28.0, rect.center().y),
        16.0,
        crate::icons::Icon::Speaker,
        color,
    );
    text_fit(
        ui.painter(),
        egui::pos2(rect.left() + 46.0, rect.center().y),
        label,
        FontId::proportional(15.0),
        color,
        rect.right() - 12.0 - (rect.left() + 46.0),
    );
    response
}

/// Что показать у участника голосового канала.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "независимые флаги состояния")]
pub(crate) struct OccupantFlags {
    /// Микрофон выключен.
    pub(crate) muted: bool,
    /// Звук выключен.
    pub(crate) deafened: bool,
    /// Говорит прямо сейчас.
    pub(crate) speaking: bool,
    /// Включена камера.
    pub(crate) camera: bool,
    /// Показывает экран («В ЭФИРЕ»).
    pub(crate) streaming: bool,
}

/// Участник голосового канала под его строкой.
pub(crate) fn occupant_row(
    ui: &mut egui::Ui,
    name: &str,
    flags: OccupantFlags,
    image: Option<&str>,
) -> egui::Response {
    let OccupantFlags {
        muted,
        deafened,
        speaking,
        camera,
        streaming,
    } = flags;
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
    let icons: f32 = [deafened, muted || deafened, camera]
        .iter()
        .map(|on| if *on { 1.0 } else { 0.0 })
        .sum();
    let badge = if streaming { 64.0 } else { 0.0 };
    text_fit(
        ui.painter(),
        egui::pos2(rect.left() + 58.0, rect.center().y),
        name,
        FontId::proportional(13.5),
        if speaking { green() } else { text_muted() },
        rect.right() - 14.0 - icons.mul_add(18.0, badge) - (rect.left() + 58.0),
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
        x -= 18.0;
    }
    if camera {
        crate::icons::paint(
            ui.painter(),
            egui::pos2(x, rect.center().y),
            14.0,
            crate::icons::Icon::Camera,
            text_muted(),
        );
        x -= 18.0;
    }
    if streaming {
        let galley = ui.painter().layout_no_wrap(
            crate::i18n::tr("В ЭФИРЕ").to_owned(),
            FontId::proportional(9.5),
            Color32::WHITE,
        );
        let badge = egui::Rect::from_center_size(
            egui::pos2(x + 7.0 - galley.size().x / 2.0, rect.center().y),
            galley.size() + egui::vec2(8.0, 3.0),
        );
        ui.painter().rect_filled(badge, 4.0, red());
        ui.painter()
            .galley(badge.center() - galley.size() / 2.0, galley, Color32::WHITE);
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
            let width = ui.available_width() - if add { 34.0 } else { 8.0 };
            let label = ui
                .scope(|ui| {
                    ui.set_max_width(width.max(1.0));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(name.to_uppercase())
                                .size(11.0)
                                .family(bold_family())
                                .color(text_faint()),
                        )
                        .truncate()
                        .sense(egui::Sense::click()),
                    )
                })
                .inner;
            if add {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(10.0);
                    plus = Some(
                        ui.add(crate::widgets::flat_icon(
                            crate::icons::Icon::Plus,
                            16.0,
                            text_faint(),
                        ))
                        .on_hover_text(tr("Создать канал")),
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

fn bot_badge_galley(ui: &egui::Ui) -> std::sync::Arc<egui::Galley> {
    ui.painter().layout_no_wrap(
        crate::i18n::tr("БОТ").to_owned(),
        FontId::new(10.0, bold_family()),
        on_accent(),
    )
}

/// Размер метки «БОТ».
pub(crate) fn bot_badge_size(ui: &egui::Ui) -> egui::Vec2 {
    bot_badge_galley(ui).size() + egui::vec2(10.0, 4.0)
}

/// Рисует метку «БОТ»; `pos` — левый край по центру строки.
pub(crate) fn paint_bot_badge(ui: &egui::Ui, pos: egui::Pos2) {
    let galley = bot_badge_galley(ui);
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let rect = egui::Rect::from_min_size(egui::pos2(pos.x, pos.y - size.y / 2.0), size);
    ui.painter().rect_filled(rect, rounded(4), accent());
    ui.painter()
        .galley(rect.min + egui::vec2(5.0, 2.0), galley, on_accent());
}

/// Метка «БОТ» в строке рядом с именем.
pub(crate) fn bot_badge(ui: &mut egui::Ui) -> egui::Response {
    let size = bot_badge_size(ui);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    paint_bot_badge(ui, rect.left_center());
    response.on_hover_text(crate::i18n::tr("Бот: пишет программа"))
}

/// Участник в правой панели: аватар со статусом и имя; офлайн — приглушён.
pub(crate) fn member_row<'a>(
    ui: &mut egui::Ui,
    face: impl Into<Face<'a>>,
    status: UserStatus,
    name_color: Option<Color32>,
    image: Option<&str>,
    activity: Option<(crate::icons::Icon, &str, Color32)>,
    bot: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, rounded(8), bg_hover());
    }
    let face = face.into();
    let name = face.name;
    let offline = status == UserStatus::Offline;
    let avatar_rect = egui::Rect::from_min_size(
        rect.left_top() + egui::vec2(8.0, 6.0),
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
    let left = avatar_rect.right() + 12.0;
    let width = rect.right() - 8.0 - left;
    let name_y = if activity.is_some() {
        rect.center().y - 8.0
    } else {
        rect.center().y
    };
    let badge = if bot { bot_badge_size(ui).x + 6.0 } else { 0.0 };
    let name_width = text_fit(
        ui.painter(),
        egui::pos2(left, name_y),
        name,
        FontId::proportional(15.0),
        // Цвет роли не зависит от присутствия: у офлайн он просто приглушён.
        match (name_color, offline) {
            (Some(color), true) => color.gamma_multiply(0.55),
            (Some(color), false) => color,
            (None, true) => text_faint(),
            (None, false) => text(),
        },
        width - badge,
    );
    if bot {
        paint_bot_badge(ui, egui::pos2(left + name_width + 6.0, name_y));
    }
    // Вторая строка: во что играет или что слушает.
    if let Some((icon, line, color)) = activity {
        let y = rect.center().y + 9.0;
        crate::icons::paint(ui.painter(), egui::pos2(left + 6.0, y), 12.0, icon, color);
        text_fit(
            ui.painter(),
            egui::pos2(left + 16.0, y),
            line,
            FontId::proportional(12.0),
            text_muted(),
            width - 16.0,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Однострочный текст, обрезанный многоточием по ширине `max_width`;
/// `pos` — левый край по центру строки. Так подпись не залезает на значки
/// справа, сколько бы букв в ней ни было.
pub(crate) fn text_fit(
    painter: &egui::Painter,
    pos: egui::Pos2,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> f32 {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(1.0));
    let galley = painter.layout_job(job);
    let top = egui::pos2(pos.x, pos.y - galley.size().y / 2.0);
    let width = galley.size().x;
    painter.galley(top, galley, color);
    width
}

/// Подпись, обрезанная многоточием по ширине `width` (а не по числу букв).
pub(crate) fn label_fit(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
    width: f32,
) -> egui::Response {
    ui.scope(|ui| {
        ui.set_max_width(width.max(1.0));
        ui.add(egui::Label::new(text).truncate())
    })
    .inner
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
        0 => crate::tf!("Сегодня в {a0}", a0 = at.format("%H:%M")),
        1 => crate::tf!("Вчера в {a0}", a0 = at.format("%H:%M")),
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
        1 => tr("вчера").to_owned(),
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
    use super::{stamp, truncate_chars};

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
    fn gradients_match_the_original_client() {
        use super::{avatar_gradient, djb2, rgb, space_gradient};
        // Значения hashString из web/src/utils/gradients.ts.
        assert_eq!(djb2(""), 5381);
        assert_eq!(djb2("a"), 177_670);
        assert_eq!(djb2("alice"), 253_185_475);
        // Сохранённый цвет главнее хэша.
        assert_eq!(
            avatar_gradient("whatever", Some("teal")).from,
            rgb(0x14_b8a6)
        );
        assert_eq!(
            space_gradient("whatever", Some("mint")).from,
            rgb(0x05_9669)
        );
        assert_eq!(
            space_gradient("whatever", Some("coral")).from,
            rgb(0xef_4444)
        );
        // Без цвета — устойчивый выбор по ключу.
        assert_eq!(avatar_gradient("42", None), avatar_gradient("42", None));
        assert_eq!(
            avatar_gradient("x", Some("unknown")),
            avatar_gradient("x", None)
        );
    }

    #[test]
    fn first_letter_is_uppercase() {
        use super::first_letter;
        assert_eq!(first_letter("alice"), "A");
        assert_eq!(first_letter("  борис"), "Б");
        assert_eq!(first_letter(""), "?");
    }
}
