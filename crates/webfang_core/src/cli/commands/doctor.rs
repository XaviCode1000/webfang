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

use crate::cli::error::CliExit;
use crate::cli::llm_wire;
use crate::domain::model_profile::{ModelProfileRegistry, GRANITE_97M_PROFILE_ID};
use crate::domain::network_policy::NetworkPolicyMode;
use crate::domain::options_spec::ai::{DEFAULT_CHARS_PER_TOKEN, DEFAULT_MAX_CHARS};
use crate::domain::providers::{Capability, ProviderConfig, ProviderRegistry, ProvidersConfig};

/// Run the diagnosis against the providers declared in `config_path`.
pub async fn run(config_path: &Path) -> CliExit {
    let defaults = crate::cli::config::ConfigDefaults::load(config_path);
    let providers = defaults.providers;
    let registry = ProviderRegistry::new(ProvidersConfig {
        providers: providers.clone(),
    });

    match registry.resolve_default_embedding() {
        Ok(provider) => diagnose_provider(&provider.clone(), &providers).await,
        Err(_) => diagnose_no_usable_provider(&providers),
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
async fn diagnose_provider(provider: &ProviderConfig, all: &[ProviderConfig]) -> CliExit {
    println!("\n=== doctor: ruta de embeddings ===\n");
    print_provider_block(provider);
    print_profile_and_budget();

    // 1. Credential — a hard failure blocks the run before any network hop.
    if let Err(exit) = check_credential(provider) {
        return exit;
    }

    // 2. Reachability — the real preflight, same as the scrape path.
    diagnose_reachability(provider, all).await
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
async fn diagnose_reachability(provider: &ProviderConfig, _all: &[ProviderConfig]) -> CliExit {
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

/// The profile / budget block: what the cleaner would use for this run.
fn print_profile_and_budget() {
    let registry = ModelProfileRegistry::new(Default::default());
    let context = match registry.resolve(GRANITE_97M_PROFILE_ID) {
        Ok(profile) => profile.context,
        // The shipped profiles are repo constants; a miss here is a build bug,
        // not operator input. Fall back to the spec context so doctor still
        // prints a coherent budget rather than panicking.
        Err(_) => 32_768,
    };

    println!("\nBudget del cleaner:");
    println!("  --max-chars (o WEBFANG_MAX_CHARS): {DEFAULT_MAX_CHARS} por defecto");
    println!("  chars_per_token (default del perfil): {DEFAULT_CHARS_PER_TOKEN}");
    println!("  contexto del perfil (default repo): {context} tokens");
    let derived = (context as f64 * DEFAULT_CHARS_PER_TOKEN as f64).round() as usize;
    println!("  budget derivado (contexto × chars_per_token): {derived} caracteres");
    println!("  umbral de relevancia: 0.3 (default; sin flag explícita todavía)\n");
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
}
