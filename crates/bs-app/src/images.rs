//! Картинки интерфейса: аватары, иконки, баннеры, вложения и стикеры.
//!
//! [`ImageHub`] — общий кэш текстур. Любой виджет спрашивает у него картинку
//! по значению поля («имя файла», путь, `sticker:<url>`); хаб проверяет путь
//! ([`bs_api::resolve_asset`]), один раз просит мост скачать и декодировать
//! файл и дальше отдаёт готовую текстуру. Пока файла нет — виджет рисует
//! заглушку (инициалы), так что интерфейс никогда не ждёт сеть.
//!
//! Правила, из-за которых хаб не создаёт нагрузку:
//! - один запрос на путь: пока идёт загрузка, повторных нет;
//! - **ошибка не повторяется** до смены сессии (иначе битый файл превращается
//!   в лавину запросов);
//! - общий бюджет видеопамяти ограничен, лишнее вытесняется по давности.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use bs_api::{AssetPath, resolve_asset};
use bs_media::Decoded;
use egui::epaint::RectShape;
use egui::{Color32, Context, CornerRadius, Id, Rect, TextureHandle, TextureOptions, pos2};
use tokio::sync::mpsc::UnboundedSender;

use crate::bridge::BridgeRequest;

/// Сколько байт текстур держим в памяти до вытеснения.
const TEXTURE_BUDGET: usize = 96 * 1024 * 1024;

/// Условный «адрес» для файлов не с инстанса (GIF с CDN).
pub(crate) const EXTERNAL: &str = "external";

/// Готовая картинка: размер и кадры-текстуры.
pub(crate) struct Loaded {
    /// Ширина кадра.
    pub(crate) width: u32,
    /// Высота кадра.
    pub(crate) height: u32,
    frames: Vec<(TextureHandle, u32)>,
    /// Заранее уменьшенная копия для мелких мест.
    thumbs: Vec<TextureHandle>,
    total_ms: u64,
    bytes: usize,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("size", &(self.width, self.height))
            .field("frames", &self.frames.len())
            .finish_non_exhaustive()
    }
}

impl Loaded {
    /// Есть ли анимация.
    pub(crate) fn is_animated(&self) -> bool {
        self.frames.len() > 1
    }

    /// Кадр на момент `time_ms` (по кругу) и сколько мс он ещё показывается.
    pub(crate) fn frame_at(&self, time_ms: u64) -> (&TextureHandle, u32) {
        if self.frames.len() < 2 || self.total_ms == 0 {
            return (&self.frames[0].0, u32::MAX);
        }
        let mut t = time_ms % self.total_ms;
        for (texture, delay) in &self.frames {
            let delay_u64 = u64::from(*delay);
            if t < delay_u64 {
                return (texture, u32::try_from(delay_u64 - t).unwrap_or(u32::MAX));
            }
            t -= delay_u64;
        }
        (&self.frames[0].0, u32::MAX)
    }

    /// Размер кадра для расчётов раскладки. Кадры не больше 2048 пикселей,
    /// так что потери точности нет.
    #[allow(clippy::cast_precision_loss, reason = "стороны ограничены 2048 px")]
    pub(crate) const fn size_f32(&self) -> [f32; 2] {
        [self.width as f32, self.height as f32]
    }

    /// Первый кадр: для статичного показа.
    pub(crate) fn first(&self) -> &TextureHandle {
        &self.frames[0].0
    }
}

#[derive(Debug)]
enum Entry {
    Loading,
    Ready { image: Arc<Loaded>, last_used: u64 },
    Failed(String),
}

/// Состояние картинки для виджета.
#[derive(Debug, Clone)]
pub(crate) enum ImageState {
    /// Нет значения, путь недопустим или сессии нет.
    Absent,
    /// Идёт загрузка.
    Loading,
    /// Не удалось (текст ошибки).
    Failed(String),
    /// Готово.
    Ready(Arc<Loaded>),
}

/// Ключ картинки: инстанс-владелец и путь на нём.
pub(crate) type Key = (String, AssetPath);

#[derive(Debug, Default)]
struct Inner {
    /// Подключённые инстансы: адрес и канал запросов его сессии.
    links: Vec<(String, UnboundedSender<BridgeRequest>)>,
    /// Номер инстанса, относительно которого читаются имена файлов без адреса.
    /// Адрес инстанса, которому принадлежат значения без адреса.
    current: Option<String>,
    entries: HashMap<Key, Entry>,
    bytes: usize,
    tick: u64,
    next_tag: u64,
    raw_pending: HashMap<u64, RawPurpose>,
    raw_done: Vec<(RawPurpose, Result<Vec<u8>, String>)>,
}

/// Общий кэш картинок; дёшево клонируется.
#[derive(Debug, Clone, Default)]
pub(crate) struct ImageHub(Arc<Mutex<Inner>>);

