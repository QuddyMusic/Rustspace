//! Форматирование в поле ввода: кнопки разметки и конструктор карточек.
//!
//! Кнопки оборачивают выделение (`**…**`) или ставят приставку строкам
//! (`## `, `- `). Карточка — блок с цветной полосой, заголовком, текстом и
//! картинкой, как embed у Discord; уходит отдельным запросом
//! (`POST /api/rustspace/channels/:id/cards`).

use egui::{Color32, Id, Margin, RichText, Vec2};

use crate::i18n::tr;
use crate::theme;
use crate::widgets;

/// Цвета полосы карточки на выбор.
const COLORS: [(&str, u32); 8] = [
    ("Ржавчина", 0x00e0_6c3a),
    ("Мята", 0x0010_b981),
    ("Небо", 0x005b_7cf6),
    ("Лаванда", 0x00b4_84fa),
    ("Янтарь", 0x00f5_a50b),
    ("Роза", 0x00f0_4a8a),
    ("Красный", 0x00ef_4444),
    ("Серый", 0x006b_7280),
];

/// Действие кнопки разметки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Обернуть выделение: до и после.
    Wrap(&'static str, &'static str),
    /// Поставить приставку каждой выделенной строке.
    Prefix(&'static str),
    /// Нумерованный список.
    Numbered,
    /// Вставить отдельной строкой.
    Line(&'static str),
    /// Ссылка `[текст](https://)`.
    Link,
}

/// Кнопки панели: подпись, подсказка, действие.
const BUTTONS: [(&str, &str, Action); 12] = [
    ("B", "Жирный — **текст**", Action::Wrap("**", "**")),
    ("I", "Курсив — *текст*", Action::Wrap("*", "*")),
    ("S", "Зачёркнутый — ~~текст~~", Action::Wrap("~~", "~~")),
    ("</>", "Код — `текст`", Action::Wrap("`", "`")),
    ("▮▮", "Спойлер — ||текст||", Action::Wrap("||", "||")),
    ("H1", "Крупный заголовок — # ", Action::Prefix("# ")),
    ("H2", "Заголовок — ## ", Action::Prefix("## ")),
    ("❝", "Цитата — > ", Action::Prefix("> ")),
    ("•", "Список — - ", Action::Prefix("- ")),
    ("1.", "Нумерованный список", Action::Numbered),
    ("—", "Разделитель — ---", Action::Line("---")),
    ("🔗", "Ссылка — [текст](адрес)", Action::Link),
];

/// Байтовое смещение `chars`-го символа.
fn byte_at(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(index, _)| index)
}

/// Применяет действие к `text` с выделением `[start, end)` в символах.
/// Возвращает новое выделение (в символах).
fn apply(text: &mut String, start: usize, end: usize, action: Action) -> (usize, usize) {
    let (start, end) = (start.min(end), start.max(end));
    let (a, b) = (byte_at(text, start), byte_at(text, end));
    match action {
        Action::Wrap(before, after) => {
            let selected = text[a..b].to_owned();
            text.replace_range(a..b, &format!("{before}{selected}{after}"));
            let open = before.chars().count();
            (start + open, end + open)
        }
        Action::Link => {
            let selected = text[a..b].to_owned();
            let label = if selected.is_empty() {
                tr("текст").to_owned()
            } else {
                selected
            };
            let inserted = format!("[{label}](https://)");
            text.replace_range(a..b, &inserted);
            // Курсор — после «https://», чтобы сразу вставить адрес.
            let at = start + inserted.chars().count() - 1;
            (at, at)
        }
        Action::Line(line) => {
            let need_break = a > 0 && !text[..a].ends_with('\n');
            let inserted = format!("{}{line}\n", if need_break { "\n" } else { "" });
            text.replace_range(a..b, &inserted);
            let at = start + inserted.chars().count();
            (at, at)
        }
        Action::Prefix(_) | Action::Numbered => {
            let line_start = text[..a].rfind('\n').map_or(0, |i| i + 1);
            let line_end = text[b..].find('\n').map_or(text.len(), |i| b + i);
            let block = text[line_start..line_end].to_owned();
            let changed: Vec<String> = block
                .split('\n')
                .enumerate()
                .map(|(index, line)| match action {
                    Action::Prefix(prefix) => format!("{prefix}{line}"),
                    _ => format!("{}. {line}", index + 1),
                })
                .collect();
            let changed = changed.join("\n");
            let grown = changed.chars().count() - block.chars().count();
            text.replace_range(line_start..line_end, &changed);
            (start, end + grown)
        }
    }
}

