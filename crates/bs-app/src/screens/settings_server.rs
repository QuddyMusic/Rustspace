//! Сервер для администратора: нагрузка, диски, резервные копии и перенос
//! файлов пользователей на другой диск.
//!
//! Сведения спрашиваются только пока раздел на экране: закрыл вкладку —
//! опрос прекратился, и ни клиент, ни сервер фоном ничего не считают.

use bs_api::{DiskInfo, HelperJob, ServerState};
use egui::{Align, Layout, RichText, Vec2};

use super::dialogs::card;
use super::settings_extra::{Note, fail, show_note};
use crate::bridge::{AdminEvent, AdminRequest, BridgeRequest};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::media::human_size;
use crate::theme;
use crate::widgets;

/// Как часто обновлять, пока раздел открыт, с.
const POLL_SECS: f64 = 2.0;

/// Куда переносим.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// Каталог резервных копий.
    Backups,
    /// Файлы пользователей.
    Uploads,
}

impl Target {
    const fn job(self) -> &'static str {
        match self {
            Self::Backups => "backup-dir",
            Self::Uploads => "upload-dir",
        }
    }

    const fn folder(self) -> &'static str {
        match self {
            Self::Backups => "rustspace-backups",
            Self::Uploads => "rustspace-uploads",
        }
    }
}

/// Состояние разделов о сервере.
#[derive(Debug, Default)]
pub(crate) struct ServerUi {
    state: Option<ServerState>,
    note: Option<Note>,
    /// Когда спросили последний раз (время egui).
    polled: f64,
    /// Ответ ещё не пришёл.
    waiting: bool,
    /// Резервная копия вместе с файлами.
    with_files: bool,
    /// Ждёт подтверждения удаления: имя копии.
    confirm_delete: Option<String>,
    /// Выбор каталога: для чего и что выбрано.
    picking: Option<(Target, String)>,
    /// Свой путь в поле ввода.
    custom: String,
}

impl ServerUi {
    /// Забирает ответ о сервере.
    pub(crate) fn accept(&mut self, event: AdminEvent) -> Option<AdminEvent> {
        match event {
            AdminEvent::Server(result) => {
                self.waiting = false;
                match result {
                    Ok(state) => {
                        self.state = Some(*state);
                        self.note = None;
                    }
                    // Прежние сведения остаются: сервер мог перезапускаться.
                    Err(message) => self.note = Some(fail(message)),
                }
                None
            }
            other => Some(other),
        }
    }

    /// Спрашивает сервер, если пора; `live` — обновлять по таймеру.
    fn poll(&mut self, ui: &egui::Ui, bridge: &Bridge, live: bool) {
        let now = ui.input(|i| i.time);
        let due =
            self.state.is_none() && self.polled == 0.0 || live && now - self.polled >= POLL_SECS;
        if due && !self.waiting {
            self.polled = now;
            self.waiting = request(bridge, AdminRequest::LoadServer);
        }
        if live {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(POLL_SECS));
        }
    }

    fn busy(&self) -> bool {
        self.state.as_ref().is_some_and(|s| {
            s.backups.pending || s.backups.job.as_ref().is_some_and(|j| j.state == "running")
        })
    }

    fn job(&mut self, bridge: &Bridge, job: &str, arg: &str) {
        self.waiting = request(
            bridge,
            AdminRequest::ServerJob {
                job: job.to_owned(),
                arg: arg.to_owned(),
            },
        );
    }
}

fn request(bridge: &Bridge, request: AdminRequest) -> bool {
    bridge.request(BridgeRequest::Admin(request))
}

fn size(bytes: u64) -> String {
    human_size(i64::try_from(bytes).unwrap_or(i64::MAX))
}

/// Доля для полосы: 0..=1.
#[allow(clippy::cast_precision_loss, reason = "доля нужна лишь для полосы")]
fn fraction(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64).clamp(0.0, 1.0) as f32
    }
}

/// Полоса заполнения: зелёная, жёлтая с 70 %, красная с 90 %.
fn meter(ui: &mut egui::Ui, value: f32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 6.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, theme::rounded(3), theme::bg_hover());
    let color = if value >= 0.9 {
        theme::rose()
    } else if value >= 0.7 {
        theme::amber()
    } else {
        theme::mint()
    };
    let mut fill = rect;
    fill.set_width(rect.width() * value.clamp(0.0, 1.0));
    if fill.width() > 0.5 {
        painter.rect_filled(fill, theme::rounded(3), color);
    }
}

