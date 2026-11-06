//! Показ текста сообщения (markdown), цитаты-ответа и реакций.

use egui::{Color32, CornerRadius, Margin, RichText, Sense, Stroke, Vec2};

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
    let mut text = RichText::new(&span.text).color(theme::TEXT);
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
                    .color(theme::TEXT_BRIGHT)
                    .background_color(theme::BG_INPUT),
            );
        }
        Kind::Link => {
            ui.add(egui::Hyperlink::from_label_and_url(
                RichText::new(&span.text).color(theme::SKY),
                &span.text,
            ))
            .on_hover_text(&span.text);
        }
        Kind::Mention => {
            ui.label(
                RichText::new(&span.text)
                    .color(theme::TEXT_BRIGHT)
                    .family(theme::bold_family())
                    .background_color(theme::ACCENT.gamma_multiply(0.45)),
            );
        }
        Kind::Spoiler => {
            let id = ui.make_persistent_id((salt, index, "spoiler"));
            let shown = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
            if shown {
                ui.label(rich(span).background_color(theme::BG_ACTIVE));
            } else {
                let response = ui.label(
                    RichText::new(&span.text)
                        .color(theme::BG_ACTIVE)
                        .background_color(theme::BG_ACTIVE),
                );
                if response
                    .interact(Sense::click())
                    .on_hover_text("Показать спойлер")
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
    let mut tail = edited.then_some(" (изменено)");
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
                        .rect_filled(bar, CornerRadius::same(2), theme::TEXT_FAINT);
                    ui.vertical(|ui| show_spans(ui, &format!("{salt}{index}"), spans, None));
                });
            }
            Block::Code(code) => {
                egui::Frame::new()
                    .fill(theme::BG_INPUT)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(code).monospace().color(theme::TEXT_BRIGHT));
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
            .rect_filled(bar, CornerRadius::same(2), theme::ACCENT);
        ui.label(
            RichText::new(author)
                .family(theme::bold_family())
                .size(12.5)
                .color(theme::TEXT_MUTED),
        );
        ui.add(egui::Label::new(theme::muted(text.replace('\n', " ")).size(12.5)).truncate());
    })
    .response
}

/// Строка реакций с кнопкой «добавить».
#[allow(clippy::cast_precision_loss)]
pub(crate) fn show_reactions(ui: &mut egui::Ui, groups: &[ReactionGroup]) -> Option<ReactionClick> {
    let mut click = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
        for group in groups {
            let count = group.count.to_string();
            let width = 7.0f32.mul_add(count.chars().count().min(6) as f32, 42.0);
            let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::click());
            let fill = if group.mine {
                theme::ACCENT.gamma_multiply(0.35)
            } else if response.hovered() {
                theme::BG_ACTIVE
            } else {
                theme::BG_HOVER
            };
            ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
            if group.mine {
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(12),
                    Stroke::new(1.0, theme::ACCENT),
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
                    theme::TEXT,
                );
            }
            ui.painter().text(
                egui::pos2(icon.right() + 6.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                &count,
                egui::FontId::proportional(13.0),
                if group.mine {
                    Color32::WHITE
                } else {
                    theme::TEXT_MUTED
                },
            );
            let response = response
                .on_hover_text(group.names.join(", "))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                click = Some(ReactionClick::Toggle(group.emoji.clone(), group.mine));
            }
        }
        if !groups.is_empty() {
            let (rect, response) = ui.allocate_exact_size(Vec2::new(30.0, 24.0), Sense::click());
            if response.hovered() {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(12), theme::BG_HOVER);
            }
            crate::icons::paint(
                ui.painter(),
                rect.center(),
                14.0,
                crate::icons::Icon::Smile,
                theme::TEXT_FAINT,
            );
            if response.on_hover_text("Добавить реакцию").clicked() {
                click = Some(ReactionClick::OpenPicker);
            }
        }
    });
    click
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
