//! Asset downloading (images and documents)
//!
//! Feature-gated module for downloading assets found in scraped pages.
//! Enabled with --features images or --features documents.

use crate::config::ScraperConfig;
use crate::domain::DownloadedAsset;
use crate::error::Result;
use futures::stream::{self, StreamExt};
use std::path::PathBuf;
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

    // Download images if enabled
    if config.download_images {
        let images = crate::extractor::extract_images(html, base_url);
        if !images.is_empty() {
            tracing::info!("🖼️  Found {} images to download", images.len());
            let downloaded = download_image_batch(&images, &config.output_dir).await;
            assets.extend(downloaded);
        }
    }

    // Download documents if enabled
    if config.download_documents {
        let documents = crate::extractor::extract_documents(html, base_url);
        if !documents.is_empty() {
            tracing::info!("📄 Found {} documents to download", documents.len());
            let downloaded = download_document_batch(&documents, &config.output_dir).await;
            assets.extend(downloaded);
        }
    }

    Ok(assets)
}

/// Download a batch of images with concurrency control
async fn download_image_batch(
    images: &[crate::adapters::extractor::AssetUrl],
    output_dir: &PathBuf,
) -> Vec<DownloadedAsset> {
    let tasks = images.iter().map(|img| {
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
                }
            }
        })
        .collect()
        .await
}

/// Download a batch of documents with concurrency control
async fn download_document_batch(
    documents: &[crate::adapters::extractor::AssetUrl],
    output_dir: &PathBuf,
) -> Vec<DownloadedAsset> {
    let tasks = documents.iter().map(|doc| {
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
                }
            }
        })
        .collect()
        .await
}

/// Download a single asset
async fn download_single_asset(
    url: &str,
    asset_type: &str,
    output_dir: &PathBuf,
) -> Result<DownloadedAsset> {
    use reqwest::Client;
    use sha2::{Digest, Sha256};
    use std::fs;

    let client = Client::builder()
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

    let extension = crate::extractor::get_extension(url).unwrap_or_else(|| "bin".into());
    let filename = format!("{}.{}", &hash[..12], extension);

    let subdir = if asset_type == "image" {
        "images"
    } else {
        "documents"
    };
    let local_path = output_dir.join(subdir).join(&filename);

    // Create directory and write file
    if let Some(parent) = local_path.parent() {
        fs::create_dir_all(parent).map_err(|e| crate::error::ScraperError::Io(e))?;
    }

    fs::write(&local_path, &bytes).map_err(|e| crate::error::ScraperError::Io(e))?;

    tracing::info!("Downloaded: {} -> {:?}", url, local_path);

    Ok(DownloadedAsset {
        url: url.to_string(),
        local_path: local_path.to_string_lossy().into_owned(),
        asset_type: asset_type.to_string(),
        size: bytes.len() as u64,
    })
}