/// Ключ, под которым хаб лежит в памяти контекста `egui`.
fn hub_id() -> Id {
    Id::unique("bs_image_hub")
}

/// Зачем скачивается файл целиком.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RawPurpose {
    /// Положить картинку в буфер обмена.
    Copy,
    /// Добавить в личные стикеры под этим именем.
    Sticker(String),
}

impl ImageHub {
    /// Просит владельца инстанса скачать файл как есть; `false`, если не удалось.
    pub(crate) fn request_raw(&self, value: &str, purpose: RawPurpose) -> bool {
        let Some(key) = self.resolve(value) else {
            return false;
        };
        let mut inner = self.lock();
        inner.next_tag += 1;
        let tag = inner.next_tag;
        let sent = inner
            .links
            .iter()
            .find(|(base, _)| key.0 == EXTERNAL || *base == key.0)
            .is_some_and(|(_, tx)| {
                tx.send(BridgeRequest::Raw {
                    tag,
                    path: key.1.clone(),
                })
                .is_ok()
            });
        if sent {
            inner.raw_pending.insert(tag, purpose);
        }
        sent
    }

    /// Принимает скачанный файл.
    pub(crate) fn accept_raw(&self, tag: u64, result: Result<Vec<u8>, String>) {
        let mut inner = self.lock();
        if let Some(purpose) = inner.raw_pending.remove(&tag) {
            inner.raw_done.push((purpose, result));
        }
    }

    /// Забирает готовые скачивания.
    pub(crate) fn take_raw_done(&self) -> Vec<(RawPurpose, Result<Vec<u8>, String>)> {
        std::mem::take(&mut self.lock().raw_done)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Делает хаб доступным виджетам через контекст.
    pub(crate) fn install(&self, ctx: &Context) {
        ctx.data_mut(|d| d.insert_temp(hub_id(), self.clone()));
    }

    /// Хаб, установленный в контекст.
    pub(crate) fn of(ctx: &Context) -> Option<Self> {
        ctx.data(|d| d.get_temp::<Self>(hub_id()))
    }

    /// Обновляет список живых инстансов (каждый кадр).
    pub(crate) fn set_links(&self, links: Vec<(String, UnboundedSender<BridgeRequest>)>) {
        self.lock().links = links;
    }

    /// Инстанс, которому принадлежат значения без адреса (имена файлов).
    ///
    /// Задаётся адресом, а не номером: в `links` только работающие инстансы,
    /// и номер слота указывал бы не на тот инстанс, как только один из
    /// предыдущих отключится.
    pub(crate) fn set_current(&self, base: Option<&str>) {
        self.lock().current = base.map(str::to_owned);
    }

    /// Забывает всё, что относилось к прошлой сессии.
    pub(crate) fn clear(&self) {
        let mut inner = self.lock();
        *inner = Inner::default();
    }

    /// Начало кадра: двигает счётчик давности.
    pub(crate) fn begin_frame(&self) {
        self.lock().tick += 1;
    }

    /// Приводит значение поля к проверенному пути на одном из подключённых
    /// инстансов. Полный адрес подходит только своему инстансу; имя файла
    /// без адреса читается относительно текущего.
    pub(crate) fn resolve(&self, value: &str) -> Option<Key> {
        // GIF живут на CDN сервиса, а не на инстансе: общий ключ «внешние».
        if let Some(path) = bs_api::AssetPath::external(value) {
            return Some((EXTERNAL.to_owned(), path));
        }
        let inner = self.lock();
        let key = if value.contains("://") {
            inner
                .links
                .iter()
                .find_map(|(base, _)| Some((base.clone(), resolve_asset(base, value)?)))
        } else {
            inner
                .links
                .iter()
                .find(|(base, _)| Some(base) == inner.current.as_ref())
                .and_then(|(base, _)| Some((base.clone(), resolve_asset(base, value)?)))
        };
        drop(inner);
        key
    }

    /// Состояние картинки по значению поля; при необходимости просит загрузку.
    pub(crate) fn state(&self, value: &str) -> ImageState {
        let Some(key) = self.resolve(value) else {
            return ImageState::Absent;
        };
        self.state_for(&key)
    }

    /// Состояние картинки по проверенному ключу.
    pub(crate) fn state_for(&self, key: &Key) -> ImageState {
        let mut inner = self.lock();
        let tick = inner.tick;
        match inner.entries.get_mut(key) {
            Some(Entry::Ready { image, last_used }) => {
                *last_used = tick;
                return ImageState::Ready(Arc::clone(image));
            }
            Some(Entry::Loading) => return ImageState::Loading,
            Some(Entry::Failed(message)) => return ImageState::Failed(message.clone()),
            None => {}
        }
        // Внешний файл качает любая живая сессия (без токена); ссылка
        // домашнего инстанса стоит первой.
        let sent = inner
            .links
            .iter()
            .find(|(base, _)| key.0 == EXTERNAL || *base == key.0)
            .is_some_and(|(_, tx)| tx.send(BridgeRequest::Image(key.1.clone())).is_ok());
        if sent {
            inner.entries.insert(key.clone(), Entry::Loading);
            ImageState::Loading
        } else {
            ImageState::Absent
        }
    }

    /// Принимает результат загрузки от моста и создаёт текстуры.
    pub(crate) fn accept(&self, ctx: &Context, key: &Key, result: Result<Decoded, String>) {
        let tick = self.lock().tick;
        let entry = match result {
            Ok(decoded) => Entry::Ready {
                image: Arc::new(upload(ctx, key, &decoded)),
                last_used: tick,
            },
            Err(message) => Entry::Failed(message),
        };
        let mut inner = self.lock();
        if let Entry::Ready { image, .. } = &entry {
            inner.bytes += image.bytes;
        }
        let replaced = inner.entries.insert(key.clone(), entry);
        if let Some(Entry::Ready { image, .. }) = replaced {
            inner.bytes = inner.bytes.saturating_sub(image.bytes);
        }
        evict(&mut inner);
        drop(inner);
    }
}

/// Делает значение поля полным адресом на инстансе `base`: так оно однозначно
/// указывает владельца, даже когда его читает код другого инстанса.
/// Полные адреса и значения со схемой не трогаются.
#[must_use]
pub(crate) fn absolute(base: &str, value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.contains("://") || trimmed.is_empty() {
        trimmed.to_owned()
    } else if trimmed.starts_with('/') {
        format!("{base}{trimmed}")
    } else {
        format!("{base}/api/uploads/{trimmed}")
    }
}

/// Загружает кадры в видеопамять.
fn upload(ctx: &Context, key: &Key, decoded: &Decoded) -> Loaded {
    let size = [decoded.width as usize, decoded.height as usize];
    let frames: Vec<(TextureHandle, u32)> = decoded
        .frames
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            let image = egui::ColorImage::from_rgba_unmultiplied(size, &frame.rgba);
            let texture = ctx.load_texture(
                format!("{}{}#{index}", key.0, key.1.as_str()),
                image,
                TextureOptions::LINEAR,
            );
            (texture, frame.delay_ms)
        })
        .collect();
    let total_ms = frames.iter().map(|(_, d)| u64::from(*d)).sum();
    let thumbs = decoded
        .thumbs
        .iter()
        .map(|thumb| {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [thumb.width as usize, thumb.height as usize],
                &thumb.rgba,
            );
            ctx.load_texture(
                format!("{}{}#thumb{}", key.0, key.1.as_str(), thumb.width),
                image,
                TextureOptions::LINEAR,
            )
        })
        .collect();
    Loaded {
        thumbs,
        width: decoded.width,
        height: decoded.height,
        frames,
        total_ms,
        bytes: decoded.byte_size(),
    }
}

