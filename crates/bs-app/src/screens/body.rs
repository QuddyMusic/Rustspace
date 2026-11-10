//! Показ текста сообщения (markdown), цитаты-ответа и реакций.

use egui::{CornerRadius, Margin, RichText, Sense, Stroke, Vec2};

use crate::i18n::tr;
use crate::markdown::{Block, Kind, Span};
use crate::theme;

/// Реакции одного эмодзи под сообщением.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReactionGroup {
    /// Эмодзи.
    pub(crate) emoji: String,
    /// Сколько человек поставили.
    pub(crate) count: usize,
    /// Поставил ли я.
    pub(crate) mine: bool,
    /// Имена поставивших (для подсказки).
    pub(crate) names: Vec<String>,
}

/// Результат щелчка по строке реакций.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReactionClick {
    /// Поставить (`false`) или снять (`true`) реакцию.
    Toggle(String, bool),
    /// Открыть выбор эмодзи.
    OpenPicker,
}

/// Группирует плоские реакции по эмодзи, сохраняя порядок появления.
pub(crate) fn group_reactions<'a>(
    reactions: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>,
    me: &str,
) -> Vec<ReactionGroup> {
    let mut groups: Vec<ReactionGroup> = Vec::new();
    for (emoji, user_id, name) in reactions {
        let mine = user_id == me;
        if let Some(group) = groups.iter_mut().find(|g| g.emoji == emoji) {
            group.count += 1;
            group.mine |= mine;
            group.names.push(name.to_owned());
        } else {
            groups.push(ReactionGroup {
                emoji: emoji.to_owned(),
                count: 1,
                mine,
                names: vec![name.to_owned()],
            });
        }
    }
    groups
}

fn rich(span: &Span) -> RichText {
    let mut text = RichText::new(&span.text).color(theme::text());
    if span.bold {
        text = text.family(theme::bold_family());
    }
    if span.italic {
        text = text.italics();
    }
    if span.strike {
        text = text.strikethrough();
    }
    text
}

fn show_span(ui: &mut egui::Ui, salt: &str, index: usize, span: &Span) {
    match span.kind {
        Kind::Text => {
            ui.label(rich(span));
        }
        Kind::Code => {
            ui.label(
                RichText::new(&span.text)
                    .monospace()
                    .color(theme::text_bright())
                    .background_color(theme::bg_input()),
            );
        }
        Kind::Link => {
            ui.add(egui::Hyperlink::from_label_and_url(
                RichText::new(&span.text).color(theme::sky()),
                &span.text,
            ))
            .on_hover_text(&span.text);
        }
        Kind::Mention => {
            ui.label(
                RichText::new(&span.text)
                    .color(theme::text_bright())
                    .family(theme::bold_family())
                    .background_color(theme::accent().gamma_multiply(0.45)),
            );
        }
        Kind::Spoiler => {
            let id = ui.make_persistent_id((salt, index, "spoiler"));
            let shown = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
            if shown {
                ui.label(rich(span).background_color(theme::bg_active()));
            } else {
                let response = ui.label(
                    RichText::new(&span.text)
                        .color(theme::bg_active())
                        .background_color(theme::bg_active()),
                );
                if response
                    .interact(Sense::click())
                    .on_hover_text(tr("Показать спойлер"))
                    .clicked()
                {
                    ui.data_mut(|d| d.insert_temp(id, true));
                }
            }
        }
    }
}

/// Делит отрезки по переводам строк.
fn lines(spans: &[Span]) -> Vec<Vec<Span>> {
    let mut out: Vec<Vec<Span>> = vec![Vec::new()];
    for span in spans {
        let mut parts = span.text.split('\n');
        if let Some(first) = parts.next() {
            if let Some(line) = out.last_mut() {
                line.push(Span {
                    text: first.to_owned(),
                    ..span.clone()
                });
            }
        }
        for part in parts {
            out.push(vec![Span {
                text: part.to_owned(),
                ..span.clone()
            }]);
        }
    }
    out
}

fn show_spans(ui: &mut egui::Ui, salt: &str, spans: &[Span], tail: Option<&str>) {
    let rows = lines(spans);
    let last = rows.len().saturating_sub(1);
    for (row_index, row) in rows.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (index, span) in row.iter().enumerate() {
                if !span.text.is_empty() {
                    show_span(ui, salt, row_index * 1000 + index, span);
                }
            }
            if row_index == last
                && let Some(tail) = tail
            {
                ui.label(theme::muted(tail).size(11.0));
            }
        });
    }
}

