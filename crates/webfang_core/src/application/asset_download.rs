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
/// entirely within this synchronous phase. `ValidUrl` output is syntactic
/// validation only (#1117: scheme allow-list + credential strip) — it is NOT
/// destination safety; literal-IP SSRF rejection of the extracted targets is
/// enforced in [`download_asset_urls`], before any socket opens.
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
/// this phase. `ValidUrl` output is syntactic validation only (#1117) — the
/// destination-safety check (literal-IP SSRF rejection) runs in
/// [`download_asset_urls`].
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
/// otherwise. Operation order: empty short-circuit first, SSRF literal-IP
/// filter (rejected targets are skipped with a `warn!`, never dialed), then
/// downloader construction (only when there is real work), progress log, then
/// the batch transfer. The construction-first order was the historical shape;
/// #1426 changed it because it built a full TLS client only to discard it on
/// the empty path, and surfaced network-config errors for jobs that download
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

    // SSRF literal-IP filter (PI-1 / SEC F1, #1217): these URLs were
    // extracted from caller-supplied HTML, so `ValidUrl`'s syntactic
    // validation (#1117) says nothing about where they point — an attacker
    // page can make this batch dial the cloud-metadata, loopback or RFC1918
    // address. Rejected targets are skipped before any downloader
    // construction or socket; the rest of the batch proceeds. The pure
    // verdict (`seed_guard_refusal`) is the shared entry guard minus its own
    // log line — identical hatch and deny list by construction — so this
    // `warn!` can carry the batch's `url` field in a single event, which the
    // shared `reject_forbidden_literal_url` log does not.
    let mut allowed: Vec<crate::domain::ValidUrl> = Vec::with_capacity(urls.len());
    for url in urls {
        match crate::domain::ssrf_guard::seed_guard_refusal(url.as_url()) {
            Some(rejection) => {
                tracing::warn!(
                    url = %url,
                    host = %rejection.host,
                    ip = %rejection.ip,
                    "SSRF literal-IP asset target rejected (skipped, no socket opened)"
                );
            },
            None => allowed.push(url.clone()),
        }
    }

    if allowed.is_empty() {
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
        assets = allowed.len(),
        skipped = urls.len() - allowed.len(),
        shared = _shared_downloader.is_some(),
        "📦 Downloading assets via AssetDownloaderPort"
    );

    downloader.download_batch(&allowed).await
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

    /// Records every batch the port receives and answers with one synthetic
    /// asset per requested URL — enough to observe what the port saw without
    /// any real network.
    struct RecordingPort {
        seen: std::sync::Mutex<Vec<String>>,
    }

    impl RecordingPort {
        fn seen(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl crate::domain::ports::AssetDownloaderPort for RecordingPort {
        fn download_batch(
            &self,
            urls: &[crate::domain::ValidUrl],
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Vec<DownloadedAsset>>> + Send + '_>,
        > {
            let urls: Vec<String> = urls.iter().map(|u| u.as_str().to_owned()).collect();
            Box::pin(async move {
                let assets = urls
                    .iter()
                    .map(|url| DownloadedAsset {
                        url: url.clone(),
                        local_path: "/tmp/fake-asset".to_owned(),
                        asset_type: "image".to_owned(),
                        size: 1,
                    })
                    .collect();
                self.seen
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend(urls);
                Ok(assets)
            })
        }
    }

    /// Forbidden literals that survive `ValidUrl` parsing (http, no creds):
    /// cloud metadata, loopback, RFC1918.
    fn forbidden_urls() -> Vec<crate::domain::ValidUrl> {
        [
            "http://169.254.169.254/latest/meta-data/logo.png",
            "http://127.0.0.1:9/x.png",
            "http://10.0.0.7/img.png",
        ]
        .iter()
        .map(|s| crate::domain::ValidUrl::parse(s).expect("test url must parse"))
        .collect()
    }

    /// Guard window with the layer-2 hatch guaranteed absent — the
    /// read-or-assert window is serialized under ENV_LOCK (#1308).
    fn entry_guard_armed() -> webfang_test_utils::EnvGuard {
        webfang_test_utils::EnvGuard::clean(&[crate::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV])
    }

    /// Fresh port with an empty observation log.
    fn recording_port() -> RecordingPort {
        RecordingPort {
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// SEC F1 (PI-1): with the entry hatch guaranteed absent, every forbidden
    /// literal is filtered BEFORE the port is consulted — `Ok(vec![])`, the
    /// port never sees the URL, no socket (a real adapter would dial the
    /// metadata address; the mock makes that failure observable instead).
    #[tokio::test]
    async fn download_asset_urls_skips_every_forbidden_literal_without_the_port() {
        let _guard = entry_guard_armed();
        let config = ScraperConfig::default();
        let urls = forbidden_urls();
        let port = recording_port();

        let result = download_asset_urls(&urls, &config, Some(&port))
            .await
            .expect("rejections must be skips, not errors");

        assert!(
            result.is_empty(),
            "no forbidden literal may yield an asset: {result:?}"
        );
        assert!(
            port.seen().is_empty(),
            "the port must never see a forbidden literal, saw: {:?}",
            port.seen()
        );
    }

    /// A mixed batch keeps exactly the allowed targets, in order, and the
    /// port observes only those.
    #[tokio::test]
    async fn download_asset_urls_keeps_only_allowed_targets_from_a_mixed_batch() {
        let _guard = entry_guard_armed();
        let config = ScraperConfig::default();
        let mut urls = forbidden_urls();
        urls.push(
            crate::domain::ValidUrl::parse("https://example.com/logo.png")
                .expect("test url must parse"),
        );
        let port = recording_port();

        let result = download_asset_urls(&urls, &config, Some(&port))
            .await
            .expect("mixed batch must succeed");

        assert_eq!(result.len(), 1, "only the allowed target may download");
        assert_eq!(result[0].url, "https://example.com/logo.png");
        assert_eq!(
            port.seen(),
            vec!["https://example.com/logo.png".to_owned()],
            "the port must observe only the allowed target"
        );
    }

    /// The documented hatch (`DISABLE_ENTRY_GUARD_ENV` exact `"1"`, #1578)
    /// disarms this layer too: the same forbidden literals then reach the
    /// port. Mocked here — the test never dials the real address.
    #[tokio::test]
    async fn download_asset_urls_entry_hatch_disarms_the_literal_filter() {
        let _guard = webfang_test_utils::EnvGuard::entry_guard_off();
        let config = ScraperConfig::default();
        let urls = forbidden_urls();
        let port = recording_port();

        let result = download_asset_urls(&urls, &config, Some(&port))
            .await
            .expect("hatch disarmed: the batch must proceed");

        assert_eq!(
            result.len(),
            urls.len(),
            "hatch disarmed: every URL must proceed to the port"
        );
        assert_eq!(port.seen().len(), urls.len());
    }
}
