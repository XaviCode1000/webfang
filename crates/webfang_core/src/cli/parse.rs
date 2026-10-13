//! CLI argument parsing helpers.
//!
//! Small pure parsers that translate raw CLI strings into domain types
//! plus shell completion dispatch.

use crate::cli::args::{Args, SubcommandArgs};
use crate::cli::completions::generate_completions;
use crate::cli::error::CliExit;
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

/// Collect the diagnostics-relevant inputs for the `doctor` / `show`
/// subcommands from the already-parsed [`Args`].
///
/// A **pure projection** of the argv boundary: it reads what the operator
/// supplied and reports which knobs were explicit. It resolves **no defaults**
/// beyond reporting the effective values the run would use — a diagnostic that
/// invented a default would be a claim about a value the operator never typed.
///
/// Provenance for `--max-chars` is carried by
/// [`AiArgs::max_chars_explicit`](crate::cli::args::ai::AiArgs::max_chars_explicit),
/// which the argv boundary already computed from
/// `clap::ArgMatches::value_source` (the only place the distinction exists).
/// `--threshold` has no explicitness flag today, so it is always reported as
/// the spec default.
pub fn subcommand_args(args: &Args) -> SubcommandArgs {
    SubcommandArgs {
        // `--embedding-provider` names the slot these diagnostics inspect.
        provider_id: args.llm.embedding_provider.clone(),
        max_chars_explicit: args.ai.max_chars_explicit,
        max_chars_value: Some(args.ai.max_chars),
        threshold_value: Some(args.ai.threshold),
    }
}

/// Validate a provider id at the argv boundary.
///
/// Mirrors what the providers registry enforces today: ASCII alphanumerics,
/// `_`, `-`, `.` — the same shape `openai`, `ollama-local` use. Anything else
/// is a typo the operator can correct, not a value to normalize silently.
///
/// # Errors
/// An empty id, or one carrying a character outside the allowed set.
pub fn parse_provider_id(raw: &str) -> Result<String, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("el identificador del provider no puede ser vacío".into());
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err(format!(
            "el identificador «{s}» contiene caracteres no permitidos (solo alfanuméricos, '_', '-', '.')"
        ));
    }
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::test_support::parse_args_hermetic;

    #[test]
    fn subcommand_args_without_provider_reports_no_provider_id() {
        let args = parse_args_hermetic(&[]).expect("bare invocation parses");
        let sub = subcommand_args(&args);
        assert!(sub.provider_id.is_none());
        // `max_chars` always carries the (default) value, but it is not explicit.
        assert!(!sub.max_chars_explicit);
        assert!(sub.max_chars_value.is_some());
    }

    #[test]
    fn subcommand_args_reads_the_embedding_provider_slot() {
        let args = parse_args_hermetic(&["--embedding-provider", "ollama"])
            .expect("embedding-provider parses");
        assert_eq!(
            subcommand_args(&args).provider_id.as_deref(),
            Some("ollama")
        );
    }

    #[test]
    fn subcommand_args_marks_an_explicit_max_chars() {
        let args = parse_args_hermetic(&["--max-chars", "4096"]).expect("max-chars parses");
        let sub = subcommand_args(&args);
        assert!(sub.max_chars_explicit);
        assert_eq!(sub.max_chars_value, Some(4096));
    }

    #[test]
    fn parse_provider_id_accepts_registry_shaped_ids() {
        assert_eq!(
            parse_provider_id("ollama-local").expect("valid"),
            "ollama-local"
        );
        assert_eq!(parse_provider_id(" openai ").expect("trimmed"), "openai");
        assert_eq!(parse_provider_id("a.b_c-1").expect("valid"), "a.b_c-1");
    }

    #[test]
    fn parse_provider_id_rejects_empty_and_illegal_characters() {
        assert!(parse_provider_id("   ").is_err());
        assert!(parse_provider_id("has space").is_err());
        assert!(parse_provider_id("slash/id").is_err());
    }
}
