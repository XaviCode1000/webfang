//! Resume mode tests — StateStore persistence and filtering contracts.
//!
//! Tests `StateStore` (create, save, load, mark_processed, is_processed)
//! and `ExportState` (mark_processed, is_processed) via their public APIs.
//! `apply_resume_mode` is tested inline in `scrape_flow.rs` since the
//! function is crate-private.
//!
//! Following contract-based-test-audit: observable behavior only, tempfile for filesystem.

use tempfile::TempDir;

// ===========================================================================
// StateStore — public API tests
// ===========================================================================

/// StateStore creates a new store with correct domain.
#[test]
fn state_store_new_has_correct_domain() {
    let store = webfang_core::infrastructure::export::state_store::StateStore::new("example.com");
    let path = store.get_state_path();
    assert!(path.to_string_lossy().contains("example.com.json"));
}

/// StateStore saves and loads state correctly.
#[test]
fn state_store_save_and_load_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let mut store = webfang_core::infrastructure::export::state_store::StateStore::new("test.com");
    store.set_cache_dir(tmp.path().to_path_buf());

    let mut state = webfang_core::domain::ExportState::new("test.com");
    state.mark_processed("https://test.com/page1");
    state.mark_processed("https://test.com/page2");

    store.save(&state).expect("save should succeed");

    let loaded = store.load().expect("load should succeed");
    assert_eq!(loaded.domain, "test.com");
    assert_eq!(loaded.processed_urls.len(), 2);
    assert!(loaded.is_processed("https://test.com/page1"));
    assert!(loaded.is_processed("https://test.com/page2"));
}

/// StateStore load_or_default returns new state when file doesn't exist.
#[test]
fn state_store_load_or_default_creates_new_when_missing() {
    let tmp = TempDir::new().unwrap();
    let mut store = webfang_core::infrastructure::export::state_store::StateStore::new("new.com");
    store.set_cache_dir(tmp.path().to_path_buf());

    let state = store
        .load_or_default()
        .expect("load_or_default should succeed");
    assert_eq!(state.domain, "new.com");
    assert!(state.processed_urls.is_empty());
}

/// StateStore load_or_default returns existing state when file exists.
#[test]
fn state_store_load_or_default_returns_existing() {
    let tmp = TempDir::new().unwrap();
    let mut store =
        webfang_core::infrastructure::export::state_store::StateStore::new("existing.com");
    store.set_cache_dir(tmp.path().to_path_buf());

    // Save first
    let mut state = webfang_core::domain::ExportState::new("existing.com");
    state.mark_processed("https://existing.com/page1");
    store.save(&state).unwrap();

    // Load should return existing
    let loaded = store.load_or_default().unwrap();
    assert_eq!(loaded.processed_urls.len(), 1);
    assert!(loaded.is_processed("https://existing.com/page1"));
}

/// StateStore marks URL as processed and checks correctly.
#[test]
fn state_store_mark_and_check_processed() {
    let store = webfang_core::infrastructure::export::state_store::StateStore::new("test.com");
    let mut state = webfang_core::domain::ExportState::new("test.com");

    assert!(!store.is_processed(&state, "https://test.com/page1"));

    store.mark_processed(&mut state, "https://test.com/page1");
    assert!(store.is_processed(&state, "https://test.com/page1"));

    // Duplicate marking doesn't duplicate
    store.mark_processed(&mut state, "https://test.com/page1");
    assert_eq!(state.processed_urls.len(), 1);
}

/// StateStore load fails with informative error for nonexistent file.
#[test]
fn state_store_load_nonexistent_returns_error() {
    let store =
        webfang_core::infrastructure::export::state_store::StateStore::new("nonexistent.com");
    let result = store.load();
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not found"),
        "error should mention 'not found': {err_msg}"
    );
}

/// StateStore uses custom cache directory.
#[test]
fn state_store_custom_cache_dir() {
    let tmp = TempDir::new().unwrap();
    let mut store =
        webfang_core::infrastructure::export::state_store::StateStore::new("custom.com");
    store.set_cache_dir(tmp.path().to_path_buf());

    let path = store.get_state_path();
    assert!(path.starts_with(tmp.path()));
    assert!(path.to_string_lossy().contains("custom.com.json"));
}

// ===========================================================================
// ExportState — public API tests
// ===========================================================================

/// ExportState::new creates empty state with correct domain.
#[test]
fn export_state_new_has_correct_domain() {
    let state = webfang_core::domain::ExportState::new("example.com");
    assert_eq!(state.domain, "example.com");
    assert!(state.processed_urls.is_empty());
    assert_eq!(state.total_exported, 0);
}

/// ExportState::mark_processed adds URL and increments counter.
#[test]
fn export_state_mark_processed_adds_url() {
    let mut state = webfang_core::domain::ExportState::new("test.com");
    state.mark_processed("https://test.com/page1");
    assert_eq!(state.processed_urls.len(), 1);
    assert_eq!(state.total_exported, 1);
}

/// ExportState::mark_processed deduplicates URLs.
#[test]
fn export_state_mark_processed_deduplicates() {
    let mut state = webfang_core::domain::ExportState::new("test.com");
    state.mark_processed("https://test.com/page1");
    state.mark_processed("https://test.com/page1");
    assert_eq!(state.processed_urls.len(), 1);
    assert_eq!(state.total_exported, 1);
}

/// ExportState::is_processed returns true for marked URLs.
#[test]
fn export_state_is_processed_works() {
    let mut state = webfang_core::domain::ExportState::new("test.com");
    assert!(!state.is_processed("https://test.com/page1"));
    state.mark_processed("https://test.com/page1");
    assert!(state.is_processed("https://test.com/page1"));
}

/// ExportState serialization roundtrip (serde).
#[test]
fn export_state_serde_roundtrip() {
    let mut state = webfang_core::domain::ExportState::new("serde.com");
    state.mark_processed("https://serde.com/page1");
    state.mark_processed("https://serde.com/page2");

    let json = serde_json::to_string(&state).unwrap();
    let deserialized: webfang_core::domain::ExportState = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.domain, "serde.com");
    assert_eq!(deserialized.processed_urls.len(), 2);
    assert!(deserialized.is_processed("https://serde.com/page1"));
}

/// StateStore with corrupted JSON file returns error.
#[test]
fn state_store_corrupted_json_returns_error() {
    let tmp = TempDir::new().unwrap();
    let mut store =
        webfang_core::infrastructure::export::state_store::StateStore::new("corrupt.com");
    store.set_cache_dir(tmp.path().to_path_buf());

    // Write corrupted JSON to the state file path
    let path = store.get_state_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ not valid json!!! ").unwrap();

    let result = store.load();
    assert!(result.is_err(), "corrupted JSON should produce an error");
}
