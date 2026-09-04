//! CLI command handlers
//!
//! Extracted from orchestrator.rs to reduce monolithism and improve testability.
//! Each command handler is isolated and can be tested independently.

use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;

/// Reject an explicit `--vault` combined with an explicit non-default output
/// directory (#762).
///
/// Since #762 an explicit `--vault` flag IS the output base: Markdown,
/// assets and the RAG export all land inside the vault. Passing a custom
/// `-o`/`WEBFANG_OUTPUT` alongside it is a contradiction — the binary would
/// not know which base wins. Fail fast with a usage error that names the two
/// valid invocations.
///
/// `--quick-save` is exempt on purpose: it keeps its historical contract
/// (#638) where the vault receives `_inbox` content while `-o` stays the RAG
/// export destination — an intentional, documented split, not a conflict.
///
/// `output_dir` only ever comes from the CLI or `WEBFANG_OUTPUT` (the config
/// file never sets it), so comparing against clap's `"output"` default is a
/// reliable explicitness probe.
pub fn validate_vault_output_conflict(opts: &CrawlOptions) -> Result<(), CliExit> {
    if opts.export.vault_is_explicit
        && !opts.export.quick_save
        && opts.export.output_dir != std::path::Path::new("output")
    {
        return Err(CliExit::UsageError(format!(
            "--vault y un directorio de output personalizado ({}) no pueden combinarse: \
             usá --vault <path> solo (redirige todo el output al vault) o -o <dir> sin --vault",
            opts.export.output_dir.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ===== #762 — validate_vault_output_conflict tests =====

    /// Canonical conflict trigger: explicit `--vault` plus a non-default `-o`,
    /// without `--quick-save`.
    fn explicit_vault_with_custom_output() -> CrawlOptions {
        let mut opts = CrawlOptions::default();
        opts.export.obsidian_vault = Some(PathBuf::from("/tmp/vault"));
        opts.export.vault_is_explicit = true;
        opts.export.output_dir = PathBuf::from("/tmp/custom");
        opts
    }

    /// Explicit `--vault` + custom `-o` is a contradiction (#762): the vault
    /// IS the output base, so a second custom base has no meaning. The
    /// preflight must reject it with a usage error naming the flag.
    #[test]
    fn conflict_rejects_explicit_vault_with_custom_output() {
        let opts = explicit_vault_with_custom_output();

        match validate_vault_output_conflict(&opts) {
            Err(CliExit::UsageError(msg)) => {
                assert!(
                    msg.contains("--vault"),
                    "usage error must name the --vault flag: {msg}"
                );
                assert!(
                    msg.contains("no pueden combinarse"),
                    "usage error must state the contradiction in Spanish: {msg}"
                );
            },
            other => panic!("expected CliExit::UsageError, got {other:?}"),
        }
    }

    /// `--quick-save` is exempt (#638 contract): markdown goes to the vault
    /// `_inbox` while `-o` keeps the RAG export — a documented split, not a
    /// conflict.
    #[test]
    fn quick_save_is_exempt_from_vault_output_conflict() {
        let mut opts = explicit_vault_with_custom_output();
        opts.export.quick_save = true;

        assert!(
            validate_vault_output_conflict(&opts).is_ok(),
            "--quick-save must remain compatible with --vault + -o"
        );
    }

    /// Leaving `-o` at clap's `"output"` default means "--vault <path> only":
    /// the vault redirects everything, which is exactly the valid invocation.
    #[test]
    fn explicit_vault_with_default_output_is_allowed() {
        let mut opts = explicit_vault_with_custom_output();
        opts.export.output_dir = PathBuf::from("output");

        assert!(
            validate_vault_output_conflict(&opts).is_ok(),
            "default -o must not trigger the conflict"
        );
    }

    /// A vault filled from `config.toml` (`vault_is_explicit = false`) does
    /// NOT redirect output, so a custom `-o` alongside it keeps its historical
    /// meaning and must never conflict.
    #[test]
    fn config_filled_vault_does_not_conflict_with_custom_output() {
        let mut opts = explicit_vault_with_custom_output();
        opts.export.vault_is_explicit = false;

        assert!(
            validate_vault_output_conflict(&opts).is_ok(),
            "config-filled vault must not conflict with custom -o"
        );
    }
}
