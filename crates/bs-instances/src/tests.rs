//! Юнит-тесты реестра и ошибок (без сети).

use std::time::Duration;

use bs_types::Origin;

use crate::error::InstanceError;
use crate::reconnect::ReconnectPolicy;
use crate::registry::InstanceRegistry;

#[test]
fn empty_registry_reports_unknown_instance() {
    let registry = InstanceRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);

    let err = registry.get(&Origin::Home).unwrap_err();
    match err {
        InstanceError::UnknownInstance { origin } => assert_eq!(origin, ""),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn unknown_remote_keeps_url_in_error() {
    let mut registry = InstanceRegistry::new();
    let origin = Origin::parse_web_str("https://peer.example.com").unwrap();
    match registry.get_mut(&origin).unwrap_err() {
        InstanceError::UnknownInstance { origin } => {
            assert_eq!(origin, "https://peer.example.com");
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn remove_missing_returns_none() {
    let mut registry = InstanceRegistry::new();
    assert!(registry.remove(&Origin::Home).is_none());
    assert!(registry.origins().next().is_none());
}

#[test]
fn default_policy_is_sane() {
    let policy = ReconnectPolicy::default();
    assert!(policy.initial <= policy.max);
    assert_eq!(policy.factor, 2);
    assert!(policy.delay(0) >= Duration::from_secs(1));
    assert!(policy.delay(0) <= policy.max);
}
