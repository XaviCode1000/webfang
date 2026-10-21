//! `webfang show` — dump the effective embeddings provider, the configured
//! budget, and the historical model-profile metadata (issue #2106, ADR-0004
//! step 4).
//!
//! The **provider** is the identity of the run: it is the endpoint a scrape
//! would dial, resolved through the very same path the run uses
//! ([`crate::cli::llm_wire::resolve_embedding_config`]). The budget knobs
//! report whether they came from the operator or from the spec default.
//!
//! The `[[model_profiles]]` layer is **historical metadata** and is printed
//! as such: it describes the LOCAL ONNX models that ADR-0004 step 6 retired,
//! and no run consumes it. Presenting `granite-97m` as "the profile of the
//! run" was the false-provenance defect #2106 closes.
//!
//! This subcommand performs **no network I/O**: it reads the user's config and
//! prints what will be used. Probing an endpoint is `connect` / `doctor`'s job.
//!
//! ## Scope: step 4 is configuration, not consumption
//!
//! ADR-0004 step 4 is *easy configuration* — the profiles plus the
//! `connect`/`doctor`/`show` surface that lets the operator see what a config
//! means. **Consuming** a profile (deriving the cleaner's chunk budget from
//! `context × chars_per_token`, pinning the vault dimension from `dim`) is
//! Tramo D, steps 2–3. Until that wiring lands, this output describes the
//! historical profile metadata — it does **not** claim the run reads it.

use std::path::Path;

use crate::cli::args::SubcommandArgs;
use crate::cli::error::CliExit;
use crate::domain::model_profile::ModelProfileRegistry;
use crate::domain::providers::{ProviderConfig, ProvidersConfig};

/// Print the effective provider, the budget and the historical profile
/// metadata.
///
/// The provider is resolved exactly as the scrape path resolves it, so the
/// identity `show` prints is the identity the run uses. An id the operator
/// named (`--embedding-provider` / `WEBFANG_EMBEDDING_PROVIDER`) that does
/// not resolve to an embedding-capable provider is a hard `ConfigError`,
/// never a silent fallback to the config-order default.
///
/// The config itself is loaded with the diagnostic policy
/// [`crate::cli::config::load_diagnostic_defaults_or_exit`]: a config the run
/// could not load is a hard `ConfigError` naming the file, never a degraded
/// render about defaults the operator never wrote.
pub fn run(args: &SubcommandArgs, config_path: &Path) -> CliExit {
    let defaults = match crate::cli::config::load_diagnostic_defaults_or_exit(config_path) {
        Ok(defaults) => defaults,
        Err(exit) => return exit,
    };

    println!("\n=== show: provider de embeddings efectivo ===\n");
    match resolve_effective_provider(args, &defaults) {
        Ok(Some(provider)) => {
            crate::cli::commands::blocks::print_provider_block(
                &provider,
                args.provider_id.is_some(),
            );
        },
        Ok(None) => print_no_provider(),
        // The `ConfigError` payload is rendered by the binary boundary
        // (`Error: {msg}` on stderr, exit 78) — never duplicated here.
        Err(exit) => return exit,
    }

    print_budget(args);

    let registry = ModelProfileRegistry::new(defaults.model_profiles.clone());
    println!("\n=== show: metadata histórica de perfiles de modelo (ADR-0004 paso 4) ===\n");
    crate::cli::commands::blocks::print_historical_profiles(&registry);

    print_notes();

    CliExit::Success
}

/// Resolve the embeddings provider the run would use.
///
/// The SAME resolution the scrape path performs — flag (or
/// `WEBFANG_EMBEDDING_PROVIDER`) first, then the first embedding-capable
/// provider in config order. `Ok(None)` means no embedding provider is
/// configured at all; `Err` is the hard configuration error the operator must
/// see (exit 78).
fn resolve_effective_provider(
    args: &SubcommandArgs,
    defaults: &crate::cli::config::ConfigDefaults,
) -> Result<Option<ProviderConfig>, CliExit> {
    let providers = ProvidersConfig {
        providers: defaults.providers.clone(),
    };
    let opts = crate::application::crawl_options::CrawlOptions {
        embedding_provider: args.provider_id.clone(),
        ..crate::application::crawl_options::CrawlOptions::default()
    };
    crate::cli::llm_wire::resolve_embedding_config(&opts, &providers)
}

/// No embedding provider is configured: say so, and say what a run would do.
fn print_no_provider() {
    println!("(sin provider de embeddings configurado)");
    println!();
    println!("Ninguna corrida puede limpiar semánticamente: `--clean-ai` exige un provider");
    println!("remoto de embeddings (`--embedding-provider <id>` o WEBFANG_EMBEDDING_PROVIDER,");
    println!("con el provider declarado en la config) y falla con ConfigError (exit 78) sin él.");
}

