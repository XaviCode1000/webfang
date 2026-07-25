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
};

use std::sync::Arc;
use std::sync::RwLock;
use url::Url;

use super::cookie_bridge::CookieBridge;
#[cfg(feature = "chromium")]
use super::resource_governor::ResourceGovernor;
use super::{DownloadError, Downloader, FetchedPage};

/// Memory budget for one Chrome tab (~200 MB).
#[cfg(feature = "chromium")]
const CHROMIUMOXIDE_MEMORY_COST: usize = 200_000_000;

/// CDP downloader that spawns a headless Chrome instance per fetch.
pub struct ChromiumoxideDownloader {
    #[cfg(feature = "chromium")]
    governor: ResourceGovernor,
    #[cfg(feature = "chromium")]
    cookie_bridge: Arc<RwLock<CookieBridge>>,
}

impl ChromiumoxideDownloader {
    #[cfg(feature = "chromium")]
    pub(crate) fn new(cookie_bridge: Arc<RwLock<CookieBridge>>) -> Self {
        Self {
            governor: ResourceGovernor::new(),
            cookie_bridge,
        }
    }

    #[cfg(not(feature = "chromium"))]
    pub(crate) fn new(_cookie_bridge: Arc<RwLock<CookieBridge>>) -> Self {
        Self {}
    }
}

#[cfg(feature = "chromium")]
impl Downloader for ChromiumoxideDownloader {
    async fn fetch(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        // 1. RAM gating via resource governor
        let _permit = self.governor.acquire().await?;

        // 2. Browser config with sandbox bypass for CI/Docker
        let config = BrowserConfig::builder()
            .headless_mode(HeadlessMode::True)
            .no_sandbox()
            .build()
            .map_err(DownloadError::Internal)?;

        let (mut browser, mut handler) = Browser::launch(config)
            .await
            .map_err(|_| DownloadError::Internal("Chrome binary not found".into()))?;

        // 3. Process CDP messages in isolated task to prevent hangs
        let handler_job = tokio::spawn(async move { while handler.next().await.is_some() {} });

        // 4. Inject cookies from L1 cookie bridge before navigation
        let cdp_cookies: Vec<CookieParam> = {
            let bridge = self
                .cookie_bridge
                .read()
                .map_err(|e| DownloadError::Internal(e.to_string()))?;
            bridge
                .to_cdp_cookies()
                .into_iter()
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

        // 5. Navigate and extract rendered DOM
        page.goto(url.as_str())
            .await
            .map_err(|e| DownloadError::Internal(e.to_string()))?;

        let html = page
            .content()
            .await
            .map_err(|e| DownloadError::Internal(e.to_string()))?;

        // 6. Deterministic shutdown — prevents zombie processes
        browser.close().await.ok();
        handler_job.await.ok();

        Ok(FetchedPage {
            url: url.clone(),
            html,
            status: 200,
            cookies: Vec::new(),
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
    async fn fetch(&self, _url: &Url) -> Result<FetchedPage, DownloadError> {
        Err(DownloadError::Internal(
            "Chromiumoxide not enabled (compile with --features chromium)".to_string(),
        ))
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
