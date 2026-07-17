//! Crawl error types
//!
//! Following **err-thiserror-for-libraries**: Uses thiserror for library error types.
//! Following **api-non-exhaustive**: Can add variants without breaking changes.
//! Following **clean-architecture**: NO dependencies on reqwest/anyhow (Infra layer)
//!
//! # Architecture Note
//!
//! This error type does NOT contain `reqwest::Error` or `anyhow::Error`.
//! Those are infrastructure details. The Infrastructure layer converts
//! `reqwest::Error` → `CrawlError::Network` and `anyhow::Error` → specific variants.

use thiserror::Error;

/// Crawl errors
///
/// Following **err-thiserror-for-libraries**: Uses thiserror for library error types.
/// Following **api-non-exhaustive**: Can add variants without breaking changes.
/// Following **clean-architecture**: NO dependencies on reqwest/anyhow (Infra layer)
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CrawlError {
    /// Network error during HTTP request
    ///
    /// Note: Does NOT contain reqwest::Error (that's Infra detail).
    /// Infrastructure layer converts reqwest::Error → this variant.
    #[error("network error: {message} (status: {status_code:?})")]
    Network {
        message: String,
        status_code: Option<u16>,
    },

    /// HTTP error (status code or request failure)
    #[error("HTTP error: {0}")]
    Http(String),

    /// URL parsing error
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// HTML parsing error
    #[error("parse error: {0}")]
    Parse(String),

    /// Rate limit exceeded
    #[error("rate limit exceeded")]
    RateLimit,

    /// Maximum depth exceeded
    #[error("maximum depth {max} exceeded at depth {current}")]
    MaxDepthExceeded { current: u8, max: u8 },

    /// Maximum pages exceeded
    #[error("maximum pages {max} exceeded")]
    MaxPagesExceeded { max: usize },

    /// URL excluded by pattern
    #[error("URL excluded: {0}")]
    UrlExcluded(String),

    /// Invalid content type
    #[error("invalid content type: {0}")]
    InvalidContentType(String),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Semaphore error (concurrency control)
    #[error("semaphore error: {0}")]
    Semaphore(String),

    /// Internal error (unspecified)
    #[error("internal error: {0}")]
    Internal(String),

    /// Sitemap parsing error (FASE 3)
    /// Note: Does NOT contain sitemap_parser::SitemapError (that's Infra detail).
    /// Infrastructure layer converts SitemapError → this variant.
    #[error("sitemap error: {0}")]
    Sitemap(String),

    /// Sitemap not found during auto-discovery
    #[error("sitemap not found for {0}")]
    SitemapNotFound(String),

    /// Storage error (append-only log corruption, backpressure, serialization)
    #[error("error de almacenamiento: {0}")]
    Storage(String),

    /// Checkpoint serialization/deserialization error
    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    /// Session pool error (connection or lifecycle failure)
    #[error("session pool error: {0}")]
    SessionPool(String),

    /// Discovery error (robots.txt or sitemap auto-discovery failure)
    #[error("discovery error: {0}")]
    Discovery(String),

    /// Download error (fetch failed, SPA detected, or WAF blocked during download)
    ///
    /// Carries the underlying error as `#[source]` so the cause chain is
    /// preserved through `Error::source()` (D4).
    #[error("download error: {0}")]
    Download(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crawl_error_network_no_reqwest() {
        let error = CrawlError::Network {
            message: "timeout".to_string(),
            status_code: Some(408),
        };
        assert!(error.to_string().contains("timeout"));
        assert!(error.to_string().contains("408"));
    }

    #[test]
    fn test_crawl_error_network_no_status() {
        let error = CrawlError::Network {
            message: "connection refused".to_string(),
            status_code: None,
        };
        assert!(error.to_string().contains("connection refused"));
        assert!(error.to_string().contains("None"));
    }

    #[test]
    fn test_crawl_error_io() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let error = CrawlError::from(io_error);
        assert!(matches!(error, CrawlError::Io(_)));
        assert!(error.to_string().contains("file not found"));
    }

    #[test]
    fn test_crawl_error_semaphore() {
        let error = CrawlError::Semaphore("permit lost".to_string());
        assert!(error.to_string().contains("permit lost"));
    }

    #[test]
    fn test_crawl_error_internal() {
        let error = CrawlError::Internal("something went wrong".to_string());
        assert!(error.to_string().contains("something went wrong"));
    }

    #[test]
    fn test_crawl_error_storage_display() {
        let error = CrawlError::Storage("archivo corrupto".to_string());
        assert!(
            error.to_string().contains("error de almacenamiento"),
            "expected Storage display to contain 'error de almacenamiento', got: {}",
            error
        );
        assert!(error.to_string().contains("archivo corrupto"));
    }

    #[test]
    fn test_crawl_error_storage_empty_message() {
        let error = CrawlError::Storage(String::new());
        assert!(error.to_string().contains("error de almacenamiento"));
    }

    #[test]
    fn test_crawl_error_display_all_variants() {
        let error = CrawlError::InvalidUrl("bad-url".to_string());
        assert!(error.to_string().contains("bad-url"));

        let error = CrawlError::Parse("html parse failed".to_string());
        assert!(error.to_string().contains("html parse failed"));

        let error = CrawlError::RateLimit;
        assert_eq!(error.to_string(), "rate limit exceeded");

        let error = CrawlError::MaxDepthExceeded { current: 5, max: 3 };
        assert_eq!(error.to_string(), "maximum depth 3 exceeded at depth 5");

        let error = CrawlError::MaxPagesExceeded { max: 100 };
        assert_eq!(error.to_string(), "maximum pages 100 exceeded");

        let error = CrawlError::UrlExcluded("https://evil.com".to_string());
        assert!(error.to_string().contains("evil.com"));

        let error = CrawlError::InvalidContentType("image/png".to_string());
        assert!(error.to_string().contains("image/png"));
    }

    #[test]
    fn test_crawl_error_checkpoint() {
        let error = CrawlError::Checkpoint("json decode failed".to_string());
        assert!(error.to_string().contains("checkpoint error"));
        assert!(error.to_string().contains("json decode failed"));
    }

    #[test]
    fn test_crawl_error_session_pool() {
        let error = CrawlError::SessionPool("pool exhausted".to_string());
        assert!(error.to_string().contains("session pool error"));
        assert!(error.to_string().contains("pool exhausted"));
    }

    #[test]
    fn test_crawl_error_discovery() {
        let error = CrawlError::Discovery("robots.txt unreachable".to_string());
        assert!(error.to_string().contains("discovery error"));
        assert!(error.to_string().contains("robots.txt unreachable"));
    }

    #[test]
    fn test_crawl_error_download() {
        let error = CrawlError::Download(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection reset",
        )));
        assert!(error.to_string().contains("download error"));
        assert!(error.to_string().contains("connection reset"));
    }
}
