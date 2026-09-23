//! Provider configuration and registry for multi-provider AI
//! (`docs/src/ai-providers-design.md` §4-5).
//!
//! Domain-only: no I/O, no HTTP, no credential resolution. The config is
//! declarative (serde) and the registry is a concrete lookup struct — the
//! design rejected a `ProviderPlugin` trait and dynamic discovery in favor of
//! a `match` on [`ProviderKind`] in the binary layer.
//!
//! Resolution is **by slot**: a caller names the capability it needs and gets
//! the matching config or a typed error. There is no `AnyProviderHandle`.

use url::Url;

use crate::domain::auth_source::AuthSource;

/// Kind of provider backend behind a [`ProviderConfig`].
///
/// The binary layer `match`es on this to select the concrete adapter; adding
/// a variant is a compile-time-visible change, never a runtime lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI-compatible chat/completions API (OpenAI, OpenRouter, Groq,
    /// Together, Ollama, FreeLLM…).
    OpenAiCompatible,
    /// Local ONNX inference pool (embeddings via `InferencePool`).
    LocalOnnx,
}

/// Capability a provider offers.
///
/// Used by the registry to reject a resolution that asks for something the
/// provider cannot do, instead of letting the failure surface at call time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Can produce embeddings for a text batch.
    Embedding,
    /// Can produce chat completions.
    Completion,
}

/// Error returned by [`ProviderRegistry`] lookups.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// No provider with the requested id exists in the config.
    #[error("provider '{provider_id}' no está configurado")]
    ProviderNotFound {
        /// The id that was requested.
        provider_id: String,
    },
    /// The provider exists but does not offer the requested capability.
    #[error("provider '{provider_id}' no ofrece la capacidad '{capability:?}'")]
    CapabilityMismatch {
        /// The id that was requested.
        provider_id: String,
        /// The capability that was missing.
        capability: Capability,
    },
}

/// Declarative configuration of one provider (the wizard writes this into the
/// config file).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderConfig {
    /// Unique provider identifier (e.g. `"openai"`, `"ollama-local"`).
    pub id: String,
    /// Human-readable name.
    pub display_name: String,
    /// Backend kind — selects the concrete adapter in the binary layer.
    pub kind: ProviderKind,
    /// OpenAI-compatible base URL (e.g. `https://api.openai.com/v1`).
    pub base_url: Url,
    /// Declared credential source. Resolved once at construction.
    pub auth: AuthSource,
    /// Capabilities this provider offers.
    pub capabilities: Vec<Capability>,
    /// Default chat/completions model for this provider.
    pub model: Option<String>,
    /// Explicit embedding dimension when it differs from the model default.
    ///
    /// `None` (absent) means "adopt the probed dim at startup". An explicit
    /// `null` is a deserialization error, never a silent adopt — a `null` in
    /// the config file is a typo, and typos must fail loudly (#1462).
    #[serde(default, deserialize_with = "deserialize_dim_no_null")]
    pub embedding_dim: Option<usize>,
    /// Permit loopback dials (exactly `127.0.0.1` / `::1`) for this provider.
    ///
    /// Config-file-only opt-in for self-hosted embedding endpoints on the
    /// same machine; there is deliberately NO CLI flag (flags leak into
    /// process lists, and this relaxes an SSRF protection). Absent means
    /// `false`; explicit `null` is a deserialization error. The flag is a
    /// per-client parameter threaded into the guard check — never registry
    /// state (#1462).
    #[serde(default)]
    pub allow_loopback: bool,
}

impl ProviderConfig {
    /// Whether this provider declares `capability`.
    #[must_use]
    pub fn has_capability(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// Rejection message for an explicit `null` dimension: `null` in the config
/// file is a typo and must fail loudly — shared by both null-rejecting
/// visitor methods so the two arms cannot drift (#1462).
const NULL_EMBEDDING_DIM_MSG: &str =
    "embedding_dim: null explícito no permitido; omití el campo para adoptar la dimensión remota";

/// Build the explicit-`null` rejection error for [`deserialize_dim_no_null`].
fn null_dim_error<E>() -> E
where
    E: serde::de::Error,
{
    E::custom(NULL_EMBEDDING_DIM_MSG)
}

/// Reject an explicit `null` for `embedding_dim` while keeping the
/// absent-means-`None` default.
///
/// With plain `#[serde(default)]`, `Option<usize>` maps explicit `null` to
/// `None` — a silent adopt. Routing through `deserialize_option` with a
/// visitor that errors on `None` keeps absent → `None` (serde never calls
/// the deserializer for missing fields) while failing loudly on `null`.
fn deserialize_dim_no_null<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct NoNull;

    impl<'de> serde::de::Visitor<'de> for NoNull {
        type Value = Option<usize>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an embedding dimension (explicit null is not allowed; omit the field)")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Err(null_dim_error())
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Err(null_dim_error())
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            <usize as serde::Deserialize<'de>>::deserialize(deserializer).map(Some)
        }
    }

    deserializer.deserialize_option(NoNull)
}

/// Top-level provider configuration — the `[[providers]]` array of the config
/// file.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    /// Every declared provider, in config order.
    pub providers: Vec<ProviderConfig>,
}

