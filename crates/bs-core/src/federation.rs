//! Федеративные хелперы мультиинстансного клиента.
//!
//! Порт вспомогательной логики из `packages/web/src/hooks/useWebSocket.ts`
//! и `docs/client-federation.md`: как определить домашнюю идентичность
//! пира, увиденного на удалённом инстансе, как достать токен удалённого
//! инстанса и нужно ли автоматически переподключаться.
//!
//! # Пример
//! ```
//! use bs_core::{maybe_auto_reattach, ReattachDecision};
//! use bs_instances::InstanceRegistry;
//! use bs_types::Origin;
//!
//! let registry = InstanceRegistry::new();
//! assert_eq!(
//!     maybe_auto_reattach(&registry, &Origin::Home, 5),
//!     ReattachDecision::Unknown,
//! );
//! ```

use bs_identity::{AccessToken, CredentialStore, HomeIdentity, IdentityFields, home_identity_of};
use bs_instances::{ConnectionState, InstanceRegistry};
use bs_types::Origin;

use crate::error::CoreError;

/// Домашняя идентичность пира, увиденного на инстансе `origin`.
///
/// В вебе это `peerHomeWithRemote`: для записи-реплики возвращает
/// `(homeHost, homeUserId)`, для нативной записи — `(deliveringHost, rowId)`.
/// `Origin::Home` передаётся в нижележащий [`home_identity_of`] пустой
/// строкой — ровно как `''` в веб-клиенте.
///
/// Возвращает `None`, если запись реплицирована, но `homeInstance` или
/// `homeUserId` пусты (legacy stub) — склеивать её с чужой нельзя.
///
/// # Пример
/// ```
/// use bs_core::peer_home_with_remote;
/// use bs_identity::IdentityFields;
/// use bs_types::Origin;
///
/// let row = IdentityFields {
///     id: "42".to_owned(),
///     home_user_id: None,
///     home_instance: None,
/// };
/// let home = peer_home_with_remote(&row, &Origin::Home, "chat.example.com")
///     .expect("нативная запись");
/// assert_eq!(home.host, "chat.example.com");
/// assert_eq!(home.user_id, "42");
/// ```
#[must_use]
pub fn peer_home_with_remote(
    row: &IdentityFields,
    origin: &Origin,
    local_host: &str,
) -> Option<HomeIdentity> {
    home_identity_of(row, &origin.as_web_str(), local_host)
}

/// Достаёт (или подтверждает наличие) токен удалённого инстанса.
///
/// В вебе `ensureRemoteCredential` лениво запрашивает токен у удалённого
/// инстанса и кладёт его в общий кейчейн. Здесь сетевая часть живёт в
/// `bs-identity::InstanceSession`, а эта функция — единая точка чтения
/// токена по [`Origin`] из [`CredentialStore`] с ошибкой в терминах
/// [`CoreError`].
///
/// Для [`Origin::Home`] возвращается [`CoreError::HomeCredentialNotRemote`]:
/// домашний токен берётся из сессии и через этот путь не запрашивается.
///
/// # Ошибки
/// - [`CoreError::HomeCredentialNotRemote`] — передан `Origin::Home`;
/// - [`CoreError::Credentials`] — токена для origin в хранилище нет.
///
/// # Пример
/// ```
/// use bs_core::{ensure_remote_credential, CoreError};
/// use bs_identity::CredentialStore;
/// use bs_types::Origin;
///
/// # fn demo(store: &mut CredentialStore) {
/// let err = ensure_remote_credential(store, &Origin::Home).unwrap_err();
/// assert!(matches!(err, CoreError::HomeCredentialNotRemote));
/// # }
/// ```
pub fn ensure_remote_credential(
    store: &CredentialStore,
    origin: &Origin,
) -> Result<AccessToken, CoreError> {
    if origin.is_home() {
        return Err(CoreError::HomeCredentialNotRemote);
    }
    store.get(origin).cloned().map_err(CoreError::Credentials)
}

/// Что делать с подключением к инстансу при попытке «дотянуться» до него.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReattachDecision {
    /// Инстанс уже подключён или подключается — вмешиваться не нужно.
    AlreadyConnected,
    /// Инстанс отключён, попытки не исчерпаны — переподключиться.
    ShouldReattach,
    /// Инстанс отключён, но лимит попыток исчерпан — оставить как есть.
    GiveUp,
    /// Инстанс вообще не зарегистрирован — решение не принимается.
    Unknown,
}

impl ReattachDecision {
    /// Нужно ли начинать переподключение.
    #[must_use]
    pub const fn is_reattach(self) -> bool {
        matches!(self, Self::ShouldReattach)
    }

    /// Считается ли инстанс подключённым (активно или в процессе).
    #[must_use]
    pub const fn is_connected(self) -> bool {
        matches!(self, Self::AlreadyConnected)
    }
}

