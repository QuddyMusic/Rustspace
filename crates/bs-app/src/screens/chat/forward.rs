//! «Переслать…»: выбор чата того же инстанса и отправка цитатой с подписью
//! автора (у оригинала пересылки нет — сообщение уходит разметкой, его видно
//! в любом клиенте).

use egui::{Align2, RichText};

use crate::bridge::BridgeRequest;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Что пересылаем.
#[derive(Debug, Clone)]
pub(super) struct ForwardDraft {
    /// Инстанс сообщения: пересылаем в чаты этого же инстанса.
    pub(super) origin: usize,
    pub(super) author_id: String,
    pub(super) author_name: String,
    pub(super) content: String,
    /// Ссылки на вложения.
    pub(super) attachments: Vec<String>,
    /// Поиск по чатам.
    pub(super) filter: String,
}

/// Куда можно переслать: (id, это ЛС, название).
fn targets(bridge: &Bridge, origin: usize) -> Vec<(String, bool, String)> {
    let Some(cache) = bridge.cache_at(origin) else {
        return Vec::new();
    };
    let me = cache.me().id.as_str();
    let mut out: Vec<(String, bool, String)> = cache
        .dm_channels()
        .map(|c| (c.id.clone(), true, crate::social::dm_display_name(c, me)))
        .collect();
    // Переписки — по имени («Избранное» первым): порядок не прыгает.
    let saved = tr("Избранное");
    out.sort_by_key(|(_, _, name)| (name != saved, name.to_lowercase()));
    for space in cache.spaces() {
        for channel in space
            .channels
            .iter()
            .filter(|c| c.channel_type == bs_types::ChannelType::Text)
        {
            out.push((
                channel.id.clone(),
                false,
                format!("#{} · {}", channel.name, space.space.name),
            ));
        }
    }
    out
}

/// Окно выбора чата. `false` — закрыть.
pub(super) fn show(ctx: &egui::Context, draft: &mut ForwardDraft, bridge: &Bridge) -> bool {
    let mut keep = true;
    let mut chosen = None;
    egui::Window::new(tr("Переслать"))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(360.0)
        .show(ctx, |ui| {
            ui.label(theme::muted(crate::tf!(
                "Сообщение {name} уйдёт цитатой с подписью автора.",
                name = draft.author_name
            )));
            ui.add_space(6.0);
            crate::widgets::search_box(ui, &mut draft.filter, tr("Найти чат"));
            ui.add_space(6.0);
            let filter = draft.filter.to_lowercase();
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for (id, dm, name) in
                        targets(bridge, draft.origin)
                            .into_iter()
                            .filter(|(_, _, name)| {
                                filter.is_empty() || name.to_lowercase().contains(&filter)
                            })
                    {
                        let label = RichText::new(&name).color(theme::text());
                        if ui
                            .add(
                                egui::Button::new(label)
                                    .frame(false)
                                    .min_size([330.0, 26.0].into()),
                            )
                            .clicked()
                        {
                            chosen = Some((id, dm));
                        }
                    }
                });
            ui.add_space(6.0);
            if ui.button(tr("Отмена")).clicked() {
                keep = false;
            }
        });
    if let Some((target, dm)) = chosen {
        if let Some(session) = bridge.session_at(draft.origin) {
            session.request(BridgeRequest::Forward {
                target,
                dm,
                author_id: draft.author_id.clone(),
                author_name: draft.author_name.clone(),
                content: draft.content.clone(),
                attachments: draft.attachments.clone(),
            });
        }
        keep = false;
    }
    keep && !ctx.input(|i| i.key_pressed(egui::Key::Escape))
}
