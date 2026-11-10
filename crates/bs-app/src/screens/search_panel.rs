//! Поиск по сообщениям канала или личного канала.
//!
//! Панель открывается из шапки: строка поиска, фильтры («от кого», «есть»,
//! «до», «после»), результаты с подсветкой совпадения и «Показать ещё».
//! Поиск стартует сам через паузу после ввода; клик по результату — переход
//! к сообщению в ленте.

use egui::{Align2, Color32, FontId, Margin, RichText, Sense, text::LayoutJob};

use crate::bridge::{ChannelKind, SearchHit};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::theme;
use crate::widgets;

/// Пауза после последнего нажатия клавиши перед отправкой запроса, секунд.
const DEBOUNCE: f64 = 0.4;
/// Сколько символов текста показываем в строке результата.
const PREVIEW_CHARS: usize = 150;
/// Примерная высота строки результата.
const ROW_HEIGHT: f32 = 64.0;
/// Предельная высота списка результатов.
const MAX_LIST_HEIGHT: f32 = 380.0;
/// Ширина панели.
const WIDTH: f32 = 400.0;

/// Фильтр «есть».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Has {
    /// Без фильтра.
    #[default]
    Any,
    /// Есть вложение.
    File,
    /// Есть картинка.
    Image,
    /// Есть ссылка.
    Link,
}

impl Has {
    const ALL: [Self; 4] = [Self::Any, Self::File, Self::Image, Self::Link];

    fn label(self) -> &'static str {
        match self {
            Self::Any => tr("Любые"),
            Self::File => tr("Файл"),
            Self::Image => tr("Картинка"),
            Self::Link => tr("Ссылка"),
        }
    }

    const fn query(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::File => Some("file"),
            Self::Image => Some("image"),
            Self::Link => Some("link"),
        }
    }
}

/// Панель поиска и её поля.
#[derive(Debug, Default)]
pub(crate) struct SearchPanel {
    /// Показана ли панель.
    pub(crate) open: bool,
    text: String,
    from: String,
    has: Has,
    before: String,
    after: String,
    /// Отправленный запрос: пока поля не изменились, повторно не шлём.
    sent: Option<String>,
    /// Когда поля менялись в последний раз (время egui).
    edited_at: Option<f64>,
    /// Для какого канала собраны результаты.
    channel: Option<String>,
    /// Текст ошибки в полях (например, неверная дата).
    field_error: Option<String>,
    /// Закрыта из шапки: результаты в мосте ещё надо сбросить.
    stale: bool,
    /// Постоянный id окна: иначе поле ввода теряет фокус каждый кадр.
    area_id: Option<egui::Id>,
}

/// Проверяет дату `ГГГГ-ММ-ДД`; пустая строка допустима.
fn valid_date(raw: &str) -> bool {
    let raw = raw.trim();
    raw.is_empty() || chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").is_ok()
}

/// Склоняет «результат».
pub(crate) fn plural_results(count: usize) -> String {
    let word = match (count % 10, count % 100) {
        (1, n) if n != 11 => tr("результат"),
        (2..=4, n) if !(12..=14).contains(&n) => tr("результата"),
        _ => tr("результатов"),
    };
    format!("{count} {word}")
}

/// Однострочный фрагмент текста вокруг первого совпадения.
///
/// Переводы строк заменяются пробелами; длинный текст обрезается так,
/// чтобы совпадение осталось видно.
pub(crate) fn preview(content: &str, needle: &str, max: usize) -> String {
    let flat: String = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    if chars.len() <= max {
        return flat;
    }
    let lower = flat.to_lowercase();
    let needle = needle.trim().to_lowercase();
    let at = if needle.is_empty() {
        0
    } else {
        lower
            .find(&needle)
            .map_or(0, |byte| lower[..byte].chars().count())
    };
    let start = at.saturating_sub(max / 4).min(chars.len() - max);
    let mut out: String = chars[start..start + max].iter().collect();
    if start > 0 {
        out.insert(0, '…');
    }
    if start + max < chars.len() {
        out.push('…');
    }
    out
}