/// Строка «подпись … значение».
fn row(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(theme::text_bright()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(theme::muted(value.into()));
        });
    });
}

/// Сколько работает: «3 д 4 ч», «2 ч 5 мин», «5 мин».
fn uptime_text(secs: u64) -> String {
    let (days, hours, minutes) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    if days > 0 {
        crate::tf!("{d} д {h} ч", d = days, h = hours)
    } else if hours > 0 {
        crate::tf!("{h} ч {m} мин", h = hours, m = minutes)
    } else {
        crate::tf!("{m} мин", m = minutes)
    }
}

/// Тип хранилища по файловой системе.
fn storage_kind(fs: &str) -> String {
    if fs.starts_with("nfs") {
        "NFS".to_owned()
    } else if matches!(fs, "cifs" | "smb3" | "smbfs") {
        "SMB".to_owned()
    } else if fs.contains("davfs") {
        "WebDAV".to_owned()
    } else if let Some(fuse) = fs.strip_prefix("fuse.") {
        fuse.to_owned()
    } else {
        fs.to_owned()
    }
}

fn role_name(role: &str) -> &'static str {
    match role {
        "server" => tr("сервер"),
        "database" => tr("база"),
        "uploads" => tr("файлы пользователей"),
        "backups" => tr("резервные копии"),
        _ => "",
    }
}

/// Ход заявки помощнику понятными словами и цвет.
fn job_text(job: &HelperJob) -> (&'static str, egui::Color32) {
    let text = match job.step.as_str() {
        "backup" => tr("Делаю резервную копию базы…"),
        "files" => tr("Упаковываю файлы пользователей…"),
        "backup-done" => tr("Резервная копия готова."),
        "backup-failed" => tr("Резервная копия не получилась."),
        "deleted" => tr("Копия удалена."),
        "not-found" => tr("Такой копии уже нет."),
        "moving" => tr("Переношу резервные копии…"),
        "stopping" => tr("Останавливаю сервер…"),
        "copying" => tr("Копирую файлы пользователей…"),
        "starting" => tr("Запускаю сервер с новым каталогом…"),
        "moved" if job.job == "upload-dir" => tr("Файлы пользователей перенесены."),
        "moved" => tr("Резервные копии перенесены."),
        "same-path" => tr("Уже там."),
        "bad-path" => tr(
            "Каталог не подходит: нужен существующий несистемный каталог без символических ссылок.",
        ),
        "write-failed" => tr("Не удалось записать в каталог."),
        "move-failed" => tr("Не удалось перенести копии — часть осталась на прежнем месте."),
        "target-exists" => tr("В этом каталоге уже есть rustspace-uploads — выберите другой."),
        "copy-failed" => tr("Копирование не удалось — всё осталось как было."),
        "start-failed" => tr("Сервер не запустился с новым каталогом — всё возвращено как было."),
        _ => tr("Сбой помощника — подробности в журнале сервера."),
    };
    let color = match job.state.as_str() {
        "done" => theme::mint(),
        "failed" => theme::rose(),
        _ => theme::text(),
    };
    (text, color)
}

/// Нагрузка, диски и копии — в разделе «Обновления»; живое, пока на экране.
pub(crate) fn server_section(ui: &mut egui::Ui, srv: &mut ServerUi, bridge: &Bridge) {
    srv.poll(ui, bridge, true);
    ui.add_space(14.0);
    widgets::section_label(ui, tr("Состояние сервера"));
    ui.add_space(6.0);
    show_note(ui, srv.note.as_ref());
    let Some(state) = srv.state.clone() else {
        ui.label(theme::muted(tr("Загрузка…")));
        return;
    };
    health_card(ui, &state);
    ui.add_space(14.0);
    widgets::section_label(ui, tr("Диски"));
    ui.add_space(6.0);
    disks_card(ui, &state.disks);
    ui.add_space(14.0);
    widgets::section_label(ui, tr("Резервные копии"));
    ui.add_space(6.0);
    backups_card(ui, srv, bridge, &state);
}

