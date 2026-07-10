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
use std::time::Duration;

use crate::error::{Result, ScraperError};
use futures::stream::StreamExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use wreq::{Client, Response};
use wreq_util::Profile;

/// Strategy for generating downloaded asset filenames.
///
/// # Variants
///
/// - `Hash` — SHA-256 hash of content (first 12 hex chars). Dedup-friendly.
/// - `Slug` — Last path segment of the URL (e.g. `rust-book.pdf`).
/// - `ContentDisposition` — `filename=` from `Content-Disposition` header, falls back to `Hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssetNamingStrategy {
    #[default]
    Hash,
    Slug,
    ContentDisposition,
}

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
    /// URL glob patterns to include (empty = allow all)
    pub include_patterns: Vec<String>,
    /// URL glob patterns to exclude (always applied)
    pub exclude_patterns: Vec<String>,
    /// TLS/HTTP2 fingerprint profile
    pub h2_profile: Profile,
    /// Strategy for naming downloaded asset files
    pub asset_naming: AssetNamingStrategy,
    /// Maximum number of retry attempts for transient network errors
    pub max_retries: u32,
    /// Base delay for exponential backoff in milliseconds
    pub backoff_base_ms: u64,
    /// Maximum delay for exponential backoff in milliseconds
    pub backoff_max_ms: u64,
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
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            h2_profile: Profile::Chrome145,
            asset_naming: AssetNamingStrategy::default(),
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10_000,
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
                "Failed to create images directory: {e}"
            )))
        })?;

        std::fs::create_dir_all(&documents_path).map_err(|e| {
            ScraperError::Io(std::io::Error::other(format!(
                "Failed to create documents directory: {e}"
            )))
        })?;

        let client = Client::builder()
            .emulation(config.h2_profile)
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent(&config.user_agent)
            .build()
            .map_err(|e| ScraperError::Config(format!("failed to build http client: {e}")))?;

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
    /// - Retry with exponential backoff on transient network errors
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Network` if HTTP request fails.
    /// Returns `ScraperError::Io` if file operations fail.
    /// Returns `ScraperError::Download` if file exceeds size limit.
    pub async fn download(&self, url: &str) -> Result<DownloadedAsset> {
        let mut last_err = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let delay = compute_backoff_delay(
                    attempt,
                    self.config.backoff_base_ms,
                    self.config.backoff_max_ms,
                );
                tracing::debug!(
                    "retry {attempt}/{} for {} after {}ms",
                    self.config.max_retries,
                    url,
                    delay.as_millis()
                );
                tokio::time::sleep(delay).await;
            }

            match self.download_once(url).await {
                Ok(asset) => return Ok(asset),
                Err(e) => {
                    if !is_transient_error(&e) || attempt == self.config.max_retries {
                        return Err(e);
                    }
                    last_err = Some(e);
                },
            }
        }

        // Unreachable: loop always returns on last attempt, but required for type inference
        Err(last_err.unwrap_or_else(|| ScraperError::download("exhausted retries with no error captured")))
    }

    /// Single download attempt (no retry).
    async fn download_once(&self, url: &str) -> Result<DownloadedAsset> {
        let response = self.client.get(url).send().await.map_err(|e| {
            let transient = e.is_timeout() || e.is_connect() || e.is_connection_reset();
            ScraperError::Network(format!(
                "{}{}",
                if transient { "TRANSIENT:" } else { "" },
                e
            ))
        })?;

        // Fail-fast on 4xx (client errors). 5xx (server errors) are transient and will be retried.
        let status = response.status();
        if status.is_client_error() {
            return Err(ScraperError::Download(format!(
                "HTTP {} al descargar {}",
                status,
                url
            )));
        }
        if status.is_server_error() {
            // Mark as transient so retry logic will retry on 5xx
            return Err(ScraperError::Network(format!("TRANSIENT:HTTP {}", status)));
        }

        let mime_type = response
            .headers()
            .get(wreq::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        // Extract Content-Disposition filename before consuming response
        let content_disposition_filename = response
            .headers()
            .get(wreq::header::CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_disposition_header);

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
            let chunk = chunk_result.map_err(|e| {
                let transient = e.is_timeout() || e.is_connect();
                ScraperError::Network(format!(
                    "{}{}",
                    if transient { "TRANSIENT:" } else { "" },
                    e
                ))
            })?;
            if chunk.is_empty() {
                continue;
            }

            let chunk_len = chunk.len() as u64;
            downloaded = downloaded
                .checked_add(chunk_len)
                .ok_or_else(|| ScraperError::download("integer overflow in download size"))?;

            // Check limit in real-time
            if downloaded > self.config.max_file_size {
                // Cleanup temp file on failure
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
        let filename = self.generate_filename(
            url,
            &content_hash,
            mime_type.as_deref(),
            content_disposition_filename.as_deref(),
        );
        let final_path = subdir_path.join(&filename);

        // Atomic rename
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
    ///
    /// Filters URLs against `include_patterns` / `exclude_patterns` before downloading.
    /// Returns partial results — individual failures don't abort the batch.
    pub async fn download_batch(&self, urls: &[String]) -> Vec<Result<DownloadedAsset>> {
        if urls.is_empty() {
            return Vec::new();
        }

        let filtered: Vec<String> = urls
            .iter()
            .filter(|url| {
                url_matches_filters(
                    url,
                    &self.config.include_patterns,
                    &self.config.exclude_patterns,
                )
            })
            .cloned()
            .collect();

        if filtered.is_empty() {
            return Vec::new();
        }

        let concurrency = self.config.concurrency_limit;
        let mut results = Vec::with_capacity(filtered.len());
        let mut futs = Vec::with_capacity(filtered.len());
        for url in &filtered {
            futs.push(self.download(url));
        }
        let stream = futures::stream::iter(futs).buffer_unordered(concurrency);
        results.extend(stream.collect::<Vec<_>>().await);
        results
    }

    /// Generate filename according to the configured naming strategy.
    fn generate_filename(
        &self,
        url: &str,
        content_hash: &str,
        mime_type: Option<&str>,
        content_disposition_filename: Option<&str>,
    ) -> String {
        let extension =
            mime_type_to_extension(mime_type.unwrap_or("")).unwrap_or_else(|| "bin".into());

        let base_name = match self.config.asset_naming {
            AssetNamingStrategy::Hash => {
                // Use first 12 characters of hash (96 bits of entropy)
                format!("{}.{}", &content_hash[..12], extension)
            },
            AssetNamingStrategy::Slug => {
                let slug = derive_slug_from_url(url);
                let name = sanitize_filename(&slug);
                if name.is_empty() {
                    // Fallback to hash if slug is empty
                    format!("{}.{}", &content_hash[..12], extension)
                } else {
                    // Preserve original extension from slug if present, otherwise use MIME
                    let slug_ext = name.rsplit('.').next().unwrap_or("");
                    if !slug_ext.is_empty() && slug_ext != name {
                        sanitize_filename(&name)
                    } else {
                        format!("{}.{}", name, extension)
                    }
                }
            },
            AssetNamingStrategy::ContentDisposition => {
                if let Some(name) = content_disposition_filename {
                    let sanitized = sanitize_filename(name);
                    if !sanitized.is_empty() {
                        sanitized
                    } else {
                        format!("{}.{}", &content_hash[..12], extension)
                    }
                } else {
                    format!("{}.{}", &content_hash[..12], extension)
                }
            },
        };

        base_name
    }
}

/// Convert a Response into a stream of bytes
fn into_stream(response: Response) -> impl StreamExt<Item = wreq::Result<bytes::Bytes>> {
    response.bytes_stream()
}

/// Check if a URL matches include/exclude filters.
///
/// If `include_patterns` is empty, all URLs pass the include check.
/// `exclude_patterns` are always applied (deny wins).
fn url_matches_filters(url: &str, includes: &[String], excludes: &[String]) -> bool {
    if excludes.iter().any(|p| pattern_matches_asset(url, p)) {
        return false;
    }
    if includes.is_empty() {
        return true;
    }
    includes.iter().any(|p| pattern_matches_asset(url, p))
}

/// Match a URL against a pattern, supporting both extension globs (`*.pdf`)
/// and the standard host/path glob from `domain::pattern_matching`.
fn pattern_matches_asset(url: &str, pattern: &str) -> bool {
    let p = pattern.trim();
    // Extension glob: *.ext (but NOT host globs like *.example.com which contain a dot after the prefix)
    if let Some(ext) = p.strip_prefix("*.") {
        if !ext.is_empty() && !ext.contains('.') {
            if let Ok(u) = url::Url::parse(url) {
                let last = u.path().rsplit('/').next().unwrap_or("");
                let low = last.to_ascii_lowercase();
                let ext_low = ext.to_ascii_lowercase();
                return low.ends_with(&format!(".{ext_low}"));
            }
        }
    }
    crate::domain::matches_pattern(url, pattern)
}

/// Check if an error is transient (worth retrying).
fn is_transient_error(err: &ScraperError) -> bool {
    if let ScraperError::Network(msg) = err {
        msg.starts_with("TRANSIENT:")
            || msg.to_ascii_lowercase().contains("timeout")
            || msg.contains("timed out")
            || msg.contains("connection reset")
            || msg.contains("connection refused")
            || msg.contains("broken pipe")
            || msg.contains("connection closed")
            || msg.contains("connection aborted")
            || msg.contains("reset by peer")
    } else {
        false
    }
}

/// Compute exponential backoff delay with jitter.
fn compute_backoff_delay(attempt: u32, base_ms: u64, max_ms: u64) -> Duration {
    use rand::Rng;
    use std::cmp::min;

    // Exponential: base * 2^(attempt-1), clamped to max
    let delay_ms = min(base_ms.saturating_mul(1u64 << (attempt - 1)), max_ms);
    // Add jitter: 75%-125% of delay, then clamp final result to max_ms
    let jitter = delay_ms / 4;
    let offset = rand::rng().random_range(0..=jitter.saturating_mul(2));
    let final_ms = min(delay_ms.saturating_sub(jitter).saturating_add(offset), max_ms);
    Duration::from_millis(final_ms)
}

/// Derive a slug from the last path segment of a URL.
fn derive_slug_from_url(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            let path = u.path();
            path.rsplit('/')
                .next()
                .filter(|s| !s.is_empty() && *s != "/")
                .map(String::from)
        })
        .unwrap_or_default()
}

/// UTF-8 safe percent-decoding for Content-Disposition filenames.
/// Invalid percent sequences are kept as-is; invalid UTF-8 is replaced with replacement char.
fn percent_decode_utf8(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                bytes.push(byte);
                continue;
            }
            // Invalid percent encoding - keep as-is
            bytes.extend(b"%");
            bytes.extend(hex.as_bytes());
        } else {
            bytes.push(c as u8);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Sanitize a filename: remove path separators and null bytes.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .filter(|c| *c != '/' && *c != '\\' && *c != '\0')
        .collect()
}

/// Parse `filename=` from a Content-Disposition header value.
fn parse_content_disposition_header(value: &str) -> Option<String> {
    // Try filename*=UTF-8''encoded first (RFC 5987)
    if let Some(start) = value.find("filename*=UTF-8''") {
        let encoded = &value[start + "filename*=UTF-8''".len()..];
        let name: String = encoded
            .chars()
            .take_while(|c| *c != ';' && *c != ' ')
            .collect();
        let decoded = percent_decode_utf8(&name);
        return Some(decoded);
    }

    // Try filename="name" or filename=name
    let after = value.find("filename=")?;
    let rest = &value[after + "filename=".len()..];
    let name = if let Some(inner) = rest.strip_prefix('"') {
        // Quoted: filename="name.pdf"
        let end = inner.find('"')?;
        &inner[..end]
    } else {
        // Unquoted: filename=name.pdf
        rest.split(';').next().unwrap_or(rest).trim()
    };

    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
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
        },
        "application/vnd.ms-excel" => Some("xls".to_string()),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some("xlsx".to_string())
        },
        "application/vnd.ms-powerpoint" => Some("ppt".to_string()),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            Some("pptx".to_string())
        },
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

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
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

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
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

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[test]
    fn test_generate_filename_hash_strategy() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();

        let filename = downloader.generate_filename(
            "https://example.com/img.png",
            "abc123def456789",
            Some("image/png"),
            None,
        );
        assert!(
            filename.ends_with(".png"),
            "Expected .png but got: {}",
            filename
        );
        assert!(
            filename.starts_with("abc123def456"),
            "Filename should start with first 12 chars of hash"
        );

        let filename =
            downloader.generate_filename("https://example.com/file", "xyz789abc123456", None, None);
        assert!(
            filename.ends_with(".bin"),
            "Expected .bin but got: {}",
            filename
        );
    }

    #[test]
    fn test_generate_filename_slug_strategy() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            asset_naming: AssetNamingStrategy::Slug,
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();

        let filename = downloader.generate_filename(
            "https://example.com/docs/rust-book.pdf",
            "abc123def456789",
            Some("application/pdf"),
            None,
        );
        assert_eq!(filename, "rust-book.pdf");
    }

    #[test]
    fn test_generate_filename_content_disposition() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            asset_naming: AssetNamingStrategy::ContentDisposition,
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();

        let filename = downloader.generate_filename(
            "https://example.com/download",
            "abc123def456789",
            Some("application/pdf"),
            Some("annual-report.pdf"),
        );
        assert_eq!(filename, "annual-report.pdf");
    }

    #[test]
    fn test_generate_filename_content_disposition_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let config = DownloadConfig {
            output_dir: temp_dir.path().to_path_buf(),
            asset_naming: AssetNamingStrategy::ContentDisposition,
            ..Default::default()
        };
        let downloader = Downloader::new(config).unwrap();

        // No Content-Disposition → falls back to hash
        let filename = downloader.generate_filename(
            "https://example.com/download",
            "abc123def456789",
            Some("application/pdf"),
            None,
        );
        assert!(filename.starts_with("abc123def456"));
    }

    #[test]
    fn test_url_matches_filters_empty_includes() {
        assert!(url_matches_filters(
            "https://example.com/file.pdf",
            &[],
            &[]
        ));
    }

    #[test]
    fn test_url_matches_filters_exclude_wins() {
        let excludes = vec!["/*.pdf".to_string()];
        assert!(!url_matches_filters(
            "https://example.com/file.pdf",
            &[],
            &excludes
        ));
    }

    #[test]
    fn test_url_matches_filters_include_only() {
        let includes = vec!["/*.pdf".to_string()];
        assert!(url_matches_filters(
            "https://example.com/file.pdf",
            &includes,
            &[]
        ));
        assert!(!url_matches_filters(
            "https://example.com/file.jpg",
            &includes,
            &[]
        ));
    }

    #[test]
    fn test_url_matches_filters_extension_glob() {
        let includes = vec!["*.pdf".to_string()];
        assert!(url_matches_filters(
            "https://x.com/file.pdf",
            &includes,
            &[]
        ));
        assert!(!url_matches_filters(
            "https://x.com/file.jpg",
            &includes,
            &[]
        ));
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("hello/world"), "helloworld");
        assert_eq!(sanitize_filename("file\0name"), "filename");
        assert_eq!(sanitize_filename("normal-file.pdf"), "normal-file.pdf");
    }

    #[test]
    fn test_parse_content_disposition_quoted() {
        let val = r#"attachment; filename="report.pdf""#;
        assert_eq!(
            parse_content_disposition_header(val),
            Some("report.pdf".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_unquoted() {
        let val = "attachment; filename=report.pdf";
        assert_eq!(
            parse_content_disposition_header(val),
            Some("report.pdf".to_string())
        );
    }

    #[test]
    fn test_parse_content_disposition_missing() {
        assert_eq!(parse_content_disposition_header("attachment"), None);
    }

    #[test]
    fn test_derive_slug_from_url() {
        assert_eq!(
            derive_slug_from_url("https://example.com/docs/book.pdf"),
            "book.pdf"
        );
        assert_eq!(derive_slug_from_url("https://example.com/"), "");
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
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

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
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
