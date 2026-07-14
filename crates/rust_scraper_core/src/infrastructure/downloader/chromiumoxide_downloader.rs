//! Chromiumoxide (CDP) downloader — placeholder.
//!
//! Full Chrome DevTools Protocol integration is planned for a future PR.
//! This stub satisfies the trait contract so the HybridRouter can reference
//! the Layer 3 slot without compile errors.

use url::Url;

use super::{DownloadError, Downloader, FetchedPage};

/// Memory budget for one Chrome tab (~200 MB).
const CHROMIUMOXIDE_MEMORY_COST: usize = 200_000_000;

/// Stub CDP downloader. Every `fetch` call returns an explicit "not ready" error.
pub struct ChromiumoxideDownloader;

impl ChromiumoxideDownloader {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ChromiumoxideDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl Downloader for ChromiumoxideDownloader {
    async fn fetch(&self, _url: &Url) -> Result<FetchedPage, DownloadError> {
        Err(DownloadError::Internal(
            "Chromiumoxide not yet integrated".to_string(),
        ))
    }

    fn supports_interactions(&self) -> bool {
        true
    }

    fn memory_cost(&self) -> usize {
        CHROMIUMOXIDE_MEMORY_COST
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_chromiumoxide_returns_stub_error() {
        let dl = ChromiumoxideDownloader::new();
        let url: Url = "https://example.com".parse().unwrap();
        let err = dl.fetch(&url).await.unwrap_err();
        assert!(
            matches!(err, DownloadError::Internal(ref msg) if msg.contains("not yet integrated")),
            "expected stub error, got: {err}"
        );
    }

    #[test]
    fn test_chromiumoxide_metadata() {
        let dl = ChromiumoxideDownloader;
        assert!(dl.supports_interactions());
        assert_eq!(dl.memory_cost(), 200_000_000);
    }
}
