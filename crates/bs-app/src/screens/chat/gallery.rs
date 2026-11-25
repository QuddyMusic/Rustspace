//! Галерея вложений беседы или канала (запрос оригинала #458): картинки,
//! видео, аудио, GIF, ссылки и файлы — по вкладкам. Строится поиском сервера
//! «есть файл» и «есть ссылка», без текста, по 50 сообщений; щелчок ведёт к
//! сообщению в ленте.

use egui::{Align2, RichText, Sense};

use crate::bridge::{BridgeRequest, ChannelKind, GalleryEntry};
use crate::fleet::Bridge;
use crate::i18n::tr;
use crate::theme;

/// Вкладка галереи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tab {
    Images,
    Videos,
    Audio,
    Gifs,
    Links,
    Files,
}

impl Tab {
    const ALL: [Self; 6] = [
        Self::Images,
        Self::Videos,
        Self::Audio,
        Self::Gifs,
        Self::Links,
        Self::Files,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Images => tr("Картинки"),
            Self::Videos => tr("Видео"),
            Self::Audio => tr("Аудио"),
            Self::Gifs => tr("GIF"),
            Self::Links => tr("Ссылки"),
            Self::Files => tr("Файлы"),
        }
    }

    /// Плиткой (с картинкой), а не строкой.
    const fn tiles(self) -> bool {
        matches!(self, Self::Images | Self::Gifs | Self::Videos)
    }
}

/// Элемент галереи.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Item {
    pub(super) tab: Tab,
    pub(super) message_id: String,
    /// Имя файла или адрес ссылки.
    pub(super) title: String,
    /// Абсолютный адрес файла или ссылка.
    pub(super) url: String,
    /// Картинка для плитки.
    pub(super) thumb: Option<String>,
    pub(super) size: i64,
    pub(super) author: String,
    pub(super) created_at: i64,
}

/// Ссылки из текста сообщения (без завершающей пунктуации и скобок).
fn urls(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '(' || c == ')')
        .filter(|word| word.starts_with("https://") || word.starts_with("http://"))
        .map(|word| {
            word.trim_end_matches(['.', ',', '!', '?', ';', ':', '"', '\'', ']', '*'])
                .to_owned()
        })
        .filter(|url| url.len() > "https://x".len())
        .collect()
}

/// Ссылка прямо на файл `.gif` (регистр не важен).
fn gif_file(url: &str) -> bool {
    std::path::Path::new(url.split(['?', '#']).next().unwrap_or_default())
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gif"))
}

/// GIF-ссылка: прямой `.gif` или страница сервисов GIF.
fn gif_link(url: &str) -> bool {
    let lower = url.to_lowercase();
    gif_file(url)
        || ["tenor.com/", "giphy.com/", "media.tenor.com", "klipy.com/"]
            .iter()
            .any(|host| lower.contains(host))
}

/// Раскладывает сообщения по вкладкам: из сообщений с файлами берутся
/// вложения, из сообщений со ссылками — ссылки.
pub(super) fn classify(entries: &[GalleryEntry], base: &str, links: bool) -> Vec<Item> {
    let file_url = |name: &str| {
        if name.starts_with("http://") || name.starts_with("https://") {
            name.to_owned()
        } else if name.starts_with('/') {
            format!("{base}{name}")
        } else {
            format!("{base}/api/uploads/{name}")
        }
    };
    let mut out = Vec::new();
    for entry in entries {
        let item = |tab, title: String, url: String, thumb, size| Item {
            tab,
            message_id: entry.message_id.clone(),
            title,
            url,
            thumb,
            size,
            author: entry.author.clone(),
            created_at: entry.created_at,
        };
        if links {
            for url in urls(&entry.content) {
                let gif = gif_link(&url);
                let thumb = gif_file(&url).then(|| url.clone());
                let tab = if gif { Tab::Gifs } else { Tab::Links };
                out.push(item(tab, url.clone(), url, thumb, 0));
            }
            continue;
        }
        for file in &entry.attachments {
            let url = file_url(&file.filename);
            let mime = file.mimetype.to_lowercase();
            let (tab, thumb) = if mime == "image/gif" {
                (Tab::Gifs, Some(url.clone()))
            } else if mime.starts_with("image/") {
                let thumb = file
                    .thumbnail_filename
                    .as_deref()
                    .map_or_else(|| url.clone(), file_url);
                (Tab::Images, Some(thumb))
            } else if mime.starts_with("video/") {
                (
                    Tab::Videos,
                    file.thumbnail_filename.as_deref().map(file_url),
                )
            } else if mime.starts_with("audio/") {
                (Tab::Audio, None)
            } else {
                (Tab::Files, None)
            };
            out.push(item(tab, file.original_name.clone(), url, thumb, file.size));
        }
    }
    out
}

