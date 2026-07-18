//! Cache configuration for AI model management
//!
//! Controls cache behavior, validation, and lifecycle settings.

use std::path::PathBuf;

/// Default cache directory path
///
/// Uses XDG cache convention: `~/.cache/webfang/ai_models/`
const CACHE_DIR_NAME: &str = "webfang";
const AI_MODELS_SUBDIR: &str = "ai_models";

/// Default model repository (IBM Granite ONNX-converted version)
pub const DEFAULT_MODEL_REPO: &str = "ibm-granite/granite-embedding-97m-multilingual-r2";

/// Default model file name (in onnx/ subdirectory on HuggingFace)
pub const DEFAULT_MODEL_FILE: &str = "onnx/model.onnx";

/// Expected SHA256 for Granite-97M ONNX model
pub const DEFAULT_MODEL_SHA256: &str =
    "68e592b160673d30250824c1116bc6ab33f70efb22b97c9e1d7ce1e69c1c9d70";

/// Fallback model repository (Granite-311M for higher precision)
pub const DEFAULT_FALLBACK_MODEL_REPO: &str = "ibm-granite/granite-embedding-311m-multilingual-r2";

/// Fallback model file name
pub const DEFAULT_FALLBACK_MODEL_FILE: &str = "onnx/model.onnx";

/// Expected SHA256 for Granite-311M ONNX model
/// Verified via HuggingFace API: https://huggingface.co/ibm-granite/granite-embedding-311m-multilingual-r2
pub const DEFAULT_FALLBACK_MODEL_SHA256: &str =
    "49158cc56a6ae40b0ab0634706d7e524c33e105f358a6fb7ed4f63c5e1187fbe";

/// Environment variable for model selection
pub const MODEL_SELECTION_ENV: &str = "AI_MODEL_ID";

/// AI model variants supported by the inference engine
///
/// Two-tier model architecture:
/// - `Granite97M` (default): 97M params, 384d native, ~120MB, fast
/// - `Granite311M`: 311M params, 768d native (Matryoshka→384d), ~350MB, higher quality
///
/// Both produce 384-dimensional embeddings for unified storage schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AiModel {
    /// IBM Granite-97M (384d native, ~120MB, default)
    #[default]
    Granite97M,
    /// IBM Granite-311M (768d native, Matryoshka-truncated to 384d, ~350MB)
    Granite311M,
}

impl AiModel {
    /// HuggingFace repository ID
    #[must_use]
    pub fn repo_id(&self) -> &'static str {
        match self {
            AiModel::Granite97M => DEFAULT_MODEL_REPO,
            AiModel::Granite311M => DEFAULT_FALLBACK_MODEL_REPO,
        }
    }

    /// Model file path within the repository
    #[must_use]
    pub fn model_file(&self) -> &'static str {
        match self {
            AiModel::Granite97M => DEFAULT_MODEL_FILE,
            AiModel::Granite311M => DEFAULT_FALLBACK_MODEL_FILE,
        }
    }

    /// Expected SHA256 hash for the ONNX model
    #[must_use]
    pub fn sha256(&self) -> &'static str {
        match self {
            AiModel::Granite97M => DEFAULT_MODEL_SHA256,
            AiModel::Granite311M => DEFAULT_FALLBACK_MODEL_SHA256,
        }
    }

    /// Native embedding dimension (before Matryoshka truncation)
    ///
    /// Granite-97M: 384 → no truncation needed
    /// Granite-311M: 768 → truncated to 384 via Matryoshka
    #[must_use]
    pub fn embedding_dim(&self) -> usize {
        match self {
            AiModel::Granite97M => 384,
            AiModel::Granite311M => 768,
        }
    }

    /// Output dimension after processing (always 384 for unified storage)
    #[must_use]
    pub fn output_dim(&self) -> usize {
        384 // Unified 384d across both tiers
    }

    /// Human-readable display name
    #[must_use]
    pub fn display_name(&self) -> &'static str {
        match self {
            AiModel::Granite97M => "granite-97m",
            AiModel::Granite311M => "granite-311m",
        }
    }

    /// Parse model ID from environment variable or CLI flag
    ///
    /// Valid values: `granite-97m` (default), `granite-311m`
    ///
    /// # Errors
    ///
    /// Returns `None` if the model ID is unrecognized.
    /// Callers should handle this gracefully, listing valid options.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "granite-97m" => Some(AiModel::Granite97M),
            "granite-311m" => Some(AiModel::Granite311M),
            _ => None,
        }
    }

    /// Resolve from environment variable, defaulting to Granite-97M
    #[must_use]
    pub fn from_env_or_default() -> Self {
        std::env::var(MODEL_SELECTION_ENV)
            .ok()
            .and_then(|s| AiModel::parse(&s))
            .unwrap_or(AiModel::Granite97M)
    }
}

impl std::str::FromStr for AiModel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        AiModel::parse(s).ok_or_else(|| {
            format!(
                "Unknown AI model '{}'. Valid values: granite-97m, granite-311m",
                s
            )
        })
    }
}

impl std::fmt::Display for AiModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

/// Cache configuration
///
/// Controls cache behavior and validation settings.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Cache directory path (default: ~/.cache/webfang/ai_models/)
    pub cache_dir: PathBuf,
    /// Enable SHA256 validation (default: true)
    pub validate_sha256: bool,
    /// Expected SHA256 hash (optional, uses default if None)
    pub expected_sha256: Option<String>,
    /// Enable offline mode (default: false)
    pub offline_mode: bool,
    /// Cache TTL in days (default: 30)
    pub cache_ttl_days: Option<u64>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            cache_dir: default_cache_dir(),
            validate_sha256: true,
            expected_sha256: None,
            offline_mode: false,
            cache_ttl_days: Some(30),
        }
    }
}

