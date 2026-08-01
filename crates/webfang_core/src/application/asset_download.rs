//! Asset download orchestration — application-layer glue for asset downloading.
//!
//! # Design note (#443)
//!
//! [`download_assets_if_enabled`] is application-layer orchestration, not
//! adapter logic: it reads [`ScraperConfig`](crate::ScraperConfig), extracts
//! asset URLs from the HTML via [`crate::extractor`], deduplicates them, and
//! delegates the actual transfer to the [`AssetDownloaderPort`] adapter
//! (`adapters::downloader::Downloader` implements the port). Pushing this into
//! the adapter would invert the Clean Architecture dependency direction
//! (adapters implement domain ports; they do not orchestrate application
//! config), so the function lives here and delegates downward through the port.

use crate::domain::DownloadedAsset;
use crate::error::Result;

/// Helper: Download assets if config has downloads enabled
///
/// Uses the `AssetDownloaderPort` trait for testability.
/// Falls back to constructing a concrete `Downloader` when no trait object is provided.
pub async fn download_assets_if_enabled(
    _html: &str,
    _base_url: &url::Url,
    _config: &crate::ScraperConfig,
    _shared_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
) -> Result<Vec<DownloadedAsset>> {
    if !_config.has_downloads() {
        return Ok(Vec::new());
    }

    #[cfg(any(feature = "images", feature = "documents"))]
    {
        // Use shared downloader when provided; create a fallback one otherwise
        let owned_downloader;
        let downloader: &dyn crate::domain::ports::AssetDownloaderPort = match _shared_downloader {
            Some(dl) => dl,
            None => {
                owned_downloader =
                    crate::adapters::downloader::Downloader::new(_config.to_download_config())?;
                &owned_downloader
            },
        };

        // Extract URLs from HTML
        let mut urls: Vec<String> = Vec::new();
        {
            let document = scraper::Html::parse_document(_html);
            if _config.download_images {
                let images = crate::extractor::extract_images(&document, _base_url);
                urls.extend(images.into_iter().map(|a| a.url));
            }
            if _config.download_documents {
                let docs = crate::extractor::extract_documents(&document, _base_url);
                urls.extend(docs.into_iter().map(|a| a.url));
            }
        }

        if urls.is_empty() {
            return Ok(Vec::new());
        }

        // Deduplicate URLs to avoid downloading the same asset multiple times
        // (e.g., same image referenced from multiple <img> tags).
        use std::collections::HashSet;
        let mut seen = HashSet::with_capacity(urls.len());
        urls.retain(|url| seen.insert(url.clone()));

        tracing::info!(
            "📦 Downloading {} assets via adapters::Downloader",
            urls.len()
        );

        let results = downloader.download_batch(&urls).await?;

        // Trait impl already returns domain::DownloadedAsset — collect directly
        Ok(results)
    }

    #[cfg(not(any(feature = "images", feature = "documents")))]
    {
        Ok(Vec::new())
    }
}
