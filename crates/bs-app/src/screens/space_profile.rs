//! «Мой профиль в пространстве»: свой аватар, баннер и статус только для
//! этого пространства (запрос оригинала #273). Пустое — берётся из общего
//! профиля. Хранит инстанс пространства, видят участники в клиенте Rustspace.

use std::sync::mpsc::{Receiver, channel};

use egui::{Align2, RichText};

use crate::bridge::ImageChange;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Самая большая картинка.
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;

/// Выбранный файл: куда, имя и байты (или ошибка чтения).
type Picked = (Slot, String, Result<Vec<u8>, String>);

/// Какую картинку выбирают.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Avatar,
    Banner,
}

/// Открытое окно.
#[derive(Debug)]
pub(crate) struct SpaceProfileDialog {
    origin: usize,
    space_id: String,
    space_name: String,
    avatar: ImageChange,
    banner: ImageChange,
    status: String,
    /// Статус ещё не взят из сохранённого профиля.
    status_loaded: bool,
    saving: bool,
    error: Option<String>,
    /// Выбор файла идёт в отдельном потоке.
    picking: Option<Receiver<Picked>>,
}

impl SpaceProfileDialog {
    pub(crate) fn open(origin: usize, space_id: &str, space_name: &str) -> Self {
        Self {
            origin,
            space_id: space_id.to_owned(),
            space_name: space_name.to_owned(),
            avatar: ImageChange::Keep,
            banner: ImageChange::Keep,
            status: String::new(),
            status_loaded: false,
            saving: false,
            error: None,
            picking: None,
        }
    }

    fn pick(&mut self, ctx: &egui::Context, slot: Slot) {
        if self.picking.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        crate::sys::background(move || {
            let chosen = crate::sys::FileDialog::new()
                .add_filter(tr("Изображения"), &["png", "jpg", "jpeg", "gif", "webp"])
                .pick_file();
            if let Some(path) = chosen {
                let name = path
                    .file_name()
                    .map_or_else(|| "image".to_owned(), |n| n.to_string_lossy().into_owned());
                let bytes = match std::fs::metadata(&path) {
                    Ok(meta) if meta.len() > MAX_IMAGE_BYTES => {
                        Err(tr("Картинка больше 16 МБ").to_owned())
                    }
                    Ok(_) => crate::files::read(&path).map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send((slot, name, bytes));
            }
            ctx.request_repaint();
        });
        self.picking = Some(rx);
    }

    /// Забирает файл, выбранный в диалоге.
    fn take_picked(&mut self) {
        let Some(Ok((slot, name, bytes))) = self.picking.as_ref().map(Receiver::try_recv) else {
            return;
        };
        self.picking = None;
        match bytes {
            Ok(bytes) => {
                let change = ImageChange::Upload(name, bytes);
                match slot {
                    Slot::Avatar => self.avatar = change,
                    Slot::Banner => self.banner = change,
                }
            }
            Err(err) => self.error = Some(err),
        }
    }

    /// Окно. `false` — закрыть.
    pub(crate) fn show(&mut self, ctx: &egui::Context, bridge: &mut Bridge) -> bool {
        self.take_picked();
        let Some(session) = bridge.e2ee_session(self.origin) else {
            return false;
        };
        let me = session
            .cache()
            .map(|c| c.me().id.clone())
            .unwrap_or_default();
        let current = session
            .space_profile(&self.space_id, &me)
            .cloned()
            .unwrap_or_default();
        if !self.status_loaded && session.space_profile(&self.space_id, &me).is_some() {
            self.status = current.custom_status.clone().unwrap_or_default();
            self.status_loaded = true;
        }
        if let Some(result) = session.take_space_profile_saved() {
            self.saving = false;
            match result {
                Ok(()) => return false,
                Err(err) => self.error = Some(err),
            }
        }
        let mut open = true;
        let mut save = false;
        let mut reset = false;
        let mut pick = None;
        egui::Window::new(crate::tf!("Мой профиль в «{a0}»", a0 = self.space_name))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label(
                    theme::muted(tr(
                        "Аватар, баннер и статус только для этого пространства. Пустое — как в общем профиле.",
                    ))
                    .size(13.0),
                );
                ui.add_space(8.0);
                pick = image_rows(ui, &mut self.avatar, &mut self.banner, &current);
                ui.add_space(6.0);
                ui.label(RichText::new(tr("Статус")).color(theme::text_bright()));
                ui.add(
                    egui::TextEdit::singleline(&mut self.status)
                        .char_limit(128)
                        .desired_width(f32::INFINITY)
                        .hint_text(current.custom_status.as_deref().unwrap_or_else(|| tr("как в общем профиле"))),
                );
                if let Some(err) = &self.error {
                    ui.label(RichText::new(err).color(theme::amber()).size(12.5));
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    save = ui
                        .add_enabled(!self.saving, theme::primary_button(tr("Сохранить")))
                        .clicked();
                    reset = ui
                        .add_enabled(!self.saving, egui::Button::new(tr("Сбросить всё")))
                        .on_hover_text(tr("Вернуть общий профиль в этом пространстве"))
                        .clicked();
                    if self.saving {
                        ui.spinner();
                    }
                });
            });
        if let Some(slot) = pick {
            self.pick(ctx, slot);
        }
        if save || reset {
            let (avatar, banner, status) = if reset {
                (ImageChange::Clear, ImageChange::Clear, Some(String::new()))
            } else {
                (
                    std::mem::take(&mut self.avatar),
                    std::mem::take(&mut self.banner),
                    Some(self.status.trim().to_owned()),
                )
            };
            self.saving = true;
            self.error = None;
            if let Some(session) = bridge.e2ee_session(self.origin) {
                session.set_space_profile(&self.space_id, avatar, banner, status);
            }
        }
        open && !ctx.input(|i| i.key_pressed(egui::Key::Escape))
    }
}

/// Строки «Аватар» и «Баннер»: что будет и кнопки. Возвращает, для какой
/// картинки выбрать файл.
fn image_rows(
    ui: &mut egui::Ui,
    avatar: &mut ImageChange,
    banner: &mut ImageChange,
    current: &bs_api::SpaceProfile,
) -> Option<Slot> {
    let mut pick = None;
    for (slot, title, change, set) in [
        (Slot::Avatar, tr("Аватар"), avatar, current.avatar.is_some()),
        (Slot::Banner, tr("Баннер"), banner, current.banner.is_some()),
    ] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).color(theme::text_bright()));
            let state = match change {
                ImageChange::Upload(name, _) => crate::tf!("новый: {a0}", a0 = name),
                ImageChange::Clear => tr("будет как в общем профиле").to_owned(),
                ImageChange::Keep if set => tr("свой").to_owned(),
                ImageChange::Keep => tr("как в общем профиле").to_owned(),
            };
            ui.label(theme::muted(state).size(12.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if (set || matches!(change, ImageChange::Upload(..)))
                    && ui.small_button(tr("Сбросить")).clicked()
                {
                    *change = ImageChange::Clear;
                }
                if ui.small_button(tr("Выбрать…")).clicked() {
                    pick = Some(slot);
                }
            });
        });
    }
    pick
}
