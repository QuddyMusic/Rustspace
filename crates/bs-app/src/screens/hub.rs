//! Страница «Backspace» — проектный хаб оригинала (`ProjectHubPage`):
//! что нового в домашнем инстансе, ссылки проекта, сообщить об ошибке,
//! развернуть свой сервер и сведения об этом сервере.
//!
//! Точка «обновлено» в боковой панели горит, пока пользователь не открыл
//! страницу после смены версии домашнего инстанса; видённая версия хранится
//! по пользователю (`hubSeenVersion` оригинала).

use egui::{Align, Color32, Layout, Margin, RichText, Sense};

use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::icons::Icon;
use crate::theme;

const REPOSITORY: &str = "https://github.com/TheZwiss/backspace";
const INSIGHTS: &str = "https://backspacechat.com/insights/";
const FUNDING: &str = "https://ko-fi.com/backspacechat";

/// Как версия домашнего инстанса относится к видённой (`HubUpdateState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HubUpdate {
    /// Версия неизвестна.
    Unknown,
    /// Страницу ещё не открывали.
    FirstRun,
    /// Видели эту версию.
    Current,
    /// Версия сменилась (обновление или откат).
    Updated,
}

/// Состояние по видённой и текущей версиям.
#[must_use]
pub(crate) fn hub_update(seen: Option<&str>, version: Option<&str>) -> HubUpdate {
    match (seen, version) {
        (_, None) => HubUpdate::Unknown,
        (None, Some(_)) => HubUpdate::FirstRun,
        (Some(seen), Some(version)) if seen == version => HubUpdate::Current,
        _ => HubUpdate::Updated,
    }
}

/// Ключ пользователя для видённой версии: `id@адрес`.
fn seen_key(bridge: &Bridge) -> Option<String> {
    let me = bridge.cache_at(0)?.me().id.clone();
    let base = bridge.base_at(0)?;
    Some(format!("{me}@{base}"))
}

/// Версия домашнего инстанса, если сведения уже пришли.
fn home_version(bridge: &Bridge) -> Option<String> {
    match bridge.home_info()? {
        Ok(info) if !info.version.is_empty() => Some(info.version.clone()),
        _ => None,
    }
}

/// Нужна ли точка «обновлено» у пункта в боковой панели.
pub(crate) fn has_update(bridge: &Bridge) -> bool {
    let version = home_version(bridge);
    let Some(key) = seen_key(bridge) else {
        return false;
    };
    let seen = crate::prefs::get().hub_seen.get(&key).cloned();
    hub_update(seen.as_deref(), version.as_deref()) == HubUpdate::Updated
}

/// Состояние страницы между кадрами.
#[derive(Debug, Default)]
pub(crate) struct HubPage {
    /// Каким было обновление, когда пользователь пришёл: карточка «Что
    /// нового» говорит «Обновлено» весь визит, хотя точка уже погасла.
    arrival: Option<(String, HubUpdate, String)>,
}

impl HubPage {
    /// Запоминает состояние на входе и отмечает версию видённой.
    fn arrive(&mut self, bridge: &Bridge) -> (HubUpdate, Option<String>) {
        let version = home_version(bridge);
        let Some(key) = seen_key(bridge) else {
            return (HubUpdate::Unknown, version);
        };
        if let Some((owner, state, seen_version)) = &self.arrival
            && *owner == key
        {
            return (*state, Some(seen_version.clone()));
        }
        let Some(version) = version else {
            return (HubUpdate::Unknown, None);
        };
        let seen = crate::prefs::get().hub_seen.get(&key).cloned();
        let state = hub_update(seen.as_deref(), Some(&version));
        self.arrival = Some((key.clone(), state, version.clone()));
        if state != HubUpdate::Current {
            let stored = version.clone();
            crate::prefs::update(|prefs| {
                prefs.hub_seen.insert(key, stored);
            });
        }
        (state, Some(version))
    }
}

/// Ссылка на заметки к выпуску версии (или на список выпусков).
fn release_notes(version: Option<&str>) -> String {
    match version {
        Some(v) if semver::Version::parse(v).is_ok() && !v.contains('-') => {
            format!("{REPOSITORY}/releases/tag/v{v}")
        }
        _ => format!("{REPOSITORY}/releases"),
    }
}

