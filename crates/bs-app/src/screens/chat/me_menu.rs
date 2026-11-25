//! Меню своей аватарки в нижней панели: статус, свой статус, профиль —
//! как в Discord, без похода в настройки.

use bs_types::{ChosenUserStatus, UserStatus};
use egui::RichText;

use super::Nav;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Аватар и имя в нижней панели — одна кнопка, открывающая меню.
pub(super) fn button(
    ui: &egui::Ui,
    left: egui::Pos2,
    bridge: &Bridge,
    status: UserStatus,
    nav: &mut Option<Nav>,
) {
    let rect = egui::Rect::from_min_max(
        egui::pos2(left.x - 6.0, ui.max_rect().top() + 4.0),
        egui::pos2(ui.min_rect().right() + 4.0, ui.max_rect().bottom() - 4.0),
    );
    let response = ui
        .interact(rect, ui.scope_id().with("me_menu"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            crate::theme::rounded(8),
            theme::text_bright().gamma_multiply(0.05),
        );
    }
    poll_avatar(ui.ctx(), bridge);
    if let Some(me) = bridge
        .cache_at(0)
        .map(|c| crate::images::with_absolute_images(c.me().clone(), bridge.base_at(0)))
    {
        menu(&response, bridge, &me, status, nav);
    }
}

fn update(bridge: &Bridge, request: bs_api::UpdateProfileRequest) {
    bridge.request(crate::bridge::BridgeRequest::UpdateProfile(request));
}

/// Ширина меню.
const WIDTH: f32 = 290.0;
/// Высота полосы баннера.
const BANNER: f32 = 64.0;
/// Аватар в меню.
const AVATAR: f32 = 64.0;

fn menu(
    button: &egui::Response,
    bridge: &Bridge,
    me: &bs_types::User,
    status: UserStatus,
    nav: &mut Option<Nav>,
) {
    egui::Popup::from_toggle_button_response(button)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::TOP_START)
        .gap(8.0)
        .frame(
            egui::Frame::new()
                .fill(theme::bg_sidebar())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .corner_radius(theme::rounded(12))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 6],
                    blur: 24,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(90),
                }),
        )
        .show(|ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 2.0;
            header(ui, me, status);
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 12,
                    top: 0,
                    bottom: 12,
                })
                .show(ui, |ui| {
                    ui.set_width(WIDTH - 24.0);
                    section(ui, |ui| statuses(ui, bridge, status));
                    ui.add_space(8.0);
                    section(ui, |ui| {
                        custom_status(ui, bridge, me, button.id.with("custom_status"));
                    });
                    ui.add_space(8.0);
                    section(ui, |ui| {
                        if row(ui, crate::icons::Icon::User, tr("Мой профиль")).clicked()
                        {
                            *nav = Some(Nav::OwnProfile);
                            ui.close();
                        }
                        if row(ui, crate::icons::Icon::Gear, tr("Изменить профиль…")).clicked()
                        {
                            *nav = Some(Nav::Settings);
                            ui.close();
                        }
                    });
                    ui.add_space(8.0);
                    section(ui, |ui| accounts(ui, bridge, nav));
                });
        });
}