impl CacheConfig {
    /// Create a new cache configuration with default values
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set custom cache directory
    #[must_use]
    pub fn with_cache_dir(mut self, dir: PathBuf) -> Self {
        self.cache_dir = dir;
        self
    }

    /// Enable or disable SHA256 validation
    #[must_use]
    pub fn with_validation(mut self, validate: bool) -> Self {
        self.validate_sha256 = validate;
        self
    }

    /// Set expected SHA256 hash
    #[must_use]
    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.expected_sha256 = Some(sha256.into());
        self
    }

    /// Enable offline mode
    #[must_use]
    pub fn with_offline_mode(mut self, offline: bool) -> Self {
        self.offline_mode = offline;
        self
    }

    /// Set cache TTL
    #[must_use]
    pub fn with_ttl_days(mut self, days: Option<u64>) -> Self {
        self.cache_ttl_days = days;
        self
    }
}

/// Get the default cache directory path
///
/// Returns `~/.cache/webfang/ai_models/` on Linux/macOS
#[must_use]
pub fn default_cache_dir() -> PathBuf {
    if let Some(cache_base) = dirs::cache_dir() {
        cache_base.join(CACHE_DIR_NAME).join(AI_MODELS_SUBDIR)
    } else {
        PathBuf::from("./.cache/ai_models")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CacheConfig::default();
        assert!(config.validate_sha256);
        assert!(!config.offline_mode);
        assert_eq!(config.cache_ttl_days, Some(30));
        assert!(config.cache_dir.to_string_lossy().contains("ai_models"));
    }

    #[test]
    fn test_custom_config() {
        let config = CacheConfig::new()
            .with_validation(false)
            .with_offline_mode(true)
            .with_ttl_days(None);

        assert!(!config.validate_sha256);
        assert!(config.offline_mode);
        assert_eq!(config.cache_ttl_days, None);
    }

    #[test]
    fn test_default_cache_dir() {
        let dir = default_cache_dir();
        assert!(dir.to_string_lossy().contains("ai_models"));
    }

    // ========== AiModel tests ==========

    #[test]
    fn test_ai_model_default_is_granite_97m() {
        let model = AiModel::default();
        assert_eq!(model, AiModel::Granite97M);
    }

    #[test]
    fn test_ai_model_granite_97m_repo_id() {
        assert_eq!(
            AiModel::Granite97M.repo_id(),
            "ibm-granite/granite-embedding-97m-multilingual-r2"
        );
    }

    #[test]
    fn test_ai_model_granite_311m_repo_id() {
        assert_eq!(
            AiModel::Granite311M.repo_id(),
            "ibm-granite/granite-embedding-311m-multilingual-r2"
        );
    }

    #[test]
    fn test_ai_model_granite_97m_embedding_dim() {
        assert_eq!(AiModel::Granite97M.embedding_dim(), 384);
    }

    #[test]
    fn test_ai_model_granite_311m_embedding_dim() {
        assert_eq!(AiModel::Granite311M.embedding_dim(), 768);
    }

    #[test]
    fn test_ai_model_output_dim_is_always_384() {
        assert_eq!(AiModel::Granite97M.output_dim(), 384);
        assert_eq!(AiModel::Granite311M.output_dim(), 384);
    }

    #[test]
    fn test_ai_model_from_str_valid() {
        assert_eq!(AiModel::parse("granite-97m"), Some(AiModel::Granite97M));
        assert_eq!(AiModel::parse("granite-311m"), Some(AiModel::Granite311M));
        // Case-insensitive
        assert_eq!(AiModel::parse("GRANITE-97M"), Some(AiModel::Granite97M));
        // With trim
        assert_eq!(
            AiModel::parse("  granite-311m "),
            Some(AiModel::Granite311M)
        );
    }

    #[test]
    fn test_ai_model_from_str_invalid() {
        assert_eq!(AiModel::parse("unknown-model"), None);
        assert_eq!(AiModel::parse(""), None);
        assert_eq!(AiModel::parse("granite-100m"), None);
    }

    #[test]
    fn test_ai_model_display_name() {
        assert_eq!(AiModel::Granite97M.display_name(), "granite-97m");
        assert_eq!(AiModel::Granite311M.display_name(), "granite-311m");
        assert_eq!(AiModel::Granite97M.to_string(), "granite-97m");
    }

    #[test]
    fn test_ai_model_sha256_not_empty() {
        assert!(!AiModel::Granite97M.sha256().is_empty());
        assert!(!AiModel::Granite311M.sha256().is_empty());
        assert_eq!(AiModel::Granite97M.sha256().len(), 64);
        assert_eq!(AiModel::Granite311M.sha256().len(), 64);
    }

    #[test]
    fn test_ai_model_from_env_or_default() {
        // Without AI_MODEL_ID set, returns default Granite-97M.
        // The env var may or may not be set in the test environment,
        // so we use from_env_or_default which never fails.
        let model = AiModel::from_env_or_default();
        assert!(
            model == AiModel::Granite97M || model == AiModel::Granite311M,
            "from_env_or_default() should return a valid model, got {:?}",
            model
        );
    }
}
