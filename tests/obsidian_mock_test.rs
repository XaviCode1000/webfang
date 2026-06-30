//! Integration test for MockVault — verifies the test helper creates
//! a valid Obsidian vault structure.

mod common;

use common::MockVault;

#[test]
fn mock_vault_path_exists() {
    let vault = MockVault::new();
    assert!(vault.path().exists(), "vault path should exist");
    assert!(vault.path().is_dir(), "vault path should be a directory");
}

#[test]
fn mock_vault_has_obsidian_directory() {
    let vault = MockVault::new();
    let obsidian_dir = vault.path().join(".obsidian");
    assert!(obsidian_dir.exists(), ".obsidian/ should exist");
    assert!(obsidian_dir.is_dir(), ".obsidian/ should be a directory");
}

#[test]
fn mock_vault_has_obsidian_json() {
    let vault = MockVault::new();
    let json_path = vault.vault_json();
    assert!(json_path.exists(), "obsidian.json should exist");
    assert!(json_path.is_file(), "obsidian.json should be a file");

    let content = std::fs::read_to_string(&json_path).expect("failed to read obsidian.json");
    let parsed: serde_json::Value =
        serde_json::from_str(&content).expect("obsidian.json should be valid JSON");

    let vault_obj = &parsed["vault"];
    assert_eq!(vault_obj["id"], "test-vault-id");
    assert_eq!(vault_obj["name"], "TestVault");
    assert!(vault_obj["fsPath"].is_string(), "fsPath should be a string");
}

#[test]
fn mock_vault_has_workspace_json() {
    let vault = MockVault::new();
    let ws_path = vault.path().join(".obsidian").join("workspace.json");
    assert!(ws_path.exists(), "workspace.json should exist");
}

#[test]
fn mock_vault_has_test_note() {
    let vault = MockVault::new();
    let note_path = vault.path().join("test-note.md");
    assert!(note_path.exists(), "test-note.md should exist");

    let content = std::fs::read_to_string(&note_path).expect("failed to read test-note.md");
    assert!(
        content.contains("tags: [test]"),
        "note should have frontmatter"
    );
    assert!(content.contains("# Test Note"), "note should have heading");
}

#[test]
fn mock_vault_is_recognized_as_vault() {
    let vault = MockVault::new();
    assert!(
        vault.is_recognized_as_vault(),
        "MockVault should be recognized as a valid vault"
    );
}

#[test]
fn mock_vault_metadata_json_content() {
    let vault = MockVault::new();
    let content = std::fs::read_to_string(vault.vault_json()).expect("read obsidian.json");

    // Verify the fsPath matches the actual vault path
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("valid JSON");
    let fs_path = parsed["vault"]["fsPath"]
        .as_str()
        .expect("fsPath is a string");
    assert_eq!(
        fs_path,
        vault.path().to_string_lossy(),
        "fsPath should match vault path"
    );
}