/// Постраничная подгрузка одного источника.
#[derive(Debug, Default)]
struct Pager {
    /// Следующее смещение.
    offset: u32,
    /// Всего сообщений (известно после первой страницы).
    total: Option<i64>,
    loading: bool,
    error: Option<String>,
}

impl Pager {
    fn more(&self) -> bool {
        !self.loading
            && self
                .total
                .is_none_or(|total| i64::from(self.offset) < total)
    }
}

/// Открытая галерея.
#[derive(Debug)]
pub(super) struct Gallery {
    origin: usize,
    channel: String,
    kind: ChannelKind,
    tab: Tab,
    files: Pager,
    links: Pager,
    items: Vec<Item>,
}

/// Что сделать после кадра.
pub(super) enum Action {
    /// Окно открыто.
    Keep,
    /// Закрыть.
    Close,
    /// Перейти к сообщению и закрыть.
    Jump(String),
}

impl Gallery {
    /// Открывает галерею канала и просит первые страницы.
    pub(super) fn open(bridge: &Bridge, origin: usize, channel: &str, kind: ChannelKind) -> Self {
        let mut gallery = Self {
            origin,
            channel: channel.to_owned(),
            kind,
            tab: Tab::Images,
            files: Pager::default(),
            links: Pager::default(),
            items: Vec::new(),
        };
        gallery.request(bridge, false);
        gallery.request(bridge, true);
        gallery
    }

    fn request(&mut self, bridge: &Bridge, links: bool) {
        let pager = if links {
            &mut self.links
        } else {
            &mut self.files
        };
        if !pager.more() {
            return;
        }
        pager.loading = true;
        if let Some(session) = bridge.session_at(self.origin) {
            session.request(BridgeRequest::Gallery {
                channel_id: self.channel.clone(),
                kind: self.kind,
                links,
                offset: pager.offset,
            });
        }
    }

    /// Забирает пришедшие страницы.
    fn poll(&mut self, bridge: &mut Bridge) {
        let base = bridge.base_at(self.origin).unwrap_or_default().to_owned();
        let Some(session) = bridge.e2ee_session(self.origin) else {
            return;
        };
        for page in session.take_gallery_pages() {
            if page.channel_id != self.channel {
                continue;
            }
            let pager = if page.links {
                &mut self.links
            } else {
                &mut self.files
            };
            if page.offset != pager.offset {
                continue;
            }
            pager.loading = false;
            match page.result {
                Ok((entries, total)) => {
                    pager.total = Some(total);
                    pager.offset += u32::try_from(entries.len()).unwrap_or(u32::MAX);
                    if entries.is_empty() {
                        pager.total = Some(i64::from(pager.offset));
                    }
                    for item in classify(&entries, &base, page.links) {
                        if !self.items.iter().any(|known| {
                            known.message_id == item.message_id && known.url == item.url
                        }) {
                            self.items.push(item);
                        }
                    }
                }
                Err(err) => pager.error = Some(err),
            }
        }
    }

    /// Окно галереи.
    pub(super) fn show(&mut self, ctx: &egui::Context, bridge: &mut Bridge) -> Action {
        self.poll(bridge);
        let mut action = Action::Keep;
        let mut open = true;
        let mut load_more = false;
        egui::Window::new(tr("Вложения"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .default_size([660.0, 520.0])
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for tab in Tab::ALL {
                        let count = self.items.iter().filter(|i| i.tab == tab).count();
                        let label = if count == 0 {
                            tab.title().to_owned()
                        } else {
                            format!("{} {count}", tab.title())
                        };
                        if ui.selectable_label(self.tab == tab, label).clicked() {
                            self.tab = tab;
                        }
                    }
                });
                ui.separator();
                let mut items: Vec<&Item> =
                    self.items.iter().filter(|i| i.tab == self.tab).collect();
                items.sort_by_key(|item| std::cmp::Reverse(item.created_at));
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(ui.available_height() - 36.0)
                    .show(ui, |ui| {
                        if items.is_empty() && !self.files.loading && !self.links.loading {
                            theme::empty_state(ui, tr("Здесь пока ничего нет"));
                        } else if self.tab.tiles() {
                            if let Some(id) = tiles(ui, &items, self.tab) {
                                action = Action::Jump(id);
                            }
                        } else if let Some(id) = rows(ui, &items, self.tab) {
                            action = Action::Jump(id);
                        }
                    });
                ui.horizontal(|ui| {
                    if self.files.loading || self.links.loading {
                        ui.spinner();
                    } else if self.files.more() || self.links.more() {
                        load_more = ui.button(tr("Загрузить ещё")).clicked();
                    }
                    if let Some(err) = self.files.error.as_ref().or(self.links.error.as_ref()) {
                        ui.label(RichText::new(err).color(theme::amber()).size(12.5));
                    }
                });
            });
        if load_more {
            self.request(bridge, false);
            self.request(bridge, true);
        }
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return Action::Close;
        }
        action
    }
}

