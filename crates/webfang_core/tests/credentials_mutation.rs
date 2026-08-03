//! Mutation-targeted tests for the credentials domain.
//!
//! These tests exist to kill specific surviving mutants found by the weekly
//! cargo-mutants baseline (2026-08). They are defensive/preventive coverage:
//! with the corrected hot-path scope in `.cargo/mutants.toml`,
//! `domain/credentials.rs` is no longer mutated by CI, but these invariants
//! are structural security contracts and must hold regardless.
//!
//! Targeted mutants (verified against cargo-mutants 27.1.0 output):
//! - `SensitiveString::eq` replaced with `true`/`false`/`!=`
//! - `SensitiveString::as_str` replaced with `""`/`"xyzzy"`
//! - `CredentialStore::contains` replaced with `true`/`false`
//! - `CredentialStore::remove` replaced with `None`
//! - `CredentialStore::len` replaced with `1`
//! - `CredentialStore::is_empty` replaced with `true`/`false`

use webfang_core::domain::credentials::{
    ApiKey, CredentialStore, SecretCredential, SensitiveString,
};

#[test]
fn sensitive_string_eq_distinguishes_secrets() {
    let a = SensitiveString::new("secret_a");
    let b = SensitiveString::new("secret_b");
    // Kills: eq -> true, eq -> false, eq's == replaced with !=
    assert_ne!(a, b);
}

#[test]
fn sensitive_string_eq_matches_identical_secrets() {
    let a = SensitiveString::new("shared_secret");
    let b = SensitiveString::new("shared_secret");
    // Kills: eq -> false (the != replacement makes this fail too)
    assert_eq!(a, b);
}

#[test]
fn sensitive_string_as_str_returns_original_value() {
    let secret = SensitiveString::new("sk-live-42");
    // Kills: as_str -> "", as_str -> "xyzzy"
    assert_eq!(secret.as_str(), "sk-live-42");
}

#[test]
fn credential_store_contains_is_accurate() {
    let mut store = CredentialStore::new();
    store.add(SecretCredential::new("openai", ApiKey::new("sk-openai-1")));
    // Kills: contains -> false
    assert!(store.contains("openai"));
    // Kills: contains -> true
    assert!(!store.contains("unknown-provider"));
}

#[test]
fn credential_store_remove_actually_removes() {
    let mut store = CredentialStore::new();
    store.add(SecretCredential::new("openai", ApiKey::new("sk-openai-1")));
    // Kills: remove -> None
    let removed = store.remove("openai");
    assert!(removed.is_some());
    assert!(!store.contains("openai"));
}

#[test]
fn credential_store_len_tracks_insertions() {
    let mut store = CredentialStore::new();
    // Kills: len -> 1 on an empty store
    assert_eq!(store.len(), 0);
    store.add(SecretCredential::new("openai", ApiKey::new("sk-openai-1")));
    assert_eq!(store.len(), 1);
}

#[test]
fn credential_store_is_empty_reflects_state() {
    let mut store = CredentialStore::new();
    // Kills: is_empty -> false
    assert!(store.is_empty());
    store.add(SecretCredential::new("openai", ApiKey::new("sk-openai-1")));
    // Kills: is_empty -> true
    assert!(!store.is_empty());
}