impl ProvidersConfig {
    /// Number of declared providers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Whether no provider is declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Look up a provider by id.
    #[must_use]
    pub fn find(&self, provider_id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == provider_id)
    }
}

/// Lookup struct over [`ProvidersConfig`].
///
/// Resolves **by slot**: the caller names the capability it needs and gets
/// either the provider config or a typed error — never a silent `None` that
/// resurfaces as a late failure.
#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    config: ProvidersConfig,
}

impl ProviderRegistry {
    /// Wrap the parsed config.
    #[must_use]
    pub fn new(config: ProvidersConfig) -> Self {
        Self { config }
    }

    /// The wrapped config.
    #[must_use]
    pub fn config(&self) -> &ProvidersConfig {
        &self.config
    }

    /// Resolve the config of a provider that declares `capability`.
    ///
    /// # Errors
    ///
    /// [`RegistryError::ProviderNotFound`] when the id is unknown,
    /// [`RegistryError::CapabilityMismatch`] when it lacks the capability.
    pub fn resolve(
        &self,
        provider_id: &str,
        capability: Capability,
    ) -> Result<&ProviderConfig, RegistryError> {
        let provider =
            self.config
                .find(provider_id)
                .ok_or_else(|| RegistryError::ProviderNotFound {
                    provider_id: provider_id.to_string(),
                })?;
        if !provider.has_capability(capability) {
            return Err(RegistryError::CapabilityMismatch {
                provider_id: provider_id.to_string(),
                capability,
            });
        }
        Ok(provider)
    }

    /// Resolve the default provider for `capability`: the first provider in
    /// config order that declares it.
    ///
    /// Single choke point behind [`Self::resolve_default_completion`] and
    /// [`Self::resolve_default_embedding`] so the two default slots cannot
    /// drift (#1462).
    ///
    /// # Errors
    ///
    /// [`RegistryError::ProviderNotFound`] (`<default>`) when no provider
    /// declares `capability`.
    fn resolve_default_by_capability(
        &self,
        capability: Capability,
    ) -> Result<&ProviderConfig, RegistryError> {
        self.config
            .providers
            .iter()
            .find(|p| p.has_capability(capability))
            .ok_or_else(|| RegistryError::ProviderNotFound {
                provider_id: "<default>".to_string(),
            })
    }

    /// Resolve the default completion provider: the first provider in config
    /// order that declares [`Capability::Completion`].
    ///
    /// This is the slot the CLI uses for `--extract-with-llm` when the
    /// operator does not name a provider explicitly.
    ///
    /// # Errors
    ///
    /// [`RegistryError::ProviderNotFound`] when no provider declares
    /// `Completion`.
    pub fn resolve_default_completion(&self) -> Result<&ProviderConfig, RegistryError> {
        self.resolve_default_by_capability(Capability::Completion)
    }

    /// Resolve the default embedding provider: the first provider in config
    /// order that declares [`Capability::Embedding`].
    ///
    /// This is the slot the embedding wiring uses when the operator does not
    /// name a provider explicitly (`--embedding-provider` absent). The kind
    /// match happens in the binary layer: `LocalOnnx` serves the local pool
    /// adapter (no probe), `OpenAiCompatible` builds the remote adapter.
    ///
    /// # Errors
    ///
    /// [`RegistryError::ProviderNotFound`] (`<default>`) when no provider
    /// declares `Embedding` — the caller falls back to local-only, never to
    /// a silent `None` that resurfaces as a late failure.
    pub fn resolve_default_embedding(&self) -> Result<&ProviderConfig, RegistryError> {
        self.resolve_default_by_capability(Capability::Embedding)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, capabilities: Vec<Capability>) -> ProviderConfig {
        ProviderConfig {
            id: id.to_string(),
            display_name: id.to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: Url::parse("https://api.example.com/v1").expect("url"),
            auth: AuthSource::Env {
                var: "WEBFANG_TEST_KEY".to_string(),
            },
            capabilities,
            model: Some("gpt-test".to_string()),
            embedding_dim: None,
            allow_loopback: false,
        }
    }

    #[test]
    fn resolve_returns_provider_when_capability_present() {
        let registry = ProviderRegistry::new(ProvidersConfig {
            providers: vec![provider("openai", vec![Capability::Completion])],
        });
        let resolved = registry
            .resolve("openai", Capability::Completion)
            .expect("completion capability declared");
        assert_eq!(resolved.id, "openai");
    }

    #[test]
    fn resolve_unknown_id_is_provider_not_found() {
        let registry = ProviderRegistry::new(ProvidersConfig::default());
        let err = registry
            .resolve("missing", Capability::Completion)
            .expect_err("unknown id must fail");
        assert!(matches!(err, RegistryError::ProviderNotFound { .. }));
    }

    #[test]
    fn resolve_missing_capability_is_mismatch() {
        let registry = ProviderRegistry::new(ProvidersConfig {
            providers: vec![provider("local", vec![Capability::Embedding])],
        });
        let err = registry
            .resolve("local", Capability::Completion)
            .expect_err("completion not declared");
        assert!(matches!(err, RegistryError::CapabilityMismatch { .. }));
    }

