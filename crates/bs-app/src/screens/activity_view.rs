//! Как выглядит чужая активность: строка под именем в списке участников и
//! карточка в профиле. По мотивам `ActivityCard.tsx` из PR #482
//! оригинального Backspace (QuddyMusic, cirzear), в оформлении Rustspace.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use web_time::Instant;

use bs_types::{Activity, ActivityType};
use egui::{Color32, RichText, Sense, Vec2};

use crate::i18n::tr;
use crate::icons::Icon;
use crate::theme;

/// Значок вида активности.
pub(crate) const fn icon(kind: ActivityType) -> Icon {
    match kind {
        ActivityType::Listening => Icon::Music,
        ActivityType::Watching => Icon::Play,
        ActivityType::Streaming => Icon::Broadcast,
        ActivityType::Playing | ActivityType::Custom => Icon::Gamepad,
    }
}

/// Цвет вида активности.
pub(crate) fn color(kind: ActivityType) -> Color32 {
    match kind {
        ActivityType::Listening => theme::sky(),
        ActivityType::Watching => theme::amber(),
        ActivityType::Streaming => theme::rose(),
        ActivityType::Playing | ActivityType::Custom => theme::mint(),
    }
}

/// Заголовок карточки.
pub(crate) fn headline(kind: ActivityType) -> &'static str {
    match kind {
        ActivityType::Listening => tr("Слушает"),
        ActivityType::Watching => tr("Смотрит"),
        ActivityType::Streaming => tr("Стримит"),
        ActivityType::Playing | ActivityType::Custom => tr("Играет"),
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Длительность: «12 мин», «1 ч 05 мин» — или «3:07» для треков.
pub(crate) fn duration(ms: i64, clock: bool) -> String {
    let total = (ms.max(0) / 1000) as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if clock {
        return if h > 0 {
            format!("{h}:{m:02}:{s:02}")
        } else {
            format!("{m}:{s:02}")
        };
    }
    match (h, m) {
        (0, 0) => tr("меньше минуты").to_owned(),
        (0, m) => crate::tf!("{m} мин", m = m),
        (h, m) => crate::tf!("{h} ч {m} мин", h = h, m = format!("{m:02}")),
    }
}

/// Позиция трека на наших часах: позиция из пуша плюс время с получения.
/// Так часы отправителя, сколь бы ни спешили, не сбивают прогресс.
fn position(user: &str, activity: &Activity) -> Option<(i64, Option<i64>)> {
    type Seen = Mutex<HashMap<(String, String, i64), Instant>>;
    static SEEN: OnceLock<Seen> = OnceLock::new();
    let playback = activity.playback?;
    let key = (user.to_owned(), activity.name.clone(), playback.position_ms);
    let mut seen = SEEN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if seen.len() > 512 {
        seen.clear();
    }
    let at = *seen.entry(key).or_insert_with(Instant::now);
    drop(seen);
    let elapsed = i64::try_from(at.elapsed().as_millis()).unwrap_or(0);
    let now = playback.position_ms + elapsed;
    Some((
        playback.duration_ms.map_or(now, |d| d.min(now)),
        playback.duration_ms,
    ))
}

/// Главная активность: игра или трансляция важнее музыки.
fn primary(list: &[Activity]) -> Option<&Activity> {
    let find = |kind| list.iter().find(|a| a.activity_type == kind);
    find(ActivityType::Streaming)
        .or_else(|| find(ActivityType::Playing))
        .or_else(|| find(ActivityType::Watching))
        .or_else(|| find(ActivityType::Listening))
}

/// Строка под именем участника: значок, текст, цвет.
pub(crate) fn summary(list: &[Activity]) -> Option<(Icon, String, Color32)> {
    let main = primary(list)?;
    let games = list
        .iter()
        .filter(|a| a.activity_type == ActivityType::Playing)
        .count();
    let text = match main.activity_type {
        ActivityType::Listening | ActivityType::Watching => match main.details.as_deref() {
            Some(artist) if !artist.is_empty() => format!("{} · {artist}", main.name),
            _ => main.name.clone(),
        },
        ActivityType::Playing if games > 1 => format!("{} +{}", main.name, games - 1),
        _ => main.name.clone(),
    };
    Some((icon(main.activity_type), text, color(main.activity_type)))
}

/// Квадрат с картинкой активности или значком вида.
fn artwork(ui: &mut egui::Ui, activity: &Activity, side: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    let painter = ui.painter();
    let kind = activity.activity_type;
    let radius = theme::rounded(10);
    let large = activity
        .assets
        .as_ref()
        .and_then(|a| a.large_image.as_deref());
    let drawn = large.is_some_and(|url| {
        crate::images::paint_value(painter, rect, url, radius, true, Color32::WHITE)
    });
    if !drawn {
        painter.rect_filled(rect, radius, color(kind).gamma_multiply(0.18));
        crate::icons::paint(painter, rect.center(), side * 0.5, icon(kind), color(kind));
    }
    let small = activity
        .assets
        .as_ref()
        .and_then(|a| a.small_image.as_deref());
    if let Some(small) = small {
        let badge = egui::Rect::from_center_size(
            rect.right_bottom() - Vec2::splat(4.0),
            Vec2::splat(side * 0.36),
        );
        painter.circle_filled(
            badge.center(),
            badge.width() / 2.0 + 2.0,
            theme::bg_sidebar(),
        );
        crate::images::paint_value(
            painter,
            badge,
            small,
            theme::rounded(255),
            true,
            Color32::WHITE,
        );
    }
}

/// Полоса прогресса трека с временем по краям.
fn progress(ui: &mut egui::Ui, at: i64, total: i64, tint: Color32) {
    #[allow(clippy::cast_precision_loss, reason = "доля для полосы")]
    let fraction = (at as f32 / total.max(1) as f32).clamp(0.0, 1.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 4.0), Sense::hover());
    ui.painter().rect_filled(
        rect,
        theme::rounded(2),
        theme::text_faint().gamma_multiply(0.35),
    );
    let mut done = rect;
    done.set_width(rect.width() * fraction);
    ui.painter().rect_filled(done, theme::rounded(2), tint);
    ui.horizontal(|ui| {
        ui.label(theme::muted(duration(at, true)).size(11.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(theme::muted(duration(total, true)).size(11.0));
        });
    });
}

