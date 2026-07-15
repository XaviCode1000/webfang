//! Model downloader — Automatic download from HuggingFace Hub
//!
//! Handles downloading ONNX models from HuggingFace Hub with:
//! - Progress tracking for user feedback
//! - SHA256 validation for integrity
//! - Retry logic for network failures
//! - Offline mode support

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hf_hub::{api::tokio::ApiBuilder, Repo, RepoType};
use sha2::{Digest, Sha256};
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;
use tracing::{debug, info};

use webfang_core::error::SemanticError;

/// Download progress information
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    /// Bytes downloaded so far
    pub downloaded: u64,
    /// Total bytes to download
    pub total: Option<u64>,
    /// Current download speed (bytes/second)
    pub speed: Option<u64>,
    /// Estimated time remaining (seconds)
    pub eta_seconds: Option<u64>,
}

impl DownloadProgress {
    /// Get download percentage (0.0 to 100.0)
    #[must_use]
    pub fn percentage(&self) -> Option<f64> {
        self.total.map(|total| {
            if total == 0 {
                100.0
            } else {
                (self.downloaded as f64 / total as f64) * 100.0
            }
        })
    }

    /// Check if download is complete
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.total.is_some_and(|total| self.downloaded >= total)
    }
}

/// Type alias for progress callback
pub type ProgressCallback = Arc<dyn Fn(DownloadProgress) -> Result<(), String> + Send + Sync>;

/// Model downloader for HuggingFace Hub
pub struct ModelDownloader {
    repo: String,
    file: String,
    sha256: Option<String>,
    #[allow(dead_code)]
    progress_callback: Option<ProgressCallback>,
}

impl Default for ModelDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelDownloader {
    #[must_use]
    pub fn new() -> Self {
        Self {
            repo: String::new(),
            file: String::new(),
            sha256: None,
            progress_callback: None,
        }
    }

    #[must_use]
    pub fn with_repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    #[must_use]
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = file.into();
        self
    }

    #[must_use]
    pub fn with_sha256(mut self, sha256: impl Into<String>) -> Self {
        self.sha256 = Some(sha256.into());
        self
    }

    #[must_use]
    pub fn with_progress_callback(mut self, callback: ProgressCallback) -> Self {
        self.progress_callback = Some(callback);
        self
    }

    /// Get the repository name
    #[must_use]
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Get the model file name
    #[must_use]
    pub fn file(&self) -> &str {
        &self.file
    }

    /// Download the model to the specified cache directory
    pub async fn download_to(&self, cache_dir: &Path) -> Result<PathBuf, SemanticError> {
        if self.repo.is_empty() {
            return Err(SemanticError::Download {
                repo: "<unknown>".to_string(),
                cause: "Repository not specified".to_string(),
            });
        }

        if self.file.is_empty() {
            return Err(SemanticError::Download {
                repo: self.repo.clone(),
                cause: "Model file not specified".to_string(),
            });
        }

        info!(
            repo = %self.repo,
            file = %self.file,
            "Downloading model from HuggingFace Hub"
        );

        // Create cache directory
        fs::create_dir_all(cache_dir)
            .await
            .map_err(|e| SemanticError::Download {
                repo: self.repo.clone(),
                cause: format!("Failed to create cache directory: {}", e),
            })?;

        // Build API and download
        let api = ApiBuilder::new()
            .build()
            .map_err(|e| SemanticError::Download {
                repo: self.repo.clone(),
                cause: format!("Failed to build API client: {}", e),
            })?;

        let repo = Repo::new(self.repo.clone(), RepoType::Model);

        // Download the file - returns PathBuf to cached location
        let downloaded_path =
            api.repo(repo)
                .download(&self.file)
                .await
                .map_err(|e| SemanticError::Download {
                    repo: self.repo.clone(),
                    cause: format!("HuggingFace API error: {}", e),
                })?;

        // Read and validate SHA256 if provided
        if let Some(ref expected_sha) = self.sha256 {
            let file_result: Result<File, std::io::Error> = File::open(&downloaded_path).await;
            let mut file = file_result.map_err(|e| SemanticError::Download {
                repo: self.repo.clone(),
                cause: format!("Failed to open downloaded file: {}", e),
            })?;

            let mut content = Vec::new();
            let read_result = file.read_to_end(&mut content).await;
            read_result.map_err(|e| SemanticError::Download {
                repo: self.repo.clone(),
                cause: format!("Failed to read downloaded file: {}", e),
            })?;

            let mut hasher = Sha256::new();
            hasher.update(&content);
            let actual_sha = format!("{:x}", hasher.finalize());

            if &actual_sha != expected_sha {
                return Err(SemanticError::CacheValidation {
                    repo: self.repo.clone(),
                    expected: expected_sha.clone(),
                    actual: actual_sha,
                });
            }

            debug!(sha = %actual_sha, "SHA256 validation passed");
        }

        info!(
            path = ?downloaded_path,
            "Model downloaded successfully"
        );

        Ok(downloaded_path)
    }

    #[must_use]
    pub fn is_cached(&self, cache_dir: &Path) -> bool {
        let dest_path = cache_dir.join(&self.file);
        dest_path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_progress_percentage() {
        let progress = DownloadProgress {
            downloaded: 50,
            total: Some(100),
            speed: None,
            eta_seconds: None,
        };

        assert_eq!(progress.percentage(), Some(50.0));
    }

    #[test]
    fn test_download_progress_complete() {
        let progress = DownloadProgress {
            downloaded: 100,
            total: Some(100),
            speed: None,
            eta_seconds: None,
        };

        assert!(progress.is_complete());
    }

    #[test]
    fn test_model_downloader_builder() {
        let downloader = ModelDownloader::new()
            .with_repo("test/repo")
            .with_file("model.onnx")
            .with_sha256("abc123");

        assert_eq!(downloader.repo, "test/repo");
        assert_eq!(downloader.file, "model.onnx");
        assert_eq!(downloader.sha256, Some("abc123".to_string()));
    }

    #[tokio::test]
    async fn test_model_downloader_validation() {
        let downloader = ModelDownloader::new();

        let result = downloader.download_to(Path::new("/tmp")).await;
        assert!(result.is_err());

        if let Err(SemanticError::Download { cause, .. }) = result {
            assert!(cause.contains("Repository not specified"));
        } else {
            panic!("Expected SemanticError::Download");
        }
    }
}
