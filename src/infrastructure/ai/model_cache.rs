//! Model cache management — Cache directory, validation, and lifecycle
//!
//! Handles caching of AI models with:
//! - Automatic cache directory creation
//! - SHA256 integrity validation
//! - Cache cleanup and lifecycle management
//! - Offline mode support
//!
//! # Cache Structure
//!
//! ```text
//! ~/.cache/rust-scraper/ai_models/
//! ├── model.onnx              # ONNX model file
//! ├── model.onnx.sha256       # SHA256 checksum
//! ├── tokenizer.json          # Tokenizer configuration
//! └── metadata.json           # Model metadata (version, download date)
//! ```
//!
//! # Design Decisions
//!
//! - **XDG cache convention** (`dirs` crate): Follows OS cache directory standards
//! - **SHA256 validation**: Ensures cache integrity after download
//! - **Async file operations** (`async-tokio-fs`): Non-blocking I/O

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, BufReader};
use tracing::{debug, info};

use crate::error::SemanticError;

// Re-import from cache_config module
use super::cache_config::{CacheConfig, DEFAULT_MODEL_REPO, DEFAULT_MODEL_SHA256};

/// Model cache manager
///
/// Handles cache lifecycle: creation, validation, cleanup.
pub struct ModelCache {
    config: CacheConfig,
}

impl ModelCache {
    /// Create a new model cache manager
    #[must_use]
    pub fn new(config: CacheConfig) -> Self {
        Self { config }
    }

    /// Get the cache directory path
    #[must_use]
    pub fn cache_dir(&self) -> &Path {
        &self.config.cache_dir
    }

    /// Ensure cache directory exists
    pub async fn ensure_cache_dir(&self) -> Result<(), SemanticError> {
        fs::create_dir_all(&self.config.cache_dir)
            .await
            .map_err(|e| {
                SemanticError::ModelLoad(std::io::Error::other(format!(
                    "Failed to create cache directory: {}",
                    e,
                )))
            })?;

        debug!(path = ?self.config.cache_dir, "Cache directory ready");
        Ok(())
    }

    /// Check if a model file exists in cache
    #[must_use]
    pub fn is_model_cached(&self, model_file: &str) -> bool {
        self.config.cache_dir.join(model_file).exists()
    }

    /// Validate model file integrity using SHA256
    pub async fn validate_model(
        &self,
        model_file: &str,
        expected_sha256: Option<&str>,
    ) -> Result<(), SemanticError> {
        let model_path = self.config.cache_dir.join(model_file);

        if !model_path.exists() {
            return Err(SemanticError::ModelLoad(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Model file not found: {:?}", model_path),
            )));
        }

        let expected = expected_sha256
            .map(String::from)
            .or_else(|| self.config.expected_sha256.clone())
            .unwrap_or_else(|| DEFAULT_MODEL_SHA256.to_string());

        let file = File::open(&model_path).await.map_err(|e| {
            SemanticError::ModelLoad(std::io::Error::other(format!(
                "Failed to open model file: {}",
                e
            )))
        })?;

        let mut reader = BufReader::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 8192];

        loop {
            let bytes_read = reader.read(&mut buffer).await.map_err(|e| {
                SemanticError::ModelLoad(std::io::Error::other(format!(
                    "Failed to read model file: {}",
                    e
                )))
            })?;

            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        let actual_sha = format!("{:x}", hasher.finalize());

        if actual_sha != expected {
            return Err(SemanticError::CacheValidation {
                repo: DEFAULT_MODEL_REPO.to_string(),
                expected,
                actual: actual_sha,
            });
        }

        debug!(path = ?model_path, sha = %actual_sha, "Model validation passed");
        Ok(())
    }

    /// Check if cached model is stale
    pub async fn is_model_stale(&self, model_file: &str) -> Result<bool, SemanticError> {
        let Some(ttl_days) = self.config.cache_ttl_days else {
            return Ok(false);
        };

        let model_path = self.config.cache_dir.join(model_file);
        if !model_path.exists() {
            return Ok(true);
        }

        let metadata = fs::metadata(&model_path).await.map_err(|e| {
            SemanticError::ModelLoad(std::io::Error::other(format!(
                "Failed to read model metadata: {}",
                e
            )))
        })?;

        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let age = modified.elapsed().unwrap_or(Duration::ZERO);
        let ttl = Duration::from_secs(ttl_days * 24 * 60 * 60);

        Ok(age > ttl)
    }

    /// Get the full path to a cached model file
    #[must_use]
    pub fn model_path(&self, model_file: &str) -> PathBuf {
        self.config.cache_dir.join(model_file)
    }

    /// Clear the entire cache
    pub async fn clear(&self) -> Result<(), SemanticError> {
        if !self.config.cache_dir.exists() {
            return Ok(());
        }

        fs::remove_dir_all(&self.config.cache_dir)
            .await
            .map_err(|e| {
                SemanticError::ModelLoad(std::io::Error::other(format!(
                    "Failed to clear cache: {}",
                    e
                )))
            })?;

        info!(path = ?self.config.cache_dir, "Cache cleared");
        Ok(())
    }

    /// Get cache size in bytes
    pub async fn size(&self) -> Result<u64, SemanticError> {
        if !self.config.cache_dir.exists() {
            return Ok(0);
        }

        let mut total_size = 0u64;
        let mut entries = fs::read_dir(&self.config.cache_dir).await.map_err(|e| {
            SemanticError::ModelLoad(std::io::Error::other(format!(
                "Failed to read cache directory: {}",
                e,
            )))
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            SemanticError::ModelLoad(std::io::Error::other(format!(
                "Failed to read directory entry: {}",
                e
            )))
        })? {
            let metadata = entry.metadata().await.map_err(|e| {
                SemanticError::ModelLoad(std::io::Error::other(format!(
                    "Failed to read entry metadata: {}",
                    e
                )))
            })?;

            if metadata.is_file() {
                total_size += metadata.len();
            }
        }

        Ok(total_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ensure_cache_dir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir.path().join("test_cache");

        let config = CacheConfig::new().with_cache_dir(cache_dir.clone());
        let cache = ModelCache::new(config);

        assert!(!cache_dir.exists());
        cache.ensure_cache_dir().await.unwrap();
        assert!(cache_dir.exists());
    }

    #[tokio::test]
    async fn test_is_model_cached() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir.path().join("test_cache");

        let config = CacheConfig::new().with_cache_dir(cache_dir.clone());
        let cache = ModelCache::new(config);

        assert!(!cache.is_model_cached("model.onnx"));

        fs::create_dir_all(&cache_dir).await.unwrap();
        File::create(cache_dir.join("model.onnx")).await.unwrap();

        assert!(cache.is_model_cached("model.onnx"));
    }

    #[tokio::test]
    async fn test_model_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir.path().join("test_cache");

        let config = CacheConfig::new().with_cache_dir(cache_dir.clone());
        let cache = ModelCache::new(config);

        let model_path = cache.model_path("model.onnx");
        assert_eq!(model_path, cache_dir.join("model.onnx"));
    }
}