/// Черновик карточки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CardForm {
    /// Текст сообщения над карточкой.
    pub(crate) content: String,
    title: String,
    description: String,
    url: String,
    image: String,
    color: u32,
}

impl Default for CardForm {
    fn default() -> Self {
        Self {
            content: String::new(),
            title: String::new(),
            description: String::new(),
            url: String::new(),
            image: String::new(),
            color: COLORS[0].1,
        }
    }
}

impl CardForm {
    /// Карточка для отправки; `None` — пустая.
    pub(crate) fn draft(&self) -> Option<bs_api::CardDraft> {
        let some = |text: &str| {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        };
        let web = |text: &str| some(text).filter(|url| url.starts_with("http"));
        let draft = bs_api::CardDraft {
            title: some(&self.title),
            description: some(&self.description),
            url: web(&self.url),
            color: Some(format!("#{:06x}", self.color)),
            image: web(&self.image).map(|url| bs_api::CardImage { url }),
        };
        (draft.title.is_some() || draft.description.is_some() || draft.image.is_some())
            .then_some(draft)
    }

    /// Как карточку покажет лента.
    fn preview(&self) -> bs_types::Embed {
        let draft = self.draft().unwrap_or_default();
        bs_types::Embed {
            id: String::new(),
            message_id: None,
            dm_message_id: None,
            url: draft.url.unwrap_or_default(),
            embed_type: bs_types::EmbedType::Generic,
            provider: None,
            title: draft.title,
            description: draft.description,
            image: None,
            embed_url: None,
            width: None,
            height: None,
            color: draft.color,
            created_at: 0,
        }
    }
}

/// Итог окна карточки.
pub(crate) enum CardResult {
    /// Окно открыто.
    Open,
    /// Закрыли без отправки.
    Closed,
    /// Отправить.
    Send(Option<String>, bs_api::CardDraft),
}

/// Панель разметки над полем ввода. `true` — нажата «Карточка».
pub(crate) fn bar(ui: &mut egui::Ui, draft: &mut String, edit_id: Id, cards: bool) -> bool {
    let mut card = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (label, hint, action) in BUTTONS {
            let text = RichText::new(label).size(13.0).color(theme::text_muted());
            let text = match label {
                "B" => text.family(theme::bold_family()),
                "I" => text.italics(),
                "S" => text.strikethrough(),
                _ => text,
            };
            let button = egui::Button::new(text)
                .frame(false)
                .min_size(Vec2::new(28.0, 24.0));
            if ui.add(button).on_hover_text(tr(hint)).clicked() {
                let mut state = egui::TextEdit::load_state(ui.ctx(), edit_id).unwrap_or_default();
                let total = draft.chars().count();
                let (start, end) = state.cursor.char_range().map_or((total, total), |range| {
                    (range.primary.index.into(), range.secondary.index.into())
                });
                let (start, end) = apply(draft, start.min(total), end.min(total), action);
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(start),
                        egui::text::CCursor::new(end),
                    )));
                state.store(ui.ctx(), edit_id);
                ui.memory_mut(|m| m.request_focus(edit_id));
            }
        }
        if cards {
            ui.add_space(6.0);
            if ui
                .add(egui::Button::new(
                    RichText::new(tr("Карточка…"))
                        .size(13.0)
                        .color(theme::text()),
                ))
                .on_hover_text(tr("Блок с цветной полосой, заголовком и картинкой"))
                .clicked()
            {
                card = true;
            }
        }
    });
    card
}

