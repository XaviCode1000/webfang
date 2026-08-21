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

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::SitemapUrls => "sitemap URLs",
            Self::SitemapDepth => "sitemap depth",
            Self::RamBudget => "RAM budget",
        };
        f.write_str(label)
    }
}

/// Crawl errors
///
/// Following **err-thiserror-for-libraries**: Uses thiserror for library error types.
/// Following **error-classification-matrix**: workspace-internal enum, NOT
/// `#[non_exhaustive]` — every variant must be classified in [`Self::classify`]
/// and in the matrix doc in the same change; the compiler enforces it.
/// Following **clean-architecture**: NO dependencies on reqwest/anyhow (Infra layer)
#[derive(Debug, Error)]
pub enum CrawlError {
    /// Network error during HTTP request
    ///
    /// Note: Does NOT contain reqwest::Error (that's Infra detail).
    /// Infrastructure layer converts reqwest::Error → this variant.
    #[error("network error: {message} (status: {status_code:?})")]
    Network {
        /// Human-readable error description.
        message: String,
        /// HTTP status code if the server responded; `None` for connection failures.
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
    MaxDepthExceeded {
        /// Depth at which the limit was exceeded.
        current: u8,
        /// Configured maximum crawl depth.
        max: u8,
    },

    /// Maximum pages exceeded
    #[error("maximum pages {max} exceeded")]
    MaxPagesExceeded {
        /// Configured maximum page count.
        max: usize,
    },

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
        /// Name of the WAF provider (e.g., "Cloudflare", "AWS WAF").
        provider: String,
        /// How the WAF challenge was detected.
        kind: WafDetectionKind,
        /// URL that triggered the WAF challenge.
        url: String,
    },

    /// Retry attempts exhausted for a URL
    #[error("retry exhausted for {url} after {attempts} attempts")]
    RetryExhausted {
        /// URL that failed after exhausting retries.
        url: String,
        /// Total number of attempts made.
        attempts: usize,
    },

    /// Transient HTTP error (5xx, retryable)
    #[error("transient HTTP {status} at {url}")]
    TransientHttp {
        /// HTTP status code (typically 5xx).
        status: u16,
        /// URL that returned the transient error.
        url: String,
    },

    /// Rate limited with retry-after duration in seconds
    #[error("rate limited, retry after {0}s")]
    RateLimited(u64),

    /// Request timeout
    #[error("request timeout")]
    Timeout,

    /// Connection error
    #[error("connection error: {0}")]
    Connection(String),

    /// Request construction or body-read failure (non-transient)
    ///
    /// Produced when the HTTP request itself cannot be built or its body
    /// cannot be read — as opposed to timeout/connection failures, which are
    /// transient. Maps to `ScraperError::Internal` (InternalFatal).
    #[error("request failed: {0}")]
    RequestFailed(String),