    #[test]
    fn default_completion_picks_first_completion_provider() {
        let registry = ProviderRegistry::new(ProvidersConfig {
            providers: vec![
                provider("local-embeddings", vec![Capability::Embedding]),
                provider("openai", vec![Capability::Completion]),
                provider("groq", vec![Capability::Completion]),
            ],
        });
        let resolved = registry
            .resolve_default_completion()
            .expect("one completion provider exists");
        assert_eq!(resolved.id, "openai");
    }

    #[test]
    fn default_completion_without_any_completion_provider_errors() {
        let registry = ProviderRegistry::new(ProvidersConfig {
            providers: vec![provider("local", vec![Capability::Embedding])],
        });
        let err = registry
            .resolve_default_completion()
            .expect_err("no completion provider configured");
        assert!(matches!(err, RegistryError::ProviderNotFound { .. }));
    }

    #[test]
    fn empty_config_reports_zero_len() {
        let config = ProvidersConfig::default();
        assert!(config.is_empty());
        assert_eq!(config.len(), 0);
    }

    /// Shared default-slot fixture (#1462): the first provider declaring
    /// `Embedding` is `local-embeddings` and the first declaring
    /// `Completion` is `openai` — one registry for both default-slot tests
    /// instead of two mirrored setups. The trailing dual-capability
    /// provider pins first-wins ordering on both slots at once.
    fn default_slot_registry() -> ProviderRegistry {
        ProviderRegistry::new(ProvidersConfig {
            providers: vec![
                provider("local-embeddings", vec![Capability::Embedding]),
                provider("openai", vec![Capability::Completion]),
                provider(
                    "ollama",
                    vec![Capability::Embedding, Capability::Completion],
                ),
            ],
        })
    }

    #[test]
    fn default_embedding_picks_first_embedding_provider() {
        let registry = default_slot_registry();
        let resolved = registry
            .resolve_default_embedding()
            .expect("one embedding provider exists");
        assert_eq!(resolved.id, "local-embeddings");
    }

    #[test]
    fn default_embedding_without_any_embedding_provider_is_not_found() {
        let registry = ProviderRegistry::new(ProvidersConfig {
            providers: vec![provider("openai", vec![Capability::Completion])],
        });
        let err = registry
            .resolve_default_embedding()
            .expect_err("no embedding provider configured");
        assert!(
            matches!(
                err,
                RegistryError::ProviderNotFound { ref provider_id }
                if provider_id == "<default>"
            ),
            "default slot must report ProviderNotFound(<default>), got: {err}"
        );
    }

    const PROVIDER_JSON_TEMPLATE: &str = r#"{
        "id": "ollama",
        "display_name": "Ollama",
        "kind": "open_ai_compatible",
        "base_url": "https://api.example.com/v1",
        "auth": {"source": "env", "var": "WEBFANG_TEST_KEY"},
        "capabilities": ["embedding"],
        "model": "nomic-embed"__DIM____LOOPBACK__
    }"#;

    fn provider_json(dim: &str, loopback: &str) -> String {
        PROVIDER_JSON_TEMPLATE
            .replace("__DIM__", dim)
            .replace("__LOOPBACK__", loopback)
    }

    #[test]
    fn allow_loopback_absent_deserializes_to_false() {
        let parsed: ProviderConfig =
            serde_json::from_str(&provider_json("", "")).expect("absent fields must parse");
        assert!(!parsed.allow_loopback);
        assert_eq!(parsed.embedding_dim, None);
    }

    #[test]
    fn allow_loopback_explicit_true_parses() {
        let parsed: ProviderConfig =
            serde_json::from_str(&provider_json("", ",\n        \"allow_loopback\": true"))
                .expect("explicit true must parse");
        assert!(parsed.allow_loopback);
    }

    #[test]
    fn allow_loopback_explicit_null_is_deserialization_error() {
        let err = serde_json::from_str::<ProviderConfig>(&provider_json(
            "",
            ",\n        \"allow_loopback\": null",
        ))
        .expect_err("explicit null must never silently default");
        assert!(
            err.to_string().contains("invalid type"),
            "null must fail as invalid type, got: {err}"
        );
    }

    #[test]
    fn embedding_dim_absent_deserializes_to_none() {
        let parsed: ProviderConfig =
            serde_json::from_str(&provider_json("", "")).expect("absent dim must parse");
        assert_eq!(parsed.embedding_dim, None);
    }

    #[test]
    fn embedding_dim_explicit_value_parses() {
        let parsed: ProviderConfig =
            serde_json::from_str(&provider_json(",\n        \"embedding_dim\": 1536", ""))
                .expect("explicit dim must parse");
        assert_eq!(parsed.embedding_dim, Some(1536));
    }

    #[test]
    fn embedding_dim_explicit_null_is_deserialization_error() {
        let err = serde_json::from_str::<ProviderConfig>(&provider_json(
            ",\n        \"embedding_dim\": null",
            "",
        ))
        .expect_err("explicit null must never silently adopt");
        assert!(
            err.to_string().contains("null"),
            "null must fail loudly, got: {err}"
        );
    }
}
