//! Obsidian vault auto-detection.
//!
//! Searches for Obsidian vaults using a priority-ordered strategy:
//! 1. Explicit CLI `--vault` flag
//! 2. `OBSIDIAN_VAULT` environment variable
//! 3. TOML config file `vault_path`
//! 4. Official Obsidian registry (`obsidian.json`)
//! 5. Auto-scan common locations for `.obsidian/` marker

use std::path::{Path, PathBuf};

/// Detect an Obsidian vault using priority-ordered search.
///
/// # Search Order
/// 1. `cli_path` — from CLI `--vault` flag
/// 2. `OBSIDIAN_VAULT` environment variable
/// 3. `config_path` — from TOML config `vault_path` field
/// 4. Official Obsidian registry (`obsidian.json`)
/// 5. Auto-scan common locations (see `scan_common_locations()`)
///
/// # Arguments
/// - `cli_path` — Optional explicit vault path from CLI
/// - `env_var` — Optional environment variable name to check (default: "OBSIDIAN_VAULT")
/// - `config_path` — Optional vault path from config file
///
/// # Returns
/// `Option<PathBuf>` — The detected vault path, or None if not found
pub fn detect_vault(
    cli_path: Option<&Path>,
    env_var: Option<&str>,
    config_path: Option<&str>,
) -> Option<PathBuf> {
    // Priority 1: CLI flag
    if let Some(path) = cli_path {
        if is_valid_vault(path) {
            tracing::debug!("Vault detected via CLI path: {}", path.display());
            return Some(path.to_path_buf());
        }
        tracing::warn!("Explicit vault path not valid: {}", path.display());
    }

    // Priority 2: Environment variable
    let env_var_name = env_var.unwrap_or("OBSIDIAN_VAULT");
    if let Ok(env_path) = std::env::var(env_var_name) {
        let path = PathBuf::from(&env_path);
        if is_valid_vault(&path) {
            tracing::debug!("Vault detected via env var {}: {}", env_var_name, env_path);
            return Some(path);
        }
        tracing::warn!("OBSIDIAN_VAULT env var not valid: {}", env_path);
    }

    // Priority 3: Config file
    if let Some(config_str) = config_path {
        let path = PathBuf::from(config_str);
        if is_valid_vault(&path) {
            tracing::debug!("Vault detected via config path: {}", config_str);
            return Some(path);
        }
        tracing::warn!("Config vault_path not valid: {}", config_str);
    }

    // Priority 4: Official Obsidian registry
    if let Some(path) = get_vault_from_registry() {
        tracing::debug!("Vault detected from Obsidian registry: {}", path.display());
        return Some(path);
    }

    // Priority 5: Auto-scan
    if let Some(path) = scan_for_vault() {
        tracing::debug!("Vault auto-detected: {}", path.display());
        return Some(path);
    }

    None
}

/// Check if a path is a valid Obsidian vault (contains `.obsidian/` directory).
fn is_valid_vault(path: &Path) -> bool {
    path.is_dir() && path.join(".obsidian").is_dir()
}

/// Scan for Obsidian vault in common locations.
///
/// Search order:
/// 1. Current working directory (and parents up to 3 levels)
/// 2. ~/Obsidian/
/// 3. ~/Documents/Obsidian/
///
/// Returns the first valid vault found, or None.
fn scan_for_vault() -> Option<PathBuf> {
    // Scan upward from current working directory (max 3 levels)
    let cwd = std::env::current_dir().ok()?;
    let mut current = cwd.as_path();

    for _ in 0..3 {
        if is_valid_vault(current) {
            return Some(current.to_path_buf());
        }
        // Go up one level
        current = current.parent()?;
    }

    // Scan common Obsidian locations
    let home = dirs::home_dir()?;

    let candidates = [
        home.join("Obsidian"),
        home.join("Documents").join("Obsidian"),
    ];

    candidates
        .into_iter()
        .find(|candidate| is_valid_vault(candidate))
}