/// Верстает фрагмент с подсвеченными совпадениями (без учёта регистра).
fn highlighted(text: &str, needle: &str) -> LayoutJob {
    let plain = egui::TextFormat {
        font_id: FontId::proportional(13.5),
        color: theme::text_muted(),
        ..Default::default()
    };
    let hit = egui::TextFormat {
        font_id: FontId::proportional(13.5),
        color: theme::text_bright(),
        background: Color32::from_rgba_unmultiplied(252, 211, 77, 70),
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    let needle = needle.trim();
    if needle.is_empty() {
        job.append(text, 0.0, plain);
        return job;
    }
    let chars: Vec<char> = text.chars().collect();
    let pattern: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let lowered: Vec<char> = chars.iter().flat_map(|c| c.to_lowercase()).collect();
    // Регистр меняет длину редко; если длины разошлись, подсветку не рисуем.
    if lowered.len() != chars.len() {
        job.append(text, 0.0, plain);
        return job;
    }
    let (mut i, mut run_start) = (0, 0);
    while i < chars.len() {
        if lowered[i..].starts_with(&pattern) {
            if run_start < i {
                let part: String = chars[run_start..i].iter().collect();
                job.append(&part, 0.0, plain.clone());
            }
            let end = i + pattern.len();
            let part: String = chars[i..end].iter().collect();
            job.append(&part, 0.0, hit.clone());
            i = end;
            run_start = end;
        } else {
            i += 1;
        }
    }
    if run_start < chars.len() {
        let part: String = chars[run_start..].iter().collect();
        job.append(&part, 0.0, plain);
    }
    job
}

impl SearchPanel {
    /// Переключает панель. Результаты в мосте сбросит [`Self::sync`].
    pub(crate) const fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.stale = true;
        }
    }

    /// Применяет отложенный сброс после закрытия из шапки.
    pub(crate) fn sync(&mut self, bridge: &mut Bridge) {
        if self.stale {
            self.reset(bridge);
        }
    }

    /// Закрывает панель и забывает запрос.
    pub(crate) fn reset(&mut self, bridge: &mut Bridge) {
        self.open = false;
        self.stale = false;
        self.sent = None;
        self.edited_at = None;
        self.channel = None;
        self.field_error = None;
        self.text.clear();
        self.from.clear();
        self.before.clear();
        self.after.clear();
        self.has = Has::Any;
        bridge.clear_search();
    }

    /// Собирает запрос из полей. `Ok(None)` — поля пусты.
    fn query(&self) -> Result<Option<bs_api::MessageSearchQuery>, String> {
        if !valid_date(&self.before) || !valid_date(&self.after) {
            return Err(tr("Дата в виде ГГГГ-ММ-ДД, например 2026-05-31").to_owned());
        }
        let clean = |s: &str| {
            let t = s.trim();
            (!t.is_empty()).then(|| t.to_owned())
        };
        let query = bs_api::MessageSearchQuery {
            text: clean(&self.text),
            from: clean(self.from.trim_start_matches('@')),
            has: self.has.query().map(str::to_owned),
            before: clean(&self.before),
            after: clean(&self.after),
            ..bs_api::MessageSearchQuery::default()
        };
        let empty = query.text.is_none()
            && query.from.is_none()
            && query.has.is_none()
            && query.before.is_none()
            && query.after.is_none();
        Ok((!empty).then_some(query))
    }

    /// Ключ запроса: меняется, когда нужен новый поиск.
    fn key(&self) -> String {
        format!(
            "{}\u{1}{}\u{1}{:?}\u{1}{}\u{1}{}",
            self.text.trim(),
            self.from.trim(),
            self.has,
            self.before.trim(),
            self.after.trim()
        )
    }

    /// Рисует панель. Возвращает id сообщения, по которому кликнули.
    pub(crate) fn show(
        &mut self,
        ctx: &egui::Context,
        bridge: &mut Bridge,
        kind: ChannelKind,
        channel_id: &str,
    ) -> Option<String> {
        if !self.open {
            return None;
        }
        if self.channel.as_deref() != Some(channel_id) {
            // Другой канал: старые результаты к нему не относятся.
            let was_open = self.open;
            self.reset(bridge);
            self.open = was_open;
            self.channel = Some(channel_id.to_owned());
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.reset(bridge);
            return None;
        }

        let mut picked = None;
        let mut close = false;
        let now = ctx.input(|i| i.time);
        let top = 58.0;
        let id = *self
            .area_id
            .get_or_insert_with(|| egui::Id::unique("message_search"));
        egui::Area::new(id)
            .anchor(Align2::RIGHT_TOP, [-16.0, top])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::bg_bubble())
                    .stroke(egui::Stroke::new(1.0, theme::border()))
                    .corner_radius(crate::theme::rounded(10))
                    .inner_margin(Margin::same(14))
                    .shadow(egui::Shadow {
                        offset: [0, 6],
                        blur: 20,
                        spread: 0,
                        color: Color32::from_black_alpha(120),
                    })
                    .show(ui, |ui| {
                        ui.set_width(WIDTH);
                        close = self.fields(ui, now);
                        ui.add_space(8.0);
                        picked = self.results(ui, bridge, kind, channel_id);
                    });
            });

        self.maybe_send(ctx, bridge, kind, channel_id, now);
        if close {
            self.reset(bridge);
        }
        picked
    }

    /// Поля ввода. Возвращает `true`, если нажали «закрыть».
    fn fields(&mut self, ui: &mut egui::Ui, now: f64) -> bool {
        let mut close = false;
        let mut changed = false;
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .hint_text(tr("Поиск по сообщениям"))
                    .desired_width(WIDTH - 40.0)
                    .margin(egui::vec2(10.0, 7.0)),
            );
            if !edit.has_focus() && self.sent.is_none() && self.text.is_empty() {
                edit.request_focus();
            }
            changed |= edit.changed();
            if ui
                .add(widgets::flat_icon(Icon::Close, 16.0, theme::text_faint()))
                .on_hover_text(tr("Закрыть"))
                .clicked()
            {
                close = true;
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(theme::muted(tr("От")).size(12.0));
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.from)
                        .hint_text(tr("имя пользователя"))
                        .desired_width(120.0),
                )
                .changed();
            ui.label(theme::muted(tr("Есть")).size(12.0));
            egui::ComboBox::from_id_salt("search_has")
                .selected_text(self.has.label())
                .width(90.0)
                .show_ui(ui, |ui| {
                    for option in Has::ALL {
                        changed |= ui
                            .selectable_value(&mut self.has, option, option.label())
                            .changed();
                    }
                });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(theme::muted(tr("После")).size(12.0));
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.after)
                        .hint_text(tr("ГГГГ-ММ-ДД"))
                        .desired_width(100.0),
                )
                .changed();
            ui.label(theme::muted(tr("До")).size(12.0));
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.before)
                        .hint_text(tr("ГГГГ-ММ-ДД"))
                        .desired_width(100.0),
                )
                .changed();
        });
        if changed {
            self.edited_at = Some(now);
        }
        close
    }

    /// Отправляет поиск после паузы в наборе.
    fn maybe_send(
        &mut self,
        ctx: &egui::Context,
        bridge: &mut Bridge,
        kind: ChannelKind,
        channel_id: &str,
        now: f64,
    ) {
        let Some(edited) = self.edited_at else {
            return;
        };
        if now - edited < DEBOUNCE {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
            return;
        }
        self.edited_at = None;
        let key = self.key();
        if self.sent.as_deref() == Some(key.as_str()) {
            return;
        }
        match self.query() {
            Err(message) => {
                self.field_error = Some(message);
                bridge.clear_search();
                self.sent = None;
            }
            Ok(None) => {
                self.field_error = None;
                bridge.clear_search();
                self.sent = Some(key);
            }
            Ok(Some(query)) => {
                self.field_error = None;
                bridge.run_search(kind, channel_id, query, false);
                self.sent = Some(key);
            }
        }
    }

    /// Результаты. Возвращает id выбранного сообщения.
    fn results(
        &self,
        ui: &mut egui::Ui,
        bridge: &mut Bridge,
        kind: ChannelKind,
        channel_id: &str,
    ) -> Option<String> {
        if let Some(error) = &self.field_error {
            ui.label(RichText::new(error).color(theme::rose()).size(12.5));
            return None;
        }
        let state = bridge.search_state();
        let searching = self.sent.is_some() || self.edited_at.is_some();
        if let Some(error) = &state.error {
            ui.label(RichText::new(error).color(theme::rose()).size(12.5));
        } else if state.loading && state.hits.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(theme::muted(tr("Ищу…")));
            });
        } else if state.hits.is_empty() {
            let text =
                if searching && self.edited_at.is_none() && self.query().ok().flatten().is_some() {
                    tr("Ничего не найдено")
                } else {
                    tr("Введите текст или задайте фильтры")
                };
            ui.label(theme::muted(text).size(12.5));
        }
        if state.hits.is_empty() {
            return None;
        }
        ui.label(
            theme::muted(plural_results(usize::try_from(state.total).unwrap_or(0))).size(12.0),
        );
        ui.add_space(4.0);
        let needle = self.text.clone();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let hits: Vec<SearchHit> = state.hits.clone();
        let remaining = state.remaining();
        let loading = state.loading;
        let mut picked = None;
        let mut more = false;
        // Примерная высота строки: сам `ScrollArea` внутри `Area` не растёт.
        #[allow(clippy::cast_precision_loss, reason = "строк не больше сотни")]
        let wanted = (hits.len() as f32 * ROW_HEIGHT).min(MAX_LIST_HEIGHT);
        egui::ScrollArea::vertical()
            // Новый запрос — список снова с начала.
            .id_salt(self.sent.clone())
            .min_scrolled_height(wanted)
            .max_height(MAX_LIST_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for hit in &hits {
                    if hit_row(ui, hit, &needle, now_ms) {
                        picked = Some(hit.id.clone());
                    }
                }
                if remaining > 0 {
                    ui.add_space(4.0);
                    let label = if loading {
                        tr("Загрузка…").to_owned()
                    } else {
                        crate::tf!("Показать ещё (осталось {remaining})", remaining = remaining)
                    };
                    if ui.add_enabled(!loading, egui::Button::new(label)).clicked() {
                        more = true;
                    }
                }
            });
        if more && let Ok(Some(query)) = self.query() {
            bridge.run_search(kind, channel_id, query, true);
        }
        picked
    }
}