/// Окно конструктора карточки.
pub(crate) fn card_window(ctx: &egui::Context, form: &mut CardForm) -> CardResult {
    let mut result = CardResult::Open;
    let modal = egui::Modal::new(Id::unique("card_builder")).show(ctx, |ui| {
        ui.set_width(520.0);
        ui.label(
            RichText::new(tr("Карточка"))
                .size(18.0)
                .family(theme::bold_family())
                .color(theme::text_bright()),
        );
        ui.label(theme::muted(tr(
            "Блок, как у ботов Discord: удобно для правил, объявлений и новостей. \
             В тексте работает разметка: **жирный**, списки, ## заголовки.",
        )));
        ui.add_space(8.0);
        let max_height = (ui.ctx().content_rect().height() - 200.0).max(200.0);
        egui::ScrollArea::vertical()
            .max_height(max_height)
            .show(ui, |ui| card_fields(ui, form));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let ready = form.draft();
            if ui
                .add_enabled(ready.is_some(), theme::primary_button(tr("Отправить")))
                .clicked()
                && let Some(card) = ready
            {
                let content = form.content.trim();
                result = CardResult::Send((!content.is_empty()).then(|| content.to_owned()), card);
            }
            if ui.button(tr("Отмена")).clicked() {
                result = CardResult::Closed;
            }
        });
    });
    if matches!(result, CardResult::Open) && modal.should_close() {
        result = CardResult::Closed;
    }
    result
}

/// Поля карточки и предпросмотр.
fn card_fields(ui: &mut egui::Ui, form: &mut CardForm) {
    let field = |ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str| {
        widgets::section_label(ui, label);
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(f32::INFINITY)
                .margin(Vec2::new(10.0, 6.0)),
        );
        ui.add_space(4.0);
    };
    field(ui, tr("Заголовок"), &mut form.title, tr("Правила сервера"));
    widgets::section_label(ui, tr("Текст"));
    ui.add(
        egui::TextEdit::multiline(&mut form.description)
            .hint_text(tr("- Не спамить\n- Уважать друг друга"))
            .desired_rows(5)
            .desired_width(f32::INFINITY)
            .margin(Vec2::new(10.0, 6.0)),
    );
    ui.add_space(4.0);
    widgets::section_label(ui, tr("Цвет полосы"));
    ui.horizontal_wrapped(|ui| {
        for (name, color) in COLORS {
            let [_, r, g, b] = color.to_be_bytes();
            let fill = Color32::from_rgb(r, g, b);
            let (rect, response) = ui.allocate_exact_size(Vec2::splat(24.0), egui::Sense::click());
            ui.painter().circle_filled(rect.center(), 10.0, fill);
            if form.color == color {
                ui.painter().circle_stroke(
                    rect.center(),
                    12.0,
                    egui::Stroke::new(2.0, theme::text_bright()),
                );
            }
            if response.on_hover_text(tr(name)).clicked() {
                form.color = color;
            }
        }
    });
    ui.add_space(4.0);
    field(ui, tr("Ссылка заголовка"), &mut form.url, "https://");
    field(
        ui,
        tr("Картинка (адрес)"),
        &mut form.image,
        "https://…/banner.png",
    );
    field(
        ui,
        tr("Текст над карточкой (необязательно)"),
        &mut form.content,
        tr("@everyone, обновили правила"),
    );
    widgets::section_label(ui, tr("Как будет выглядеть"));
    egui::Frame::new()
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            if form.draft().is_some() {
                crate::screens::body::show_embed(ui, "card_preview", &form.preview(), &|_| None);
            } else {
                ui.label(theme::muted(tr("Заполните заголовок или текст")));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{Action, apply};

    #[test]
    fn wraps_selection_and_prefixes_lines() {
        let mut text = "сделай жирным".to_owned();
        let range = apply(&mut text, 7, 13, Action::Wrap("**", "**"));
        assert_eq!(text, "сделай **жирным**");
        assert_eq!(range, (9, 15));

        let mut text = "раз\nдва\nтри".to_owned();
        apply(&mut text, 1, 5, Action::Prefix("- "));
        assert_eq!(text, "- раз\n- два\nтри");

        let mut text = "а\nб".to_owned();
        apply(&mut text, 0, 3, Action::Numbered);
        assert_eq!(text, "1. а\n2. б");

        let mut text = "текст".to_owned();
        let range = apply(&mut text, 5, 5, Action::Line("---"));
        assert_eq!(text, "текст\n---\n");
        assert_eq!(range, (10, 10));
    }
}
