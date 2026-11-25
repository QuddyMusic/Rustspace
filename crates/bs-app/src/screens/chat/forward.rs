//! «Переслать…»: выбор чата того же инстанса и отправка цитатой с подписью
//! автора (у оригинала пересылки нет — сообщение уходит разметкой, его видно
//! в любом клиенте).

use egui::{Align2, RichText};

use crate::bridge::{BridgeRequest, ForwardItem};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Что пересылаем.
#[derive(Debug, Clone)]
pub(super) struct ForwardDraft {
    /// Инстанс сообщений: пересылаем в чаты этого же инстанса.
    pub(super) origin: usize,
    /// Сообщения по порядку ленты.
    pub(super) items: Vec<ForwardItem>,
    /// Поиск по чатам.
    pub(super) filter: String,
}

/// Больше за раз не выбрать (как в запросе оригинала, #395).
pub(super) const MAX_PICKED: usize = 100;

/// Режим выбора сообщений галочками.
#[derive(Debug, Clone)]
pub(super) struct Picking {
    /// Инстанс и канал, в котором выбираем.
    pub(super) origin: usize,
    pub(super) channel: String,
    /// Выбранные: id, время (для порядка) и что пересылать.
    pub(super) picked: Vec<(String, i64, ForwardItem)>,
}

impl Picking {
    /// Выбрано ли сообщение.
    pub(super) fn has(&self, id: &str) -> bool {
        self.picked.iter().any(|(known, ..)| known == id)
    }

    /// Ставит или снимает галочку (не больше [`MAX_PICKED`]).
    pub(super) fn toggle(&mut self, id: &str, at: i64, item: ForwardItem) {
        if let Some(index) = self.picked.iter().position(|(known, ..)| known == id) {
            self.picked.remove(index);
        } else if self.picked.len() < MAX_PICKED {
            self.picked.push((id.to_owned(), at, item));
        }
    }

    /// Черновик пересылки — по порядку ленты.
    pub(super) fn draft(&self) -> ForwardDraft {
        let mut picked = self.picked.clone();
        picked.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        ForwardDraft {
            origin: self.origin,
            items: picked.into_iter().map(|(_, _, item)| item).collect(),
            filter: String::new(),
        }
    }
}

/// Что пересылать из строки ленты: вложения — абсолютными ссылками.
pub(super) fn item_of(row: &super::model::MessageRow, base: &str) -> ForwardItem {
    ForwardItem {
        author_id: row.author_id.clone(),
        author_name: row.author.clone(),
        content: row.body.clone(),
        attachments: row
            .attachments
            .iter()
            .map(|a| {
                if a.filename.starts_with('/') {
                    format!("{base}{}", a.filename)
                } else {
                    format!("{base}/api/uploads/{}", a.filename)
                }
            })
            .collect(),
    }
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
            let note = match draft.items.as_slice() {
                [one] => crate::tf!(
                    "Сообщение {name} уйдёт цитатой с подписью автора.",
                    name = one.author_name
                ),
                many => crate::tf!(
                    "Сообщения ({a0}) уйдут цитатами с подписями авторов.",
                    a0 = many.len()
                ),
            };
            ui.label(theme::muted(note));
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
                items: draft.items.clone(),
            });
        }
        keep = false;
    }
    keep && !ctx.input(|i| i.key_pressed(egui::Key::Escape))
}
