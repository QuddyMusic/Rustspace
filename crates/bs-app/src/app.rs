//! Корень GUI-приложения: роутинг между экранами и хранение состояния.

use std::time::Duration;

use tokio::runtime::Handle;

use crate::bridge::ChannelKind;
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::images::ImageHub;
use crate::logic::{connection_label, next_screen, should_wake};
use crate::screens::chat::{ChatAction, ChatScreen, SelectedChannel};
use crate::screens::login::LoginScreen;
use crate::theme;
use bs_instances::ConnectionState;

/// Как часто будить UI, пока идёт логин или есть живое соединение.
const WAKE_INTERVAL: Duration = Duration::from_millis(200);

/// Какой экран сейчас показан.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    /// Форма входа.
    Login,
    /// Основной интерфейс.
    Chat,
}

/// Состояние приложения целиком.
pub(crate) struct BackspaceApp {
    pub(crate) screen: Screen,
    pub(crate) bridge: Bridge,
    pub(crate) login: LoginScreen,
    pub(crate) chat: ChatScreen,
    /// Общий кэш картинок.
    images: ImageHub,
    /// Трей и глобальные клавиши.
    desktop: crate::desktop::Desktop,
}

impl BackspaceApp {
    /// Создаёт приложение; `runtime` — handle фонового tokio-runtime.
    ///
    /// Загружает сохранённые настройки и сразу применяет оформление и масштаб.
    pub(crate) fn new(ctx: &egui::Context, runtime: Handle) -> Self {
        let prefs = crate::prefs::load(crate::prefs::default_path());
        crate::i18n::set_language(&prefs.language);
        crate::theme::apply(ctx, &prefs.appearance);
        ctx.set_zoom_factor(prefs.scale);
        let mut bridge = Bridge::new(runtime);
        let mut login = LoginScreen::default();
        // Сохранённая сессия: входим по токену, без пароля.
        let path = crate::saved_session::default_path();
        if let Some(base) = bridge.restore(path.clone()) {
            login.base = base;
        } else if let Some((base, insecure)) = path.as_deref().and_then(crate::saved_session::hint)
        {
            login.base = base;
            login.allow_insecure = insecure;
        } else if crate::prefs::get().recent_instances.is_empty() {
            // Ни одного входа и ни одного инстанса: свой по умолчанию.
            crate::brand::DEFAULT_INSTANCE.clone_into(&mut login.base);
        }
        Self {
            screen: Screen::Login,
            bridge,
            login,
            chat: ChatScreen::with_prefs(),
            images: ImageHub::default(),
            desktop: crate::desktop::Desktop::start(ctx),
        }
    }
}

impl BackspaceApp {
    /// Сообщает, чем закончились скачивания файлов на диск.
    fn finish_downloads(&mut self) {
        let done = self.images.take_download_done();
        if done.is_empty() {
            return;
        }
        let failed: Vec<_> = done
            .iter()
            .filter_map(|(label, result)| {
                result.as_ref().err().map(|err| format!("{label}: {err}"))
            })
            .collect();
        let saved = done.len() - failed.len();
        let text = match (saved, failed.first()) {
            (0, Some(first)) => crate::tf!("Не удалось скачать {first}", first = first),
            (1, None) => crate::tf!("Файл сохранён: {a0}", a0 = done[0].0),
            (n, None) => crate::tf!("Сохранено файлов: {n}", n = n),
            (n, Some(first)) => crate::tf!(
                "Сохранено файлов: {n}; не удалось: {first}",
                n = n,
                first = first
            ),
        };
        self.bridge.set_info(text);
    }

    /// Завершает скачивания: картинка в буфер обмена или стикер в коллекцию.
    fn finish_raw(&mut self, ctx: &egui::Context) {
        for (purpose, result) in self.images.take_raw_done() {
            let bytes = match result {
                Ok(bytes) => bytes,
                Err(err) => {
                    self.bridge
                        .set_info(crate::tf!("Не удалось скачать: {err}", err = err));
                    continue;
                }
            };
            match purpose {
                crate::images::RawPurpose::Copy => {
                    let image = bs_media::decode(&bytes, &bs_media::Limits::default())
                        .ok()
                        .and_then(|d| {
                            let frame = d.frames.into_iter().next()?;
                            Some(egui::ColorImage::from_rgba_unmultiplied(
                                [d.width as usize, d.height as usize],
                                &frame.rgba,
                            ))
                        });
                    if let Some(image) = image {
                        ctx.copy_image(image);
                        self.bridge.set_info(tr("Картинка скопирована").to_owned());
                    } else {
                        self.bridge
                            .set_info(tr("Не удалось прочитать картинку").to_owned());
                    }
                }
                crate::images::RawPurpose::Sticker(name) => {
                    self.bridge
                        .request(crate::bridge::BridgeRequest::UploadSticker {
                            name,
                            bytes: bytes.into(),
                            pack: None,
                        });
                    self.bridge.set_info(tr("Стикер сохраняется…").to_owned());
                }
            }
        }
    }