    /// Resource limit exhausted
    #[error("resource exhausted: {resource:?} limit={limit} actual={actual}")]
    ResourceExhausted {
        /// Which resource limit was hit.
        resource: ResourceKind,
        /// Configured limit value.
        limit: usize,
        /// Actual usage that exceeded the limit.
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

    /// Task cancelled by engine shutdown (#509)
    ///
    /// Control signal, not an operational failure: the crawl engine cancelled
    /// this task while it waited for a rate-limit permit or a resource-governor
    /// permit. Handlers must NOT count it as a crawl error.
    #[error("task cancelled by engine shutdown")]
    Cancelled,
}

impl CrawlError {
    /// Classify this error per the Error Classification Matrix.
    ///
    /// Contract: `docs/error-classification-matrix.md` (closed, ID
    /// `261bdb66-197e-420f-a73b-66c0e889102d`). The match is flat with ZERO
    /// wildcard arms: adding a variant without a classification fails
    /// compilation (matrix "Exhaustiveness enforcement" #1). Comments cite the
    /// matrix row each arm implements.
    #[must_use]
    pub fn classify(&self) -> crate::domain::error::ErrorClass {
        use crate::domain::error::ErrorClass;

        match self {
            // Rows 1/8: connection-reset-style and generic indeterminate
            // network errors are overwhelmingly transient (matrix rationale
            // for #8). Typed DNS/TLS precision (row 6) is decided upstream,
            // before the infrastructure layer erases the cause into this
            // variant.
            Self::Network { .. } => ErrorClass::TransientRetriable,
            // Row 2: HTTP 5xx.
            Self::Http { status, .. } if *status >= 500 => ErrorClass::TransientRetriable,
            // Row 3: 429 honors Retry-After.
            Self::Http { status, .. } if *status == 429 => ErrorClass::TransientBackoff,
            // Row 5: HTTP 4xx except 429.
            Self::Http { .. } => ErrorClass::PermanentFatal,
            // Row 15.
            Self::InvalidUrl(_) => ErrorClass::PermanentFatal,
            // Row 16.
            Self::Parse(_) => ErrorClass::DomainRecoverable,
            // Row 9: budget exhaustion by design.
            Self::MaxDepthExceeded { .. } | Self::MaxPagesExceeded { .. } => {
                ErrorClass::DomainRecoverable
            },
            // Row 10.
            Self::UrlExcluded(_) => ErrorClass::DomainRecoverable,
            // Row 18.
            Self::InvalidContentType(_) => ErrorClass::DomainRecoverable,
            // Rows 21/22: classify by `io::ErrorKind` per the matrix cell —
            // transient (`Interrupted`, `WouldBlock`, `TimedOut`) vs permanent
            // (`NotFound`, `PermissionDenied`, rest).
            Self::Io(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) =>
            {
                ErrorClass::TransientRetriable
            },
            // Row 22 (rest of the io kinds).
            Self::Io(_) => ErrorClass::PermanentFatal,
            // Matrix taxonomy: unspecified internal error = bug indicator
            // (InternalFatal family; cf. row 23).
            Self::Internal(_) => ErrorClass::InternalFatal,
            // Row 20.
            Self::SitemapNotFound(_) => ErrorClass::DomainRecoverable,
            // Row 23: data-integrity errors are NEVER retried (Gate 2).
            Self::Storage(_) | Self::Checkpoint(_) => ErrorClass::InternalFatal,
            // Row 24.
            Self::SessionPool(_) => ErrorClass::TransientBackoff,
            // Row 20 family: robots.txt/sitemap auto-discovery failure is a
            // single-site recoverable failure; the job continues without that
            // site (same semantics as SitemapNotFound).
            Self::Discovery(_) => ErrorClass::DomainRecoverable,
            // Rows 1/8: the boxed cause is type-erased at the domain boundary
            // (infrastructure boxes a `DownloadError` here); indeterminate
            // transport failures are transient. Typed precision survives the
            // upward path: `ScraperError::classify` re-downcasts the same box.
            Self::Download(_) => ErrorClass::TransientRetriable,
            // Row 7.
            Self::WafChallenge { .. } => ErrorClass::PermanentFatal,
            // Taxonomy definition of DomainRecoverable: single-item failure;
            // the job continues without that URL. The retry budget is spent,
            // so no class offering another retry applies.
            Self::RetryExhausted { .. } => ErrorClass::DomainRecoverable,
            // Row 2: the variant is defined as transient 5xx.
            Self::TransientHttp { .. } => ErrorClass::TransientRetriable,
            // Row 3.
            Self::RateLimited(_) => ErrorClass::TransientBackoff,
            // Row 4.
            Self::Timeout => ErrorClass::TransientBackoff,
            // Row 1.
            Self::Connection(_) => ErrorClass::TransientRetriable,
            // Variant contract (variant doc): non-transient request
            // construction/body-read failure maps to InternalFatal upstream.
            Self::RequestFailed(_) => ErrorClass::InternalFatal,
            // Rows 11/12: sitemap budget kinds are by-design stops;
            // RamBudget backpressure resolves itself when memory frees.
            Self::ResourceExhausted {
                resource: ResourceKind::SitemapUrls | ResourceKind::SitemapDepth,
                ..
            } => ErrorClass::DomainRecoverable,
            Self::ResourceExhausted {
                resource: ResourceKind::RamBudget,
                ..
            } => ErrorClass::TransientBackoff,
            // Row 20.
            Self::SitemapEmpty => ErrorClass::DomainRecoverable,
            // Row 11.
            Self::SitemapDepthExceeded => ErrorClass::DomainRecoverable,
            // Row 13: backpressure configuration bug.
            Self::SemaphoreInanition => ErrorClass::InternalFatal,
            // Special cell — Cancelled: cooperative control signal intercepted
            // at the CLI boundary; this defensive fallback ensures an escaped
            // signal can never be retried or silently swallowed during teardown.
            Self::Cancelled => ErrorClass::InternalFatal,
        }
    }
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
            HttpError::Request(msg) => CrawlError::RequestFailed(msg),
            HttpError::WafChallenge(provider) => CrawlError::WafChallenge {
                provider,
                kind: WafDetectionKind::BodySignature,
                url: String::new(),
            },
            HttpError::DomainBanned(domain) => {
                CrawlError::SessionPool(format!("domain banned: {domain}"))
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
    fn test_crawl_error_cancelled_display() {
        let error = CrawlError::Cancelled;
        assert_eq!(error.to_string(), "task cancelled by engine shutdown");
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
            "expected Storage display to contain 'error de almacenamiento', got: {error}"
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
    fn test_resource_kind_display() {
        // EC-RESOURCE-DISPLAY: human-friendly Display per variant.
        assert_eq!(ResourceKind::SitemapUrls.to_string(), "sitemap URLs");
        assert_eq!(ResourceKind::SitemapDepth.to_string(), "sitemap depth");
        assert_eq!(ResourceKind::RamBudget.to_string(), "RAM budget");

        // ResourceExhausted must keep rendering the resource via Debug
        // (`{resource:?}`), so the machine-readable variant name stays in the
        // error string even though Display now exists (type-display-vs-debug).
        let error = CrawlError::ResourceExhausted {
            resource: ResourceKind::RamBudget,
            limit: 1024,
            actual: 2048,
        };
        assert!(
            error.to_string().contains("RamBudget"),
            "ResourceExhausted must keep Debug rendering of the resource: {error}"
        );
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
        assert!(matches!(crawl_err, CrawlError::Http { status: 403, .. }));
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

    // ====================================================================
    // Error Classification Matrix (DoD #2/#3)
    //
    // Contract: docs/error-classification-matrix.md (closed, ID
    // 261bdb66-197e-420f-a73b-66c0e889102d). One test per CrawlError
    // variant; every variant must be classified to its matrix row.
    // ====================================================================
    mod classify {
        use super::*;
        use crate::error::ErrorClass;

        fn assert_class(err: CrawlError, expected: ErrorClass) {
            assert_eq!(err.classify(), expected, "wrong matrix class for {err:?}");
        }

        // Family 1 — Network / HTTP / Timeout

        #[test]
        fn network_generic_is_transient_retriable() {
            // Matrix rows 1/8: connection-reset-style and indeterminate
            // network errors are overwhelmingly transient.
            assert_class(
                CrawlError::Network {
                    message: "connection reset".to_string(),
                    status_code: None,
                },
                ErrorClass::TransientRetriable,
            );
        }

        #[test]
        fn http_5xx_is_transient_retriable() {
            // Matrix row 2.
            assert_class(
                CrawlError::Http {
                    status: 503,
                    url: "https://example.com".to_string(),
                },
                ErrorClass::TransientRetriable,
            );
        }

        #[test]
        fn http_429_is_transient_backoff() {
            // Matrix row 3: honors Retry-After.
            assert_class(
                CrawlError::Http {
                    status: 429,
                    url: "https://example.com".to_string(),
                },
                ErrorClass::TransientBackoff,
            );
        }

        #[test]
        fn http_4xx_is_permanent_fatal() {
            // Matrix row 5: 4xx except 429 never succeeds on retry.
            assert_class(
                CrawlError::Http {
                    status: 404,
                    url: "https://example.com".to_string(),
                },
                ErrorClass::PermanentFatal,
            );
        }

        #[test]
        fn transient_http_is_transient_retriable() {
            // Matrix row 2: the variant is defined as 5xx retryable.
            assert_class(
                CrawlError::TransientHttp {
                    status: 500,
                    url: "https://example.com".to_string(),
                },
                ErrorClass::TransientRetriable,
            );
        }

        #[test]
        fn rate_limited_is_transient_backoff() {
            // Matrix row 3.
            assert_class(CrawlError::RateLimited(60), ErrorClass::TransientBackoff);
        }

        #[test]
        fn timeout_is_transient_backoff() {
            // Matrix row 4.
            assert_class(CrawlError::Timeout, ErrorClass::TransientBackoff);
        }

        #[test]
        fn connection_is_transient_retriable() {
            // Matrix row 1.
            assert_class(
                CrawlError::Connection("refused".to_string()),
                ErrorClass::TransientRetriable,
            );
        }

        #[test]
        fn waf_challenge_is_permanent_fatal() {
            // Matrix row 7.
            assert_class(
                CrawlError::WafChallenge {
                    provider: "Cloudflare".to_string(),
                    kind: WafDetectionKind::BodySignature,
                    url: "https://example.com".to_string(),
                },
                ErrorClass::PermanentFatal,
            );
        }

        #[test]
        fn download_is_transient_retriable() {
            // Matrix rows 1/8: the boxed cause is type-erased at the domain
            // boundary; indeterminate transport failures are transient.
            assert_class(
                CrawlError::Download(Box::new(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "reset",
                ))),
                ErrorClass::TransientRetriable,
            );
        }

        // Family 2 — Limits / Budgets

        #[test]
        fn max_depth_exceeded_is_domain_recoverable() {
            // Matrix row 9: budget exhaustion by design.
            assert_class(
                CrawlError::MaxDepthExceeded { current: 5, max: 3 },
                ErrorClass::DomainRecoverable,
            );
        }

        #[test]
        fn max_pages_exceeded_is_domain_recoverable() {
            // Matrix row 9.
            assert_class(
                CrawlError::MaxPagesExceeded { max: 100 },
                ErrorClass::DomainRecoverable,
            );
        }

        #[test]
        fn url_excluded_is_domain_recoverable() {
            // Matrix row 10.
            assert_class(
                CrawlError::UrlExcluded("https://spam.com".to_string()),
                ErrorClass::DomainRecoverable,
            );
        }

        #[test]
        fn resource_exhausted_sitemap_kinds_are_domain_recoverable() {
            // Matrix row 11.
            assert_class(
                CrawlError::ResourceExhausted {
                    resource: ResourceKind::SitemapUrls,
                    limit: 100,
                    actual: 101,
                },
                ErrorClass::DomainRecoverable,
            );
            assert_class(
                CrawlError::ResourceExhausted {
                    resource: ResourceKind::SitemapDepth,
                    limit: 10,
                    actual: 11,
                },
                ErrorClass::DomainRecoverable,
            );
        }

        #[test]
        fn resource_exhausted_ram_budget_is_transient_backoff() {
            // Matrix row 12: backpressure resolves itself when memory frees.
            assert_class(
                CrawlError::ResourceExhausted {
                    resource: ResourceKind::RamBudget,
                    limit: 1024,
                    actual: 2048,
                },
                ErrorClass::TransientBackoff,
            );
        }

        #[test]
        fn semaphore_inanition_is_internal_fatal() {
            // Matrix row 13: backpressure configuration bug.
            assert_class(CrawlError::SemaphoreInanition, ErrorClass::InternalFatal);
        }

        // Family 3 — Domain / Content

        #[test]
        fn invalid_url_is_permanent_fatal() {
            // Matrix row 15.
            assert_class(
                CrawlError::InvalidUrl("bad-url".to_string()),
                ErrorClass::PermanentFatal,
            );
        }

        #[test]
        fn parse_is_domain_recoverable() {
            // Matrix row 16.
            assert_class(
                CrawlError::Parse("html parse failed".to_string()),
                ErrorClass::DomainRecoverable,
            );
        }

        #[test]
        fn invalid_content_type_is_domain_recoverable() {
            // Matrix row 18.
            assert_class(
                CrawlError::InvalidContentType("image/png".to_string()),
                ErrorClass::DomainRecoverable,
            );
        }

        #[test]
        fn sitemap_empty_is_domain_recoverable() {
            // Matrix row 20.
            assert_class(CrawlError::SitemapEmpty, ErrorClass::DomainRecoverable);
        }

        #[test]
        fn sitemap_not_found_is_domain_recoverable() {
            // Matrix row 20.
            assert_class(
                CrawlError::SitemapNotFound("https://example.com".to_string()),
                ErrorClass::DomainRecoverable,
            );
        }

        #[test]
        fn sitemap_depth_exceeded_is_domain_recoverable() {
            // Matrix row 11.
            assert_class(
                CrawlError::SitemapDepthExceeded,
                ErrorClass::DomainRecoverable,
            );
        }

        // Family 4 — Persistence / Internal infrastructure / AI

        #[test]
        fn io_transient_kinds_are_transient_retriable() {
            // Matrix row 21: Interrupted / WouldBlock / TimedOut.
            for kind in [
                std::io::ErrorKind::Interrupted,
                std::io::ErrorKind::WouldBlock,
                std::io::ErrorKind::TimedOut,
            ] {
                assert_class(
                    CrawlError::Io(std::io::Error::new(kind, "io hiccup")),
                    ErrorClass::TransientRetriable,
                );
            }
        }

        #[test]
        fn io_permanent_kinds_are_permanent_fatal() {
            // Matrix row 22: NotFound / PermissionDenied / rest.
            for kind in [
                std::io::ErrorKind::NotFound,
                std::io::ErrorKind::PermissionDenied,
                std::io::ErrorKind::Other,
            ] {
                assert_class(
                    CrawlError::Io(std::io::Error::new(kind, "io failure")),
                    ErrorClass::PermanentFatal,
                );
            }
        }

        #[test]
        fn storage_is_internal_fatal() {
            // Matrix row 23: data-integrity errors are NEVER retried.
            assert_class(
                CrawlError::Storage("append-log corrupt".to_string()),
                ErrorClass::InternalFatal,
            );
        }

        #[test]
        fn checkpoint_is_internal_fatal() {
            // Matrix row 23.
            assert_class(
                CrawlError::Checkpoint("json decode failed".to_string()),
                ErrorClass::InternalFatal,
            );
        }

        #[test]
        fn session_pool_is_transient_backoff() {
            // Matrix row 24.
            assert_class(
                CrawlError::SessionPool("pool exhausted".to_string()),
                ErrorClass::TransientBackoff,
            );
        }

        #[test]
        fn internal_is_internal_fatal() {
            // Matrix taxonomy: unspecified internal error = bug indicator.
            assert_class(
                CrawlError::Internal("unreachable state".to_string()),
                ErrorClass::InternalFatal,
            );
        }

        #[test]
        fn request_failed_is_internal_fatal() {
            // Variant contract (crawl_error.rs doc): non-transient request
            // construction/body-read failure maps to InternalFatal.
            assert_class(
                CrawlError::RequestFailed("body read failed".to_string()),
                ErrorClass::InternalFatal,
            );
        }

        // Variants resolved by matrix family semantics (not named verbatim
        // in the matrix); see classify() comments for the row references.

        #[test]
        fn discovery_is_domain_recoverable() {
            // Row 20 family: single-site discovery failure; job continues.
            assert_class(
                CrawlError::Discovery("robots.txt unreachable".to_string()),
                ErrorClass::DomainRecoverable,
            );
        }

        #[test]
        fn retry_exhausted_is_domain_recoverable() {
            // Taxonomy: single-item failure; job continues without it.
            assert_class(
                CrawlError::RetryExhausted {
                    url: "https://example.com".to_string(),
                    attempts: 3,
                },
                ErrorClass::DomainRecoverable,
            );
        }

        // Special cell — Cancelled

        #[test]
        fn cancelled_is_internal_fatal_defensive_fallback() {
            // Matrix special cell: cooperative control signal intercepted at
            // the CLI boundary; if it ever escapes, classify() must return
            // InternalFatal so it can never be retried or swallowed.
            assert_class(CrawlError::Cancelled, ErrorClass::InternalFatal);
        }
    }
}
