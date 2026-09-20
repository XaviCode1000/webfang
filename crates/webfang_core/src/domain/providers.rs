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
    pub embedding_dim: Option<usize>,
}

impl ProviderConfig {
    /// Whether this provider declares `capability`.
    #[must_use]
    pub fn has_capability(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
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
        self.config
            .providers
            .iter()
            .find(|p| p.has_capability(Capability::Completion))
            .ok_or_else(|| RegistryError::ProviderNotFound {
                provider_id: "<default>".to_string(),
            })
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
}