fn health_card(ui: &mut egui::Ui, state: &ServerState) {
    let health = &state.health;
    card(ui, |ui| {
        let cpu = health.cpu.unwrap_or(0.0);
        row(
            ui,
            tr("Процессор"),
            health.cpu.map_or_else(
                || tr("считаю…").to_owned(),
                |cpu| {
                    crate::tf!(
                        "{p} % · ядер: {n}",
                        p = format!("{cpu:.0}"),
                        n = health.cores
                    )
                },
            ),
        );
        meter(ui, cpu / 100.0);
        ui.add_space(6.0);
        row(
            ui,
            tr("Память"),
            crate::tf!(
                "{a0} из {a1}",
                a0 = size(health.memory_used),
                a1 = size(health.memory_total)
            ),
        );
        meter(ui, fraction(health.memory_used, health.memory_total));
        ui.add_space(6.0);
        if let Some([one, five, fifteen]) = health.load {
            row(
                ui,
                tr("Средняя нагрузка"),
                format!("{one:.2} · {five:.2} · {fifteen:.2}"),
            );
        }
        if let Some(memory) = health.process_memory {
            let cpu = health
                .process_cpu
                .map(|c| crate::tf!(" · {p} % ЦП", p = format!("{c:.0}")))
                .unwrap_or_default();
            row(ui, tr("Процесс сервера"), format!("{}{cpu}", size(memory)));
        }
        if let Some(uptime) = health.uptime {
            row(ui, tr("Сервер работает"), uptime_text(uptime));
        }
        row(
            ui,
            tr("Система работает"),
            uptime_text(health.system_uptime),
        );
        row(ui, tr("В сети"), state.online.to_string());
        let kind = if state.database.kind == "postgres" {
            "PostgreSQL"
        } else {
            "SQLite"
        };
        row(
            ui,
            tr("База"),
            state
                .database
                .size
                .map_or_else(|| kind.to_owned(), |s| format!("{kind} · {}", size(s))),
        );
    });
}

fn disk_row(ui: &mut egui::Ui, disk: &DiskInfo) {
    let used = disk.total.saturating_sub(disk.available);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&disk.mount).color(theme::text_bright()));
        ui.label(theme::muted(storage_kind(&disk.fs)).size(12.0));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(theme::muted(crate::tf!(
                "свободно {a0} из {a1}",
                a0 = size(disk.available),
                a1 = size(disk.total)
            )));
        });
    });
    meter(ui, fraction(used, disk.total));
    let roles: Vec<&str> = disk.roles.iter().map(|r| role_name(r)).collect();
    if !roles.is_empty() {
        ui.label(theme::muted(crate::tf!("Здесь: {list}", list = roles.join(", "))).size(12.0));
    }
}

/// Диски: сначала те, где что-то наше, потом остальные.
fn disks_card(ui: &mut egui::Ui, disks: &[DiskInfo]) {
    card(ui, |ui| {
        let mut sorted: Vec<&DiskInfo> = disks.iter().collect();
        sorted.sort_by_key(|d| d.roles.is_empty());
        for (i, disk) in sorted.into_iter().enumerate() {
            if i > 0 {
                ui.add_space(8.0);
            }
            disk_row(ui, disk);
        }
        if disks.is_empty() {
            ui.label(theme::muted(tr("Сервер не сообщил о дисках.")));
        }
    });
}

/// Ход заявки или отметка, что помощника нет.
/// `kinds` — заявки, ход которых показывать здесь.
fn helper_line(ui: &mut egui::Ui, state: &ServerState, kinds: &[&str]) -> bool {
    let backups = &state.backups;
    if !backups.available {
        ui.label(theme::muted(tr(
            "Помощник на сервере не поставлен: копии и перенос на другой диск недоступны. \
             Поставить — sudo sh setup.sh на сервере.",
        )));
        return false;
    }
    if !backups.supported {
        ui.label(
            RichText::new(tr(
                "Помощник на сервере устарел. Выполните на сервере sudo sh setup.sh — \
             появятся копии по кнопке и перенос на другой диск.",
            ))
            .color(theme::amber()),
        );
        return false;
    }
    if backups.pending {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tr("Заявка принята, ждём помощника…"));
        });
    } else if let Some(job) = backups.job.as_ref().filter(|j| {
        // Итог показывается десять минут, ход — пока идёт.
        let recent = chrono::Utc::now().timestamp_millis() - j.at < 10 * 60_000;
        kinds.contains(&j.job.as_str()) && (j.state == "running" || recent)
    }) {
        let (text, color) = job_text(job);
        ui.horizontal(|ui| {
            if job.state == "running" {
                ui.spinner();
            }
            ui.label(RichText::new(text).color(color));
        });
    }
    true
}

