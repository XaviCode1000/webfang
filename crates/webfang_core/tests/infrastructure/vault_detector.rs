//! Integration tests for vault_detector — detect_vault with temp dirs,
//! .obsidian marker, explicit paths, env vars, and config paths.

use webfang::infrastructure::obsidian::vault_detector::detect_vault;
use tempfile::TempDir;

/// Create a temp directory that looks like an Obsidian vault.
fn make_vault(tmp: &TempDir, name: &str) -> std::path::PathBuf {
    let vault = tmp.path().join(name);
    std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
    vault
}

// ── Explicit vault path (CLI flag) ────────────────────────────────────────

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

// ── .obsidian directory detection ─────────────────────────────────────────

#[test]
fn detect_vault_with_obsidian_directory_via_config() {
    let tmp = TempDir::new().unwrap();
    let vault = make_vault(&tmp, "detected");

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

// ── Environment variable detection ────────────────────────────────────────

#[test]
#[ignore = "env-dependent: uses std::env::set_var"]
fn detect_vault_via_env_var() {
    let tmp = TempDir::new().unwrap();
    let vault = make_vault(&tmp, "env_vault");
    let env_name = "WEBFANG_VAULT_TEST_INTEGRATION";

    std::env::set_var(env_name, vault.to_str().unwrap());
    let result = detect_vault(None, Some(env_name), None);
    assert!(result.is_some());
    assert_eq!(result.unwrap(), vault);
    std::env::remove_var(env_name);
}

#[test]
#[ignore = "env-dependent: uses std::env::set_var"]
fn detect_vault_env_var_invalid_path() {
    let tmp = TempDir::new().unwrap();
    let non_vault = tmp.path().join("env_bad");
    std::fs::create_dir_all(&non_vault).unwrap();
    let env_name = "WEBFANG_VAULT_TEST_INVALID_ENV";

    std::env::set_var(env_name, non_vault.to_str().unwrap());
    let result = detect_vault(None, Some(env_name), None);
    assert!(result.is_none());
    std::env::remove_var(env_name);
}

// ── Config path detection ─────────────────────────────────────────────────

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

// ── Priority: CLI > env > config ──────────────────────────────────────────

#[test]
fn cli_path_takes_priority_over_env_var() {
    let tmp = TempDir::new().unwrap();
    let cli_vault = make_vault(&tmp, "cli_vault");
    let env_vault = make_vault(&tmp, "env_vault");
    let env_name = "WEBFANG_VAULT_TEST_PRIORITY_CLI";

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
    let env_name = "WEBFANG_VAULT_TEST_PRIORITY_ENV";

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

// ── Fallback / no-args ────────────────────────────────────────────────────

#[test]
#[ignore = "env-dependent: uses std::env::set_var"]
fn detect_all_none_does_not_panic() {
    let result = detect_vault(None, None, None);
    // Result depends on environment — just verify no panic
    let _ = result;
}