/// Ссылка «Сообщить об ошибке» с версией и окружением.
fn bug_report(version: Option<&str>) -> String {
    let environment = format!(
        "Backspace RS {} ({})",
        crate::updates::CURRENT,
        std::env::consts::OS
    );
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("template", "bug_report.yml");
    if let Some(version) = version {
        query.append_pair("version", version);
    }
    query.append_pair("environment", &environment);
    format!("{REPOSITORY}/issues/new?{}", query.finish())
}

/// Карточка страницы: значок, заголовок, текст и ссылки.
struct Card {
    tint: Color32,
    icon: Icon,
    title: String,
    body: String,
    links: Vec<(String, String)>,
}

/// Ширина карточки, к которой подгоняется число колонок.
const CARD_WIDTH: f32 = 280.0;
const CARD_GAP: f32 = 14.0;

/// Карточки сеткой: столько колонок, сколько влезает.
fn card_grid(ui: &mut egui::Ui, cards: &[Card]) {
    let available = ui.available_width();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "число колонок мало и неотрицательно"
    )]
    let columns = (((available + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).floor() as usize).max(1);
    #[allow(clippy::cast_precision_loss, reason = "колонок единицы")]
    let width = CARD_GAP.mul_add(-(columns as f32 - 1.0), available) / columns as f32;
    for row in cards.chunks(columns) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for card in row {
                show_card(ui, card, width);
            }
        });
        ui.add_space(CARD_GAP);
    }
}

fn show_card(ui: &mut egui::Ui, card: &Card, width: f32) {
    egui::Frame::new()
        .fill(theme::bg_sidebar())
        .stroke(egui::Stroke::new(1.0, theme::border()))
        .corner_radius(theme::rounded(12))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width - 34.0);
                ui.set_min_height(150.0);
                let (badge, _) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), Sense::hover());
                ui.painter()
                    .rect_filled(badge, theme::rounded(10), card.tint.gamma_multiply(0.18));
                crate::icons::paint(ui.painter(), badge.center(), 22.0, card.icon, card.tint);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(&card.title)
                        .size(15.5)
                        .family(theme::bold_family())
                        .color(theme::text_bright()),
                );
                if !card.body.is_empty() {
                    ui.label(theme::muted(&card.body).size(13.0));
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for (text, url) in &card.links {
                        if ui.button(format!("{text} ↗")).on_hover_text(url).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        }
                    }
                });
            });
        });
}

/// Рисует страницу.
pub(crate) fn show(ui: &mut egui::Ui, state: &mut HubPage, bridge: &Bridge) {
    let (update, version) = state.arrive(bridge);
    let info = bridge.home_info().and_then(|r| r.as_ref().ok()).cloned();
    egui::Panel::top("hub_header")
        .exact_size(52.0)
        .resizable(false)
        .frame(egui::Frame::new().inner_margin(Margin::symmetric(18, 0)))
        .show(ui, |ui| {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (mark, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), Sense::hover());
                crate::brand::paint(ui.painter(), mark.center(), 16.0, theme::text_faint());
                ui.label(
                    RichText::new("Backspace")
                        .family(theme::bold_family())
                        .color(theme::text_bright()),
                );
            });
        });
    egui::CentralPanel::default()
        .frame(egui::Frame::new().inner_margin(Margin::same(24)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    header(ui);
                    ui.add_space(20.0);
                    card_grid(ui, &cards(update, version.as_deref(), info.as_deref()));
                    ui.add_space(24.0);
                    instance_section(ui, bridge, info.as_deref());
                    ui.add_space(16.0);
                    footer(ui);
                });
        });
}

fn header(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let (tile, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), Sense::hover());
        ui.painter()
            .rect_filled(tile, theme::rounded(16), theme::accent());
        crate::brand::paint(ui.painter(), tile.center(), 24.0, Color32::WHITE);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new("Backspace")
                    .size(24.0)
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
            ui.label(theme::muted(tr(
                "Чат с открытым исходным кодом, который можно развернуть на своём сервере.",
            )));
        });
    });
}