/// Решение об автопереподключении к инстансу.
///
/// Аналог `maybeAutoReattach` из веб-клиента: не трогает живые соединения
/// и не пытается бесконечно переподключаться к «мёртвому» инстансу.
/// `max_attempts` — сколько попыток [`ConnectionState::Reconnecting`]
/// допустимо, прежде чем считать инстанс безнадёжным.
///
/// # Пример
/// ```
/// use bs_core::{maybe_auto_reattach, ReattachDecision};
/// use bs_instances::InstanceRegistry;
/// use bs_types::Origin;
///
/// let registry = InstanceRegistry::new();
/// assert_eq!(
///     maybe_auto_reattach(&registry, &Origin::Home, 3),
///     ReattachDecision::Unknown,
/// );
/// ```
#[must_use]
pub fn maybe_auto_reattach(
    registry: &InstanceRegistry,
    origin: &Origin,
    max_attempts: u32,
) -> ReattachDecision {
    let Ok(handle) = registry.get(origin) else {
        return ReattachDecision::Unknown;
    };
    match handle.state() {
        ConnectionState::Connected | ConnectionState::Connecting => {
            ReattachDecision::AlreadyConnected
        }
        ConnectionState::Reconnecting { attempt } => {
            if attempt >= max_attempts {
                ReattachDecision::GiveUp
            } else {
                ReattachDecision::ShouldReattach
            }
        }
        ConnectionState::Stopped => ReattachDecision::ShouldReattach,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bs_identity::user_key_of;

    const LOCAL: &str = "chat.example.com";

    fn row(id: &str, home: Option<(&str, &str)>) -> IdentityFields {
        match home {
            Some((host, uid)) => IdentityFields {
                id: id.to_owned(),
                home_user_id: Some(uid.to_owned()),
                home_instance: Some(host.to_owned()),
            },
            None => IdentityFields {
                id: id.to_owned(),
                home_user_id: None,
                home_instance: None,
            },
        }
    }

    fn remote() -> Origin {
        Origin::parse_web_str("https://nova.ddns.net").expect("url")
    }

    #[test]
    fn peer_home_native_on_home() {
        let home = peer_home_with_remote(&row("42", None), &Origin::Home, LOCAL).expect("native");
        assert_eq!(home.host, LOCAL);
        assert_eq!(home.user_id, "42");
    }

    #[test]
    fn peer_home_replica_on_remote() {
        let seen = row("7", Some(("orbit.ddns.net", "9")));
        let home = peer_home_with_remote(&seen, &remote(), LOCAL).expect("replica");
        assert_eq!(home.host, "orbit.ddns.net");
        assert_eq!(home.user_id, "9");
    }

    #[test]
    fn peer_home_none_without_home_user_id() {
        let stub = IdentityFields {
            id: "7".to_owned(),
            home_user_id: None,
            home_instance: Some("orbit.ddns.net".to_owned()),
        };
        assert!(peer_home_with_remote(&stub, &remote(), LOCAL).is_none());
    }

    #[test]
    fn peer_home_key_matches_across_origins() {
        let at_home = row("42", None);
        let at_remote = row("7", Some((LOCAL, "42")));
        let key_home = user_key_of(&at_home, "", LOCAL);
        let key_remote = user_key_of(&at_remote, &remote().as_web_str(), LOCAL);
        assert_eq!(key_home, key_remote);
        assert!(peer_home_with_remote(&at_home, &Origin::Home, LOCAL).is_some());
        assert!(peer_home_with_remote(&at_remote, &remote(), LOCAL).is_some());
    }

    #[test]
    fn peer_home_handles_port_and_case() {
        // Со схемой host нормализуется URL-парсером: нижний регистр, порт сохранён.
        let seen = row("7", Some(("https://Nova.DDNS.net:8443", "9")));
        let home = peer_home_with_remote(&seen, &remote(), LOCAL).expect("replica");
        assert_eq!(home.host, "nova.ddns.net:8443");

        // Без схемы строка сохраняется как есть (как в вебе), но user_key
        // всё равно нормализует хост через home_host_of.
        let bare = row("7", Some(("Nova.DDNS.net:8443", "9")));
        let home = peer_home_with_remote(&bare, &remote(), LOCAL).expect("replica");
        assert_eq!(home.host, "Nova.DDNS.net:8443");
        let key = user_key_of(&bare, &remote().as_web_str(), LOCAL);
        assert_eq!(key, "nova.ddns.net:9");
    }

    #[test]
    fn ensure_credential_rejects_home() {
        let store = CredentialStore::new();
        let err = ensure_remote_credential(&store, &Origin::Home).unwrap_err();
        assert!(matches!(err, CoreError::HomeCredentialNotRemote));
    }

    #[test]
    fn ensure_credential_errors_when_missing() {
        let store = CredentialStore::new();
        let err = ensure_remote_credential(&store, &remote()).unwrap_err();
        assert!(matches!(err, CoreError::Credentials(_)));
    }

    #[test]
    fn ensure_credential_reuses_stored_token() {
        let mut store = CredentialStore::new();
        let token = AccessToken::new("remote-jwt");
        store.insert(remote(), token.clone());
        let got = ensure_remote_credential(&store, &remote()).expect("token");
        assert_eq!(got.expose(), token.expose());
    }

    #[test]
    fn reattach_unknown_for_missing_instance() {
        let registry = InstanceRegistry::new();
        assert_eq!(
            maybe_auto_reattach(&registry, &Origin::Home, 3),
            ReattachDecision::Unknown
        );
        assert!(!ReattachDecision::Unknown.is_connected());
        assert!(!ReattachDecision::Unknown.is_reattach());
    }

    #[test]
    fn decision_predicates() {
        assert!(ReattachDecision::AlreadyConnected.is_connected());
        assert!(!ReattachDecision::AlreadyConnected.is_reattach());
        assert!(ReattachDecision::ShouldReattach.is_reattach());
        assert!(!ReattachDecision::ShouldReattach.is_connected());
        assert!(!ReattachDecision::GiveUp.is_reattach());
        assert!(!ReattachDecision::GiveUp.is_connected());
    }
}
