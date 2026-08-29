//! Asset download orchestration — application-layer glue for asset downloading.
//!
//! # Design note (#443)
//!
//! [`download_assets_if_enabled`] is application-layer orchestration, not
//! adapter logic: it reads [`ScraperConfig`], extracts
//! asset URLs from the HTML via [`crate::extractor`], deduplicates them, and
//! delegates the actual transfer to the [`AssetDownloaderPort`](crate::domain::ports::AssetDownloaderPort) adapter
//! (`adapters::downloader::Downloader` implements the port). Pushing this into
//! the adapter would invert the Clean Architecture dependency direction
//! (adapters implement domain ports; they do not orchestrate application
//! config), so the function lives here and delegates downward through the port.

use crate::domain::config::ScraperConfig;
use crate::domain::DownloadedAsset;
use crate::error::Result;

/// Helper: Download assets if config has downloads enabled
///
/// Uses the `AssetDownloaderPort` trait for testability.
/// Falls back to constructing a concrete `Downloader` when no trait object is provided.
pub async fn download_assets_if_enabled(
    html: &str,
    base_url: &url::Url,
    config: &ScraperConfig,
    shared_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
) -> Result<Vec<DownloadedAsset>> {
    // #962: parsing happens inside the synchronous extraction helper, so no
    // async-fn body ever mentions [`scraper::Html`] (neither `Send` nor
    // `Sync`) — the returned futures stay `Send` for Tokio `spawn` funnels.
    let urls = extract_asset_urls_from_html(html, base_url, config);
    download_asset_urls(&urls, config, shared_downloader).await
}

/// Extract deduplicated asset URLs from raw HTML.
///
/// Convenience wrapper that parses `html` once and delegates to
/// [`extract_asset_urls`]. Hot-path callers that already hold the page's
/// parsed DOM should call [`extract_asset_urls`] directly (#962).
///
/// Synchronous by design: [`scraper::Html`] contains interior mutability
/// (`Cell`) and is neither `Send` nor `Sync`, so the DOM must be consumed
/// entirely within this synchronous phase; the async download stage
/// ([`download_asset_urls`]) receives owned URL strings only.
pub fn extract_asset_urls_from_html(
    html: &str,
    _base_url: &url::Url,
    _config: &ScraperConfig,
) -> Vec<String> {
    if !_config.has_downloads() {
        return Vec::new();
    }

    let document = scraper::Html::parse_document(html);
    extract_asset_urls(&document, _base_url, _config)
}

/// Extract deduplicated asset URLs from an already-parsed DOM (#962).
///
/// Synchronous by design: [`scraper::Html`] contains interior mutability
/// (`Cell`) and is not `Send`, so the DOM must be consumed entirely within
/// this phase; the async download stage ([`download_asset_urls`]) receives
/// owned URL strings only.
pub fn extract_asset_urls(
    document: &scraper::Html,
    _base_url: &url::Url,
    _config: &ScraperConfig,
) -> Vec<String> {
    // Extract URLs from HTML
    let mut urls: Vec<String> = Vec::new();
    if _config.download_images {
        let images = crate::extractor::extract_images(document, _base_url);
        urls.extend(images.into_iter().map(|a| a.url));
    }
    if _config.download_documents {
        let docs = crate::extractor::extract_documents(document, _base_url);
        urls.extend(docs.into_iter().map(|a| a.url));
    }

    // Deduplicate URLs to avoid downloading the same asset multiple times
    // (e.g., same image referenced from multiple <img> tags).
    use std::collections::HashSet;
    let mut seen = HashSet::with_capacity(urls.len());
    urls.retain(|url| seen.insert(url.clone()));
    urls
}

/// Download previously extracted asset URLs through the
/// [`AssetDownloaderPort`](crate::domain::ports::AssetDownloaderPort).
///
/// Uses the shared downloader when provided; constructs a fallback concrete
/// `Downloader` otherwise. Operation order mirrors the historical
/// implementation exactly: downloader construction, empty short-circuit,
/// progress log, then the batch transfer.
pub async fn download_asset_urls(
    urls: &[String],
    _config: &ScraperConfig,
    _shared_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
) -> Result<Vec<DownloadedAsset>> {
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

    if urls.is_empty() {
        return Ok(Vec::new());
    }

    tracing::info!(
        "📦 Downloading {} assets via adapters::Downloader",
        urls.len()
    );

    downloader.download_batch(urls).await
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
        let config = ScraperConfig::default(); // has_downloads() == false
        let base_url = Url::parse("https://example.com").expect("valid url");
        let html = r#"<html><body><img src="/image.png"></body></html>"#;

        let result = download_assets_if_enabled(html, &base_url, &config, None)
            .await
            .expect("must return Ok");
        assert!(result.is_empty(), "disabled config must yield empty vec");
    }
}
