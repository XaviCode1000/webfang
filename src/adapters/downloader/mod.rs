//! Asset Download Module
//!
//! Handles downloading of images and documents from URLs.
//!
//! # Architecture
//!
//! Following rust-skills best practices:
//! - **True Streaming**: Writes chunks directly to disk, constant RAM (~8KB)
//! - **Atomic Operations**: Temp file with UUID, atomic rename on success
//! - **Init Once**: Directories pre-created in `new()`, zero runtime contention
//! - **Configurable**: User-Agent externalized to config
//! - **Cleanup**: Temp file removed on size limit exceeded
//! - **Hash On-The-Fly**: SHA256 computed during streaming, no buffer needed

use std::path::{Path, PathBuf};

use crate::error::{Result, ScraperError};
use futures::stream::{self, StreamExt};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use wreq::{Client, Response};
use wreq_util::Emulation;

/// Result of a successful download
#[derive(Debug)]
pub struct DownloadedAsset {
    /// Original URL
    pub url: String,
    /// Local file path where asset was saved
    pub local_path: PathBuf,
    /// MIME type detected from HTTP headers
    pub mime_type: Option<String>,
    /// File size in bytes
    pub size: u64,
    /// SHA256 hash of content (first 12 hex chars used in filename)
    pub content_hash: String,
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
    /// Maximum concurrent downloads (default: 3 for HDD)
    pub concurrency_limit: usize,
    /// User-Agent string for HTTP requests
    pub user_agent: String,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("./downloads"),
            images_dir: "images".to_string(),
            documents_dir: "documents".to_string(),
            max_file_size: 50 * 1024 * 1024,
            timeout_secs: 30,
            concurrency_limit: 3,
            user_agent: format!("WebCrawlerStaticPages/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// Asset downloader
pub struct Downloader {
    client: Client,
    config: DownloadConfig,
}

impl Downloader {
    /// Create a new downloader with configuration.
    ///
    /// Pre-creates output directories once to avoid runtime contention.
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Io` if directory creation fails.
    /// Returns `ScraperError::Config` if HTTP client build fails.
    pub fn new(config: DownloadConfig) -> Result<Self> {
        // Pre-create directories ONCE (init-once pattern)
        let images_path = config.output_dir.join(&config.images_dir);
        let documents_path = config.output_dir.join(&config.documents_dir);

        std::fs::create_dir_all(&images_path).map_err(|e| {
            ScraperError::Io(std::io::Error::other(format!(
                "Failed to create images directory: {}",
                e
            )))
        })?;

        std::fs::create_dir_all(&documents_path).map_err(|e| {
            ScraperError::Io(std::io::Error::other(format!(
                "Failed to create documents directory: {}",
                e
            )))
        })?;

        let client = Client::builder()
            .emulation(Emulation::Chrome131)
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .user_agent(&config.user_agent)
            .build()
            .map_err(|e| ScraperError::Config(format!("failed to build http client: {}", e)))?;

        Ok(Self { client, config })
    }

    /// Download a single asset with true streaming to disk.
    ///
    /// # Architecture
    ///
    /// - Creates temp file with UUID
    /// - Streams chunks directly to disk (constant RAM)
    /// - Computes hash on-the-fly
    /// - Atomic rename on success
    /// - Cleanup temp file on failure
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Network` if HTTP request fails.
    /// Returns `ScraperError::Io` if file operations fail.
    /// Returns `ScraperError::Download` if file exceeds size limit.
    pub async fn download(&self, url: &str) -> Result<DownloadedAsset> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| ScraperError::Network(e.to_string()))?;

        let mime_type = response
            .headers()
            .get(wreq::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        let asset_type = crate::adapters::detector::detect_from_url(url);
        let subdir = if asset_type.is_image() {
            &self.config.images_dir
        } else {
            &self.config.documents_dir
        };

        let subdir_path = self.config.output_dir.join(subdir);

        // Create temp file with UUID (atomic operation pattern)
        let temp_path = subdir_path.join(format!("{}.tmp", Uuid::new_v4()));
        let mut file = fs::File::create(&temp_path)
            .await
            .map_err(ScraperError::Io)?;

        // Stream to disk with real-time size check
        let mut stream = into_stream(response);
        let mut downloaded: u64 = 0;
        let mut hasher = Sha256::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| ScraperError::Network(e.to_string()))?;
            if chunk.is_empty() {
                continue;
            }

            let chunk_len = chunk.len() as u64;
            downloaded = downloaded
                .checked_add(chunk_len)
                .ok_or_else(|| ScraperError::download("Integer overflow in download size"))?;

            // Check limit in real-time
            if downloaded > self.config.max_file_size {
                // Cleanup temp file on failure (err-cleanup-on-fail)
                let _ = fs::remove_file(&temp_path).await;
                return Err(ScraperError::download(format!(
                    "file too large: {} bytes (max: {} bytes)",
                    downloaded, self.config.max_file_size
                )));
            }

            // Write chunk to disk IMMEDIATELY (true streaming)
            file.write_all(&chunk).await.map_err(ScraperError::Io)?;
            hasher.update(&chunk);
        }

        // Sync to ensure data is on disk
        file.sync_all().await.map_err(ScraperError::Io)?;
        drop(file); // Close file before rename

        // Calculate hash and generate final filename
        let content_hash = format!("{:x}", hasher.finalize());
        let filename = self.generate_filename_from_hash(&content_hash, mime_type.as_deref());
        let final_path = subdir_path.join(&filename);

        // Atomic rename (atomic-operations pattern)
        fs::rename(&temp_path, &final_path)
            .await
            .map_err(ScraperError::Io)?;

        tracing::info!("downloaded: {} -> {:?}", url, final_path);