/// Вытесняет давно не использованные картинки, пока бюджет превышен.
/// Картинки, показанные в текущем кадре, не трогаем.
fn evict(inner: &mut Inner) {
    while inner.bytes > TEXTURE_BUDGET {
        let tick = inner.tick;
        let victim = inner
            .entries
            .iter()
            .filter_map(|(path, entry)| match entry {
                Entry::Ready { last_used, .. } if *last_used < tick => {
                    Some((path.clone(), *last_used))
                }
                _ => None,
            })
            .min_by_key(|(_, last_used)| *last_used)
            .map(|(path, _)| path);
        let Some(victim) = victim else {
            return;
        };
        if let Some(Entry::Ready { image, .. }) = inner.entries.remove(&victim) {
            inner.bytes = inner.bytes.saturating_sub(image.bytes);
        }
    }
}

/// Прямоугольник UV, при котором картинка закрывает `target` без искажений
/// (обрезка по центру, как `object-fit: cover`).
pub(crate) fn cover_uv(image: [f32; 2], target: [f32; 2]) -> Rect {
    let image_ratio = image[0] / image[1].max(1.0);
    let target_ratio = target[0] / target[1].max(1.0);
    if image_ratio > target_ratio {
        let visible = target_ratio / image_ratio;
        let margin = (1.0 - visible) / 2.0;
        Rect::from_min_max(pos2(margin, 0.0), pos2(1.0 - margin, 1.0))
    } else {
        let visible = image_ratio / target_ratio;
        let margin = (1.0 - visible) / 2.0;
        Rect::from_min_max(pos2(0.0, margin), pos2(1.0, 1.0 - margin))
    }
}