/// Карточка одной активности.
fn card(ui: &mut egui::Ui, user: &str, activity: &Activity) {
    let kind = activity.activity_type;
    egui::Frame::new()
        .fill(theme::inset())
        .corner_radius(theme::rounded(12))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                crate::icons::paint(
                    ui.painter(),
                    ui.cursor().min + Vec2::new(6.0, 7.0),
                    12.0,
                    icon(kind),
                    color(kind),
                );
                ui.add_space(16.0);
                ui.label(
                    RichText::new(headline(kind).to_uppercase())
                        .size(11.0)
                        .color(color(kind))
                        .strong(),
                );
            });
            ui.add_space(4.0);
            ui.horizontal_top(|ui| {
                artwork(ui, activity, 56.0);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let title = RichText::new(&activity.name)
                        .color(theme::text_bright())
                        .family(theme::bold_family())
                        .size(14.0);
                    match activity.url.as_deref().filter(|u| crate::app::safe_link(u)) {
                        Some(url) => {
                            if ui
                                .add(egui::Label::new(title).sense(Sense::click()))
                                .on_hover_text(tr("Открыть трек"))
                                .clicked()
                            {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                            }
                        }
                        None => {
                            ui.add(egui::Label::new(title).truncate());
                        }
                    }
                    for line in [activity.details.as_deref(), activity.state.as_deref()]
                        .into_iter()
                        .flatten()
                    {
                        ui.add(
                            egui::Label::new(RichText::new(line).color(theme::text()).size(12.5))
                                .truncate(),
                        );
                    }
                    let start = activity.timestamps.as_ref().and_then(|t| t.start);
                    let end = activity.timestamps.as_ref().and_then(|t| t.end);
                    let now = now_ms();
                    if position(user, activity).is_none() {
                        if let Some(end) = end.filter(|end| *end > now) {
                            ui.label(
                                theme::muted(crate::tf!(
                                    "осталось {d}",
                                    d = duration(end - now, false)
                                ))
                                .size(12.0),
                            );
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_secs(1));
                        } else if let Some(start) = start {
                            ui.label(
                                theme::muted(crate::tf!(
                                    "уже {d}",
                                    d = duration(now - start, false)
                                ))
                                .size(12.0),
                            );
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_secs(30));
                        }
                    }
                    if let Some(player) = activity
                        .assets
                        .as_ref()
                        .and_then(|a| a.small_text.as_deref())
                        .filter(|_| kind == ActivityType::Listening)
                    {
                        ui.label(theme::muted(player).size(11.5));
                    }
                });
            });
            if let Some((at, Some(total))) = position(user, activity) {
                ui.add_space(6.0);
                progress(ui, at, total, color(kind));
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs(1));
            }
        });
}

/// Раздел «Сейчас» в профиле: карточки всех активностей.
pub(crate) fn section(ui: &mut egui::Ui, user: &str, list: &[Activity]) {
    if list.iter().any(|a| a.activity_type != ActivityType::Custom) {
        ui.add_space(10.0);
        ui.label(theme::section_title(tr("СЕЙЧАС")));
        ui.add_space(2.0);
    }
    cards(ui, user, list);
}

/// Карточки активностей без заголовка.
pub(crate) fn cards(ui: &mut egui::Ui, user: &str, list: &[Activity]) {
    let shown: Vec<&Activity> = list
        .iter()
        .filter(|a| a.activity_type != ActivityType::Custom)
        .collect();
    for activity in shown {
        card(ui, user, activity);
        ui.add_space(6.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity(kind: ActivityType, name: &str) -> Activity {
        Activity {
            activity_type: kind,
            name: name.into(),
            details: None,
            state: None,
            timestamps: None,
            playback: None,
            assets: None,
            url: None,
        }
    }

    #[test]
    fn games_win_over_music_in_the_member_list() {
        let mut song = activity(ActivityType::Listening, "Song");
        song.details = Some("Band".into());
        assert_eq!(
            summary(&[song.clone()]).map(|s| s.1).as_deref(),
            Some("Song · Band")
        );
        let list = [
            song,
            activity(ActivityType::Playing, "Dota 2"),
            activity(ActivityType::Playing, "CS2"),
        ];
        let (icon, text, _) =
            summary(&list).unwrap_or((Icon::Close, String::new(), Color32::BLACK));
        assert_eq!((icon, text.as_str()), (Icon::Gamepad, "Dota 2 +1"));
        assert_eq!(duration(185_000, true), "3:05");
        assert_eq!(duration(3_725_000, true), "1:02:05");
    }
}