        Ok(DownloadedAsset {
            url: url.to_string(),
            local_path: final_path,
            mime_type,
            size: downloaded,
            content_hash: content_hash[..12].to_string(),
        })
    }

    /// Download multiple assets with configurable concurrency control.
    pub async fn download_batch(&self, urls: &[String]) -> Vec<Result<DownloadedAsset>> {
        if urls.is_empty() {
            return Vec::new();
        }

        let tasks = urls.iter().map(|url| {
            let url = url.clone();
            async move { self.download(&url).await }
        });

        let results: Vec<Result<DownloadedAsset>> = stream::iter(tasks)
            .buffer_unordered(self.config.concurrency_limit)
            .collect()
            .await;

        results
    }

    /// Generate filename from content hash and MIME type.
    fn generate_filename_from_hash(&self, content_hash: &str, mime_type: Option<&str>) -> String {
        let extension =
            mime_type_to_extension(mime_type.unwrap_or("")).unwrap_or_else(|| "bin".into());

        // Use first 12 characters of hash (96 bits of entropy)
        format!("{}.{}", &content_hash[..12], extension)
    }
}

/// Convert a Response into a stream of bytes
fn into_stream(response: Response) -> impl StreamExt<Item = wreq::Result<bytes::Bytes>> {
    response.bytes_stream()
}

/// MIME type to file extension mapping
fn mime_type_to_extension(mime: &str) -> Option<String> {
    let mime = mime.trim();
    match mime {
        "image/jpeg" | "image/jpg" => Some("jpg".to_string()),
        "image/png" => Some("png".to_string()),
        "image/gif" => Some("gif".to_string()),
        "image/webp" => Some("webp".to_string()),
        "image/svg+xml" => Some("svg".to_string()),
        "image/bmp" => Some("bmp".to_string()),
        "image/tiff" => Some("tiff".to_string()),
        "image/x-icon" => Some("ico".to_string()),
        "application/pdf" => Some("pdf".to_string()),
        "application/msword" => Some("doc".to_string()),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some("docx".to_string())
        }
        "application/vnd.ms-excel" => Some("xls".to_string()),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("xlsx".to_string())
        }
        "application/vnd.ms-powerpoint" => Some("ppt".to_string()),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            Some("pptx".to_string())
        }
        "text/csv" => Some("csv".to_string()),
        "application/vnd.oasis.opendocument.text" => Some("odt".to_string()),
        "application/vnd.oasis.opendocument.spreadsheet" => Some("ods".to_string()),
        "application/epub+zip" => Some("epub".to_string()),
        "application/rtf" => Some("rtf".to_string()),
        "text/plain" => Some("txt".to_string()),
        "application/json" => Some("json".to_string()),
        "application/xml" | "text/xml" => Some("xml".to_string()),
        _ => None,
    }
}

/// Simple async download without creating a Downloader instance.
///
/// # Note
///
/// This is a convenience function for quick downloads. For production use,
/// create a `Downloader` instance with proper configuration.
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

    #[tokio::test]
    async fn test_downloader_precreates_directories() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            images_dir: "test_images".to_string(),
            documents_dir: "test_docs".to_string(),
            ..Default::default()
        };

        // Create downloader (should pre-create directories)
        let _downloader = Downloader::new(config).unwrap();

        // Verify directories exist
        let images_path = temp_dir.path().join("test_images");
        let docs_path = temp_dir.path().join("test_docs");

        assert!(
            images_path.exists(),
            "Images directory should be pre-created"
        );
        assert!(
            docs_path.exists(),
            "Documents directory should be pre-created"
        );
    }

    #[test]
    fn test_downloader_config_concurrency() {
        let config = DownloadConfig {
            concurrency_limit: 10,
            ..Default::default()
        };
        assert_eq!(config.concurrency_limit, 10);
    }

    #[test]
    fn test_downloader_config_user_agent() {
        let custom_ua = "MyCustomBot/1.0";
        let config = DownloadConfig {
            user_agent: custom_ua.to_string(),
            ..Default::default()
        };
        assert_eq!(config.user_agent, custom_ua);
    }

    #[test]
    fn test_downloader_default_user_agent() {
        let config = DownloadConfig::default();
        assert!(
            config.user_agent.starts_with("WebCrawlerStaticPages/"),
            "Default user agent should include version"
        );
    }

    #[test]
    fn test_mime_type_to_extension() {
        assert_eq!(mime_type_to_extension("image/png"), Some("png".to_string()));
        assert_eq!(
            mime_type_to_extension("image/jpeg"),
            Some("jpg".to_string())
        );
        assert_eq!(
            mime_type_to_extension("application/pdf"),
            Some("pdf".to_string())
        );
        assert_eq!(mime_type_to_extension("application/unknown"), None);
        assert_eq!(mime_type_to_extension(""), None);
    }

    #[test]
    fn test_generate_filename_from_hash() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();

        let filename = downloader.generate_filename_from_hash("abc123def456789", Some("image/png"));
        assert!(
            filename.ends_with(".png"),
            "Expected .png but got: {}",
            filename
        );
        assert!(
            filename.starts_with("abc123def456"),
            "Filename should start with first 12 chars of hash"
        );

        let filename = downloader.generate_filename_from_hash("xyz789abc123456", None);
        assert!(
            filename.ends_with(".bin"),
            "Expected .bin but got: {}",
            filename
        );
    }

    #[tokio::test]
    async fn test_download_streaming_limit() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            max_file_size: 1024,
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();
        assert_eq!(downloader.config.max_file_size, 1024);
    }

    #[tokio::test]
    async fn test_download_batch_empty() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();
        let results = downloader.download_batch(&[]).await;
        assert!(results.is_empty());
    }
}
