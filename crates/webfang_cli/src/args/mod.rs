//! D1 provenance-capture spike (stabilization-config-normalization).
//!
//! Temporary work-unit module: proves that `clap::ArgMatches::value_source(id)`
//! resolves every contested arg id against the derive-generated `Args`, whose
//! sub-structs enter the command via `#[command(flatten)]`. Per design.md D1,
//! each `#[arg]` keeps its own unique id inside `ArgMatches`, so flattened
//! groups must NOT break per-arg lookup.
//!
//! This module is absorbed into Phase 5 rewiring; delete after `ArgSources`
//! moves to its permanent home.

use clap::parser::ValueSource;
use clap::{CommandFactory, FromArgMatches};
use webfang_core::Args;

/// Contested arg ids probed by the spike — one per flattened derive group
/// (`crawler:`, `export:`, `obsidian:`, `tui:`) plus the top-level positional.
const SPIKE_CONTESTED_IDS: [&str; 6] = [
    "url",
    "max_pages",
    "format",
    "vault",
    "interactive",
    "sitemap_url",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Build matches for the given argv exactly like `parse_args()` does.
    fn matches_for(argv: &[&str]) -> clap::ArgMatches {
        Args::command()
            .try_get_matches_from(argv)
            .expect("spike argv must parse")
    }

    /// Every contested id resolves against the flattened derive ids — no
    /// unknown-argument panic and no silent `None` for an *explicitly set*
    /// flag from ANY flattened group.
    #[test]
    fn value_source_resolves_ids_across_flattened_groups() {
        let argv = [
            "webfang",
            "--url",
            "https://example.com",
            "--max-pages",
            "10",
            "--format",
            "json",
            "--vault",
            "/tmp/vault",
            "--interactive",
            "--sitemap-url",
            "https://example.com/sitemap.xml",
        ];
        let m = matches_for(&argv);
        // Rebuild typed Args from the same matches — the single-pass pattern.
        let args = Args::from_arg_matches(&m).expect("matches must rebuild Args");
        assert_eq!(args.crawler.max_pages, 10);
        assert_eq!(args.export.format, webfang_core::OutputFormat::Json);

        for id in SPIKE_CONTESTED_IDS {
            let src = m.value_source(id);
            assert_eq!(
                src,
                Some(ValueSource::CommandLine),
                "explicitly-set id `{id}` must report CommandLine"
            );
        }
    }

    /// An explicit `--max-pages 10` reports `ValueSource::CommandLine` even
    /// though 10 equals the struct default — this is the whole point of D1.
    #[test]
    fn explicit_max_pages_equal_to_default_reports_command_line() {
        let m = matches_for(&["webfang", "--max-pages", "10"]);
        assert_eq!(
            m.value_source("max_pages"),
            Some(ValueSource::CommandLine),
            "default-equal explicit flag must still carry CLI provenance"
        );
    }

    /// An unset contested arg does NOT claim command-line provenance.
    #[test]
    fn unset_max_pages_does_not_report_command_line() {
        let m = matches_for(&["webfang"]);
        let src = m.value_source("max_pages");
        assert_ne!(
            src,
            Some(ValueSource::CommandLine),
            "unset flag must never look explicitly provided"
        );
    }

    /// Env-only provenance: an env-backed arg provided ONLY through its
    /// `WEBFANG_*` variable reports `ValueSource::EnvVariable`. (When flag and
    /// env are both present clap reports CommandLine — CLI outranks env.)
    /// nextest runs every test in its own process, so env mutation here is
    /// hermetic.
    #[test]
    fn env_sourced_value_reports_env_variable() {
        std::env::set_var("WEBFANG_DELAY_MS", "42");
        let m = matches_for(&["webfang"]);
        assert_eq!(
            m.value_source("delay_ms"),
            Some(ValueSource::EnvVariable),
            "env-provided value must report EnvVariable"
        );
        std::env::remove_var("WEBFANG_DELAY_MS");
    }

    /// `ArgSources::capture` classifies contested ids into explicit-Cli /
    /// explicit-Environment / absent — the exact map the pipeline consumes.
    /// (Phase-1 prototype API; re-homed onto `ConfigSource` in Phase 3.)
    #[test]
    fn capture_classifies_contested_sources() {
        let m = matches_for(&["webfang", "--max-pages", "10", "--use-sitemap"]);
        let sources = crate::ArgSources::capture(&m);
        assert!(sources.is_cli("max_pages"), "explicit flag captures as Cli");
        assert!(sources.is_cli("use_sitemap"), "flag captures as Cli");
        assert!(!sources.is_cli("selector"), "unset arg is not Cli");
        assert!(!sources.is_cli("url"), "absent positional is not Cli");

        std::env::set_var("WEBFANG_MAX_PAGES", "7");
        let m2 = matches_for(&["webfang"]);
        let sources2 = crate::ArgSources::capture(&m2);
        assert!(
            sources2.is_env("max_pages"),
            "env var captures as Environment"
        );
        assert!(!sources2.is_env("selector"));
        std::env::remove_var("WEBFANG_MAX_PAGES");
    }
}
