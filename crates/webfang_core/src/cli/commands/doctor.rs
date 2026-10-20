//! `webfang doctor` — diagnose the full embedding path: config, credential,
//! network reachability, and budget.
//!
//! The three diagnostics the issue asks to keep distinguishable (#1933):
//! 1. **No provider configured** — no embedding-capable provider exists at all.
//! 2. **Credential not resolvable** — the provider declares a credential source
//!    that cannot be satisfied (env var missing, keyring miss, unreadable
//!    encrypted file).
//! 3. **Provider configured but unreachable** — resolves and authenticates, but
//!    the endpoint cannot be probed (DNS, connection, status, parse).
//!
//! Each prints a human-readable block and exits non-zero only when the path is
//! broken: `ConfigError` (78) for config/credential, `NetworkError` (69) for
//! reachability. A fully healthy path exits 0 with a summary.
//!
//! # What "reachable" means
//!
//! Reachability is measured by the SAME preflight the scrape path uses
//! ([`llm_wire::build_embedding_provider`]): build the real adapter and run its
//! startup `probe_dim`. A provider that passes here is one that will start.

use std::path::Path;

use crate::cli::args::SubcommandArgs;
use crate::cli::error::CliExit;
use crate::cli::llm_wire;
use crate::domain::model_profile::{
    ModelProfileRegistry, ModelProfilesConfig, GRANITE_97M_PROFILE_ID,
};
use crate::domain::network_policy::NetworkPolicyMode;
use crate::domain::options_spec::ai::{DEFAULT_CHARS_PER_TOKEN, DEFAULT_MAX_CHARS};
use crate::domain::providers::{Capability, ProviderConfig, ProviderRegistry, ProvidersConfig};

/// Run the diagnosis against the providers declared in `config_path`.
///
/// `args` carries the operator's budget knobs and the explicit
/// `--embedding-provider` id, if any. When the id is present it is resolved by
/// id — the same provider the scrape path would dial — and an unknown id is a
/// hard `ConfigError`, never a silent fallback to the config-order default. A
/// diagnostic that reports OK about a provider the run never uses is the worst
/// failure mode for `doctor`.
///
/// The config itself is loaded with the diagnostic policy
/// [`crate::cli::config::load_config_defaults_or_diag`]: a config the run
/// could not load is a hard `ConfigError` naming the file, never a degraded
/// diagnosis about defaults the operator never wrote.
pub async fn run(config_path: &Path, args: &SubcommandArgs) -> CliExit {
    let defaults = match crate::cli::config::load_diagnostic_defaults_or_exit(config_path) {
        Ok(defaults) => defaults,
        Err(exit) => return exit,
    };
    let profiles = defaults.model_profiles.clone();
    let providers = defaults.providers;
    let registry = ProviderRegistry::new(ProvidersConfig {
        providers: providers.clone(),
    });

    match args.provider_id.as_deref() {
        Some(id) => match registry.resolve(id, Capability::Embedding) {
            Ok(provider) => diagnose_provider(&provider.clone(), args, &profiles).await,
            Err(e) => {
                println!("\n=== doctor: ruta de embeddings ===\n");
                println!("Estado: provider '{id}' no resoluble");
                println!("  {e}");
                CliExit::ConfigError(format!(
                    "el provider de embeddings '{id}' no existe o no declara la capacidad `embedding`: {e}"
                ))
            },
        },
        None => match registry.resolve_default_embedding() {
            Ok(provider) => diagnose_provider(&provider.clone(), args, &profiles).await,
            Err(_) => diagnose_no_usable_provider(&providers),
        },
    }
}

