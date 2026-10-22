//! Хранилище учётных данных по инстансам (origin → токен).

use std::collections::HashMap;

use bs_types::Origin;

use crate::error::IdentityError;
use crate::secret::AccessToken;

/// In-memory хранилище токенов для нескольких инстансов.
///
/// Ключ — [`Origin`]: `Home` для домашнего инстанса, `Remote(url)` для
/// федеративных. Один и тот же пользователь может иметь разные токены
/// на разных инстансах.
#[derive(Debug, Default)]
pub struct CredentialStore {
    tokens: HashMap<Origin, AccessToken>,
}

impl CredentialStore {
    /// Пустое хранилище.
    pub fn new() -> Self {
        Self::default()
    }

    /// Сохраняет токен для origin, возвращая предыдущий (если был).
    pub fn insert(&mut self, origin: Origin, token: AccessToken) -> Option<AccessToken> {
        self.tokens.insert(origin, token)
    }

    /// Возвращает токен для origin.
    pub fn get(&self, origin: &Origin) -> Result<&AccessToken, IdentityError> {
        self.tokens
            .get(origin)
            .ok_or_else(|| IdentityError::NoCredentials {
                origin: origin.as_web_str(),
            })
    }

    /// Есть ли токен для origin.
    pub fn contains(&self, origin: &Origin) -> bool {
        self.tokens.contains_key(origin)
    }

    /// Удаляет токен (например, при logout или 401).
    pub fn remove(&mut self, origin: &Origin) -> Option<AccessToken> {
        self.tokens.remove(origin)
    }

    /// Число сохранённых учётных данных.
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Пусто ли хранилище.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Все origin, для которых есть токен.
    pub fn origins(&self) -> impl Iterator<Item = &Origin> {
        self.tokens.keys()
    }
}