/// Баннер, аватар (щелчок — сменить), имя и логин.
fn header(ui: &mut egui::Ui, me: &bs_types::User, status: UserStatus) {
    let top = egui::CornerRadius {
        nw: theme::rounded(12).nw,
        ne: theme::rounded(12).ne,
        sw: 0,
        se: 0,
    };
    let (banner, _) = ui.allocate_exact_size(egui::vec2(WIDTH, BANNER), egui::Sense::hover());
    let color = me
        .avatar_color
        .as_deref()
        .and_then(theme::palette)
        .unwrap_or_else(theme::accent);
    ui.painter()
        .rect_filled(banner, top, color.gamma_multiply(0.55));
    if let Some(value) = me.banner.as_deref() {
        crate::images::paint_value(ui.painter(), banner, value, top, true, egui::Color32::WHITE);
    }
    let avatar = egui::Rect::from_min_size(
        banner.left_bottom() + egui::vec2(14.0, -AVATAR / 2.0),
        egui::Vec2::splat(AVATAR),
    );
    ui.painter()
        .circle_filled(avatar.center(), AVATAR / 2.0 + 4.0, theme::bg_sidebar());
    theme::paint_avatar(
        ui.painter(),
        avatar,
        theme::Face {
            name: me.display_name.as_deref().unwrap_or(&me.username),
            key: Some(me.home_user_id.as_deref().unwrap_or(&me.id)),
            color: me.avatar_color.as_deref(),
        },
        Some(status),
        false,
        me.avatar.as_deref(),
    );
    let managed = me.home_instance.is_some() && me.detached_home_instance.is_none();
    let response = ui.interact(
        avatar,
        ui.scope_id().with("me_avatar"),
        egui::Sense::click(),
    );
    if managed {
        response.on_hover_text(tr("Аватар ведёт ваш домашний инстанс: меняйте его там."));
    } else {
        if response.hovered() {
            ui.painter().circle_filled(
                avatar.center(),
                AVATAR / 2.0,
                egui::Color32::from_black_alpha(140),
            );
            ui.painter().text(
                avatar.center(),
                egui::Align2::CENTER_CENTER,
                tr("Сменить"),
                egui::FontId::new(12.0, theme::bold_family()),
                egui::Color32::WHITE,
            );
        }
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            pick_avatar(ui.ctx());
        }
    }
    ui.add_space(AVATAR / 2.0 + 6.0);
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(14, 0))
        .show(ui, |ui| {
            ui.set_width(WIDTH - 28.0);
            ui.add(
                egui::Label::new(
                    RichText::new(me.display_name.as_deref().unwrap_or(&me.username))
                        .family(theme::bold_family())
                        .size(17.0)
                        .color(theme::text_bright()),
                )
                .truncate(),
            );
            ui.add(
                egui::Label::new(theme::muted(format!("@{}", me.username)).size(12.5)).truncate(),
            );
            if let Some(error) = ui.data(|d| d.get_temp::<String>(error_id())) {
                ui.label(RichText::new(error).color(theme::rose()).size(12.0));
            }
        });
    ui.add_space(10.0);
}

/// Блок меню на чуть более светлом фоне.
fn section(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::bg_hover().gamma_multiply(0.6))
        .corner_radius(theme::rounded(8))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            add(ui);
        });
}

/// Другие сохранённые аккаунты и «Добавить аккаунт».
fn accounts(ui: &mut egui::Ui, bridge: &Bridge, nav: &mut Option<Nav>) {
    let current = bridge.persist_path();
    for (index, account) in crate::accounts::list().iter().enumerate() {
        if crate::accounts::path_of(account) == current {
            continue;
        }
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::click());
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, theme::rounded(6), theme::bg_active());
        }
        let face = egui::Rect::from_center_size(
            rect.left_center() + egui::vec2(18.0, 0.0),
            egui::Vec2::splat(26.0),
        );
        theme::paint_avatar(ui.painter(), face, account.face(), None, false, None);
        let text = rect.left_center() + egui::vec2(38.0, 0.0);
        ui.painter().text(
            text - egui::vec2(0.0, 8.0),
            egui::Align2::LEFT_CENTER,
            &account.name,
            egui::FontId::proportional(13.5),
            theme::text_bright(),
        );
        ui.painter().text(
            text + egui::vec2(0.0, 8.0),
            egui::Align2::LEFT_CENTER,
            account.subtitle(),
            egui::FontId::proportional(11.0),
            theme::text_muted(),
        );
        if response
            .on_hover_text(tr("Переключиться на этот аккаунт"))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            *nav = Some(Nav::SwitchAccount(Some(index)));
            ui.close();
        }
    }
    if row(ui, crate::icons::Icon::Plus, tr("Добавить аккаунт")).clicked() {
        *nav = Some(Nav::SwitchAccount(None));
        ui.close();
    }
}

