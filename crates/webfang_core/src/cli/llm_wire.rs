//! LLM provider wiring for service binaries (`ai-providers-design.md` §8b).
//!
//! The Container is permissive — `llm_port()` is `None` without a provider —
//! but every binary that exposes `--extract-with-llm` **must** validate the
//! port at startup and fail loud (exit 78) when the provider is missing or
//! its credential cannot resolve. This module is that startup step: resolve
//! the configured provider from the config file and construct it once, so a
//! credential error is fatal at boot instead of surfacing on the first page.

use std::sync::Arc;

use crate::application::crawl_options::CrawlOptions;
use crate::domain::providers::{
    Capability, ProviderConfig, ProviderKind, ProviderRegistry, ProvidersConfig, RegistryError,
};
use crate::infrastructure::llm::provider::OpenAiCompatibleProvider;
use crate::CliExit;

/// Build the LLM provider for `--extract-with-llm` runs.
///
/// Returns `Ok(None)` when extraction is not requested — the binary makes no
/// startup promise it does not need. With the flag set, resolves the selected
/// (or default completion-capable) provider and **constructs it here**:
/// `OpenAiCompatibleProvider::new` performs `AuthSource::resolve()` once in
/// its constructor, so a missing credential/identity file fails startup
/// immediately with a Spanish `ProviderInitError` — never on the first
/// invocation at runtime.
///
/// The returned provider is ready to inject into the Container with
/// `with_llm_port()` (or into any other consumer of `LlmPort`).
///
/// # Errors
///
/// [`CliExit::ConfigError`] (exit 78) when:
/// - no provider with the `completion` capability is configured,
/// - `--llm-provider` names an id that is unknown or lacks the capability,
/// - the provider kind has no CLI adapter yet (`LocalOnnx` is the embedding
///   pool, not an LLM completion source),
/// - the credential does not resolve at startup.
pub fn build_llm_provider(
    opts: &CrawlOptions,
    providers: &ProvidersConfig,
) -> Result<Option<Arc<OpenAiCompatibleProvider>>, CliExit> {
    if !opts.extract_with_llm {
        return Ok(None);
    }

    let registry = ProviderRegistry::new(providers.clone());
    let resolved: Result<&ProviderConfig, RegistryError> = match opts.llm_provider.as_deref() {
        Some(id) => registry.resolve(id, Capability::Completion),
        None => registry.resolve_default_completion(),
    };
    let config = resolved.map_err(|e| {
        CliExit::ConfigError(format!(
            "'--extract-with-llm' requiere un provider LLM configurado \
             con la capacidad `completion` en el archivo de configuración: {e}"
        ))
    })?;

    match config.kind {
        ProviderKind::OpenAiCompatible => {
            let provider = OpenAiCompatibleProvider::new(config.clone())
                .map_err(|e| CliExit::ConfigError(e.to_string()))?;
            tracing::info!(
                provider_id = %provider.id(),
                base_url = %provider.config().base_url,
                "LLM extraction provider ready"
            );
            Ok(Some(Arc::new(provider)))
        },
        ProviderKind::LocalOnnx => Err(CliExit::ConfigError(
            "'--extract-with-llm' requiere un provider remoto (kind = \
             `open_ai_compatible`); `local_onnx` es el pool de embeddings \
             local, no una fuente de completions"
                .to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::auth_source::AuthSource;

    fn opts_with_llm(extract: bool, provider_id: Option<&str>) -> CrawlOptions {
        CrawlOptions {
            extract_with_llm: extract,
            llm_provider: provider_id.map(str::to_string),
            ..Default::default()
        }
    }

    fn provider(id: &str, kind: ProviderKind, auth: AuthSource) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            display_name: id.to_string(),
            kind,
            base_url: url::Url::parse("https://api.example.com/v1").expect("url"),
            auth,
            capabilities: vec![Capability::Completion],
            model: Some("gpt-test".to_string()),
            embedding_dim: None,
        }
    }

    #[test]
    fn flag_absent_returns_none_without_looking_at_config() {
        let opts = opts_with_llm(false, None);
        let out =
            build_llm_provider(&opts, &ProvidersConfig::default()).expect("flag off must not fail");
        assert!(out.is_none());
    }

    #[test]
    fn flag_on_without_providers_is_config_error() {
        let opts = opts_with_llm(true, None);
        let err = match build_llm_provider(&opts, &ProvidersConfig::default()) {
            Err(e) => e,
            Ok(_) => panic!("no providers configured must fail"),
        };
        assert!(matches!(err, CliExit::ConfigError(_)));
    }

    #[test]
    fn flag_on_with_unknown_provider_id_is_config_error() {
        let opts = opts_with_llm(true, Some("missing"));
        let providers = ProvidersConfig {
            providers: vec![provider(
                "openai",
                ProviderKind::OpenAiCompatible,
                AuthSource::Env {
                    var: "WEBFANG_TEST_KEY".to_string(),
                },
            )],
        };
        let err = match build_llm_provider(&opts, &providers) {
            Err(e) => e,
            Ok(_) => panic!("unknown id must fail"),
        };
        assert!(matches!(err, CliExit::ConfigError(_)));
    }

    #[test]
    fn flag_on_with_local_onnx_kind_is_config_error() {
        let opts = opts_with_llm(true, Some("local"));
        let providers = ProvidersConfig {
            providers: vec![provider(
                "local",
                ProviderKind::LocalOnnx,
                AuthSource::Env {
                    var: "WEBFANG_TEST_KEY".to_string(),
                },
            )],
        };
        let err = match build_llm_provider(&opts, &providers) {
            Err(e) => e,
            Ok(_) => panic!("local kind must fail"),
        };
        match err {
            CliExit::ConfigError(msg) => assert!(msg.contains("local_onnx")),
            other => panic!("expected ConfigError, got {other:?}"),
        }
    }

    /// Startup resolution: an Env credential whose variable is unset fails at
    /// construction — proving resolve happens once at boot, not on first call.
    #[test]
    fn flag_on_with_unresolvable_env_credential_fails_at_startup() {
        webfang_test_utils::env_remove("WEBFANG_TEST_LLM_WIRE_MISSING");
        let opts = opts_with_llm(true, Some("openai"));
        let providers = ProvidersConfig {
            providers: vec![provider(
                "openai",
                ProviderKind::OpenAiCompatible,
                AuthSource::Env {
                    var: "WEBFANG_TEST_LLM_WIRE_MISSING".to_string(),
                },
            )],
        };
        let err = match build_llm_provider(&opts, &providers) {
            Err(e) => e,
            Ok(_) => panic!("missing credential must fail"),
        };
        assert!(matches!(err, CliExit::ConfigError(_)));
    }

    /// Happy path: configured provider + env credential resolves at startup
    /// and the provider reports its id/base_url. Set the var via the
    /// `EnvGuard` helper (clippy bans raw `std::env` mutation).
    #[test]
    fn flag_on_with_env_credential_builds_provider() {
        // EnvGuard (not env_set) so the key is restored post-test.
        let _guard =
            webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_LLM_WIRE_OK", "test-key")]);
        let opts = opts_with_llm(true, Some("openai"));
        let providers = ProvidersConfig {
            providers: vec![provider(
                "openai",
                ProviderKind::OpenAiCompatible,
                AuthSource::Env {
                    var: "WEBFANG_TEST_LLM_WIRE_OK".to_string(),
                },
            )],
        };
        let out = match build_llm_provider(&opts, &providers) {
            Ok(o) => o,
            Err(e) => panic!("credential resolves: {e:?}"),
        };
        let provider = out.expect("provider built");
        assert_eq!(provider.id(), "openai");
        assert_eq!(
            provider.config().base_url.as_str(),
            "https://api.example.com/v1"
        );
    }

    /// `--llm-provider` absent picks the first completion-capable provider.
    #[test]
    fn flag_on_without_provider_id_uses_default_completion_resolution() {
        let _guard =
            webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_LLM_WIRE_DEFAULT", "test-key")]);
        let opts = opts_with_llm(true, None);
        let providers = ProvidersConfig {
            providers: vec![provider(
                "openai",
                ProviderKind::OpenAiCompatible,
                AuthSource::Env {
                    var: "WEBFANG_TEST_LLM_WIRE_DEFAULT".to_string(),
                },
            )],
        };
        let out = match build_llm_provider(&opts, &providers) {
            Ok(o) => o,
            Err(e) => panic!("built: {e:?}"),
        };
        assert_eq!(out.expect("present").id(), "openai");
    }
}
