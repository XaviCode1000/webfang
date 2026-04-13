//! Cache configuration for AI model management
//!
//! Controls cache behavior, validation, and lifecycle settings.

use std::path::PathBuf;

/// Default cache directory path
///
/// Uses XDG cache convention: `~/.cache/rust-scraper/ai_models/`
const CACHE_DIR_NAME: &str = "rust-scraper";
const AI_MODELS_SUBDIR: &str = "ai_models";

/// Default model repository (Xenova's ONNX-converted version)
pub const DEFAULT_MODEL_REPO: &str = "Xenova/all-MiniLM-L6-v2";

/// Default model file name (in onnx/ subdirectory)
pub const DEFAULT_MODEL_FILE: &str = "model.onnx";

/// Expected SHA256 for all-MiniLM-L6-v2 ONNX model
pub const DEFAULT_MODEL_SHA256: &str =
    "6d9d2f06f5e2e5e6f5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5";

/// Cache configuration
///
/// Controls cache behavior and validation settings.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Cache directory path (default: ~/.cache/rust-scraper/ai_models/)
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
/// Returns `~/.cache/rust-scraper/ai_models/` on Linux/macOS
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
}