/// Подпись «автор · дата».
fn byline(item: &Item) -> String {
    format!(
        "{} · {}",
        item.author,
        theme::short_stamp(item.created_at, chrono::Utc::now().timestamp_millis())
    )
}

/// Плитки картинок, GIF и видео; щелчок — к сообщению.
fn tiles(ui: &mut egui::Ui, items: &[&Item], tab: Tab) -> Option<String> {
    let mut jump = None;
    let size = 132.0;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        for item in items {
            let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), Sense::click());
            let painter = ui.painter();
            painter.rect_filled(rect, theme::rounded(8), theme::bg_input());
            let drawn = item.thumb.as_deref().is_some_and(|thumb| {
                crate::images::paint_value(
                    painter,
                    rect,
                    thumb,
                    theme::rounded(8),
                    tab == Tab::Gifs,
                    egui::Color32::WHITE,
                )
            });
            if !drawn {
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    if tab == Tab::Videos { "▶" } else { "…" },
                    egui::FontId::proportional(28.0),
                    theme::text_muted(),
                );
            }
            if tab == Tab::Videos {
                let label = rect.shrink(6.0);
                painter.text(
                    label.left_bottom(),
                    Align2::LEFT_BOTTOM,
                    theme::truncate_chars(&item.title, 18),
                    egui::FontId::proportional(11.5),
                    theme::text_bright(),
                );
            }
            if response.hovered() {
                painter.rect_stroke(
                    rect,
                    theme::rounded(8),
                    egui::Stroke::new(2.0, theme::accent()),
                    egui::StrokeKind::Inside,
                );
            }
            let response = response
                .on_hover_text(format!("{}\n{}", item.title, byline(item)))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                jump = Some(item.message_id.clone());
            }
        }
    });
    jump
}

/// Строки аудио, ссылок и файлов. Ссылка открывается в браузере, остальное
/// ведёт к сообщению (там проигрыватель и кнопка скачивания).
fn rows(ui: &mut egui::Ui, items: &[&Item], tab: Tab) -> Option<String> {
    let mut jump = None;
    for item in items {
        ui.horizontal(|ui| {
            let icon = match tab {
                Tab::Audio => "♪",
                Tab::Links => "🔗",
                _ => "📄",
            };
            ui.label(RichText::new(icon).size(16.0));
            ui.vertical(|ui| {
                let title = RichText::new(theme::truncate_chars(&item.title, 70))
                    .color(theme::text_bright());
                let clicked = ui
                    .add(egui::Label::new(title).sense(Sense::click()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(&item.url)
                    .clicked();
                let mut meta = byline(item);
                if item.size > 0 {
                    meta = format!("{} · {meta}", crate::media::human_size(item.size));
                }
                ui.label(theme::muted(meta).size(12.0));
                if clicked {
                    if tab == Tab::Links {
                        let _ = opener::open(&item.url);
                    } else {
                        jump = Some(item.message_id.clone());
                    }
                }
            });
            if tab == Tab::Links
                && ui
                    .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.small_button(tr("К сообщению")).clicked()
                    })
                    .inner
            {
                jump = Some(item.message_id.clone());
            }
        });
        ui.add_space(4.0);
    }
    jump
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(name: &str, mime: &str) -> bs_types::Attachment {
        serde_json::from_value(serde_json::json!({
            "id": name, "messageId": "m", "filename": format!("{name}.bin"),
            "originalName": name, "mimetype": mime, "size": 10, "createdAt": 0
        }))
        .expect("вложение")
    }

    #[test]
    fn entries_land_in_their_tabs() {
        let files = GalleryEntry {
            message_id: "m1".into(),
            author: "Bob".into(),
            created_at: 1,
            content: "https://example.org/x".into(),
            attachments: vec![
                attachment("cat.png", "image/png"),
                attachment("dance.gif", "image/gif"),
                attachment("clip.mp4", "video/mp4"),
                attachment("song.ogg", "audio/ogg"),
                attachment("doc.pdf", "application/pdf"),
            ],
        };
        let tabs: Vec<Tab> = classify(std::slice::from_ref(&files), "https://h", false)
            .iter()
            .map(|i| i.tab)
            .collect();
        assert_eq!(
            tabs,
            vec![Tab::Images, Tab::Gifs, Tab::Videos, Tab::Audio, Tab::Files]
        );
        let images = classify(std::slice::from_ref(&files), "https://h", false);
        assert_eq!(images[0].url, "https://h/api/uploads/cat.png.bin");
        let links = GalleryEntry {
            content: "смотри (https://example.org/page), и https://media.tenor.com/a.gif.".into(),
            attachments: Vec::new(),
            ..files
        };
        let found = classify(&[links], "https://h", true);
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].tab, found[0].url.as_str()),
            (Tab::Links, "https://example.org/page")
        );
        assert_eq!(found[1].tab, Tab::Gifs);
        assert_eq!(
            found[1].thumb.as_deref(),
            Some("https://media.tenor.com/a.gif")
        );
    }
}
