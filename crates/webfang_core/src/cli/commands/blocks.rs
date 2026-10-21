//! Shared render blocks for the `show` and `doctor` subcommands (issue #2106).
//!
//! Both commands answer the same operator question — *what does this run
//! actually use?* — and until #2106 they answered it with two different
//! blocks: `doctor` named the resolved embeddings provider, `show` named a
//! **model profile**. The profile layer (`[[model_profiles]]`) describes the
//! LOCAL ONNX models that ADR-0004 step 6 retired, and no run consumes it
//! any more, so presenting `granite-97m` as "the profile of the run" was a
//! false provenance claim (the same class of defect as #1933).
//!
//! The two blocks below are therefore shared verbatim, so `show` and `doctor`
//! can never disagree about the identity of the run:
//!
//! - [`print_provider_block`] — the provider the scrape path would dial.
//! - [`print_historical_profiles`] — the `[[model_profiles]]` metadata,
//!   explicitly labelled as history that no run consumes.
//!
//! The caller owns the section header: the two commands title the same
//! content differently.

use crate::domain::model_profile::{ModelProfileRegistry, ProfileProvenance};
use crate::domain::network_policy::NetworkPolicyMode;
use crate::domain::providers::ProviderConfig;

/// Print the shared provider block: the identity of the run.
///
/// `explicit` tells whether the operator NAMED this provider (through
/// `--embedding-provider` or `WEBFANG_EMBEDDING_PROVIDER`) or whether it is
/// the config-order default. The distinction is the same binary one the repo
/// already uses for `max_chars_explicit` / `threshold_explicit`: a value the
/// operator typed is never presented as a default.
pub fn print_provider_block(provider: &ProviderConfig, explicit: bool) {
    if explicit {
        println!("(origen: operatorio — --embedding-provider / WEBFANG_EMBEDDING_PROVIDER)");
    } else {
        println!("(origen: default de la config — primer provider con capacidad 'embedding')");
    }

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
    // `allow_loopback` is the PROVIDER's own network policy — "may this
    // provider's endpoint live on loopback?" — and NOT the `--offline`
    // guard's semantics, which reject every embeddings provider (loopback
    // included) with exit 78. The label says which of the two it is, so the
    // line cannot be read as "`--offline` allows loopback" (#2166).
    println!(
        "  allow_loopback: {}",
        if provider.allow_loopback {
            "true (política de red del provider: permite un endpoint en loopback)"
        } else {
            "false (política de red del provider: loopback bloqueado por defecto)"
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

    println!("  auth: {}", provider.auth.label());
    match provider.embedding_dim {
        Some(dim) => println!("  dim declarada: {dim}"),
        None => println!(
            "  dim declarada: no declarada en la config — el endpoint la fija en cada request"
        ),
    }
}

/// Print the shared historical block: the `[[model_profiles]]` metadata.
///
/// Every shipped profile is listed with its provenance and its cited
/// numbers. A profile the operator broke with an invalid override is
/// reported as one line and does NOT abort the listing: a broken historical
/// record must not hide the rest of the diagnostic.
pub fn print_historical_profiles(registry: &ModelProfileRegistry) {
    println!("Estos perfiles describen modelos LOCALES que la feature `ai` y el runtime ONNX");
    println!("retiraron en 3.0.0 (ADR-0004 paso 6). Ninguna corrida los consume: no son el");
    println!("perfil del run.");

    for id in registry.ids() {
        match registry.resolve(id) {
            Ok(profile) => print_historical_profile(&profile),
            Err(e) => println!("perfil: {id} (no se pudo resolver: {e})"),
        }
    }

    println!("Nota: consumir el perfil en el cleaner (budget real, dim de vault) es Tramo D");
    println!("(pasos 2–3 del ADR-0004); todavía no está cableado al run.");
}

/// Print one resolved historical profile.
fn print_historical_profile(profile: &crate::domain::model_profile::ModelProfile) {
    let provenance_label = match profile.provenance {
        ProfileProvenance::RepoVerified => "repo-verified",
        ProfileProvenance::OperatorOverride => "operator-override",
    };

    println!("perfil: {} ({provenance_label})", profile.id);
    println!("  citation: {}", profile.citation);
    println!("  schema_version: {}", profile.schema_version);
    println!("  dim (nativa): {}", profile.dim);
    println!("  output_dim (efectiva en vault): {}", profile.output_dim());
    println!("  context: {} tokens", profile.context);
    println!("  chars_per_token: {}", profile.chars_per_token);
    println!(
        "  budget derivado (contexto × chars_per_token): {} caracteres",
        profile.max_chars()
    );

    if profile.provenance == ProfileProvenance::OperatorOverride {
        println!("  ATENCIÓN: el operador sobrescribió la identidad auditada (dim / matryoshka).");
        println!("  El perfil ya no es repo-verified; la cita sigue siendo del repo por");
        println!("  rastreabilidad, y el valor declarado es el del operador.");
    }
}
