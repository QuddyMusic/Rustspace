//! Юнит-тесты слоя идентичности.

use bs_types::Origin;

use crate::error::IdentityError;
use crate::secret::AccessToken;
use crate::store::CredentialStore;

fn remote(url: &str) -> Origin {
    Origin::parse_web_str(url).unwrap()
}

#[test]
fn access_token_masks_debug() {
    let token = AccessToken::new("super-secret-jwt");
    let debug = format!("{token:?}");
    assert!(!debug.contains("super-secret-jwt"));
    assert!(debug.contains("redacted"));
    assert_eq!(format!("{token}"), "<redacted>");
    assert_eq!(token.expose(), "super-secret-jwt");
}

#[test]
fn access_token_from_str() {
    let token = AccessToken::from("abc");
    assert_eq!(token.len(), 3);
    assert!(!token.is_empty());
}

#[test]
fn store_roundtrip_and_remove() {
    let mut store = CredentialStore::new();
    assert!(store.is_empty());

    let home = Origin::Home;
    store.insert(home.clone(), AccessToken::new("home-token"));
    assert!(store.contains(&home));
    assert_eq!(store.len(), 1);
    assert_eq!(store.get(&home).unwrap().expose(), "home-token");

    let removed = store.remove(&home).unwrap();
    assert_eq!(removed.expose(), "home-token");
    assert!(!store.contains(&home));
    assert!(store.is_empty());
}

#[test]
fn store_keeps_tokens_per_origin() {
    let mut store = CredentialStore::new();
    let a = remote("https://a.example.com");
    let b = remote("https://b.example.com");
    store.insert(a.clone(), AccessToken::new("a-token"));
    store.insert(b.clone(), AccessToken::new("b-token"));
    assert_eq!(store.len(), 2);
    assert_eq!(store.get(&a).unwrap().expose(), "a-token");
    assert_eq!(store.get(&b).unwrap().expose(), "b-token");
    let mut origins: Vec<_> = store.origins().map(Origin::as_web_str).collect();
    origins.sort();
    assert_eq!(origins, ["https://a.example.com", "https://b.example.com"]);
}

#[test]
fn store_insert_returns_previous() {
    let mut store = CredentialStore::new();
    let home = Origin::Home;
    assert!(
        store
            .insert(home.clone(), AccessToken::new("one"))
            .is_none()
    );
    let prev = store.insert(home, AccessToken::new("two")).unwrap();
    assert_eq!(prev.expose(), "one");
}

#[test]
fn store_missing_origin_reports_credentials_error() {
    let store = CredentialStore::new();
    let err = store.get(&Origin::Home).unwrap_err();
    match err {
        IdentityError::NoCredentials { origin } => assert_eq!(origin, ""),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn store_missing_remote_origin_keeps_url_in_error() {
    let store = CredentialStore::new();
    let origin = remote("https://x.example.com");
    match store.get(&origin).unwrap_err() {
        IdentityError::NoCredentials { origin } => {
            assert_eq!(origin, "https://x.example.com");
        }
        other => panic!("unexpected: {other:?}"),
    }
}
