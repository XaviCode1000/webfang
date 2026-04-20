//! CLI command handlers
//!
//! Extracted from orchestrator.rs to reduce monolithism and improve testability.
//! Each command handler is isolated and can be tested independently.

use std::path::PathBuf;

use tracing::info;

use crate::cli::Args;
use crate::cli::error::CliExit;
use crate::infrastructure::obsidian::detect_vault;

/// Common preflight checks for all commands
pub struct PreflightContext {
    pub vault_path: Option<PathBuf>,
    pub config_path: PathBuf,
    pub target_url: String,
}

/// Run preflight checks and build context
pub async fn preflight(args: &Args) -> Result<PreflightContext, CliExit> {
    // Target URL is guaranteed to exist (checked by caller)
    let target_url = args.url.clone().expect("url required");

    // Emoji helpers (resolved once after NO_COLOR check)
    let _ok = crate::cli::preflight::icon("✅", "OK");

    // Config path
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rust-scraper")
        .join("config.toml");

    if config_path.exists() {
        info!("Config loaded: {}", config_path.display());
    }

    // Vault detection
    let config_defaults = crate::cli::config::ConfigDefaults::load(&config_path);

    let vault_path = detect_vault(
        args.vault.as_deref(),
        None,
        config_defaults.vault_path.as_deref(),
    );

    if let Some(ref vault) = vault_path {
        info!("Obsidian vault detected: {}", vault.display());
    } else {
        info!("No Obsidian vault detected, using output directory");
    }

    // GAP 3 (Bug #30): Warn when vault is provided but headless mode (no --quick-save)
    if let Some(ref _vault) = vault_path {
        if !args.quick_save {
            tracing::warn!("Vault path provided but --quick-save not enabled.");
            tracing::warn!("   Files will be saved to ./output/, not to the vault.");
            tracing::warn!("   Use --quick-save to save directly to vault _inbox.");
        }
    }

    Ok(PreflightContext {
        vault_path,
        config_path,
        target_url,
    })
}