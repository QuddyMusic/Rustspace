//! Выбор аудиоустройств и их горячая замена.

use serde::{Deserialize, Serialize};

/// Направление устройства.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceKind {
    /// Микрофон.
    Input,
    /// Колонки/наушники.
    Output,
}

/// Описание аудиоустройства.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDevice {
    /// Стабильный идентификатор (GUID на десктопе).
    pub id: String,
    /// Человекочитаемое имя.
    pub name: String,
    /// Индекс в системе (может меняться при hot-plug).
    pub index: usize,
    /// Направление.
    pub kind: DeviceKind,
}

/// Выбранные пользователем устройства.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DeviceSelection {
    /// Идентификатор микрофона; `None` — системный по умолчанию.
    pub input: Option<String>,
    /// Идентификатор устройства воспроизведения; `None` — системное по умолчанию.
    pub output: Option<String>,
}

impl DeviceSelection {
    /// Разрешает сохранённый id в текущий индекс устройства.
    ///
    /// Возвращает `None`, если устройство исчезло — тогда вызывающий откатывается
    /// на устройство по умолчанию.
    #[must_use]
    pub fn resolve_index<'a>(
        devices: impl IntoIterator<Item = &'a AudioDevice>,
        kind: DeviceKind,
        wanted: Option<&str>,
    ) -> Option<usize> {
        let wanted = wanted?;
        devices
            .into_iter()
            .find(|d| d.kind == kind && d.id == wanted)
            .map(|d| d.index)
    }

    /// Возвращает id, который стоит запомнить после смены устройства.
    ///
    /// Если устройство пропало, сохранённый id сбрасывается, чтобы не залипать
    /// на несуществующем девайсе.
    #[must_use]
    pub fn reconcile<'a>(
        devices: impl IntoIterator<Item = &'a AudioDevice> + Clone,
        kind: DeviceKind,
        current: Option<&str>,
    ) -> Option<String> {
        let index = Self::resolve_index(devices.clone(), kind, current)?;
        devices
            .into_iter()
            .find(|d| d.kind == kind && d.index == index)
            .map(|d| d.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, index: usize, kind: DeviceKind) -> AudioDevice {
        AudioDevice {
            id: id.to_owned(),
            name: id.to_owned(),
            index,
            kind,
        }
    }

    #[test]
    fn resolves_existing_device() {
        let devices = vec![
            dev("a", 0, DeviceKind::Input),
            dev("b", 1, DeviceKind::Input),
        ];
        assert_eq!(
            DeviceSelection::resolve_index(&devices, DeviceKind::Input, Some("b")),
            Some(1)
        );
    }

    #[test]
    fn resolves_none_for_missing_device() {
        let devices = vec![dev("a", 0, DeviceKind::Input)];
        assert_eq!(
            DeviceSelection::resolve_index(&devices, DeviceKind::Input, Some("zzz")),
            None
        );
        assert_eq!(
            DeviceSelection::resolve_index(&devices, DeviceKind::Input, None),
            None
        );
    }

    #[test]
    fn kind_is_respected() {
        let devices = vec![
            dev("a", 0, DeviceKind::Input),
            dev("a", 5, DeviceKind::Output),
        ];
        assert_eq!(
            DeviceSelection::resolve_index(&devices, DeviceKind::Output, Some("a")),
            Some(5)
        );
    }

    #[test]
    fn reconcile_drops_vanished_device() {
        let devices = vec![dev("a", 0, DeviceKind::Input)];
        assert_eq!(
            DeviceSelection::reconcile(&devices, DeviceKind::Input, Some("gone")),
            None
        );
        assert_eq!(
            DeviceSelection::reconcile(&devices, DeviceKind::Input, Some("a")),
            Some("a".to_owned())
        );
    }
}