fn backups_card(ui: &mut egui::Ui, srv: &mut ServerUi, bridge: &Bridge, state: &ServerState) {
    card(ui, |ui| {
        if !helper_line(ui, state, &["backup", "delete", "backup-dir"]) {
            return;
        }
        let backups = &state.backups;
        ui.horizontal(|ui| {
            ui.label(RichText::new(&backups.dir).color(theme::text_bright()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(theme::muted(crate::tf!(
                    "{n} шт. · {a0}",
                    n = backups.items.len(),
                    a0 = size(backups.total)
                )));
            });
        });
        ui.add_space(6.0);
        let now = chrono::Utc::now().timestamp_millis();
        for item in &backups.items {
            backup_row(ui, srv, bridge, item, now);
        }
        if backups.items.is_empty() {
            ui.label(theme::muted(tr("Резервных копий пока нет.")));
        }
        ui.add_space(8.0);
        let busy = srv.busy();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new(tr("Сделать копию сейчас")))
                .clicked()
            {
                let arg = if srv.with_files { "files" } else { "" };
                srv.job(bridge, "backup", arg);
            }
            widgets::toggle(ui, &mut srv.with_files);
            ui.label(theme::muted(tr("с файлами пользователей")).size(12.5));
        });
        ui.label(
            theme::muted(tr(
                "Копия базы делается без остановки сервера (SQLite без sqlite3 на сервере — с \
                 остановкой на пару секунд). Перед каждым обновлением помощник делает свою; из \
                 них хранятся пять последних, сделанные вручную — пока вы их не удалите.",
            ))
            .size(12.0),
        );
        ui.add_space(8.0);
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(tr("Хранить копии на другом диске…")),
            )
            .clicked()
        {
            srv.picking = Some((Target::Backups, String::new()));
            srv.custom.clear();
        }
        if matches!(srv.picking, Some((Target::Backups, _))) {
            picker(ui, srv, bridge, state);
        }
    });
}

fn backup_row(
    ui: &mut egui::Ui,
    srv: &mut ServerUi,
    bridge: &Bridge,
    item: &bs_api::BackupItem,
    now: i64,
) {
    let busy = srv.busy();
    ui.horizontal(|ui| {
        let kind = if item.name.ends_with("-manual") {
            tr("вручную")
        } else if item.name.ends_with("-update") {
            tr("перед обновлением")
        } else {
            tr("при установке")
        };
        ui.label(RichText::new(theme::stamp(item.at, now)).color(theme::text()));
        ui.label(theme::muted(kind).size(12.0));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if srv.confirm_delete.as_deref() == Some(&item.name) {
                if ui.small_button(tr("Отмена")).clicked() {
                    srv.confirm_delete = None;
                }
                if ui
                    .small_button(RichText::new(tr("Удалить")).color(theme::rose()))
                    .clicked()
                {
                    srv.confirm_delete = None;
                    srv.job(bridge, "delete", &item.name);
                }
            } else if ui
                .add_enabled(
                    !busy,
                    widgets::flat_icon(crate::icons::Icon::Close, 11.0, theme::text_faint()),
                )
                .on_hover_text(tr("Удалить копию"))
                .clicked()
            {
                srv.confirm_delete = Some(item.name.clone());
            }
            ui.label(theme::muted(size(item.size)));
        });
    })
    .response
    .on_hover_text(&item.name);
}

/// Каталог по умолчанию на выбранном диске: на системном — привычные места.
fn suggested_dir(mount: &str, target: Target) -> String {
    match (mount, target) {
        ("/", Target::Backups) => "/var/backups".to_owned(),
        ("/", Target::Uploads) => "/var/lib/rustspace".to_owned(),
        _ => mount.to_owned(),
    }
}