    /// Тонкая полоса над интерфейсом: показывается только если есть что
    /// сказать — отказ сервера или нестабильное соединение.
    fn notice_strip(&mut self, ui: &mut egui::Ui) {
        let notice = self.bridge.notice().map(str::to_owned);
        let unstable = self.bridge.cache().is_some()
            && *self.bridge.connection() != ConnectionState::Connected;
        if notice.is_none() && !unstable {
            return;
        }
        let mut dismiss = false;
        egui::Panel::top("notice")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(if notice.is_some() {
                        theme::rose()
                    } else {
                        theme::amber()
                    })
                    .inner_margin(egui::Margin::symmetric(16, 6)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let text = notice.unwrap_or_else(|| {
                        crate::tf!(
                            "Соединение: {a0}",
                            a0 = connection_label(self.bridge.connection())
                        )
                    });
                    ui.label(
                        egui::RichText::new(text)
                            .color(egui::Color32::BLACK)
                            .family(theme::bold_family()),
                    );
                    if self.bridge.notice().is_some() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(tr("Скрыть")).clicked() {
                                dismiss = true;
                            }
                        });
                    }
                });
            });
        if dismiss {
            self.bridge.dismiss_notice();
        }
    }
}

/// Кнопки обновления изнутри; `true` — идёт установка, остальное скрыть.
fn update_buttons(
    ui: &mut egui::Ui,
    release: &crate::updates::Release,
    progress: &crate::self_update::Progress,
    runtime: &tokio::runtime::Handle,
) -> bool {
    use crate::self_update::{Method, Progress};
    let note = |ui: &mut egui::Ui, text: String| {
        ui.label(egui::RichText::new(text).color(theme::on_accent()));
    };
    match progress {
        Progress::Downloading(got, total) => {
            #[allow(clippy::cast_precision_loss, reason = "проценты")]
            let text = total.filter(|t| *t > 0).map_or_else(
                || crate::tf!("Скачиваем… {mb} МБ", mb = got / 1_000_000),
                |total| {
                    crate::tf!(
                        "Скачиваем… {percent}%",
                        percent = (*got as f64 * 100.0 / total as f64).round()
                    )
                },
            );
            note(ui, text);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
            true
        }
        Progress::Installing => {
            note(ui, tr("Устанавливаем…").to_owned());
            true
        }
        Progress::Ready(_) => {
            if ui.small_button(tr("Перезапустить")).clicked() {
                crate::self_update::restart();
            }
            note(ui, tr("Обновление установлено").to_owned());
            true
        }
        Progress::Idle | Progress::Failed(_) => {
            if let Progress::Failed(err) = progress {
                note(ui, crate::tf!("Не вышло: {err}", err = err));
            }
            if !matches!(crate::self_update::method(), Method::Manual(_))
                && ui.small_button(tr("Обновить")).clicked()
            {
                crate::self_update::start(
                    runtime,
                    ui.ctx(),
                    release.version.clone(),
                    &release.assets,
                );
            }
            false
        }
    }
}

/// Полоса «Доступна новая версия клиента».
fn update_strip(
    ui: &mut egui::Ui,
    prefs: &crate::updates::UpdatePrefs,
    runtime: &tokio::runtime::Handle,
) {
    let Some(release) = crate::updates::offer(prefs) else {
        return;
    };
    let progress = crate::self_update::progress();
    egui::Panel::top("client_update")
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme::accent())
                .inner_margin(egui::Margin::symmetric(16, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(crate::tf!(
                        "Доступна новая версия Rustspace: {version} (у вас {current})",
                        version = release.version,
                        current = crate::updates::CURRENT
                    ))
                    .color(theme::on_accent())
                    .family(theme::bold_family()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if update_buttons(ui, &release, &progress, runtime) {
                        return;
                    }
                    if ui.small_button(tr("Пропустить версию")).clicked() {
                        let version = release.version.clone();
                        crate::prefs::update(|prefs| prefs.updates.skipped = Some(version));
                    }
                    if ui.small_button(tr("Что нового")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&release.page));
                    }
                    if let Some(download) = &release.download
                        && ui.small_button(tr("Скачать")).clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(download));
                    }
                });
            });
        });
}

