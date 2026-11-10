//! Карточка приглашения в пространство, присланного в личные сообщения
//! (системное событие `space_invite`).

use egui::{CornerRadius, Margin, RichText};

use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Сравнимый ключ адреса: схема, хост, порт (с портом по умолчанию).
/// `localhost`, `127.0.0.1` и `[::1]` — один и тот же хост.
fn origin_key(value: &str) -> Option<(String, String, u16)> {
    let url = url::Url::parse(value.trim()).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let host = match host.as_str() {
        "localhost" | "127.0.0.1" | "[::1]" | "::1" => "loopback".to_owned(),
        _ => host,
    };
    Some((url.scheme().to_owned(), host, url.port_or_known_default()?))
}

/// Инстанс сессии, которая обслуживает `origin` (`https://host[:port]`);
/// пустой `origin` — инстанс, на котором пришло приглашение.
fn slot_for_origin(bridge: &Bridge, home: usize, origin: &str) -> Option<usize> {
    if origin.trim().is_empty() {
        return Some(home);
    }
    let wanted = origin_key(origin)?;
    (0..32).find(|&index| {
        bridge
            .base_at(index)
            .and_then(origin_key)
            .is_some_and(|key| key == wanted)
    })
}

/// Карточка приглашения в пространство. Возвращает (инстанс, код), если
/// нажали «Вступить».
pub(crate) fn show(
    ui: &mut egui::Ui,
    bridge: &Bridge,
    home: usize,
    invite: &bs_types::SpaceInvite,
) -> Option<(usize, String)> {
    let slot = slot_for_origin(bridge, home, &invite.space_instance_origin);
    let joined = slot
        .and_then(|slot| bridge.cache_at(slot))
        .is_some_and(|cache| cache.space(&invite.space_id).is_some());
    let snapshot = &invite.snapshot;
    let mut join = None;
    egui::Frame::new()
        .fill(theme::bg_sidebar())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(CornerRadius::same(theme::radius().saturating_add(2)))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(360.0);
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
                let gradient =
                    theme::space_gradient(&invite.space_id, snapshot.avatar_color.as_deref());
                theme::fill_gradient(ui.painter(), rect, 12.0, gradient, 1.0);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    theme::first_letter(&snapshot.space_name),
                    egui::FontId::new(18.0, theme::bold_family()),
                    egui::Color32::WHITE,
                );
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(&snapshot.space_name)
                            .family(theme::bold_family())
                            .size(15.0)
                            .color(theme::text_bright()),
                    );
                    ui.label(
                        theme::muted(crate::tf!(
                            "{a0} · участников: {a1}",
                            a0 = snapshot.instance_name,
                            a1 = snapshot.member_count
                        ))
                        .size(12.5),
                    );
                });
            });
            if let Some(description) = snapshot
                .description
                .as_deref()
                .filter(|d| !d.trim().is_empty())
            {
                ui.add_space(6.0);
                ui.label(RichText::new(theme::truncate_chars(description, 200)).size(13.0));
            }
            ui.add_space(8.0);
            match (slot, joined) {
                (_, true) => {
                    ui.label(RichText::new(tr("Вы уже в этом пространстве")).color(theme::mint()));
                }
                (Some(slot), false) => {
                    if ui.add(theme::primary_button(tr("Вступить"))).clicked() {
                        join = Some((slot, invite.invite_code.clone()));
                    }
                }
                (None, false) => {
                    ui.label(
                        theme::muted(crate::tf!(
                            "Пространство на {a0}: подключитесь к этому инстансу в «Подключениях», \
                             чтобы вступить.",
                            a0 = invite.space_instance_origin
                        ))
                        .size(12.5),
                    );
                }
            }
        });
    join
}

#[cfg(test)]
mod tests {
    use super::origin_key;

    #[test]
    fn origins_compare_by_scheme_host_and_port() {
        assert_eq!(
            origin_key("https://Chat.Example"),
            origin_key("https://chat.example:443/")
        );
        assert_eq!(
            origin_key("http://localhost:3105"),
            origin_key("http://127.0.0.1:3105")
        );
        assert_ne!(
            origin_key("http://chat.example"),
            origin_key("https://chat.example")
        );
        assert_ne!(
            origin_key("https://a.example"),
            origin_key("https://b.example")
        );
        assert_eq!(origin_key("not a url"), None);
    }
}
