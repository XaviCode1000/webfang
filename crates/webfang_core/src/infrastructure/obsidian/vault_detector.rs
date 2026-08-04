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
/// 5. Auto-scan common locations (see `scan_for_vault()`)
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
    detect_vault_with_root(None, cli_path, env_var, config_path)
}

/// Detect an Obsidian vault with an injectable scan root for hermetic testing.
///
/// Behaves identically to [`detect_vault`] except that priority-5 auto-scan
/// uses `root` instead of the process cwd / home directory. Pass `None` for
/// `root` to get the default production behavior.
///
/// # Arguments
/// - `root` — Optional root directory for the auto-scan (replaces cwd/home)
/// - `cli_path` — Optional explicit vault path from CLI
/// - `env_var` — Optional environment variable name to check (default: "OBSIDIAN_VAULT")
/// - `config_path` — Optional vault path from config file
pub fn detect_vault_with_root(
    root: Option<&Path>,
    cli_path: Option<&Path>,
    env_var: Option<&str>,
    config_path: Option<&str>,
) -> Option<PathBuf> {
    // Priority 1: CLI flag
    if let Some(path) = detect_from_cli(cli_path) {
        return Some(path);
    }

    // Priority 2: Environment variable
    let env_var_name = env_var.unwrap_or("OBSIDIAN_VAULT");
    if let Some(path) = detect_from_env(env_var_name) {
        return Some(path);
    }

    // Priority 3: Config file
    if let Some(path) = detect_from_config(config_path) {
        return Some(path);
    }

    // Priority 4: Official Obsidian registry
    if let Some(path) = detect_from_registry() {
        return Some(path);
    }

    // Priority 5: Auto-scan (injected root or cwd/home fallback)
    let scanned = match root {
        Some(r) => scan_for_vault_from(r),
        None => scan_for_vault(),
    };
    detect_from_scan(scanned)
}

/// Priority 1: detect from the explicit CLI `--vault` flag.
fn detect_from_cli(cli_path: Option<&Path>) -> Option<PathBuf> {
    let path = cli_path?;
    if is_valid_vault(path) {
        tracing::debug!("Vault detected via CLI path: {}", path.display());
        return Some(path.to_path_buf());
    }
    tracing::warn!("Explicit vault path not valid: {}", path.display());
    None
}

/// Priority 2: detect from the `OBSIDIAN_VAULT` environment variable.
fn detect_from_env(env_var_name: &str) -> Option<PathBuf> {
    let Ok(env_path) = std::env::var(env_var_name) else {
        return None;
    };
    let path = PathBuf::from(&env_path);
    if is_valid_vault(&path) {
        tracing::debug!("Vault detected via env var {}: {}", env_var_name, env_path);
        return Some(path);
    }
    tracing::warn!("OBSIDIAN_VAULT env var not valid: {}", env_path);
    None
}

/// Priority 3: detect from the TOML config `vault_path` field.
fn detect_from_config(config_path: Option<&str>) -> Option<PathBuf> {
    let config_str = config_path?;
    let path = PathBuf::from(config_str);
    if is_valid_vault(&path) {
        tracing::debug!("Vault detected via config path: {}", config_str);
        return Some(path);
    }
    tracing::warn!("Config vault_path not valid: {}", config_str);
    None
}

/// Priority 4: detect from the official Obsidian registry.
fn detect_from_registry() -> Option<PathBuf> {
    let path = get_vault_from_registry()?;
    tracing::debug!("Vault detected from Obsidian registry: {}", path.display());
    Some(path)
}

/// Priority 5: report the auto-scan result.
fn detect_from_scan(scanned: Option<PathBuf>) -> Option<PathBuf> {
    let path = scanned?;
    tracing::debug!("Vault auto-detected: {}", path.display());
    Some(path)
}

/// Check if a path is a valid Obsidian vault (contains `.obsidian/` directory).
/// Check whether a path is a valid Obsidian vault (directory with `.obsidian/` marker).
#[must_use]
pub fn is_valid_vault(path: &Path) -> bool {
    path.is_dir() && path.join(".obsidian").is_dir()
}

