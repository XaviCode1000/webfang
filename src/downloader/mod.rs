//! Asset Download Module
//!
//! Handles downloading of images and documents from URLs.

use std::path::{Path, PathBuf};

use crate::error::{Result, ScraperError};
use reqwest::Client;
use sha2::{Digest, Sha256};

/// Result of a successful download
#[derive(Debug)]
pub struct DownloadedAsset {
    /// Original URL
    pub url: String,
    /// Local file path where asset was saved
    pub local_path: PathBuf,
    /// MIME type detected
    pub mime_type: Option<String>,
    /// File size in bytes
    pub size: u64,
}

/// Download configuration
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// Output directory for downloaded files
    pub output_dir: PathBuf,
    /// Subdirectory for images
    pub images_dir: String,
    /// Subdirectory for documents
    pub documents_dir: String,
    /// Maximum file size in bytes (default: 50MB)
    pub max_file_size: u64,
    /// Timeout for each download in seconds
    pub timeout_secs: u64,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("./downloads"),
            images_dir: "images".to_string(),
            documents_dir: "documents".to_string(),
            max_file_size: 50 * 1024 * 1024, // 50MB
            timeout_secs: 30,
        }
    }
}

/// Asset downloader
pub struct Downloader {
    client: Client,
    config: DownloadConfig,
}

impl Downloader {
    /// Create a new downloader with configuration
    pub fn new(config: DownloadConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .build()
            .map_err(|e| ScraperError::Config(format!("Failed to build HTTP client: {}", e)))?;

        Ok(Self { client, config })
    }

    /// Download a single asset
    pub async fn download(&self, url: &str) -> Result<DownloadedAsset> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(ScraperError::Network)?;

        let content_length = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);

        // Get MIME type before consuming response
        let mime_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        // Check file size limit
        if content_length > self.config.max_file_size {
            return Err(ScraperError::download(format!(
                "File too large: {} bytes (max: {} bytes)",
                content_length, self.config.max_file_size
            )));
        }

        let bytes = response.bytes().await.map_err(ScraperError::Network)?;

        let asset_type = crate::detector::detect_from_url(url);
        let subdir = if asset_type.is_image() {
            &self.config.images_dir
        } else {
            &self.config.documents_dir
        };

        // Generate filename from URL and hash
        let filename = self.generate_filename(url, &bytes);
        let local_path = self.config.output_dir.join(subdir).join(&filename);

        // Create directory if it doesn't exist
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write file
        std::fs::write(&local_path, &bytes).map_err(ScraperError::Io)?;

        tracing::info!("Downloaded: {} -> {:?}", url, local_path);

        Ok(DownloadedAsset {
            url: url.to_string(),
            local_path,
            mime_type,
            size: bytes.len() as u64,
        })
    }

    /// Download multiple assets
    pub async fn download_batch(&self, urls: &[String]) -> Vec<Result<DownloadedAsset>> {
        let mut results = Vec::new();

        for url in urls {
            let result = self.download(url).await;
            results.push(result);
        }

        results
    }

    /// Generate a unique filename from URL and content hash
    fn generate_filename(&self, url: &str, content: &[u8]) -> String {
        // Get extension from URL
        let extension = crate::detector::get_extension(url).unwrap_or_else(|| "bin".into());

        // Create hash from content
        let mut hasher = Sha256::new();
        hasher.update(content);
        let hash = format!("{:x}", hasher.finalize());

        // Use first 12 characters of hash
        format!("{}.{}", &hash[..12], extension)
    }
}

/// Simple async download without creating a Downloader instance
pub async fn quick_download(url: &str, output_dir: &Path) -> Result<DownloadedAsset> {
    let config = DownloadConfig {
        output_dir: output_dir.to_path_buf(),
        ..Default::default()
    };

    let downloader = Downloader::new(config)?;
    downloader.download(url).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_downloader_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let downloader = Downloader::new(config);
        assert!(downloader.is_ok());
    }

    #[test]
    fn test_generate_filename() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let downloader = Downloader::new(config).unwrap();

        let filename =
            downloader.generate_filename("https://example.com/image.png", b"test content");
        assert!(filename.ends_with(".png"));
        assert_eq!(filename.len(), 16); // 12 hash + . + 3 extension
    }
}