/// Строка результата. Возвращает `true`, если по ней кликнули.
fn hit_row(ui: &mut egui::Ui, hit: &SearchHit, needle: &str, now_ms: i64) -> bool {
    let inner = egui::Frame::new()
        .inner_margin(Margin::symmetric(8, 6))
        .corner_radius(crate::theme::rounded(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                theme::avatar(ui, &hit.author, 30.0, None, None);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&hit.author)
                                .family(theme::bold_family())
                                .color(theme::text_bright()),
                        );
                        ui.label(theme::muted(theme::stamp(hit.created_at, now_ms)).size(11.5));
                    });
                    let text = preview(&hit.content, needle, PREVIEW_CHARS);
                    if text.is_empty() {
                        ui.label(theme::muted(tr("Вложение")).size(13.0));
                    } else {
                        ui.add(egui::Label::new(highlighted(&text, needle)).wrap());
                    }
                    if hit.attachments > 0 {
                        ui.label(
                            theme::muted(crate::tf!("Вложений: {a0}", a0 = hit.attachments))
                                .size(11.5),
                        );
                    }
                });
            });
        });
    let response = ui
        .interact(
            inner.response.rect,
            ui.auto_id_with(("hit", &hit.id)),
            Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter().rect_filled(
            inner.response.rect,
            6.0,
            Color32::from_rgba_unmultiplied(255, 255, 255, 10),
        );
    }
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_forms_follow_russian_rules() {
        assert_eq!(plural_results(1), "1 результат");
        assert_eq!(plural_results(2), "2 результата");
        assert_eq!(plural_results(5), "5 результатов");
        assert_eq!(plural_results(11), "11 результатов");
        assert_eq!(plural_results(21), "21 результат");
        assert_eq!(plural_results(112), "112 результатов");
    }

    #[test]
    fn dates_are_validated() {
        assert!(valid_date(""));
        assert!(valid_date(" 2026-05-31 "));
        assert!(!valid_date("31.05.2026"));
        assert!(!valid_date("2026-13-01"));
    }

    #[test]
    fn empty_fields_make_no_query_and_bad_dates_fail() {
        let mut panel = SearchPanel::default();
        assert_eq!(panel.query(), Ok(None));
        panel.text = "  привет ".to_owned();
        panel.from = "@bob".to_owned();
        panel.has = Has::Image;
        let query = panel.query().unwrap().unwrap();
        assert_eq!(query.text.as_deref(), Some("привет"));
        assert_eq!(query.from.as_deref(), Some("bob"));
        assert_eq!(query.has.as_deref(), Some("image"));
        panel.after = "вчера".to_owned();
        assert!(panel.query().is_err());
    }

    #[test]
    fn only_filters_are_enough_for_a_query() {
        let panel = SearchPanel {
            has: Has::Link,
            ..SearchPanel::default()
        };
        assert!(panel.query().unwrap().is_some());
    }

    #[test]
    fn preview_flattens_and_keeps_the_match_visible() {
        assert_eq!(preview("a\n\nb   c", "", 50), "a b c");
        let long = format!("{} нужное слово {}", "а".repeat(200), "б".repeat(200));
        let cut = preview(&long, "нужное", 60);
        assert!(cut.contains("нужное"), "{cut}");
        assert!(cut.starts_with('…') && cut.ends_with('…'));
        assert_eq!(cut.chars().count(), 62);
    }

    #[test]
    fn highlight_marks_every_case_insensitive_match() {
        let job = highlighted("Привет, привет!", "ПРИВЕТ");
        let marked: Vec<&str> = job
            .sections
            .iter()
            .filter(|s| s.format.background != Color32::TRANSPARENT)
            .map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0])
            .collect();
        assert_eq!(marked, ["Привет", "привет"]);
    }

    #[test]
    fn key_changes_with_any_field() {
        let mut panel = SearchPanel::default();
        let base = panel.key();
        panel.has = Has::File;
        assert_ne!(panel.key(), base);
    }
}