/// Scan for Obsidian vault starting from an injected root directory.
///
/// Search order:
/// 1. Walk upward from `root` (max 3 levels) checking for `.obsidian/` marker
/// 2. `root/Obsidian/`
/// 3. `root/Documents/Obsidian/`
///
/// Returns the first valid vault found, or None.
fn scan_for_vault_from(root: &Path) -> Option<PathBuf> {
    let mut current = root;

    for _ in 0..3 {
        if is_valid_vault(current) {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }

    let candidates = [
        root.join("Obsidian"),
        root.join("Documents").join("Obsidian"),
    ];

    candidates
        .into_iter()
        .find(|candidate| is_valid_vault(candidate))
}

/// Scan for Obsidian vault in common locations (production entry point).
///
/// Tries the current working directory first, then the home directory.
/// Delegates to [`scan_for_vault_from`] for each root.
fn scan_for_vault() -> Option<PathBuf> {
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(path) = scan_for_vault_from(&cwd) {
            return Some(path);
        }
    }

    let home = dirs::home_dir()?;
    scan_for_vault_from(&home)
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
        return missing_registry(&registry_path);
    }

    // Read and parse registry
    let content = read_registry(&registry_path)?;
    let json = parse_registry(&content)?;

    // Extract vaults object
    let vaults = json.get("vaults")?.as_object()?;

    // Find vault with highest timestamp (most recent)
    let (_id, vault_data, ts) = find_most_recent_vault(vaults)?;
    let path = vault_data.get("path")?.as_str()?;

    resolve_registry_vault(ts, path, &PathBuf::from(path))
}

/// Report a missing registry file and return `None`.
fn missing_registry(registry_path: &Path) -> Option<PathBuf> {
    tracing::trace!("Obsidian registry not found: {}", registry_path.display());
    None
}

/// Verify the resolved registry vault still exists, or report it as stale.
fn resolve_registry_vault(ts: i64, path: &str, vault_path: &Path) -> Option<PathBuf> {
    tracing::debug!("Found vault from registry (ts={}): {}", ts, path);

    if is_valid_vault(vault_path) {
        Some(vault_path.to_path_buf())
    } else {
        tracing::warn!("Registry vault no longer exists: {}", path);
        None
    }
}

/// Read the Obsidian registry file contents.
fn read_registry(registry_path: &Path) -> Option<String> {
    match std::fs::read_to_string(registry_path) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!("Failed to read Obsidian registry: {}", e);
            None
        },
    }
}

/// Parse the registry JSON.
fn parse_registry(content: &str) -> Option<serde_json::Value> {
    match serde_json::from_str(content) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("Failed to parse Obsidian registry: {}", e);
            None
        },
    }
}

/// Find the vault entry with the most recent `ts` timestamp (last opened).
fn find_most_recent_vault(
    vaults: &serde_json::Map<String, serde_json::Value>,
) -> Option<(&str, &serde_json::Value, i64)> {
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

    best_vault
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_is_valid_vault_true() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".obsidian")).unwrap();
        fs::write(tmp.path().join(".obsidian").join("app.json"), "{}").unwrap();
        assert!(is_valid_vault(tmp.path()));
    }

    #[test]
    fn test_is_valid_vault_false_no_obsidian() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_valid_vault(tmp.path()));
    }

    #[test]
    fn test_is_valid_vault_with_obsidian_dir() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".obsidian")).unwrap();
        assert!(is_valid_vault(tmp.path()));
    }

    #[test]
    fn test_detect_vault_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".obsidian")).unwrap();
        let result = detect_vault(Some(tmp.path()), None, None);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), tmp.path());
    }

    #[test]
    fn test_detect_vault_env_var() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".obsidian")).unwrap();

        let _guard = webfang_test_utils::EnvGuard::with(&[(
            "WEBFANG_TEST_VAULT",
            tmp.path().to_str().unwrap(),
        )]);
        let result = detect_vault(None, Some("WEBFANG_TEST_VAULT"), None);
        assert!(result.is_some());
    }

    #[test]
    fn test_detect_vault_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = webfang_test_utils::EnvGuard::clean(&["OBSIDIAN_VAULT"]);
        let result = detect_vault_with_root(Some(tmp.path()), None, None, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_vault_invalid_path() {
        let tmp = tempfile::tempdir().unwrap();
        let non_existent = PathBuf::from("/nonexistent/path/to/vault");
        let _guard = webfang_test_utils::EnvGuard::clean(&["OBSIDIAN_VAULT"]);
        let result = detect_vault_with_root(Some(tmp.path()), Some(&non_existent), None, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_vault_with_fixture() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".obsidian")).unwrap();

        let _guard = webfang_test_utils::EnvGuard::clean(&["OBSIDIAN_VAULT"]);
        let result = detect_vault_with_root(Some(tmp.path()), None, None, None);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), tmp.path());
    }

    #[test]
    fn test_detect_vault_config_path() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".obsidian")).unwrap();

        let result = detect_vault(None, None, Some(tmp.path().to_str().unwrap()));
        assert!(result.is_some());
    }

    #[test]
    fn test_get_registry_path() {
        let path = get_registry_path();
        let _ = path;
    }
}