/// Рисует картинку в `rect` с обрезкой «cover» и скруглением.
/// Для анимации выбирает кадр по времени и просит перерисовку.
pub(crate) fn paint_cover(
    painter: &egui::Painter,
    rect: Rect,
    image: &Loaded,
    radius: impl Into<CornerRadius>,
    animate: bool,
    tint: Color32,
) {
    // Края по границам физических пикселей: иначе картинка «плывёт» и мылится.
    let ppp = painter.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let rect = Rect::from_min_max(
        pos2(snap(rect.min.x), snap(rect.min.y)),
        pos2(snap(rect.max.x), snap(rect.max.y)),
    );
    let texture = if animate && image.is_animated() {
        let now_ms = painter.ctx().input(|i| (i.time * 1000.0) as u64);
        let (texture, remaining) = image.frame_at(now_ms);
        painter
            .ctx()
            .request_repaint_after(std::time::Duration::from_millis(u64::from(remaining)));
        texture
    } else {
        // Для мелких мест берём заранее уменьшенную копию: без усреднения
        // большая картинка в десятках пикселей выглядит рваной.
        let wanted = rect.width().max(rect.height()) * painter.ctx().pixels_per_point();
        // Копия не меньше нужного размера, ближайшая к нему.
        let side = |t: &TextureHandle| {
            let [w, h] = t.size();
            f32::from(u16::try_from(w.max(h)).unwrap_or(u16::MAX))
        };
        image
            .thumbs
            .iter()
            .find(|t| side(t) >= wanted)
            .unwrap_or_else(|| image.first())
    };
    let uv = cover_uv(image.size_f32(), [rect.width(), rect.height()]);
    painter.add(RectShape::filled(rect, radius, tint).with_texture(texture.id(), uv));
}

/// Рисует картинку по значению поля, если она уже загружена.
/// Возвращает `true`, если нарисовала (иначе вызывающий рисует заглушку).
pub(crate) fn paint_value(
    painter: &egui::Painter,
    rect: Rect,
    value: &str,
    radius: impl Into<CornerRadius>,
    animate: bool,
    tint: Color32,
) -> bool {
    let Some(hub) = ImageHub::of(painter.ctx()) else {
        return false;
    };
    match hub.state(value) {
        ImageState::Ready(image) => {
            paint_cover(painter, rect, &image, radius, animate, tint);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{ImageHub, absolute, cover_uv};

    #[test]
    fn bare_names_belong_to_the_focused_instance_not_to_a_list_position() {
        let hub = ImageHub::default();
        let (a, _ra) = tokio::sync::mpsc::unbounded_channel();
        let (b, _rb) = tokio::sync::mpsc::unbounded_channel();
        // Между домашним и третьим инстансом второй не работает: в списке
        // ссылок третий стоит вторым.
        hub.set_links(vec![
            ("https://home.example".to_owned(), a),
            ("https://third.example".to_owned(), b),
        ]);
        hub.set_current(Some("https://third.example"));
        let key = hub.resolve("pic.png").expect("resolved");
        assert_eq!(key.0, "https://third.example");
        hub.set_current(None);
        assert!(hub.resolve("pic.png").is_none());
        let owned = hub
            .resolve("https://home.example/api/uploads/x.png")
            .expect("abs");
        assert_eq!(owned.0, "https://home.example");
    }

    #[test]
    fn values_become_absolute_for_their_owner() {
        let base = "https://orbit.example";
        assert_eq!(
            absolute(base, "a.png"),
            "https://orbit.example/api/uploads/a.png"
        );
        assert_eq!(
            absolute(base, "/api/stickers/assets/s.webp"),
            "https://orbit.example/api/stickers/assets/s.webp"
        );
        assert_eq!(
            absolute(base, "https://x.example/a.png"),
            "https://x.example/a.png"
        );
        assert_eq!(absolute(base, "  "), "");
    }

    #[test]
    fn wide_image_is_cropped_horizontally() {
        let uv = cover_uv([200.0, 100.0], [100.0, 100.0]);
        assert!((uv.min.x - 0.25).abs() < 1e-6 && (uv.max.x - 0.75).abs() < 1e-6);
        assert!((uv.min.y).abs() < 1e-6 && (uv.max.y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tall_image_is_cropped_vertically() {
        let uv = cover_uv([100.0, 200.0], [100.0, 100.0]);
        assert!((uv.min.y - 0.25).abs() < 1e-6 && (uv.max.y - 0.75).abs() < 1e-6);
    }

    #[test]
    fn same_ratio_uses_whole_image() {
        let uv = cover_uv([50.0, 50.0], [100.0, 100.0]);
        assert!((uv.width() - 1.0).abs() < 1e-6 && (uv.height() - 1.0).abs() < 1e-6);
    }
}
