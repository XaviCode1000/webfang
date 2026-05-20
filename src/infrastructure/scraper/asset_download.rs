//! Asset downloading (images and documents)
//!
//! Feature-gated module for downloading assets found in scraped pages.
//! Enabled with --features images or --features documents.

use crate::domain::DownloadedAsset;
use crate::error::Result;
use crate::ScraperConfig;
use futures::stream::{self, StreamExt};
use scraper;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Concurrency limit for asset downloads
const DOWNLOAD_CONCURRENCY: usize = 3;

/// Download all assets from HTML content
///
/// # Arguments
/// * `html` - HTML content containing asset URLs
/// * `base_url` - Base URL for resolving relative URLs
/// * `config` - Configuration with download options
///
/// # Returns
/// * `Vec<DownloadedAsset>` - Successfully downloaded assets
pub async fn download_all(
    html: &str,
    base_url: &url::Url,
    config: &ScraperConfig,
) -> Result<Vec<DownloadedAsset>> {
    let mut assets = Vec::new();

    // Extract URLs in a scope so `document` is dropped BEFORE any .await
    // scraper::Html contains NonAtomic (Cell<usize>) which is not Send
    let (image_urls, document_urls) = {
        let document = scraper::Html::parse_document(html);
        let images = if config.download_images {
            crate::extractor::extract_images(&document, base_url)
        } else {
            Vec::new()
        };
        let docs = if config.download_documents {
            crate::extractor::extract_documents(&document, base_url)
        } else {
            Vec::new()
        };
        (images, docs)
        // `document` dropped here at end of scope
    };

    // Download images if enabled
    if !image_urls.is_empty() {
        tracing::info!("🖼️  Found {} images to download", image_urls.len());
        let downloaded = download_image_batch(&image_urls, &config.output_dir).await;
        assets.extend(downloaded);
    }

    // Download documents if enabled
    if !document_urls.is_empty() {
        tracing::info!("📄 Found {} documents to download", document_urls.len());
        let downloaded = download_document_batch(&document_urls, &config.output_dir).await;
        assets.extend(downloaded);
    }

    Ok(assets)
}

/// Download a batch of images with concurrency control
async fn download_image_batch(
    images: &[crate::adapters::extractor::AssetUrl],
    output_dir: &Path,
) -> Vec<DownloadedAsset> {
    let output_dir = output_dir.to_path_buf();
    let tasks = images.iter().cloned().map(|img| {
        let output_dir = output_dir.clone();
        async move { download_single_asset(&img.url, "image", &output_dir).await }
    });

    stream::iter(tasks)
        .buffer_unordered(DOWNLOAD_CONCURRENCY)
        .filter_map(|result| async {
            match result {
                Ok(asset) => Some(asset),
                Err(e) => {
                    warn!("Failed to download image: {}", e);
                    None
                },
            }
        })
        .collect()
        .await
}

/// Download a batch of documents with concurrency control
async fn download_document_batch(
    documents: &[crate::adapters::extractor::AssetUrl],
    output_dir: &Path,
) -> Vec<DownloadedAsset> {
    let output_dir = output_dir.to_path_buf();
    let tasks = documents.iter().cloned().map(|doc| {
        let output_dir = output_dir.clone();
        async move { download_single_asset(&doc.url, "document", &output_dir).await }
    });

    stream::iter(tasks)
        .buffer_unordered(DOWNLOAD_CONCURRENCY)
        .filter_map(|result| async {
            match result {
                Ok(asset) => Some(asset),
                Err(e) => {
                    warn!("Failed to download document: {}", e);
                    None
                },
            }
        })
        .collect()
        .await
}

/// Download a single asset
async fn download_single_asset(
    url: &str,
    asset_type: &str,
    output_dir: &Path,
) -> Result<DownloadedAsset> {
    use sha2::{Digest, Sha256};
    use std::io;
    use wreq::Client;
    use wreq_util::Emulation;

    let client = Client::builder()
        .emulation(Emulation::Chrome145)
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .map_err(|e| {
            crate::error::ScraperError::Config(format!("Failed to create download client: {}", e))
        })?;

    let response = client.get(url).send().await.map_err(|e| {
        crate::error::ScraperError::Download(format!("Failed to download {}: {}", url, e))
    })?;

    let bytes = response.bytes().await.map_err(|e| {
        crate::error::ScraperError::Download(format!("Failed to read {}: {}", url, e))
    })?;

    // Generate filename from URL hash
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let hash = format!("{:x}", hasher.finalize());

    let extension =
        crate::adapters::detector::mime::get_extension(url).unwrap_or_else(|| "bin".to_string());
    let filename = format!("{}.{}", &hash[..12], extension);

    let subdir = if asset_type == "image" {
        "images"
    } else {
        "documents"
    };
    let local_path = output_dir.join(subdir).join(&filename);
    let bytes = bytes.to_vec();
    let bytes_len = bytes.len();
    let url_str = url.to_string();
    let asset_type_str = asset_type.to_string();

    // Create directory and write file via spawn_blocking
    // Closure returns io::Result<PathBuf>
    let final_path = tokio::task::spawn_blocking(move || -> std::io::Result<PathBuf> {
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&local_path, &bytes)?;
        Ok(local_path)
    })
    .await
    .map_err(|e| io::Error::other(format!("spawn_blocking failed: {e}")))??;

    tracing::info!("Downloaded: {} -> {:?}", url_str, final_path);

    Ok(DownloadedAsset {
        url: url_str,
        local_path: final_path.to_string_lossy().into_owned(),
        asset_type: asset_type_str,
        size: bytes_len as u64,
    })
}
