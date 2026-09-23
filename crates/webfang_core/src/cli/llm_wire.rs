//! LLM provider wiring for service binaries (`ai-providers-design.md` §8b).
//!
//! The Container is permissive — `llm_port()` is `None` without a provider —
//! but every binary that exposes `--extract-with-llm` **must** validate the
//! port at startup and fail loud (exit 78) when the provider is missing or
//! its credential cannot resolve. This module is that startup step: resolve
//! the configured provider from the config file and construct it once, so a
//! credential error is fatal at boot instead of surfacing on the first page.
//!
//! The embedding slot (#1462) mirrors the completion path: resolve the
//! selected (or default embedding-capable) provider, construct the remote
//! adapter only for `OpenAiCompatible`, and verify the served dimension with
//! a startup probe. `LocalOnnx` — explicit or default — serves the local
//! pool adapter with no network I/O.

use std::sync::Arc;

use crate::application::crawl_options::CrawlOptions;
use crate::domain::providers::{
    Capability, ProviderConfig, ProviderKind, ProviderRegistry, ProvidersConfig, RegistryError,
};
use crate::infrastructure::llm::provider::OpenAiCompatibleProvider;
use crate::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter;
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

/// Resolve the embedding slot to a remote provider config (#1462).
///
/// Sync and probe-free: returns `Ok(None)` when the local pool serves —
/// no `--embedding-provider` and no (or a `LocalOnnx`) default, or an
/// explicit `LocalOnnx` id — and `Ok(Some(config))` when an
/// `OpenAiCompatible` provider must be constructed + probed by
/// [`build_embedding_provider`].
///
/// # Errors
///
/// [`CliExit::ConfigError`] (exit 78) when `--embedding-provider` names an
/// id that is unknown or lacks the `embedding` capability.
pub fn resolve_embedding_config(
    opts: &CrawlOptions,
    providers: &ProvidersConfig,
) -> Result<Option<ProviderConfig>, CliExit> {
    let registry = ProviderRegistry::new(providers.clone());
    let config = match opts.embedding_provider.as_deref() {
        Some(id) => registry.resolve(id, Capability::Embedding).map_err(|e| {
            CliExit::ConfigError(format!(
                "'--embedding-provider' requiere un provider con capacidad `embedding` \
                 en el archivo de configuración: {e}"
            ))
        })?,
        None => match registry.resolve_default_embedding() {
            Ok(config) => config,
            Err(RegistryError::ProviderNotFound { .. }) => return Ok(None),
            // Unreachable today (the default lookup only reports absence),
            // but a future registry variant must fail loud, never local.
            Err(other) => {
                return Err(CliExit::ConfigError(format!(
                    "no se pudo resolver el provider de embeddings por defecto: {other}"
                )));
            },
        },
    };
    match config.kind {
        ProviderKind::LocalOnnx => Ok(None),
        ProviderKind::OpenAiCompatible => Ok(Some(config.clone())),
    }
}

