//! Реестр живых подключений: `Origin -> InstanceHandle`.

use std::collections::HashMap;

use bs_types::Origin;

use crate::error::InstanceError;
use crate::handle::{ConnectionState, InstanceHandle};
use crate::multiplex::RegistryStatus;

/// Набор активных подключений к инстансам (домашнему и федеративным).
#[derive(Debug, Default)]
pub struct InstanceRegistry {
    handles: HashMap<Origin, InstanceHandle>,
}

impl InstanceRegistry {
    /// Пустой реестр.
    pub fn new() -> Self {
        Self::default()
    }

    /// Добавляет handle, возвращая прежний (если был).
    ///
    /// Прежний handle нужно корректно остановить через
    /// [`InstanceHandle::shutdown`]; реестр этого не делает.
    pub fn insert(&mut self, handle: InstanceHandle) -> Option<InstanceHandle> {
        self.handles.insert(handle.origin().clone(), handle)
    }

    /// Ссылка на handle по origin.
    pub fn get(&self, origin: &Origin) -> Result<&InstanceHandle, InstanceError> {
        self.handles
            .get(origin)
            .ok_or_else(|| InstanceError::UnknownInstance {
                origin: origin.as_web_str(),
            })
    }

    /// Мутабельная ссылка на handle по origin.
    pub fn get_mut(&mut self, origin: &Origin) -> Result<&mut InstanceHandle, InstanceError> {
        self.handles
            .get_mut(origin)
            .ok_or_else(|| InstanceError::UnknownInstance {
                origin: origin.as_web_str(),
            })
    }

    /// Удаляет handle (не останавливая его).
    pub fn remove(&mut self, origin: &Origin) -> Option<InstanceHandle> {
        self.handles.remove(origin)
    }

    /// Есть ли подключение к origin.
    pub fn contains(&self, origin: &Origin) -> bool {
        self.handles.contains_key(origin)
    }

    /// Число подключений.
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// Пуст ли реестр.
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }

    /// Все origin в реестре.
    pub fn origins(&self) -> impl Iterator<Item = &Origin> {
        self.handles.keys()
    }

    /// Итератор по всем handle.
    pub fn iter(&self) -> impl Iterator<Item = (&Origin, &InstanceHandle)> {
        self.handles.iter()
    }

    /// Текущее состояние подключений по всем инстансам.
    ///
    /// Для UI: переключатель инстансов и индикаторы связи.
    pub fn status(&self) -> RegistryStatus {
        let states: HashMap<Origin, ConnectionState> = self
            .handles
            .iter()
            .map(|(origin, handle)| (origin.clone(), handle.state()))
            .collect();
        RegistryStatus { states }
    }

    /// Останавливает все подключения и очищает реестр.
    ///
    /// Ошибки отдельных shutdown собираются в `Vec`, чтобы одна проблемная
    /// сессия не мешала остановить остальные.
    pub async fn shutdown_all(&mut self) -> Vec<InstanceError> {
        let handles: Vec<InstanceHandle> = self.handles.drain().map(|(_, h)| h).collect();
        let mut errors = Vec::new();
        for handle in handles {
            if let Err(err) = handle.shutdown().await {
                errors.push(err);
            }
        }
        errors
    }
}