/// The budget the cleaner is CONFIGURED with, with its provenance.
///
/// Only the knobs the run really honours live here: `--max-chars` (or
/// `WEBFANG_MAX_CHARS`) and the relevance threshold. The budget derived from
/// a model profile is historical metadata and is printed with the profiles.
fn print_budget(args: &SubcommandArgs) {
    let max_chars = args
        .max_chars_value
        .unwrap_or(crate::domain::options_spec::ai::DEFAULT_MAX_CHARS);
    let max_chars_origin = if args.max_chars_explicit {
        "operatorio (--max-chars o WEBFANG_MAX_CHARS)"
    } else {
        "default del spec"
    };

    let threshold = args.threshold_value.unwrap_or(0.3);
    let threshold_origin = if args.threshold_explicit {
        "operatorio (--threshold)"
    } else {
        "default del spec"
    };

    println!("\n=== show: budget efectivo ===\n");
    println!("max_chars: {max_chars}");
    println!("  origen: {max_chars_origin}");
    println!("threshold: {threshold}");
    println!("  origen: {threshold_origin}");
}

/// Closing notes: what the numbers are, and what they are not.
fn print_notes() {
    println!("\nNotas:");
    println!("  - La sección de metadata histórica es lo único que queda de los perfiles de");
    println!("    modelo: ninguna corrida los consume (ADR-0004 paso 6 retiró el runtime local).");
    println!(
        "  - Ningún valor viene de models.dev: los números salen del repo (profile.citation)."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::domain::model_profile::{ModelProfilesConfig, ProfileProvenance};

    fn subcmd(
        provider_id: Option<&str>,
        max_chars: Option<usize>,
        max_chars_explicit: bool,
        threshold: Option<f32>,
    ) -> SubcommandArgs {
        SubcommandArgs {
            provider_id: provider_id.map(String::from),
            max_chars_explicit,
            max_chars_value: max_chars,
            // Tests that pass a `Some(threshold)` are modelling the operator
            // having typed the flag; the render-level provenance assertions
            // live in the operator-override test below.
            threshold_explicit: threshold.is_some(),
            threshold_value: threshold,
        }
    }

    fn temp_config(contents: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("config.toml"), contents).expect("write config");
        dir
    }

    /// Render `show` for `provider_id` against the seeded `dir`.
    fn run_show(dir: &tempfile::TempDir, provider_id: Option<&str>) -> CliExit {
        let args = subcmd(provider_id, None, false, None);
        run(&args, &dir.path().join("config.toml"))
    }

    /// No configured provider: `show` still succeeds, and the identity of the
    /// run is "no provider" — never a shipped model profile (#2106).
    ///
    /// The rendered stdout is pinned by the integration test
    /// (`tests/show_provider_identity_test.rs`); here the contract is the
    /// resolution the render is built from.
    #[test]
    fn show_without_configured_providers_still_succeeds() {
        let dir = temp_config("");
        let exit = run_show(&dir, None);
        assert!(matches!(exit, CliExit::Success));

        let defaults = crate::cli::config::ConfigDefaults::load(&dir.path().join("config.toml"));
        let resolved = resolve_effective_provider(&subcmd(None, None, false, None), &defaults);
        assert!(
            matches!(resolved, Ok(None)),
            "without a configured provider the run identity is empty — \
             granite-97m must never stand in for it"
        );
    }

    /// An id the operator named that resolves to nothing is a hard
    /// configuration error naming the id (#2106).
    #[test]
    fn show_rejects_an_unknown_provider_id() {
        let dir = temp_config("");
        let exit = run_show(&dir, Some("no-such-model"));
        assert!(matches!(exit, CliExit::ConfigError(_)));
    }

    /// A shipped PROFILE id is not a provider id: `--embedding-provider` names
    /// the provider namespace, and the model-profile namespace lives in the
    /// `[[model_profiles]]` config block (#2106).
    #[test]
    fn show_rejects_a_shipped_profile_id_used_as_a_provider_id() {
        let dir = temp_config("");
        let exit = run_show(&dir, Some("granite-311m"));
        match exit {
            CliExit::ConfigError(msg) => {
                assert!(
                    msg.contains("embedding"),
                    "the error must name the missing embedding capability, got: {msg}"
                );
                assert!(
                    !msg.contains("perfil de modelo"),
                    "the error must not talk about a model profile — the id names a \
                     provider, got: {msg}"
                );
            },
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    #[test]
    fn show_honors_an_operator_override_of_the_audited_identity() {
        // Override `dim` for the shipped `granite-97m`: the profile must drop
        // to `operator-override` and carry the overridden value.
        let dir = temp_config(
            r#"
[[model_profiles.profiles]]
id = "granite-97m"
dim = 512
"#,
        );
        let defaults = crate::cli::config::ConfigDefaults::load(&dir.path().join("config.toml"));
        let registry = ModelProfileRegistry::new(defaults.model_profiles);
        let profile = registry
            .resolve(crate::domain::model_profile::GRANITE_97M_PROFILE_ID)
            .expect("resolve");
        assert_eq!(profile.provenance, ProfileProvenance::OperatorOverride);
        assert_eq!(profile.dim, 512);
        assert!(!profile.verified);
    }

    #[test]
    fn show_accepts_an_empty_model_profiles_block() {
        let defaults = crate::cli::config::ConfigDefaults::default();
        assert_eq!(defaults.model_profiles, ModelProfilesConfig::default());
    }
}