/// No embedding-capable provider resolved. Distinguish "nothing configured"
/// from "something configured, but nothing that can serve embeddings".
fn diagnose_no_usable_provider(providers: &[ProviderConfig]) -> CliExit {
    println!("\n=== doctor: ruta de embeddings ===\n");

    if providers.is_empty() {
        println!("Estado: sin provider configurado");
        println!("  No hay ningún provider en el archivo de configuración.");
        println!("  Conectá uno con:");
        println!("\n    webfang connect <url del endpoint> --model <modelo>\n");
        println!("  O declaralo a mano bajo [[providers]].\n");
        return CliExit::ConfigError("no hay ningún provider de embeddings configurado".into());
    }

    let embedding_capable = providers
        .iter()
        .filter(|p| p.has_capability(Capability::Embedding))
        .count();
    if embedding_capable == 0 {
        println!("Estado: sin provider de embeddings");
        println!(
            "  Hay {} provider(s) en el archivo, pero ninguno declara la capacidad \"embedding\".",
            providers.len()
        );
        println!("  Añadí `capabilities = [\"embedding\"]` a uno de ellos.\n");
        return CliExit::ConfigError(
            "no hay ningún provider con capacidad `embedding` en la configuración".into(),
        );
    }

    // There ARE embedding providers, but none resolved as the default remote
    // slot — today that means none is `open_ai_compatible`.
    println!("Estado: hay providers de embeddings, pero ninguno remoto por defecto");
    println!("  El slot remoto exige `kind = \"open_ai_compatible\"`. Forzá uno con:");
    println!("\n    webfang --embedding-provider <id>\n");
    CliExit::ConfigError(
        "no hay un provider de embeddings remoto (open_ai_compatible) por defecto".into(),
    )
}

/// Diagnose a resolved provider: config, credential, reachability, budget.
async fn diagnose_provider(
    provider: &ProviderConfig,
    args: &SubcommandArgs,
    profiles: &ModelProfilesConfig,
) -> CliExit {
    println!("\n=== doctor: ruta de embeddings ===\n");
    print_provider_block(provider);
    print_profile_and_budget(args, profiles);

    // 1. Credential — a hard failure blocks the run before any network hop.
    if let Err(exit) = check_credential(provider) {
        return exit;
    }

    // 2. Reachability — the real preflight, same as the scrape path.
    diagnose_reachability(provider).await
}

/// Resolve the provider's credential and report the outcome.
///
/// A provider that declares a credential it cannot resolve is a broken path
/// (exit 78), reported before any network hop: sending a request with no key
/// would only mask the real cause behind a 401.
fn check_credential(provider: &ProviderConfig) -> Result<(), CliExit> {
    match provider.auth.resolve() {
        Ok(Some(_)) => {
            println!(
                "Credencial: resuelta (fuente: {}, valor no mostrado)",
                provider.auth.label()
            );
            Ok(())
        },
        Ok(None) => {
            println!("Credencial: ninguna (endpoint anónimo por diseño)");
            Ok(())
        },
        Err(e) => {
            println!(
                "Credencial: NO resoluble (fuente: {}) — {e}",
                provider.auth.label()
            );
            Err(CliExit::ConfigError(format!(
                "no se pudo resolver la credencial del provider '{}': {e}",
                provider.id
            )))
        },
    }
}

/// Build the real adapter and probe the endpoint, reporting reachability.
async fn diagnose_reachability(provider: &ProviderConfig) -> CliExit {
    let providers = ProvidersConfig {
        providers: vec![provider.clone()],
    };
    let opts = crate::application::crawl_options::CrawlOptions {
        embedding_provider: Some(provider.id.clone()),
        ..crate::application::crawl_options::CrawlOptions::default()
    };

    match llm_wire::build_embedding_provider(&opts, &providers, false).await {
        Ok(Some(conn)) => {
            println!("Conexión: OK — endpoint alcanzable");
            println!("  Dimensión servida: {}", conn.embedding_dim());
            CliExit::Success
        },
        Ok(None) => {
            // `Ok(None)` means the slot produced no remote endpoint at all.
            // It used to also mean "the local ONNX pool serves this provider",
            // and that reading is what made `doctor` exit 0 for a backend that
            // no longer existed (ADR-0004 paso 6). With the `local_onnx` kind
            // gone, an explicit id that resolves to nothing is a config error.
            println!("Conexión: el provider no se resolvió como endpoint remoto de embeddings");
            CliExit::ConfigError(format!(
                "el provider '{}' no es un endpoint remoto de embeddings",
                provider.id
            ))
        },
        Err(CliExit::ConfigError(msg)) => {
            println!("Conexión: NO alcanzable — {msg}");
            CliExit::NetworkError(format!(
                "el provider de embeddings '{}' no es alcanzable: {msg}",
                provider.id
            ))
        },
        Err(other) => other,
    }
}

