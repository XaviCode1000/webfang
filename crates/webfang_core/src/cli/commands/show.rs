//! `webfang show` — dump the effective model profile and character budget the
//! operator has configured, with provenance per value (issue #1933, ADR-0004
//! step 4).
//!
//! Each numeric value that ships from the repo is marked `verified`; anything
//! the operator overrode in their config drops the profile to
//! `operator-override` (the citation is kept for traceability). The budget
//! knobs report whether they came from the operator or from the spec default.
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
//! profile the configuration declares — it does **not** claim the run already
//! reads it.

use std::path::Path;

use crate::cli::args::SubcommandArgs;
use crate::cli::error::CliExit;
use crate::domain::model_profile::{
    ModelProfile, ModelProfileRegistry, ProfileProvenance, GRANITE_97M_PROFILE_ID,
};

/// Print the configured profile + budget.
///
/// The profile is named by `args.provider_id` when the operator gave one;
/// otherwise the repo default (`granite-97m`) is shown and labelled as the
/// default, not a choice the operator made.
///
/// The config itself is loaded with the diagnostic policy
/// [`crate::cli::config::load_config_defaults_or_diag`]: a config the run
/// could not load is a hard `ConfigError` naming the file, never a degraded
/// render about defaults the operator never wrote.
pub fn run(args: &SubcommandArgs, config_path: &Path) -> CliExit {
    let defaults = match crate::cli::config::load_config_defaults_or_diag(config_path) {
        Ok(defaults) => defaults,
        // The exact rendering the run uses for the identical failure
        // (`main.rs`: `Err(e) => CliExit::ConfigError(e.to_string())`), so
        // the diagnostic and the run can never disagree on this message.
        Err(e) => return CliExit::ConfigError(e.to_string()),
    };
    let registry = ModelProfileRegistry::new(defaults.model_profiles.clone());

    let profile_id = args
        .provider_id
        .as_deref()
        .unwrap_or(GRANITE_97M_PROFILE_ID);

    let profile = match registry.resolve(profile_id) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Perfil de modelo '{profile_id}' desconocido: {e}");
            return CliExit::ConfigError(format!("perfil de modelo '{profile_id}' desconocido"));
        },
    };

    println!("\n=== show: perfil de modelo ===\n");
    if args.provider_id.is_none() {
        println!("(sin perfil indicado — se muestra el default del repo: {profile_id})");
    }
    print_profile(&profile);
    print_budget(&profile, args);

    CliExit::Success
}

fn print_profile(profile: &ModelProfile) {
    let provenance_label = match profile.provenance {
        ProfileProvenance::RepoVerified => "repo-verified",
        ProfileProvenance::OperatorOverride => "operator-override",
    };

    println!("id: {}", profile.id);
    println!("provenance: {provenance_label}");
    println!("  citation: {}", profile.citation);
    println!("  schema_version: {}", profile.schema_version);
    println!("  dim (nativa): {}", profile.dim);
    println!("  matryoshka: {}", profile.matryoshka);
    println!("  output_dim (efectiva en vault): {}", profile.output_dim());
    match profile.batch_max {
        Some(batch) => println!("  batch_max: {batch}"),
        None => println!("  batch_max: no declarado por el repo"),
    }
    println!("  context: {} tokens", profile.context);
    println!("  chars_per_token: {}", profile.chars_per_token);
    match profile.query_prefix.as_deref() {
        Some(prefix) => println!("  query_prefix: {prefix:?}"),
        None => println!("  query_prefix: no declarado por el repo"),
    }
    match profile.passage_prefix.as_deref() {
        Some(prefix) => println!("  passage_prefix: {prefix:?}"),
        None => println!("  passage_prefix: no declarado por el repo"),
    }

    if profile.provenance == ProfileProvenance::OperatorOverride {
        println!("\nATENCIÓN: el operador sobrescribió la identidad auditada (dim / matryoshka).");
        println!("  El perfil ya no es repo-verified; la cita sigue siendo del repo por");
        println!("  rastreabilidad, y el valor declarado es el del operador.");
    }
}

fn print_budget(profile: &ModelProfile, args: &SubcommandArgs) {
    let derived = (profile.context as f64 * profile.chars_per_token as f64).round() as usize;

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
    println!("  budget derivado del perfil (contexto × chars_per_token): {derived}");
    println!("threshold: {threshold}");
    println!("  origen: {threshold_origin}");

    println!("\nNotas:");
    println!(
        "  - '{0}' es el perfil de modelo declarado en la configuración.",
        profile.id
    );
    println!("  - El techo del cleaner lo da --max-chars (o WEBFANG_MAX_CHARS); el número");
    println!("    del perfil (contexto × chars_per_token) es el presupuesto derivado.");
    println!(
        "  - Ningún valor viene de models.dev: los números salen del repo (profile.citation)."
    );
    println!("  - Consumir el perfil en el cleaner (budget real, dim de vault) es Tramo D");
    println!("    (pasos 2–3 del ADR-0004); todavía no está cableado al run.");
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::domain::model_profile::ModelProfilesConfig;

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

    #[test]
    fn show_defaults_to_granite_97m_when_no_profile_is_named() {
        let dir = temp_config("");
        let args = subcmd(None, None, false, None);
        let exit = run(&args, &dir.path().join("config.toml"));
        assert!(matches!(exit, CliExit::Success));
    }

    #[test]
    fn show_rejects_an_unknown_profile_id() {
        let dir = temp_config("");
        let args = subcmd(Some("no-such-model"), None, false, None);
        let exit = run(&args, &dir.path().join("config.toml"));
        assert!(matches!(exit, CliExit::ConfigError(_)));
    }

    #[test]
    fn show_resolves_a_named_shipped_profile() {
        let dir = temp_config("");
        let args = subcmd(Some("granite-311m"), None, false, None);
        let exit = run(&args, &dir.path().join("config.toml"));
        assert!(matches!(exit, CliExit::Success));
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
        let profile = registry.resolve(GRANITE_97M_PROFILE_ID).expect("resolve");
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