/// Строка-кнопка с иконкой.
fn row(ui: &mut egui::Ui, icon: crate::icons::Icon, label: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, theme::rounded(6), theme::bg_active());
    }
    crate::icons::paint(
        ui.painter(),
        rect.left_center() + egui::vec2(16.0, 0.0),
        16.0,
        icon,
        theme::text_muted(),
    );
    ui.painter().text(
        rect.left_center() + egui::vec2(34.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.5),
        theme::text(),
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Выбранная картинка для аватара, пока её читают с диска.
type Picked = std::sync::mpsc::Receiver<(String, Result<Vec<u8>, String>)>;

fn picking() -> std::sync::MutexGuard<'static, Option<Picked>> {
    static PICKING: std::sync::Mutex<Option<Picked>> = std::sync::Mutex::new(None);
    PICKING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Где хранится ошибка чтения выбранной картинки.
fn error_id() -> egui::Id {
    egui::Id::unique("me_menu_avatar_error")
}

/// Самая большая картинка профиля, которую принимает сервер.
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;

/// Открывает выбор картинки для аватара (в отдельном потоке).
fn pick_avatar(ctx: &egui::Context) {
    let mut slot = picking();
    if slot.is_some() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(tr("Изображения"), &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_file()
        {
            let name = path
                .file_name()
                .map_or_else(|| "avatar".to_owned(), |n| n.to_string_lossy().into_owned());
            let bytes = match std::fs::metadata(&path) {
                Ok(meta) if meta.len() > MAX_IMAGE_BYTES => {
                    Err(tr("Картинка больше 16 МБ").to_owned())
                }
                Ok(_) => std::fs::read(&path).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send((name, bytes));
        }
        ctx.request_repaint();
    });
    *slot = Some(rx);
}

/// Забирает выбранную картинку и отправляет её как аватар.
fn poll_avatar(ctx: &egui::Context, bridge: &Bridge) {
    let mut slot = picking();
    let Some(rx) = slot.as_ref() else {
        return;
    };
    match rx.try_recv() {
        Ok((name, Ok(bytes))) => {
            *slot = None;
            bridge.request(crate::bridge::BridgeRequest::SetImage {
                kind: crate::bridge::ImageKind::Avatar,
                name,
                bytes: std::sync::Arc::new(bytes),
            });
            ctx.data_mut(|d| d.remove_temp::<String>(error_id()));
        }
        Ok((_, Err(message))) => {
            *slot = None;
            ctx.data_mut(|d| d.insert_temp(error_id(), message));
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => *slot = None,
    }
}

/// Три статуса с цветными точками; текущий подсвечен.
fn statuses(ui: &mut egui::Ui, bridge: &Bridge, status: UserStatus) {
    for (chosen, shown, label) in [
        (ChosenUserStatus::Online, UserStatus::Online, tr("В сети")),
        (ChosenUserStatus::Idle, UserStatus::Idle, tr("Неактивен")),
        (
            ChosenUserStatus::DoNotDisturb,
            UserStatus::DoNotDisturb,
            tr("Не беспокоить"),
        ),
    ] {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
        if response.hovered() || status == shown {
            ui.painter()
                .rect_filled(rect, crate::theme::rounded(6), theme::bg_active());
        }
        if shown == UserStatus::DoNotDisturb {
            ui.painter().text(
                rect.right_center() - egui::vec2(8.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                tr("без уведомлений"),
                egui::FontId::proportional(11.5),
                theme::text_muted(),
            );
        }
        ui.painter().circle_filled(
            rect.left_center() + egui::vec2(16.0, 0.0),
            5.0,
            theme::status_color(shown),
        );
        ui.painter().text(
            rect.left_center() + egui::vec2(34.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.5),
            theme::text(),
        );
        if response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            update(
                bridge,
                bs_api::UpdateProfileRequest {
                    status: Some(chosen),
                    ..Default::default()
                },
            );
            ui.close();
        }
    }
}

/// Свой статус: поле с кнопкой «сохранить» (или Enter) и «убрать».
fn custom_status(ui: &mut egui::Ui, bridge: &Bridge, me: &bs_types::User, id: egui::Id) {
    ui.label(theme::muted(tr("Свой статус")).size(11.5));
    let saved = me.custom_status.clone().unwrap_or_default();
    let mut draft = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| saved.clone());
    let changed = draft.trim() != saved.trim();
    let mut save = false;
    let mut clear = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let buttons = if saved.is_empty() { 40.0 } else { 80.0 };
        let edit = ui.add(
            egui::TextEdit::singleline(&mut draft)
                .hint_text(tr("Чем заняты?"))
                .char_limit(128)
                .desired_width(ui.available_width() - buttons),
        );
        save |= edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && changed;
        let check = ui
            .add_enabled(changed, egui::Button::new(RichText::new("✔").size(13.0)))
            .on_hover_text(tr("Сохранить"));
        save |= check.clicked();
        if !saved.is_empty() {
            clear = ui
                .button(RichText::new("✕").size(13.0))
                .on_hover_text(tr("Убрать"))
                .clicked();
        }
    });
    if clear {
        draft.clear();
        update(
            bridge,
            bs_api::UpdateProfileRequest {
                custom_status: Some(String::new()),
                ..Default::default()
            },
        );
    } else if save {
        update(
            bridge,
            bs_api::UpdateProfileRequest {
                custom_status: Some(draft.trim().to_owned()),
                ..Default::default()
            },
        );
    }
    ui.data_mut(|d| d.insert_temp(id, draft));
}