/// The provider block: identity, capabilities, model, loopback, network policy.
fn print_provider_block(provider: &ProviderConfig) {
    println!("Provider configurado:");
    println!("  id: {}", provider.id);
    println!("  display_name: {}", provider.display_name);
    println!("  kind: {:?}", provider.kind);
    println!("  base_url: {}", provider.base_url);
    println!(
        "  capabilities: {:?}",
        provider
            .capabilities
            .iter()
            .map(|c| format!("{c:?}"))
            .collect::<Vec<_>>()
    );
    match provider.model.as_deref() {
        Some(model) => println!("  model: {model} (declarado en la config)"),
        None => println!(
            "  model: NO declarado — el endpoint remoto lo exige en cada request \
             (un provider remoto sin `model` falla al construir el adapter)"
        ),
    }
    println!(
        "  allow_loopback: {}",
        if provider.allow_loopback {
            "true (loopback permitido explícitamente)"
        } else {
            "false (loopback bloqueado por defecto)"
        }
    );

    let policy = &provider.network_policy;
    if matches!(policy.mode, NetworkPolicyMode::Allowlist) {
        let cidrs: Vec<String> = policy.allow_cidrs.iter().map(|c| c.to_string()).collect();
        let hosts: Vec<String> = policy.allow_hosts.clone();
        println!(
            "  network_policy: allowlist ({} CIDR(s), {} host(s))",
            cidrs.len(),
            hosts.len()
        );
        for c in &cidrs {
            println!("    allow_cidr: {c}");
        }
        for h in &hosts {
            println!("    allow_host: {h}");
        }
    } else {
        println!("  network_policy: restringida (solo endpoints públicos, con guard SSRF)");
    }
}

