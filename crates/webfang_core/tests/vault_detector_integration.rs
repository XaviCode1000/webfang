//! Integration tests for vault_detector — Obsidian vault detection via
//! .obsidian marker, explicit paths, env vars, and config paths.

use webfang::infrastructure::obsidian::vault_detector::detect_vault;
use tempfile::TempDir;

/// Create a temp directory that looks like an Obsidian vault (contains .obsidian/).
fn make_vault(tmp: &TempDir, name: &str) -> std::path::PathBuf {
    let vault = tmp.path().join(name);
    std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
    vault
}

// ── Explicit vault path (CLI flag) ───────────────────────────────────────

#[test]
fn detect_with_explicit_valid_vault_path() {
    let tmp = TempDir::new().unwrap();
    let vault = make_vault(&tmp, "my_vault");

    let result = detect_vault(Some(&vault), None, None);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), vault);
}

#[test]
fn detect_with_explicit_invalid_path_returns_none() {
    let tmp = TempDir::new().unwrap();
    let non_vault = tmp.path().join("not_a_vault");
    std::fs::create_dir_all(&non_vault).unwrap();
    // No .obsidian/ inside

    let result = detect_vault(Some(&non_vault), None, None);
    assert!(result.is_none());
}

#[test]
fn detect_with_explicit_nonexistent_path_returns_none() {
    let tmp = TempDir::new().unwrap();
    let ghost = tmp.path().join("does_not_exist");

    let result = detect_vault(Some(&ghost), None, None);
    assert!(result.is_none());
}

// ── .obsidian directory detection ────────────────────────────────────────

#[test]
fn detect_vault_with_obsidian_directory() {
    let tmp = TempDir::new().unwrap();
    let vault = make_vault(&tmp, "detected_vault");

    // Without explicit path — should still find it via config/env fallbacks
    // Here we pass the config_path to point to it
    let result = detect_vault(None, None, Some(vault.to_str().unwrap()));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), vault);
}

#[test]
fn detect_vault_without_obsidian_directory() {
    let tmp = TempDir::new().unwrap();
    let no_vault = tmp.path().join("plain_dir");
    std::fs::create_dir_all(&no_vault).unwrap();

    let result = detect_vault(Some(&no_vault), None, None);
    assert!(result.is_none());
}

// ── Config path detection ────────────────────────────────────────────────

#[test]
fn detect_vault_via_config_path() {
    let tmp = TempDir::new().unwrap();
    let vault = make_vault(&tmp, "config_vault");

    let result = detect_vault(None, None, Some(vault.to_str().unwrap()));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), vault);
}

#[test]
fn detect_vault_invalid_config_path_returns_none() {
    let tmp = TempDir::new().unwrap();
    let non_vault = tmp.path().join("empty");
    std::fs::create_dir_all(&non_vault).unwrap();

    let result = detect_vault(None, None, Some(non_vault.to_str().unwrap()));
    assert!(result.is_none());
}

// ── Environment variable detection ───────────────────────────────────────

#[test]
fn detect_vault_via_env_var() {
    let tmp = TempDir::new().unwrap();
    let vault = make_vault(&tmp, "env_vault");
    let env_name = "WEBFANG_VAULT_TEST_DETECT";

    std::env::set_var(env_name, vault.to_str().unwrap());
    let result = detect_vault(None, Some(env_name), None);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), vault);
    std::env::remove_var(env_name);
}

#[test]
fn detect_vault_env_var_with_invalid_path() {
    let tmp = TempDir::new().unwrap();
    let non_vault = tmp.path().join("env_bad");
    std::fs::create_dir_all(&non_vault).unwrap();
    let env_name = "WEBFANG_VAULT_TEST_INVALID";

    std::env::set_var(env_name, non_vault.to_str().unwrap());
    let result = detect_vault(None, Some(env_name), None);
    assert!(result.is_none());
    std::env::remove_var(env_name);
}

// ── Priority: CLI > env > config ─────────────────────────────────────────

#[test]
fn cli_path_takes_priority_over_env_var() {
    let tmp = TempDir::new().unwrap();
    let cli_vault = make_vault(&tmp, "cli_vault");
    let env_vault = make_vault(&tmp, "env_vault");
    let env_name = "WEBFANG_VAULT_TEST_PRIORITY";

    std::env::set_var(env_name, env_vault.to_str().unwrap());
    let result = detect_vault(Some(&cli_vault), Some(env_name), None);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), cli_vault);
    std::env::remove_var(env_name);
}

#[test]
fn env_var_takes_priority_over_config() {
    let tmp = TempDir::new().unwrap();
    let env_vault = make_vault(&tmp, "env_vault");
    let config_vault = make_vault(&tmp, "config_vault");
    let env_name = "WEBFANG_VAULT_TEST_ENV_PRIORITY";

    std::env::set_var(env_name, env_vault.to_str().unwrap());
    let result = detect_vault(None, Some(env_name), Some(config_vault.to_str().unwrap()));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), env_vault);
    std::env::remove_var(env_name);
}

#[test]
fn config_path_used_when_cli_and_env_missing() {
    let tmp = TempDir::new().unwrap();
    let config_vault = make_vault(&tmp, "config_only");

    let result = detect_vault(None, None, Some(config_vault.to_str().unwrap()));
    assert!(result.is_some());
    assert_eq!(result.unwrap(), config_vault);
}

// ── None fallback ────────────────────────────────────────────────────────

#[test]
fn detect_all_none_returns_none_or_existing_vault() {
    // With no args and no env, result depends on environment (registry, cwd scan).
    // We just verify it doesn't panic.
    let result = detect_vault(None, None, None);
    let _ = result;
}