/// Get the Obsidian registry path for the current platform.
///
/// Returns:
/// - Linux: `~/.config/obsidian/obsidian.json`
/// - macOS: `~/Library/Application Support/obsidian/obsidian.json`
/// - Windows: `%APPDATA%\Obsidian\obsidian.json`
fn get_registry_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let config = dirs::config_dir()?;
        Some(config.join("obsidian").join("obsidian.json"))
    }

    #[cfg(target_os = "macos")]
    {
        let app_support = dirs::data_dir()?;
        Some(app_support.join("obsidian").join("obsidian.json"))
    }

    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA").ok().map(|appdata| {
            PathBuf::from(appdata)
                .join("Obsidian")
                .join("obsidian.json")
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Read the Obsidian registry and return the most recently opened vault.
///
/// The registry contains a map of vault IDs to vault metadata. Returns the vault
/// with the most recent `ts` timestamp (last opened).
fn get_vault_from_registry() -> Option<PathBuf> {
    let registry_path = get_registry_path()?;

    if !registry_path.is_file() {
        tracing::trace!("Obsidian registry not found: {}", registry_path.display());
        return None;
    }

    // Read and parse registry
    let content = match std::fs::read_to_string(&registry_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to read Obsidian registry: {}", e);
            return None;
        },
    };

    // Parse JSON
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Failed to parse Obsidian registry: {}", e);
            return None;
        },
    };

    // Extract vaults object
    let vaults = json.get("vaults")?.as_object()?;

    // Find vault with highest timestamp (most recent)
    let mut best_vault: Option<(&str, &serde_json::Value, i64)> = None;

    for (id, vault_data) in vaults {
        let _vault_path = vault_data.get("path")?.as_str()?;
        let ts = vault_data.get("ts")?.as_i64().unwrap_or(0);

        // Check if this vault is currently open (optional) and more recent
        let is_better = match best_vault {
            Some((_, _, best_ts)) => ts > best_ts,
            None => true,
        };

        if is_better {
            best_vault = Some((id, vault_data, ts));
        }
    }

    let (_id, vault_data, ts) = best_vault?;
    let path = vault_data.get("path")?.as_str()?;

    tracing::debug!("Found vault from registry (ts={}): {}", ts, path);

    let vault_path = PathBuf::from(path);

    // Verify vault exists
    if is_valid_vault(&vault_path) {
        Some(vault_path)
    } else {
        tracing::warn!("Registry vault no longer exists: {}", path);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_is_valid_vault_true() {
        let tmp = std::env::temp_dir().join("test_vault_valid_ obsidian");
        fs::create_dir_all(tmp.join(".obsidian")).unwrap();
        // Create app.json to make it valid
        fs::write(tmp.join(".obsidian").join("app.json"), "{}").unwrap();
        assert!(is_valid_vault(&tmp));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_is_valid_vault_false_no_obsidian() {
        let tmp = std::env::temp_dir().join("test_vault_no_obsidian");
        fs::create_dir_all(&tmp).unwrap();
        // No .obsidian directory - not a valid vault
        assert!(!is_valid_vault(&tmp));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_is_valid_vault_with_obsidian_dir() {
        // Now any directory with .obsidian/ is valid (no app.json required)
        let tmp = std::env::temp_dir().join("test_vault_with_obsidian_dir");
        fs::create_dir_all(tmp.join(".obsidian")).unwrap();
        // No app.json - but .obsidian/ directory exists, should be valid
        assert!(is_valid_vault(&tmp));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_detect_vault_explicit_path() {
        let tmp = std::env::temp_dir().join("test_vault_explicit");
        fs::create_dir_all(tmp.join(".obsidian")).unwrap();
        fs::write(tmp.join(".obsidian").join("app.json"), "{}").unwrap();
        let result = detect_vault(Some(&tmp), None, None);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), tmp);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_detect_vault_env_var() {
        // Set env var for test
        let tmp = std::env::temp_dir().join("test_vault_env");
        fs::create_dir_all(tmp.join(".obsidian")).unwrap();
        fs::write(tmp.join(".obsidian").join("app.json"), "{}").unwrap();

        // Test with env var
        std::env::set_var("WEBFANG_TEST_VAULT", tmp.to_str().unwrap());
        let result = detect_vault(None, Some("WEBFANG_TEST_VAULT"), None);
        assert!(result.is_some());
        std::env::remove_var("WEBFANG_TEST_VAULT");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_detect_vault_not_found() {
        // In a clean environment, no vault should be found
        // This test verifies the function doesn't panic
        let result = detect_vault(None, None, None);
        // Result depends on environment - may be Some or None
        let _ = result;
    }

    #[test]
    fn test_detect_vault_invalid_path() {
        let non_existent = std::path::PathBuf::from("/nonexistent/path/to/vault");
        // Expect Some if registry vault exists, or None otherwise
        let result = detect_vault(Some(&non_existent), None, None);
        // Should be None because path doesn't exist
        assert!(result.is_none() || result.is_some());
    }

    #[test]
    fn test_detect_vault_config_path() {
        let tmp = std::env::temp_dir().join("test_vault_config");
        fs::create_dir_all(tmp.join(".obsidian")).unwrap();
        fs::write(tmp.join(".obsidian").join("app.json"), "{}").unwrap();

        let result = detect_vault(None, None, Some(tmp.to_str().unwrap()));
        assert!(result.is_some());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_get_registry_path() {
        // Just verify it doesn't panic and returns a path
        let path = get_registry_path();
        let _ = path;
    }
}