/// The profile / budget block: the budget the cleaner is CONFIGURED with.
///
/// It is **not** what a scrape run uses today. ADR-0004 schedules wiring
/// `[[model_profiles]]` into the semantic-cleaner run path for Tramo D (steps
/// 2–3); step 4 — this subcommand — only added the connect/doctor/show surface
/// and the config key. Claiming otherwise would hand the operator a number the
/// run would not honour, which is exactly the false-provenance defect this
/// block must not reintroduce.
///
/// Reads the same inputs `show` does — the operator's `[[model_profiles]]`
/// overrides and the argv budget knobs — so the two diagnostics cannot
/// contradict each other on identical input.
fn print_profile_and_budget(args: &SubcommandArgs, profiles: &ModelProfilesConfig) {
    let registry = ModelProfileRegistry::new(profiles.clone());
    let profile = match registry.resolve(GRANITE_97M_PROFILE_ID) {
        Ok(profile) => profile,
        // The shipped profiles are repo constants; a miss here is a build bug,
        // not operator input. Fall back to the spec context so doctor still
        // prints a coherent budget rather than panicking.
        Err(_) => {
            let max_chars = args.max_chars_value.unwrap_or(DEFAULT_MAX_CHARS);
            println!("\nBudget del cleaner:");
            println!("  --max-chars (o WEBFANG_MAX_CHARS): {max_chars}");
            println!("  chars_per_token (default del spec): {DEFAULT_CHARS_PER_TOKEN}");
            println!(
                "  umbral de relevancia: {}",
                args.threshold_value.unwrap_or(0.3)
            );
            println!("  (no se pudo resolver el perfil {GRANITE_97M_PROFILE_ID})");
            println!("  Nota: consumir el perfil en el cleaner es Tramo D del ADR-0004;");
            println!("  todavía no está cableado al run.\n");
            return;
        },
    };

    let max_chars = args.max_chars_value.unwrap_or(DEFAULT_MAX_CHARS);
    let max_chars_origin = if args.max_chars_explicit {
        "operatorio"
    } else {
        "default del spec"
    };
    let threshold_origin = if args.threshold_explicit {
        "operatorio"
    } else {
        "default del spec"
    };
    let context_origin = if profile.provenance
        == crate::domain::model_profile::ProfileProvenance::OperatorOverride
    {
        "operatorio"
    } else {
        "default del repo"
    };

    println!("\nBudget del cleaner:");
    println!("  --max-chars (o WEBFANG_MAX_CHARS): {max_chars} ({max_chars_origin})");
    println!(
        "  chars_per_token (perfil {}): {} ({context_origin})",
        profile.id, profile.chars_per_token
    );
    println!(
        "  contexto del perfil: {} tokens ({context_origin})",
        profile.context
    );
    let derived = (profile.context as f64 * f64::from(profile.chars_per_token)).round() as usize;
    println!("  budget derivado (contexto × chars_per_token): {derived} caracteres");
    println!(
        "  umbral de relevancia: {} ({threshold_origin})\n",
        args.threshold_value.unwrap_or(0.3)
    );
    println!("  Nota: consumir el perfil en el cleaner (budget real, dim de vault) es");
    println!("  Tramo D (pasos 2–3 del ADR-0004); todavía no está cableado al run.\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::domain::auth_source::AuthSource;
    use crate::domain::network_policy::NetworkPolicy;
    use crate::domain::providers::ProviderKind;

    fn provider(id: &str, auth: AuthSource, caps: Vec<Capability>) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            display_name: id.to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://api.example.test/v1").expect("url"),
            auth,
            capabilities: caps,
            model: Some("nomic-embed-text".to_string()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
        }
    }

    #[test]
    fn empty_config_is_a_config_error_with_instructions() {
        let exit = diagnose_no_usable_provider(&[]);
        assert!(matches!(exit, CliExit::ConfigError(_)));
    }

    #[test]
    fn a_provider_without_embedding_capability_is_a_config_error() {
        let p = provider("completion", AuthSource::None, vec![Capability::Completion]);
        let exit = diagnose_no_usable_provider(&[p]);
        assert!(matches!(exit, CliExit::ConfigError(_)));
    }

    #[test]
    fn an_unresolvable_credential_is_a_config_error() {
        // `Env` pointing at a variable that is not set: resolution fails.
        //
        // Hermetic on purpose: the contract under test is "the variable is
        // ABSENT, so resolution fails". A shell that exported
        // WEBFANG_TEST_DOCTOR_MISSING would otherwise turn this red with the
        // production code perfectly healthy. `EnvGuard::clean` removes the
        // variable under ENV_LOCK and restores the operator's value on drop,
        // so the test is identical in every environment (issue #926 class).
        let _guard = webfang_test_utils::EnvGuard::clean(&["WEBFANG_TEST_DOCTOR_MISSING"]);
        let p = provider(
            "needs-key",
            AuthSource::Env {
                var: "WEBFANG_TEST_DOCTOR_MISSING".to_string(),
            },
            vec![Capability::Embedding],
        );
        let exit = check_credential(&p);
        assert!(matches!(exit, Err(CliExit::ConfigError(_))));
    }

    #[test]
    fn an_anonymous_provider_resolves_no_credential_without_failing() {
        let p = provider("anon", AuthSource::None, vec![Capability::Embedding]);
        assert!(check_credential(&p).is_ok());
    }

    /// `--embedding-provider <unknown>` must be a hard error naming the id,
    /// never a silent fallback to the config-order default. Regression guard
    /// for the false-green where doctor probed a provider the run never used.
    #[tokio::test]
    async fn an_unknown_explicit_provider_id_is_a_config_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[providers]]\nid = \"known\"\ndisplay_name = \"Known\"\n\
             kind = \"open_ai_compatible\"\nbase_url = \"https://api.example.test/v1\"\n\
             auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
             model = \"nomic-embed-text\"\n",
        )
        .expect("write config");

        let exit = run(
            &path,
            &SubcommandArgs {
                provider_id: Some("does-not-exist".into()),
                max_chars_explicit: false,
                max_chars_value: None,
                threshold_explicit: false,
                threshold_value: None,
            },
        )
        .await;
        match exit {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("does-not-exist"),
                "the error must name the unknown id, got: {msg}"
            ),
            other => panic!("expected ConfigError naming the id, got {other:?}"),
        }
    }
}
