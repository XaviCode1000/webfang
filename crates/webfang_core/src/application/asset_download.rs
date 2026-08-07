//! Asset download orchestration — application-layer glue for asset downloading.
//!
//! # Design note (#443)
//!
//! [`download_assets_if_enabled`] is application-layer orchestration, not
//! adapter logic: it reads [`ScraperConfig`](crate::ScraperConfig), extracts
//! asset URLs from the HTML via [`crate::extractor`], deduplicates them, and
//! delegates the actual transfer to the [`AssetDownloaderPort`](crate::domain::ports::AssetDownloaderPort) adapter
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

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    /// Bug #2 regression: when config.has_downloads() is false, the function
    /// MUST return an empty vec without attempting any download — regardless
    /// of feature flags (issue #590). Previously the cfg gate would skip the
    /// inner block entirely; now the runtime check is the single gate.
    #[tokio::test]
    async fn download_assets_returns_empty_when_disabled() {
        let config = crate::ScraperConfig::default(); // has_downloads() == false
        let base_url = Url::parse("https://example.com").expect("valid url");
        let html = r#"<html><body><img src="/image.png"></body></html>"#;

        let result = download_assets_if_enabled(html, &base_url, &config, None)
            .await
            .expect("must return Ok");
        assert!(result.is_empty(), "disabled config must yield empty vec");
    }
}
