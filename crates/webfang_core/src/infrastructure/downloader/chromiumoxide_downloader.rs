//! Chromiumoxide (CDP) downloader — real Chrome DevTools Protocol integration.
//!
//! Spawns a headless Chrome instance per fetch, injects cookies from the
//! [`CookieBridge`], navigates to the target URL, and extracts the rendered
//! HTML. The browser is closed after each fetch to bound memory usage.
//!
//! When the `chromium` feature is disabled, the module compiles as a stub
//! that returns an explicit "not enabled" error from every `fetch` call.

#[cfg(feature = "chromium")]
use {
    chromiumoxide::browser::HeadlessMode,
    chromiumoxide::cdp::browser_protocol::network::{CookieParam, SetCookiesParams},
    chromiumoxide::{Browser, BrowserConfig},
    futures::StreamExt,
    tokio::time::{timeout, Duration},
};

use std::sync::Arc;
use std::sync::RwLock;

use futures::future::BoxFuture;
use url::Url;

#[cfg(feature = "chromium")]
use super::cookie_bridge::domain_matches;
use super::cookie_bridge::CookieBridge;
use super::{DownloadError, Downloader, FetchedPage};

/// Memory budget for one Chrome tab (~200 MB).
#[cfg(feature = "chromium")]
const CHROMIUMOXIDE_MEMORY_COST: usize = 200_000_000;

/// Timeout for browser navigation (page.goto).
#[cfg(feature = "chromium")]
const NAV_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for content extraction (page.content).
#[cfg(feature = "chromium")]
const CONTENT_TIMEOUT: Duration = Duration::from_secs(10);

/// CDP downloader that spawns a headless Chrome instance per fetch.
///
/// Note: Resource gating is handled by [`super::hybrid_router::HybridRouter`].
/// This downloader does NOT own a `ResourceGovernor` — the router checks
/// resources before invoking this layer.
pub struct ChromiumoxideDownloader {
    #[cfg(feature = "chromium")]
    cookie_bridge: Arc<RwLock<CookieBridge>>,
}

impl ChromiumoxideDownloader {
    #[cfg(feature = "chromium")]
    pub(crate) fn new(cookie_bridge: Arc<RwLock<CookieBridge>>) -> Self {
        Self { cookie_bridge }
    }

    #[cfg(not(feature = "chromium"))]
    pub(crate) fn new(_cookie_bridge: Arc<RwLock<CookieBridge>>) -> Self {
        Self {}
    }
}

#[cfg(feature = "chromium")]
impl Downloader for ChromiumoxideDownloader {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        Box::pin(async move {
        // 1. Early URL scheme validation
        if !url.scheme().starts_with("http") {
            return Err(DownloadError::InvalidUrl(format!(
                "unsupported scheme: {}",
                url.scheme()
            )));
        }

        // 2. Browser config with sandbox bypass for CI/Docker
        let config = BrowserConfig::builder()
            .headless_mode(HeadlessMode::True)
            .no_sandbox()
            .build()
            .map_err(DownloadError::Internal)?;

        let (mut browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|e| DownloadError::Internal(format!("Chrome launch failed: {e}")))?;

        // 3. Process CDP messages in isolated task to prevent hangs
        let handler_job = tokio::spawn(async move { while handler.next().await.is_some() {} });

        // 4. Inject cookies from L1 cookie bridge, filtered by domain
        let current_domain = url.host_str().unwrap_or("");
        let cdp_cookies: Vec<CookieParam> = {
            let bridge = self
                .cookie_bridge
                .read()
                .map_err(|e| DownloadError::Internal(format!("Lock poisoned: {e}")))?;
            bridge
                .to_cdp_cookies()
                .into_iter()
                .filter(|c| domain_matches(current_domain, &c.domain))
                .map(|c| {
                    let mut param = CookieParam::new(c.name, c.value);
                    param.domain = Some(c.domain);
                    param.path = Some(c.path);
                    param.secure = Some(c.secure);
                    param.http_only = Some(c.http_only);
                    param
                })
                .collect()
        };

        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|e| DownloadError::Internal(e.to_string()))?;

        if !cdp_cookies.is_empty() {
            page.execute(SetCookiesParams::new(cdp_cookies))
                .await
                .map_err(|e| DownloadError::Internal(e.to_string()))?;
        }

        // 5. Navigate with timeout
        timeout(NAV_TIMEOUT, page.goto(url.as_str()))
            .await
            .map_err(|_| DownloadError::Timeout(NAV_TIMEOUT.as_secs()))?
            .map_err(|e| DownloadError::Internal(e.to_string()))?;

        // 6. Extract rendered DOM with timeout
        let html = timeout(CONTENT_TIMEOUT, page.content())
            .await
            .map_err(|_| DownloadError::Timeout(CONTENT_TIMEOUT.as_secs()))?
            .map_err(|e| DownloadError::Internal(e.to_string()))?;

        // 7. Deterministic shutdown — prevents zombie processes
        browser.close().await.ok();
        handler_job.await.ok();

        Ok(FetchedPage {
            url: url.clone(),
            html,
            status: 200,
            headers: std::collections::HashMap::new(),
            cookies: Vec::new(),
        })
        })
    }

    fn supports_interactions(&self) -> bool {
        true
    }

    fn memory_cost(&self) -> usize {
        CHROMIUMOXIDE_MEMORY_COST
    }
}

#[cfg(not(feature = "chromium"))]
impl Downloader for ChromiumoxideDownloader {
    fn fetch<'a>(&'a self, _url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        Box::pin(async move {
            Err(DownloadError::Internal(
                "Chromiumoxide not enabled (compile with --features chromium)".to_string(),
            ))
        })
    }

    fn supports_interactions(&self) -> bool {
        false
    }

    fn memory_cost(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "chromium"))]
    #[tokio::test]
    async fn test_chromiumoxide_returns_stub_error() {
        let dl = ChromiumoxideDownloader::new(Arc::new(RwLock::new(CookieBridge::new())));
        let url: Url = "https://example.com".parse().unwrap();
        let err = dl.fetch(&url).await.unwrap_err();
        assert!(
            matches!(err, DownloadError::Internal(ref msg) if msg.contains("not enabled")),
            "expected stub error, got: {err}"
        );
    }

    #[test]
    #[cfg(feature = "chromium")]
    fn test_chromiumoxide_metadata() {
        let dl = ChromiumoxideDownloader::new(Arc::new(RwLock::new(CookieBridge::new())));
        assert!(dl.supports_interactions());
        assert_eq!(dl.memory_cost(), 200_000_000);
    }

    #[test]
    #[cfg(not(feature = "chromium"))]
    fn test_chromiumoxide_metadata_stub() {
        let dl = ChromiumoxideDownloader::new(Arc::new(RwLock::new(CookieBridge::new())));
        assert!(!dl.supports_interactions());
        assert_eq!(dl.memory_cost(), 0);
    }
}
