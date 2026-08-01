//! CLI argument parsing helpers.
//!
//! Small pure parsers that translate raw CLI strings into domain types
//! (asset naming strategy) plus shell completion dispatch.

use crate::cli::completions::generate_completions;
use crate::cli::error::CliExit;
use crate::Args;
use crate::Shell;

/// Handle shell completion generation.
pub fn handle_completions(shell: Shell) -> CliExit {
    let clap_shell = match shell {
        Shell::Bash => clap_complete::Shell::Bash,
        Shell::Elvish => clap_complete::Shell::Elvish,
        Shell::Fish => clap_complete::Shell::Fish,
        Shell::PowerShell => clap_complete::Shell::PowerShell,
        Shell::Zsh => clap_complete::Shell::Zsh,
    };
    generate_completions::<Args>(clap_shell)
        .map(|_| CliExit::Success)
        .unwrap_or_else(|_| CliExit::UsageError("completion generation failed".into()))
}

/// Parse asset naming strategy from CLI string.
pub(super) fn parse_asset_naming(s: &str) -> crate::adapters::downloader::AssetNamingStrategy {
    use crate::adapters::downloader::AssetNamingStrategy;
    match s.to_lowercase().as_str() {
        "slug" => AssetNamingStrategy::Slug,
        "content-disposition" => AssetNamingStrategy::ContentDisposition,
        _ => AssetNamingStrategy::Hash,
    }
}
