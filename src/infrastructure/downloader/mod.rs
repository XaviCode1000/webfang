//! Downloader abstraction for page fetching.
//!
//! Provides the [`Downloader`] trait and supporting types for fetching pages.
//! Implementations handle HTTP requests, cookie extraction, and connection pooling.
//!
//! # Architecture
//!
//! This module follows Clean Architecture: the trait defines the contract,
//! and concrete implementations (like [`WreqDownloader`]) live in the same
//! module but are swapped via dependency injection.
//!
//! [`WreqDownloader`]: wreq_downloader::WreqDownloader

// Allow async fn in traits — project convention (see domain/repository.rs).
// The trait is not dyn-compatible, which is intentional for this use case.
#![allow(async_fn_in_trait)]

pub mod spa_detector;
pub mod wreq_downloader;

use url::Url;

/// Downloader trait for fetching pages.
///
/// Implementations must be safe to share across threads (`Send + Sync`).
/// Each implementation owns its connection pool and request configuration.
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::infrastructure::downloader::{WreqDownloader, Downloader};
///
/// # tokio_test::block_on(async {
/// let downloader = WreqDownloader::new(30, 10);
/// let page = downloader.fetch(&"https://example.com".parse().unwrap()).await.unwrap();
/// assert!(!page.html.is_empty());
/// # });
/// ```
pub trait Downloader: Send + Sync {
    /// Fetch a page from the given URL.
    ///
    /// Returns the fetched page with HTML content, HTTP status, and cookies.
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError`] on network failure, timeout, or WAF detection.
    async fn fetch(&self, url: &Url) -> Result<FetchedPage, DownloadError>;

    /// Whether this downloader supports JavaScript rendering / interactions.
    ///
    /// Static HTTP downloaders return `false`. Headless browser implementations
    /// return `true`.
    fn supports_interactions(&self) -> bool;

    /// Estimated memory cost of this downloader instance in bytes.
    ///
    /// Used by the scheduler to budget total memory across concurrent downloaders.
    fn memory_cost(&self) -> usize;
}

/// A page fetched by a [`Downloader`].
#[derive(Debug, Clone)]
pub struct FetchedPage {
    /// The final URL after redirects.
    pub url: Url,
    /// Raw HTML content of the page.
    pub html: String,
    /// HTTP status code.
    pub status: u16,
    /// Cookies set by the server during this request.
    pub cookies: Vec<Cookie>,
}

/// An HTTP cookie extracted from a response.
#[derive(Debug, Clone)]
pub struct Cookie {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// Domain the cookie applies to.
    pub domain: String,
    /// Path prefix the cookie applies to.
    pub path: String,
    /// Whether the cookie is HTTP-only (not accessible via JavaScript).
    pub http_only: bool,
    /// Whether the cookie requires HTTPS.
    pub secure: bool,
}

/// Errors that can occur during page download.
///
/// Following **api-non-exhaustive**: can add variants without breaking changes.
/// Following **err-thiserror-lib**: uses thiserror for structured error messages.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DownloadError {
    /// Network-level failure (DNS, connection, timeout).
    #[error("network error: {0}")]
    Network(String),

    /// HTTP error response (non-2xx status).
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },

    /// WAF challenge detected (Cloudflare, reCAPTCHA, etc.).
    #[error("WAF challenge detected: {0}")]
    WafChallenge(String),

    /// SPA detected — page requires JavaScript rendering.
    #[error("SPA detected: {0}")]
    SpaDetected(String),

    /// URL is invalid or has an unsupported scheme.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// Request timed out.
    #[error("request timed out after {0}s")]
    Timeout(u64),

    /// Internal error (should not happen in normal operation).
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<DownloadError> for crate::domain::CrawlError {
    fn from(err: DownloadError) -> Self {
        crate::domain::CrawlError::Download(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fetched_page_clone() {
        let page = FetchedPage {
            url: "https://example.com".parse().unwrap(),
            html: "<html></html>".to_string(),
            status: 200,
            cookies: vec![],
        };
        let cloned = page.clone();
        assert_eq!(page.url, cloned.url);
        assert_eq!(page.html, cloned.html);
    }

    #[test]
    fn test_cookie_struct() {
        let cookie = Cookie {
            name: "session".into(),
            value: "abc123".into(),
            domain: ".example.com".into(),
            path: "/".into(),
            http_only: true,
            secure: true,
        };
        assert!(cookie.http_only);
        assert!(cookie.secure);
    }

    #[test]
    fn test_download_error_display() {
        let err = DownloadError::Network("connection refused".into());
        assert!(err.to_string().contains("connection refused"));

        let err = DownloadError::Http {
            status: 403,
            message: "forbidden".into(),
        };
        assert!(err.to_string().contains("403"));

        let err = DownloadError::WafChallenge("Cloudflare".into());
        assert!(err.to_string().contains("Cloudflare"));

        let err = DownloadError::SpaDetected("React SPA".into());
        assert!(err.to_string().contains("React SPA"));

        let err = DownloadError::Timeout(30);
        assert!(err.to_string().contains("30"));
    }

    #[test]
    fn test_download_error_into_crawl_error() {
        let err = DownloadError::Network("reset".into());
        let crawl_err: crate::domain::CrawlError = err.into();
        assert!(crawl_err.to_string().contains("download error"));
        assert!(crawl_err.to_string().contains("reset"));
    }
}
