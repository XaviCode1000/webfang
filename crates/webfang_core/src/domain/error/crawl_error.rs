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

/// WAF detection classification for observability and retry decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WafDetectionKind {
    /// WAF detected via control/response headers
    ControlHeader,
    /// WAF detected via body signature patterns
    BodySignature,
    /// WAF detected via silent JavaScript challenge
    SilentChallenge,
    /// WAF detected via entropy anomaly in response
    EntropyAnomaly,
}

/// Resource type for resource exhaustion errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// Sitemap URL count limit
    SitemapUrls,
    /// Sitemap crawl depth limit
    SitemapDepth,
    /// RAM budget limit
    RamBudget,
}

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

    /// HTTP error with structured status code and URL
    #[error("HTTP error {status} at {url}")]
    Http {
        /// HTTP status code (e.g. 403, 429, 500)
        status: u16,
        /// URL that triggered the error
        url: String,
    },

    /// URL parsing error
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// HTML parsing error
    #[error("parse error: {0}")]
    Parse(String),

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

    /// Internal error (unspecified)
    #[error("internal error: {0}")]
    Internal(String),

    /// Sitemap not found during auto-discovery
    #[error("no sitemap found for {0}")]
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

    // === New variants (Error Map V2) ===
    /// WAF challenge detected during crawl
    #[error("WAF challenge: {provider} ({kind:?}) at {url}")]
    WafChallenge {
        provider: String,
        kind: WafDetectionKind,
        url: String,
    },

    /// Retry attempts exhausted for a URL
    #[error("retry exhausted for {url} after {attempts} attempts")]
    RetryExhausted { url: String, attempts: usize },

    /// Transient HTTP error (5xx, retryable)
    #[error("transient HTTP {status} at {url}")]
    TransientHttp { status: u16, url: String },

    /// Rate limited with retry-after duration in seconds
    #[error("rate limited, retry after {0}s")]
    RateLimited(u64),

    /// Request timeout
    #[error("request timeout")]
    Timeout,

    /// Connection error
    #[error("connection error: {0}")]
    Connection(String),

    /// Resource limit exhausted
    #[error("resource exhausted: {resource:?} limit={limit} actual={actual}")]
    ResourceExhausted {
        resource: ResourceKind,
        limit: usize,
        actual: usize,
    },

    /// No sitemap found (empty sitemap)
    #[error("no sitemap found")]
    SitemapEmpty,

    /// Sitemap crawl depth exceeded
    #[error("sitemap depth exceeded")]
    SitemapDepthExceeded,

    /// Semaphore exhausted (backpressure)
    #[error("semáforo agotado: no hay permisos disponibles")]
    SemaphoreInanition,
}

impl From<crate::domain::http_error::HttpError> for CrawlError {
    fn from(e: crate::domain::http_error::HttpError) -> Self {
        use crate::domain::http_error::HttpError;
        match e {
            HttpError::Forbidden => CrawlError::Http {
                status: 403,
                url: String::new(),
            },
            HttpError::RateLimited(retry_after) => CrawlError::RateLimited(retry_after),
            HttpError::ClientError(code) => CrawlError::Http {
                status: code,
                url: String::new(),
            },
            HttpError::ServerError(code) => CrawlError::Http {
                status: code,
                url: String::new(),
            },
            HttpError::Timeout => CrawlError::Timeout,
            HttpError::Connection(msg) => CrawlError::Connection(msg),
            HttpError::Request(msg) => CrawlError::Internal(msg),
            HttpError::WafChallenge(provider) => CrawlError::WafChallenge {
                provider,
                kind: WafDetectionKind::BodySignature,
                url: String::new(),
            },
        }
    }
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
    fn test_crawl_error_semaphore_inanition() {
        let error = CrawlError::SemaphoreInanition;
        assert!(error.to_string().contains("semáforo agotado"));
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

        let error = CrawlError::RateLimited(60);
        assert!(error.to_string().contains("60"));

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

    #[test]
    fn test_crawl_error_waf_challenge() {
        let error = CrawlError::WafChallenge {
            provider: "Cloudflare".to_string(),
            kind: WafDetectionKind::BodySignature,
            url: "https://example.com".to_string(),
        };
        assert!(error.to_string().contains("Cloudflare"));
        assert!(error.to_string().contains("example.com"));
    }

    #[test]
    fn test_crawl_error_retry_exhausted() {
        let error = CrawlError::RetryExhausted {
            url: "https://example.com".to_string(),
            attempts: 3,
        };
        assert!(error.to_string().contains("retry exhausted"));
        assert!(error.to_string().contains("3"));
    }

    #[test]
    fn test_crawl_error_transient_http() {
        let error = CrawlError::TransientHttp {
            status: 503,
            url: "https://example.com".to_string(),
        };
        assert!(error.to_string().contains("503"));
    }

    #[test]
    fn test_crawl_error_rate_limited() {
        let error = CrawlError::RateLimited(30);
        assert!(error.to_string().contains("30"));
    }

    #[test]
    fn test_crawl_error_timeout() {
        let error = CrawlError::Timeout;
        assert!(error.to_string().contains("timeout"));
    }

    #[test]
    fn test_crawl_error_connection() {
        let error = CrawlError::Connection("refused".to_string());
        assert!(error.to_string().contains("connection error"));
        assert!(error.to_string().contains("refused"));
    }

    #[test]
    fn test_crawl_error_resource_exhausted() {
        let error = CrawlError::ResourceExhausted {
            resource: ResourceKind::RamBudget,
            limit: 1024,
            actual: 2048,
        };
        assert!(error.to_string().contains("RamBudget"));
        assert!(error.to_string().contains("1024"));
        assert!(error.to_string().contains("2048"));
    }

    #[test]
    fn test_crawl_error_sitemap_empty() {
        let error = CrawlError::SitemapEmpty;
        assert!(error.to_string().contains("no sitemap found"));
    }

    #[test]
    fn test_crawl_error_sitemap_depth_exceeded() {
        let error = CrawlError::SitemapDepthExceeded;
        assert!(error.to_string().contains("sitemap depth exceeded"));
    }

    #[test]
    fn test_crawl_error_sitemap_not_found() {
        let error = CrawlError::SitemapNotFound("https://example.com".to_string());
        assert!(error.to_string().contains("no sitemap found"));
        assert!(error.to_string().contains("example.com"));
    }

    #[test]
    fn test_http_error_to_crawl_error_conversion() {
        let http_err = crate::domain::http_error::HttpError::Forbidden;
        let crawl_err: CrawlError = http_err.into();
        assert!(matches!(
            crawl_err,
            CrawlError::Http { status: 403, .. }
        ));
    }

    #[test]
    fn test_http_error_rate_limited_to_crawl_error() {
        let http_err = crate::domain::http_error::HttpError::RateLimited(60);
        let crawl_err: CrawlError = http_err.into();
        assert!(matches!(crawl_err, CrawlError::RateLimited(60)));
    }

    #[test]
    fn test_http_error_timeout_to_crawl_error() {
        let http_err = crate::domain::http_error::HttpError::Timeout;
        let crawl_err: CrawlError = http_err.into();
        assert!(matches!(crawl_err, CrawlError::Timeout));
    }

    #[test]
    fn test_http_error_waf_to_crawl_error() {
        let http_err = crate::domain::http_error::HttpError::WafChallenge("CF".to_string());
        let crawl_err: CrawlError = http_err.into();
        assert!(matches!(
            crawl_err,
            CrawlError::WafChallenge {
                provider,
                kind: WafDetectionKind::BodySignature,
                ..
            } if provider == "CF"
        ));
    }
}
