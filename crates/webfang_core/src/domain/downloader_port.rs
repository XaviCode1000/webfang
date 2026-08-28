//! Downloader port — domain-owned trait for page fetching.
//!
//! Extracted from `infrastructure::downloader` so `application::*` can depend
//! on the trait without `application→infrastructure` (ADR-0010). Infrastructure
//! (`WreqDownloader`, `ObscuraDownloader`, `ChromiumoxideDownloader`,
//! `HybridRouter`) implements this trait; application orchestrates via
//! `Arc<dyn Downloader>` (cloned before `.await`, never held across lock).

use std::collections::HashMap;

use futures::future::BoxFuture;
use url::Url;

/// Downloader trait for fetching pages — domain-owned port.
///
/// Dyn-compatible via `BoxFuture` (like `VectorRepository` in
/// `domain::repository`). Concrete impls live in `infrastructure::downloader`.
pub trait Downloader: Send + Sync {
    /// Fetch a page from the given URL.
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>>;

    /// Whether this downloader supports JavaScript rendering / interactions.
    fn supports_interactions(&self) -> bool;

    /// Estimated memory cost in bytes (for scheduler budgeting).
    fn memory_cost(&self) -> usize;
}

/// A page fetched by a [`Downloader`].
#[derive(Debug, Clone)]
pub struct FetchedPage {
    /// Final URL after redirects.
    pub url: Url,
    /// Raw HTML content.
    pub html: String,
    /// HTTP status code.
    pub status: u16,
    /// Response headers (keys lowercased).
    pub headers: HashMap<String, String>,
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
    /// Whether the cookie is HTTP-only.
    pub http_only: bool,
    /// Whether the cookie requires HTTPS.
    pub secure: bool,
}

/// Errors that can occur during page download.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DownloadError {
    /// Network-level failure (DNS, connection, timeout).
    #[error("network error: {0}")]
    Network(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// DNS resolution failure (NXDOMAIN).
    #[error("DNS error: {0}")]
    Dns(String),

    /// TLS/SSL failure (expired cert, self-signed).
    #[error("TLS error: {0}")]
    Tls(String),

    /// I/O error with preserved `std::io::ErrorKind`.
    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),

    /// HTTP error response (non-2xx status).
    #[error("HTTP {status}: {message}")]
    #[allow(missing_docs)]
    Http { status: u16, message: String },

    /// WAF challenge detected.
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

    /// System resources (RAM) insufficient to spawn a heavyweight downloader layer.
    #[error("insufficient resources: {0}")]
    ResourceExhausted(String),

    /// Feature not compiled in — typed so classification does not rely on string matching.
    #[error("funcionalidad no disponible: {0}")]
    FeatureGated(String),

    /// Acquisition cancelled by the engine's cancellation token.
    #[error("operation cancelled while waiting for resources")]
    Cancelled,
}

impl Clone for DownloadError {
    fn clone(&self) -> Self {
        match self {
            DownloadError::Network(e) => {
                if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                    DownloadError::Network(Box::new(std::io::Error::new(
                        io_err.kind(),
                        io_err.to_string(),
                    )))
                } else {
                    DownloadError::Internal(e.to_string())
                }
            },
            DownloadError::Dns(s) => DownloadError::Dns(s.clone()),
            DownloadError::Tls(s) => DownloadError::Tls(s.clone()),
            DownloadError::Io(e) => DownloadError::Io(std::io::Error::new(e.kind(), e.to_string())),
            DownloadError::Http { status, message } => DownloadError::Http {
                status: *status,
                message: message.clone(),
            },
            DownloadError::WafChallenge(s) => DownloadError::WafChallenge(s.clone()),
            DownloadError::SpaDetected(s) => DownloadError::SpaDetected(s.clone()),
            DownloadError::InvalidUrl(s) => DownloadError::InvalidUrl(s.clone()),
            DownloadError::Timeout(s) => DownloadError::Timeout(*s),
            DownloadError::Internal(s) => DownloadError::Internal(s.clone()),
            DownloadError::ResourceExhausted(s) => DownloadError::ResourceExhausted(s.clone()),
            DownloadError::FeatureGated(s) => DownloadError::FeatureGated(s.clone()),
            DownloadError::Cancelled => DownloadError::Cancelled,
        }
    }
}

impl DownloadError {
    /// Classify this download error by operational severity.
    #[must_use]
    pub fn classify(&self) -> crate::domain::error::ErrorClass {
        use crate::domain::error::ErrorClass;

        match self {
            Self::Io(e) if is_transient_io(e) => ErrorClass::TransientRetriable,
            Self::Io(_) => ErrorClass::InternalFatal,
            Self::Dns(_) | Self::Tls(_) => ErrorClass::PermanentFatal,
            Self::Timeout(_) => ErrorClass::TransientBackoff,
            Self::Http { status, .. } if (500..=599).contains(status) => {
                ErrorClass::TransientRetriable
            },
            Self::Http { status: 429, .. } => ErrorClass::TransientBackoff,
            Self::Http { status, .. } if (400..=499).contains(status) => ErrorClass::PermanentFatal,
            Self::Http { .. } => ErrorClass::PermanentFatal,
            Self::WafChallenge(_) | Self::SpaDetected(_) => ErrorClass::PermanentFatal,
            Self::InvalidUrl(_) => ErrorClass::PermanentFatal,
            Self::Network(_) => ErrorClass::InternalFatal,
            Self::Internal(_) => ErrorClass::InternalFatal,
            Self::ResourceExhausted(_) => ErrorClass::InternalFatal,
            Self::FeatureGated(_) => ErrorClass::PermanentFatal,
            Self::Cancelled => ErrorClass::InternalFatal,
        }
    }
}

fn is_transient_io(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{
        BrokenPipe, ConnectionAborted, ConnectionReset, TimedOut, UnexpectedEof,
    };

    matches!(
        e.kind(),
        ConnectionReset | ConnectionAborted | BrokenPipe | TimedOut | UnexpectedEof
    )
}

impl From<DownloadError> for crate::domain::CrawlError {
    fn from(err: DownloadError) -> Self {
        crate::domain::CrawlError::Download(Box::new(err))
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
            headers: HashMap::new(),
            cookies: vec![],
        };
        let cloned = page.clone();
        assert_eq!(page.url, cloned.url);
        assert_eq!(page.html, cloned.html);
    }

    #[test]
    fn test_download_error_display() {
        let err = DownloadError::Http {
            status: 403,
            message: "forbidden".into(),
        };
        assert!(err.to_string().contains("403"));
    }

    #[test]
    fn test_download_error_classify() {
        use crate::domain::error::ErrorClass;
        let dns = DownloadError::Dns("NXDOMAIN".into());
        assert_eq!(dns.classify(), ErrorClass::PermanentFatal);
        let timeout = DownloadError::Timeout(5);
        assert_eq!(timeout.classify(), ErrorClass::TransientBackoff);
    }

    #[test]
    fn test_downloader_trait_is_dyn_compatible() {
        // Compile-time proof that `dyn Downloader` is object-safe via BoxFuture
        #[allow(dead_code)]
        fn assert_dyn(_: &dyn Downloader) {}
        // If this compiles, the trait is dyn-compatible
        assert_eq!(1, 1);
    }
}