fn cards(
    update: HubUpdate,
    version: Option<&str>,
    info: Option<&bs_api::InstanceInfo>,
) -> Vec<Card> {
    let card = |tint, icon, title: &str, body: &str, links: Vec<(&str, String)>| Card {
        tint,
        icon,
        title: title.to_owned(),
        body: body.to_owned(),
        links: links
            .into_iter()
            .map(|(text, url)| (text.to_owned(), url))
            .collect(),
    };
    let whats_new = match (update, version) {
        (_, None) => tr("Примечания к каждому выпуску").to_owned(),
        (HubUpdate::Updated, Some(v)) => crate::tf!("Обновлено до версии {version}", version = v),
        (_, Some(v)) => crate::tf!("Вы используете версию {version}", version = v),
    };
    let mut out = vec![card(
        theme::accent(),
        Icon::Check,
        tr("Что нового"),
        &whats_new,
        vec![(tr("Читать примечания к выпуску"), release_notes(version))],
    )];
    // Карточка поддержки — только если администратор её не выключил.
    if info.is_some_and(|info| info.support_card_enabled) {
        out.push(card(
            theme::amber(),
            Icon::Smile,
            tr("Поддержать проект"),
            tr(
                "Помогает оплатить лицензии разработчика для публикации Backspace в магазинах \
                приложений и покрыть прочие расходы проекта.",
            ),
            vec![(tr("Поддержать на Ko-fi"), FUNDING.to_owned())],
        ));
    }
    out.push(card(
        theme::sky(),
        Icon::Compass,
        "Insights",
        tr("Открытая статистика загрузок, серверов и активности."),
        vec![(tr("Открыть Insights"), INSIGHTS.to_owned())],
    ));
    out.push(card(
        theme::amber(),
        Icon::Chat,
        tr("Сообщить об ошибке или предложить функцию"),
        tr("Ваша версия и клиент указываются автоматически."),
        vec![
            (tr("Сообщить об ошибке"), bug_report(version)),
            (
                tr("Предложить функцию"),
                format!("{REPOSITORY}/issues/new?template=feature_request.yml"),
            ),
        ],
    ));
    out.push(card(
        theme::rose(),
        Icon::Download,
        tr("Развернуть свой сервер"),
        tr("Запустите Backspace на своём сервере и приглашайте кого хотите."),
        vec![(
            tr("Читать инструкцию по установке"),
            format!("{REPOSITORY}#installation"),
        )],
    ));
    out
}

fn instance_section(ui: &mut egui::Ui, bridge: &Bridge, info: Option<&bs_api::InstanceInfo>) {
    crate::widgets::section_label(ui, tr("Этот сервер"));
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if let Some(info) = info {
            ui.label(
                RichText::new(&info.name)
                    .family(theme::bold_family())
                    .color(theme::text_bright()),
            );
        }
        if let Some(base) = bridge.base_at(0) {
            ui.label(theme::muted(bs_identity::identity::host_of(base)));
        }
        if let Some(info) = info {
            let version = info.commit.as_ref().map_or_else(
                || info.version.clone(),
                |commit| format!("{} ({commit})", info.version),
            );
            match &info.source_code_url {
                Some(url) => {
                    ui.hyperlink_to(crate::tf!("Исходный код {version}", version = version), url);
                }
                None => {
                    ui.label(theme::muted(version));
                }
            }
        }
    });
}

fn footer(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        for (text, url) in [
            (
                tr("Лицензия (AGPL-3.0)"),
                format!("{REPOSITORY}/blob/main/LICENSE"),
            ),
            (
                tr("Сообщить о проблеме безопасности"),
                format!("{REPOSITORY}/blob/main/SECURITY.md"),
            ),
            (
                tr("Участники проекта"),
                format!("{REPOSITORY}/graphs/contributors"),
            ),
            (tr("Исходный код на GitHub"), REPOSITORY.to_owned()),
        ] {
            ui.hyperlink_to(
                RichText::new(text).size(12.0).color(theme::text_faint()),
                url,
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_state_matches_original_table() {
        assert_eq!(hub_update(Some("1.0.0"), None), HubUpdate::Unknown);
        assert_eq!(hub_update(None, Some("1.0.0")), HubUpdate::FirstRun);
        assert_eq!(hub_update(Some("1.0.0"), Some("1.0.0")), HubUpdate::Current);
        assert_eq!(hub_update(Some("1.1.0"), Some("1.0.0")), HubUpdate::Updated);
    }

    #[test]
    fn links_carry_version() {
        assert!(release_notes(Some("1.2.3")).ends_with("/releases/tag/v1.2.3"));
        assert!(release_notes(Some("dev")).ends_with("/releases"));
        let bug = bug_report(Some("1.2.3"));
        assert!(bug.contains("version=1.2.3"));
        assert!(bug.contains("environment=Backspace+RS"));
    }
}