/// Выбор диска или своего каталога и подтверждение.
fn picker(ui: &mut egui::Ui, srv: &mut ServerUi, bridge: &Bridge, state: &ServerState) {
    let Some((target, chosen)) = srv.picking.clone() else {
        return;
    };
    ui.add_space(8.0);
    egui::Frame::new()
        .fill(theme::bg_hover())
        .corner_radius(theme::rounded(8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            if chosen.is_empty() {
                ui.label(RichText::new(tr("Куда?")).color(theme::text_bright()));
                for disk in &state.disks {
                    ui.horizontal(|ui| {
                        ui.label(&disk.mount);
                        ui.label(theme::muted(storage_kind(&disk.fs)).size(12.0));
                        ui.label(
                            theme::muted(crate::tf!(
                                "свободно {a0}",
                                a0 = size(disk.available)
                            ))
                            .size(12.0),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button(tr("Сюда")).clicked() {
                                srv.picking =
                                    Some((target, suggested_dir(&disk.mount, target)));
                            }
                        });
                    });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut srv.custom)
                            .hint_text(tr("или свой каталог: /mnt/backup"))
                            .desired_width(240.0),
                    );
                    let custom = srv.custom.trim().trim_end_matches('/').to_owned();
                    if ui
                        .add_enabled(custom.starts_with('/'), egui::Button::new(tr("Выбрать")))
                        .clicked()
                    {
                        srv.picking = Some((target, custom));
                    }
                });
                ui.add_space(6.0);
                ui.label(
                    theme::muted(tr(
                        "NFS, SMB и WebDAV подключите на сервере (например, строкой в /etc/fstab) — \
                         диск появится в этом списке:",
                    ))
                    .size(12.0),
                );
                for example in [
                    "nas:/export/rustspace  /mnt/nas  nfs   defaults,_netdev  0 0",
                    "//nas/share  /mnt/smb  cifs  credentials=/root/.smb,_netdev  0 0",
                    "https://dav.example/files  /mnt/dav  davfs  _netdev  0 0",
                ] {
                    ui.label(RichText::new(example).monospace().size(11.5));
                }
                if ui.button(tr("Отмена")).clicked() {
                    srv.picking = None;
                }
            } else {
                let path = format!("{chosen}/{}", target.folder());
                let text = match target {
                    Target::Backups => crate::tf!(
                        "Копии будут в {path}; уже сделанные переедут туда же.",
                        path = path
                    ),
                    Target::Uploads => crate::tf!(
                        "Файлы пользователей переедут в {path}. Сервер остановится на время \
                         копирования; прежние файлы удалятся только после успешного запуска, \
                         при сбое всё вернётся как было.",
                        path = path
                    ),
                };
                ui.label(RichText::new(text).color(theme::amber()));
                ui.horizontal(|ui| {
                    if ui.button(tr("Перенести")).clicked() {
                        srv.picking = None;
                        srv.job(bridge, target.job(), &chosen);
                    }
                    if ui.button(tr("Назад")).clicked() {
                        srv.picking = Some((target, String::new()));
                    }
                });
            }
        });
}

/// «Хранилище»: где лежат файлы пользователей и перенос на другой диск.
pub(crate) fn uploads_card(ui: &mut egui::Ui, srv: &mut ServerUi, bridge: &Bridge) {
    // Живой опрос — только пока идёт перенос.
    let live = srv.busy();
    srv.poll(ui, bridge, live);
    let Some(state) = srv.state.clone() else {
        return;
    };
    ui.add_space(14.0);
    widgets::section_label(ui, tr("Где лежат файлы"));
    ui.add_space(6.0);
    show_note(ui, srv.note.as_ref());
    card(ui, |ui| {
        ui.label(RichText::new(&state.upload_dir).color(theme::text_bright()));
        if let Some(disk) = state
            .disks
            .iter()
            .find(|d| d.roles.iter().any(|r| r == "uploads"))
        {
            disk_row(ui, disk);
        }
        ui.add_space(6.0);
        if !helper_line(ui, &state, &["upload-dir"]) {
            return;
        }
        if ui
            .add_enabled(
                !srv.busy(),
                egui::Button::new(tr("Перенести на другой диск…")),
            )
            .clicked()
        {
            srv.picking = Some((Target::Uploads, String::new()));
            srv.custom.clear();
        }
        if matches!(srv.picking, Some((Target::Uploads, _))) {
            picker(ui, srv, bridge, &state);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_kinds_and_uptime() {
        assert_eq!(storage_kind("nfs4"), "NFS");
        assert_eq!(storage_kind("cifs"), "SMB");
        assert_eq!(storage_kind("fuse.davfs"), "WebDAV");
        assert_eq!(storage_kind("fuse.rclone"), "rclone");
        assert_eq!(storage_kind("ext4"), "ext4");
        assert!((fraction(5, 10) - 0.5).abs() < f32::EPSILON);
        assert!(fraction(1, 0).abs() < f32::EPSILON);
        assert_eq!(suggested_dir("/", Target::Backups), "/var/backups");
        assert_eq!(suggested_dir("/mnt/nas", Target::Uploads), "/mnt/nas");
    }
}