/// Build the remote embedding adapter for the resolved slot (#1462).
///
/// `offline` is explicit (not read from `opts`) so the MCP lazy path —
/// which owns no `CrawlOptions` — shares this preflight.
pub async fn build_embedding_provider(
    opts: &CrawlOptions,
    providers: &ProvidersConfig,
    offline: bool,
) -> Result<Option<Arc<RemoteEmbeddingAdapter>>, CliExit> {
    let Some(config) = resolve_embedding_config(opts, providers)? else {
        return Ok(None);
    };
    if offline {
        return Err(CliExit::ConfigError(
            "modo offline con provider remoto de embeddings: sin red no hay vectores \
             (quitá `--offline` o usá el modelo local)"
                .to_string(),
        ));
    }
    let adapter =
        RemoteEmbeddingAdapter::new(config).map_err(|e| CliExit::ConfigError(e.to_string()))?;
    adapter.probe_dim().await.map_err(|e| {
        CliExit::ConfigError(format!(
            "el provider de embeddings '{}' no pasó la verificación inicial: {e}",
            adapter.id()
        ))
    })?;
    tracing::info!(
        provider_id = %adapter.id(),
        dim = adapter.embedding_dim(),
        "remote embedding provider ready"
    );
    Ok(Some(Arc::new(adapter)))
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
            allow_loopback: false,
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

    // #1462 (U4): embedding preflight mirrors build_llm_provider — slot
    // resolution is sync and probe-free; only a resolved remote reaches the
    // async constructor + startup probe.
    //
    // `CliExit` carries no `Display` (exit codes render at the binary
    // boundary), so message assertions extract the `ConfigError` payload.

    fn config_message(err: &CliExit) -> &str {
        match err {
            CliExit::ConfigError(msg) => msg,
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    fn opts_with_embedding(provider_id: Option<&str>) -> CrawlOptions {
        CrawlOptions {
            embedding_provider: provider_id.map(str::to_string),
            ..Default::default()
        }
    }

    /// Embedding-slot twin of `provider`: same shape, but the provider
    /// provider declares `Embedding` and names the embedding test model —
    /// struct-update keeps the two constructors from drifting (#1462).
    fn emb_provider(id: &str, kind: ProviderKind, auth: AuthSource) -> ProviderConfig {
        ProviderConfig {
            capabilities: vec![Capability::Embedding],
            model: Some("nomic-embed-text".to_string()),
            ..provider(id, kind, auth)
        }
    }

    /// Single remote-ollama registry for the explicit-slot tests (#1462):
    /// the resolve-remote, unknown-id and missing-capability cases only
    /// need one remote embedding provider present — never three copies of
    /// its construction.
    fn single_ollama_embedding_registry() -> ProvidersConfig {
        ProvidersConfig {
            providers: vec![emb_provider(
                "ollama",
                ProviderKind::OpenAiCompatible,
                AuthSource::Env {
                    var: "WEBFANG_TEST_EMB_UNUSED".to_string(),
                },
            )],
        }
    }

    #[test]
    fn embedding_absent_without_any_embedding_provider_returns_none() {
        let opts = opts_with_embedding(None);
        let out = resolve_embedding_config(&opts, &ProvidersConfig::default())
            .expect("no embedding provider means local-only");
        assert!(out.is_none());
    }

    #[test]
    fn embedding_absent_resolves_first_embedding_provider_without_probing() {
        let opts = opts_with_embedding(None);
        let providers = ProvidersConfig {
            providers: vec![
                emb_provider(
                    "local-embeddings",
                    ProviderKind::LocalOnnx,
                    AuthSource::Env {
                        var: "WEBFANG_TEST_EMB_UNUSED".to_string(),
                    },
                ),
                emb_provider(
                    "ollama",
                    ProviderKind::OpenAiCompatible,
                    AuthSource::Env {
                        var: "WEBFANG_TEST_EMB_UNUSED".to_string(),
                    },
                ),
            ],
        };
        // First config-order embedding provider is local → local pool serves.
        let out = resolve_embedding_config(&opts, &providers).expect("resolves");
        assert!(out.is_none());
    }

    #[test]
    fn embedding_absent_with_remote_default_resolves_remote_config() {
        let opts = opts_with_embedding(None);
        let providers = single_ollama_embedding_registry();
        let out = resolve_embedding_config(&opts, &providers).expect("resolves");
        assert_eq!(out.expect("remote config").id, "ollama");
    }

    #[test]
    fn embedding_explicit_unknown_id_is_config_error() {
        let opts = opts_with_embedding(Some("missing"));
        let providers = single_ollama_embedding_registry();
        let err = match resolve_embedding_config(&opts, &providers) {
            Err(e) => e,
            Ok(_) => panic!("unknown id must fail"),
        };
        assert!(matches!(err, CliExit::ConfigError(_)));
        assert!(
            config_message(&err).contains("--embedding-provider"),
            "{err:?}"
        );
    }

    #[test]
    fn embedding_explicit_without_capability_is_config_error() {
        let opts = opts_with_embedding(Some("openai"));
        let providers = ProvidersConfig {
            providers: vec![provider(
                "openai",
                ProviderKind::OpenAiCompatible,
                AuthSource::Env {
                    var: "WEBFANG_TEST_EMB_UNUSED".to_string(),
                },
            )],
        };
        let err = match resolve_embedding_config(&opts, &providers) {
            Err(e) => e,
            Ok(_) => panic!("completion-only provider must fail the embedding slot"),
        };
        assert!(matches!(err, CliExit::ConfigError(_)));
    }

    #[test]
    fn embedding_explicit_local_onnx_returns_none() {
        let opts = opts_with_embedding(Some("local-embeddings"));
        let providers = ProvidersConfig {
            providers: vec![emb_provider(
                "local-embeddings",
                ProviderKind::LocalOnnx,
                AuthSource::Env {
                    var: "WEBFANG_TEST_EMB_UNUSED".to_string(),
                },
            )],
        };
        let out = resolve_embedding_config(&opts, &providers).expect("local resolves");
        assert!(out.is_none(), "explicit local means the pool serves");
    }

    // Async preflight: the resolved remote reaches the constructor + probe.
    // (Loopback-literal wiremock dials bypass the custom resolver, so the
    // guarded production client needs no env hatch here.)

    fn remote_providers_for(
        server: &wiremock::MockServer,
        modifier: impl FnOnce(&mut ProviderConfig),
    ) -> ProvidersConfig {
        let mut cfg = emb_provider(
            "ollama",
            ProviderKind::OpenAiCompatible,
            AuthSource::Env {
                var: "WEBFANG_TEST_EMB_OK".to_string(),
            },
        );
        cfg.base_url = url::Url::parse(&server.uri()).expect("mock uri parses");
        cfg.allow_loopback = true;
        modifier(&mut cfg);
        ProvidersConfig {
            providers: vec![cfg],
        }
    }

    async fn mount_probe(server: &wiremock::MockServer, body: &str) -> wiremock::MockGuard {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, ResponseTemplate};
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount_as_scoped(server)
            .await
    }

    const PROBE_SINGLE_8D: &str = r#"{"data": [{"index": 0,
        "embedding": [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]}]}"#;

    #[tokio::test]
    async fn embedding_offline_plus_remote_fails_fast_without_dial() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        // No mock mounted: any dial would 404 and count as a request.
        let providers = remote_providers_for(&server, |_| {});
        let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_EMB_OK", "test-key")]);

        let err = match build_embedding_provider(&opts_with_embedding(None), &providers, true).await
        {
            Err(e) => e,
            Ok(_) => panic!("offline + remote must fail fast"),
        };
        assert!(matches!(err, CliExit::ConfigError(_)));
        assert!(config_message(&err).contains("offline"), "{err:?}");
        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            0,
            "offline fast-fail must never dial"
        );
    }

    #[tokio::test]
    async fn embedding_remote_unresolvable_credential_is_config_error() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        // Hermetic: a leaked value would let construction proceed to the
        // probe (nextest isolates, plain cargo test does not).
        let _clean = webfang_test_utils::EnvGuard::clean(&["WEBFANG_TEST_EMB_MISSING_VAR"]);
        let mut providers = remote_providers_for(&server, |_| {});
        providers.providers[0].auth = AuthSource::Env {
            var: "WEBFANG_TEST_EMB_MISSING_VAR".to_string(),
        };

        let err =
            match build_embedding_provider(&opts_with_embedding(None), &providers, false).await {
                Err(e) => e,
                Ok(_) => panic!("missing credential must fail at startup"),
            };
        assert!(matches!(err, CliExit::ConfigError(_)));
        assert!(config_message(&err).contains("ollama"), "{err:?}");
        assert_eq!(
            server.received_requests().await.unwrap_or_default().len(),
            0,
            "credential failure precedes any dial"
        );
    }

    #[tokio::test]
    async fn embedding_remote_probes_and_serves_adopted_dim() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let _mock = mount_probe(&server, PROBE_SINGLE_8D).await;
        let providers = remote_providers_for(&server, |_| {});
        let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_EMB_OK", "test-key")]);

        let out =
            match build_embedding_provider(&opts_with_embedding(Some("ollama")), &providers, false)
                .await
            {
                Ok(o) => o,
                Err(e) => panic!("probe must pass: {e:?}"),
            };
        let adapter = out.expect("remote adapter built");
        assert_eq!(adapter.id(), "ollama");
        assert_eq!(adapter.embedding_dim(), 8);
    }

    #[tokio::test]
    async fn embedding_remote_pin_mismatch_is_config_error() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let _mock = mount_probe(&server, PROBE_SINGLE_8D).await;
        let providers = remote_providers_for(&server, |cfg| cfg.embedding_dim = Some(7));
        let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_EMB_OK", "test-key")]);

        let err =
            match build_embedding_provider(&opts_with_embedding(None), &providers, false).await {
                Err(e) => e,
                Ok(_) => panic!("pin mismatch must fail"),
            };
        assert!(matches!(err, CliExit::ConfigError(_)));
        let rendered = config_message(&err);
        assert!(
            rendered.contains('7') && rendered.contains('8'),
            "mismatch names declared and actual dims, got: {rendered}"
        );
    }

    #[tokio::test]
    async fn embedding_remote_missing_model_is_config_error() {
        use wiremock::MockServer;
        let server = MockServer::start().await;
        let providers = remote_providers_for(&server, |cfg| cfg.model = None);
        let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_EMB_OK", "test-key")]);

        let err =
            match build_embedding_provider(&opts_with_embedding(None), &providers, false).await {
                Err(e) => e,
                Ok(_) => panic!("missing model must fail"),
            };
        assert!(matches!(err, CliExit::ConfigError(_)));
    }
}
