//! Asset download orchestration — application-layer glue for asset downloading.
//!
//! # Design note (#443)
//!
//! [`download_assets_if_enabled`] is application-layer orchestration, not
//! adapter logic: it reads [`ScraperConfig`], extracts
//! asset URLs from the HTML via [`crate::extractor`], deduplicates them, and
//! delegates the actual transfer to the [`AssetDownloaderPort`](crate::domain::ports::AssetDownloaderPort) adapter.
//! Pushing this into
//! the adapter would invert the Clean Architecture dependency direction
//! (adapters implement domain ports; they do not orchestrate application
//! config), so the function lives here and delegates downward through the port.
//!
//! When no shared downloader is supplied, the fallback is built through
//! [`AssetDownloaderFactory`](crate::domain::asset_downloader_factory::AssetDownloaderFactory)
//! rather than by naming an adapter concrete — the `application -> adapters`
//! edge this module used to carry (ADR-0012-B cheap wins).

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
/// ([`download_asset_urls`]) receives validated `ValidUrl` values only (#1117).
pub fn extract_asset_urls_from_html(
    html: &str,
    _base_url: &url::Url,
    _config: &ScraperConfig,
) -> Vec<crate::domain::ValidUrl> {
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
/// validated `ValidUrl` values only (#1117).
pub fn extract_asset_urls(
    document: &scraper::Html,
    _base_url: &url::Url,
    _config: &ScraperConfig,
) -> Vec<crate::domain::ValidUrl> {
    // Extract URLs from HTML
    let mut urls: Vec<crate::domain::ValidUrl> = Vec::new();
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
/// Uses the shared downloader when provided; builds a fallback one through
/// the domain [`AssetDownloaderFactory`](crate::domain::asset_downloader_factory::AssetDownloaderFactory)
/// otherwise. Operation order: empty short-circuit first, then downloader
/// construction (only when there is real work), progress log, then the batch
/// transfer. The construction-first order was the historical shape; #1426
/// changed it because it built a full TLS client only to discard it on the
/// empty path, and surfaced network-config errors for jobs that download
/// nothing.
pub async fn download_asset_urls(
    urls: &[crate::domain::ValidUrl],
    _config: &ScraperConfig,
    _shared_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
) -> Result<Vec<DownloadedAsset>> {
    // Nothing to transfer: return before touching the downloader. The
    // fallback client (wreq -> BoringSSL FFI) is built only when a download
    // will actually happen (#1426). This ordering is also what keeps the
    // empty-path tests runnable under Miri: no construction, no foreign call.
    if urls.is_empty() {
        return Ok(Vec::new());
    }

    // Use shared downloader when provided; build a fallback one through the
    // domain factory otherwise. `application` never names the adapter type
    // (ADR-0012-B cheap win).
    let owned_downloader;
    let downloader: &dyn crate::domain::ports::AssetDownloaderPort = match _shared_downloader {
        Some(dl) => dl,
        None => {
            owned_downloader =
                crate::domain::asset_downloader_factory::default_factory().build(_config)?;
            &*owned_downloader
        },
    };

    tracing::info!(
        assets = urls.len(),
        shared = _shared_downloader.is_some(),
        "📦 Downloading assets via AssetDownloaderPort"
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
    ///
    /// Order pin (#1426): this test runs under Miri on purpose — the
    /// `#[cfg_attr(miri, ignore)]` it used to carry is gone. The empty path
    /// must not construct a downloader (wreq -> BoringSSL FFI), so if the
    /// construction ever moves back above the short-circuit, Miri aborts here
    /// and the lane goes red. Native builds cannot observe the ordering (both
    /// orders return Ok([])), so Miri IS the regression detector.
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

    /// Direct contract (#1426): an empty slice short-circuits to `Ok` before
    /// the `None` fallback is even consulted — no shared downloader needed,
    /// no client constructed.
    #[tokio::test]
    async fn download_asset_urls_empty_short_circuits_without_downloader() {
        let config = ScraperConfig::default();
        let urls: Vec<crate::domain::ValidUrl> = Vec::new();
        let result = download_asset_urls(&urls, &config, None)
            .await
            .expect("empty urls must return Ok");
        assert!(result.is_empty(), "empty urls must yield an empty vec");
    }
}
