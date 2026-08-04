//! CLI command handlers
//!
//! Extracted from orchestrator.rs to reduce monolithism and improve testability.
//! Each command handler is isolated and can be tested independently.

use std::path::PathBuf;

use tracing::info;

use crate::application::crawl_options::CrawlOptions;
use crate::cli::config::ConfigDefaults;
use crate::cli::error::CliExit;
use crate::infrastructure::obsidian::{detect_vault, is_valid_vault};

/// Common preflight checks for all commands
#[allow(dead_code)] // pub(crate) Phase 0 triage — internal API surface
pub struct PreflightContext {
    pub(crate) vault_path: Option<PathBuf>,
    pub(crate) config_path: PathBuf,
    pub(crate) target_url: String,
}

/// Run preflight checks and build context
pub async fn preflight(opts: &CrawlOptions) -> Result<PreflightContext, CliExit> {
    // Target URL is guaranteed to exist (checked by caller)
    let target_url = opts.url.to_string();

    // Emoji helpers (resolved once after NO_COLOR check)
    let _ok = crate::cli::preflight::icon("✅", "OK");

    // Config path
    let config_path = resolve_config_path();
    if config_path.exists() {
        info!("Config loaded: {}", config_path.display());
    }

    // Vault detection
    let config_defaults = ConfigDefaults::load(&config_path);

    // Explicit --vault must be valid; cascade only applies when no flag is set
    validate_explicit_vault(opts)?;

    let vault_path = detect_vault_path(opts, &config_defaults);

    log_vault_status(&vault_path, opts);

    Ok(PreflightContext {
        vault_path,
        config_path,
        target_url,
    })
}

/// Resolve the webfang config file path (graceful: missing file = defaults).
fn resolve_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("webfang")
        .join("config.toml")
}

/// Validate an explicit `--vault` flag before cascade detection runs.
fn validate_explicit_vault(opts: &CrawlOptions) -> Result<(), CliExit> {
    if let Some(ref explicit_vault) = opts.export.obsidian_vault {
        if !is_valid_vault(explicit_vault) {
            return Err(CliExit::UsageError(format!(
                "La ruta de vault indicada con --vault no es válida (debe ser un directorio con .obsidian/): {}",
                explicit_vault.display()
            )));
        }
    }
    Ok(())
}

/// Detect the vault path via explicit flag, config default, or cascade.
fn detect_vault_path(opts: &CrawlOptions, config_defaults: &ConfigDefaults) -> Option<PathBuf> {
    detect_vault(
        opts.export.obsidian_vault.as_deref(),
        None,
        config_defaults.vault_path.as_deref(),
    )
}

/// Log the vault detection result and GAP 3 headless-mode warning.
fn log_vault_status(vault_path: &Option<PathBuf>, opts: &CrawlOptions) {
    if let Some(ref vault) = vault_path {
        info!("Obsidian vault detected: {}", vault.display());
    } else {
        info!("No Obsidian vault detected, using output directory");
    }

    warn_headless_vault_mismatch(vault_path, opts);
}

/// GAP 3 (Bug #30): Warn when vault is provided but headless mode (no --quick-save).
fn warn_headless_vault_mismatch(vault_path: &Option<PathBuf>, opts: &CrawlOptions) {
    if let Some(ref _vault) = vault_path {
        if !opts.export.quick_save {
            tracing::warn!("Vault path provided but --quick-save not enabled.");
            tracing::warn!("   Files will be saved to ./output/, not to the vault.");
            tracing::warn!("   Use --quick-save to save directly to vault _inbox.");
        }
    }
}