impl eframe::App for BackspaceApp {
    fn on_exit(&mut self) {
        crate::prefs::flush(true);
        // Локальный переводчик живёт отдельным процессом: гасим его.
        crate::local_llm::stop();
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.desktop.logic(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.frame_ui(ui, frame);
        drop_unsafe_links(ui.ctx());
    }
}

/// Ссылка, которую можно отдать системе: только `http(s)://` с именем узла.
///
/// Адреса приходят и с сервера (исходный код инстанса, страница релиза,
/// вложения), а система открывает любую схему — `file:`, `smb:`, обработчики
/// приложений. Чужой инстанс не должен уметь запускать их одним щелчком.
#[must_use]
pub(crate) fn safe_link(link: &str) -> bool {
    url::Url::parse(link.trim()).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some_and(|host| !host.is_empty())
            && url.username().is_empty()
            && url.password().is_none()
    })
}

/// Единая застава: выбрасывает из вывода кадра все открытия небезопасных
/// ссылок, откуда бы они ни пришли (`hyperlink`, `open_url`).
fn drop_unsafe_links(ctx: &egui::Context) {
    ctx.output_mut(|output| {
        output.commands.retain(|command| match command {
            egui::OutputCommand::OpenUrl(open) => {
                let ok = safe_link(&open.url);
                if !ok {
                    tracing::warn!("небезопасная ссылка не открыта");
                }
                ok
            }
            _ => true,
        });
    });
}

impl BackspaceApp {
    fn frame_ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let prefs = crate::prefs::get();
        let input = self.desktop.frame(
            ui.ctx(),
            &prefs.keybinds,
            prefs.desktop.global_hotkeys,
            prefs.desktop.close_to_tray,
        );
        self.chat
            .accept_desktop(input, self.desktop.global_actions());
        self.bridge.poll();

        let ready = self.bridge.take_ready();
        let lost = self.bridge.take_session_lost();
        if let Some(message) = self.bridge.take_message() {
            self.bridge.set_info(message);
        }
        self.images.install(ui.ctx());
        self.images.begin_frame();
        if lost {
            self.images.clear();
        }
        self.images.set_links(self.bridge.image_links());
        for (base, path, result) in self.bridge.take_images() {
            self.images.accept(ui.ctx(), &(base, path), result);
        }
        for (tag, result) in self.bridge.take_raw() {
            self.images.accept_raw(tag, result);
        }
        for (id, state) in self.bridge.take_downloads() {
            self.images.accept_download(id, state);
        }
        self.finish_downloads();
        self.finish_raw(ui.ctx());
        if lost {
            // Кэш и каналы мост уже сбросил сам, когда сессия закрылась.
            self.chat.reset();
        }
        let entering = matches!(self.screen, Screen::Login);
        self.screen = next_screen(self.screen, ready, lost);
        // Вошли: инстанс — в недавние на экране входа.
        if entering
            && matches!(self.screen, Screen::Chat)
            && let Some((base, insecure)) = self.bridge.home()
        {
            crate::prefs::update(|prefs| {
                crate::prefs::push_recent(&mut prefs.recent_instances, &base, insecure);
            });
        }

        // Активность (игры, музыка, Rich Presence) — всем инстансам.
        if matches!(self.screen, Screen::Chat) {
            crate::presence::tick(ui.ctx(), &mut self.bridge);
        }

        // Только что открытый личный канал — сразу переключаемся в чат.
        if let Some((origin, channel)) = self.bridge.take_opened_dm() {
            self.chat.open_channel(SelectedChannel {
                origin,
                channel,
                kind: ChannelKind::Dm,
            });
            self.screen = Screen::Chat;
        }

        crate::updates::tick(&self.bridge.runtime(), ui.ctx(), &prefs.updates);
        match self.screen {
            Screen::Login => {
                update_strip(ui, &prefs.updates, &self.bridge.runtime());
                self.login.show(ui, &mut self.bridge);
            }
            Screen::Chat => {
                update_strip(ui, &prefs.updates, &self.bridge.runtime());
                self.notice_strip(ui);
                match self.chat.show(ui, &mut self.bridge) {
                    ChatAction::None => {}
                    ChatAction::Logout => self.bridge.logout(),
                    ChatAction::SwitchInstance => {
                        self.bridge.logout();
                        self.login.switch_instance();
                    }
                }
            }
        }

        crate::prefs::flush(false);

        // Фоновые события приходят из tokio; без этого окно спало бы вечно.
        if should_wake(self.bridge.is_busy(), self.bridge.cache().is_some()) {
            ui.ctx().request_repaint_after(WAKE_INTERVAL);
        }
    }
}

#[cfg(test)]
mod link_tests {
    use super::safe_link;

    #[test]
    fn only_plain_web_links_open() {
        assert!(safe_link("https://github.com/TheZwiss/backspace"));
        assert!(safe_link("http://192.168.1.2:3000/x"));
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "smb://host/share",
            "ms-msdt:/id",
            "https://user:pw@example.org",
            "https://",
            "/etc/passwd",
            "",
        ] {
            assert!(!safe_link(bad), "{bad}");
        }
    }
}