/// Показывает разобранный текст сообщения.
pub(crate) fn show_text(ui: &mut egui::Ui, salt: &str, blocks: &[Block], edited: bool) {
    let mut tail = edited.then_some(tr(" (изменено)"));
    let count = blocks.len();
    for (index, block) in blocks.iter().enumerate() {
        let is_last = index + 1 == count;
        match block {
            Block::Paragraph(spans) => {
                let marker = if is_last { tail.take() } else { None };
                show_spans(ui, &format!("{salt}{index}"), spans, marker);
            }
            Block::Quote(spans) => {
                ui.horizontal(|ui| {
                    let (bar, _) = ui.allocate_exact_size(Vec2::new(3.0, 18.0), Sense::hover());
                    ui.painter()
                        .rect_filled(bar, CornerRadius::same(2), theme::text_faint());
                    ui.vertical(|ui| show_spans(ui, &format!("{salt}{index}"), spans, None));
                });
            }
            Block::Code { lang, code } => {
                egui::Frame::new()
                    .fill(theme::bg_input())
                    .stroke(Stroke::new(1.0, theme::border()))
                    .corner_radius(crate::theme::rounded(8))
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        let font = egui::TextStyle::Monospace.resolve(ui.style());
                        let highlighted = lang.as_deref().and_then(|lang| {
                            crate::highlight::job(code, lang, &font, theme::is_dark())
                        });
                        match highlighted {
                            Some(job) => {
                                ui.label(job);
                            }
                            None => {
                                ui.label(
                                    RichText::new(code).monospace().color(theme::text_bright()),
                                );
                            }
                        }
                    });
            }
        }
    }
    if let Some(tail) = tail {
        ui.label(theme::muted(tail.trim()).size(11.0));
    }
}

/// Цитата сообщения, на которое отвечают.
pub(crate) fn show_reply_quote(ui: &mut egui::Ui, author: &str, text: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.set_max_width((ui.available_width() - 28.0).max(80.0));
        let (bar, _) = ui.allocate_exact_size(Vec2::new(3.0, 16.0), Sense::hover());
        ui.painter()
            .rect_filled(bar, CornerRadius::same(2), theme::accent());
        ui.label(
            RichText::new(author)
                .family(theme::bold_family())
                .size(12.5)
                .color(theme::text_muted()),
        );
        ui.add(egui::Label::new(theme::muted(text.replace('\n', " ")).size(12.5)).truncate());
    })
    .response
}

/// Строка реакций с кнопкой «добавить».
#[allow(clippy::cast_precision_loss)]
///
/// Чипы только рисуются и сообщают свои прямоугольники: клик над ними
/// перехватывает отклик всей строки сообщения (он создан позже и лежит
/// сверху), поэтому нажатие разбирает вызывающий по этим областям.
pub(crate) fn show_reactions(
    ui: &mut egui::Ui,
    groups: &[ReactionGroup],
) -> Vec<(egui::Rect, ReactionClick)> {
    let mut targets = Vec::new();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
        for group in groups {
            let count = group.count.to_string();
            let width = 7.0f32.mul_add(count.chars().count().min(6) as f32, 42.0);
            let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::hover());
            let hovered = ui.rect_contains_pointer(rect);
            let fill = if group.mine {
                theme::accent().gamma_multiply(0.35)
            } else if hovered {
                theme::bg_active()
            } else {
                theme::bg_hover()
            };
            ui.painter()
                .rect_filled(rect, crate::theme::rounded(12), fill);
            if group.mine {
                ui.painter().rect_stroke(
                    rect,
                    crate::theme::rounded(12),
                    Stroke::new(1.0, theme::accent()),
                    egui::StrokeKind::Inside,
                );
            }
            let icon = egui::Rect::from_min_size(
                rect.left_center() + Vec2::new(8.0, -9.0),
                Vec2::splat(18.0),
            );
            if !crate::screens::composer::paint_glyph(ui, icon, &group.emoji) {
                ui.painter().text(
                    icon.center(),
                    egui::Align2::CENTER_CENTER,
                    &group.emoji,
                    egui::FontId::proportional(14.0),
                    theme::text(),
                );
            }
            ui.painter().text(
                egui::pos2(icon.right() + 6.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                &count,
                egui::FontId::proportional(13.0),
                if group.mine {
                    theme::text_bright()
                } else {
                    theme::text_muted()
                },
            );
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                response.show_tooltip_text(group.names.join(", "));
            }
            targets.push((rect, ReactionClick::Toggle(group.emoji.clone(), group.mine)));
        }
        if !groups.is_empty() {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(30.0, 24.0), Sense::hover());
            let hovered = ui.rect_contains_pointer(rect);
            if hovered {
                ui.painter()
                    .rect_filled(rect, crate::theme::rounded(12), theme::bg_hover());
            }
            crate::icons::paint(
                ui.painter(),
                rect.center(),
                14.0,
                crate::icons::Icon::Smile,
                theme::text_faint(),
            );
            if hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                response.show_tooltip_text(tr("Добавить реакцию"));
            }
            targets.push((rect, ReactionClick::OpenPicker));
        }
    });
    targets
}

#[cfg(test)]
mod tests {
    use super::group_reactions;

    #[test]
    fn reactions_group_by_emoji_in_order() {
        let groups = group_reactions(
            [("👍", "a", "Аня"), ("🎉", "b", "Боря"), ("👍", "me", "Я")],
            "me",
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].emoji, "👍");
        assert_eq!(groups[0].count, 2);
        assert!(groups[0].mine && !groups[1].mine);
        assert_eq!(groups[0].names, vec!["Аня", "Я"]);
    }
}
