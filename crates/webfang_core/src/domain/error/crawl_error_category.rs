//! Crawl error categorization for observability (issue #374).
//!
//! Maps the 20+ `CrawlError` variants into a fixed set of operational
//! categories suitable for metrics, structured logging, and dashboards.

use serde::Serialize;

use crate::domain::CrawlError;

/// Operational category for a crawl error.
///
/// Reduces the full `CrawlError` variant space to a fixed set of
/// categories that are meaningful in dashboards and alerts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CrawlErrorCategory {
    /// WAF challenge detected (Cloudflare, AWS WAF, etc.)
    Waf,
    /// HTTP status errors (4xx/5xx, transient)
    Http,
    /// Request timeout
    Timeout,
    /// Network-level failures (DNS, connection, download)
    Network,
    /// Rate limiting (429 / retry-after)
    RateLimit,
    /// Content extraction / parsing failures
    Extraction,
    /// Internal / catch-all (storage, checkpoint, config, etc.)
    Internal,
    /// Task panicked (JoinError)
    Panic,
}

impl CrawlErrorCategory {
    /// All categories in stable order (for array indexing and iteration).
    pub const ALL: [Self; 8] = [
        Self::Waf,
        Self::Http,
        Self::Timeout,
        Self::Network,
        Self::RateLimit,
        Self::Extraction,
        Self::Internal,
        Self::Panic,
    ];

    /// Index for array-based counting.
    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Waf => 0,
            Self::Http => 1,
            Self::Timeout => 2,
            Self::Network => 3,
            Self::RateLimit => 4,
            Self::Extraction => 5,
            Self::Internal => 6,
            Self::Panic => 7,
        }
    }

    /// Tracing field name for this category.
    #[inline]
    #[must_use]
    pub const fn tracing_field(self) -> &'static str {
        match self {
            Self::Waf => "errors_waf",
            Self::Http => "errors_http",
            Self::Timeout => "errors_timeout",
            Self::Network => "errors_network",
            Self::RateLimit => "errors_rate_limit",
            Self::Extraction => "errors_extraction",
            Self::Internal => "errors_internal",
            Self::Panic => "errors_panic",
        }
    }
}

impl std::fmt::Display for CrawlErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Waf => "waf",
            Self::Http => "http",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::RateLimit => "rate_limit",
            Self::Extraction => "extraction",
            Self::Internal => "internal",
            Self::Panic => "panic",
        };
        f.write_str(label)
    }
}

/// HTTP status codes that signal a WAF-style challenge rather than a generic
/// HTTP error. These are the access-denied / rate-limit family of responses
/// (401 Unauthorized, 403 Forbidden, 429 Too Many Requests) that operators
/// should attribute to WAF blocking in `errors_waf` (issue #603).
#[inline]
#[must_use]
fn is_waf_status(status: u16) -> bool {
    matches!(status, 401 | 403 | 429)
}

impl From<&CrawlError> for CrawlErrorCategory {
    fn from(err: &CrawlError) -> Self {
        match err {
            CrawlError::WafChallenge { .. } => Self::Waf,
            CrawlError::Http { status, .. } | CrawlError::TransientHttp { status, .. } => {
                if is_waf_status(*status) {
                    Self::Waf
                } else {
                    Self::Http
                }
            },
            CrawlError::Timeout => Self::Timeout,
            CrawlError::Network { .. } | CrawlError::Connection(..) | CrawlError::Download(..) => {
                Self::Network
            },
            CrawlError::RateLimited(..) => Self::RateLimit,
            CrawlError::Parse(..) => Self::Extraction,
            _ => Self::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::WafDetectionKind;

    #[test]
    fn categorize_waf() {
        let err = CrawlError::WafChallenge {
            provider: "Cloudflare".into(),
            kind: WafDetectionKind::BodySignature,
            url: "https://example.com".into(),
        };
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Waf);
    }

    #[test]
    fn categorize_http() {
        let err = CrawlError::Http {
            status: 404,
            url: "https://example.com".into(),
        };
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Http);

        let err = CrawlError::TransientHttp {
            status: 503,
            url: "https://example.com".into(),
        };
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Http);
    }

    #[test]
    fn categorize_waf_status_as_waf() {
        // Issue #603: 401/403/429 are WAF challenges, not generic HTTP errors.
        for status in [401u16, 403, 429] {
            let err = CrawlError::Http {
                status,
                url: "https://example.com".into(),
            };
            assert_eq!(
                CrawlErrorCategory::from(&err),
                CrawlErrorCategory::Waf,
                "status {status} must be classified as waf"
            );

            let err = CrawlError::TransientHttp {
                status,
                url: "https://example.com".into(),
            };
            assert_eq!(
                CrawlErrorCategory::from(&err),
                CrawlErrorCategory::Waf,
                "transient status {status} must be classified as waf"
            );
        }
    }

    #[test]
    fn categorize_other_http_status_as_http() {
        for status in [400u16, 404, 500, 503] {
            let err = CrawlError::Http {
                status,
                url: "https://example.com".into(),
            };
            assert_eq!(
                CrawlErrorCategory::from(&err),
                CrawlErrorCategory::Http,
                "status {status} is a generic HTTP error, not waf"
            );
        }
    }

    #[test]
    fn categorize_timeout() {
        let err = CrawlError::Timeout;
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Timeout);
    }

    #[test]
    fn categorize_network() {
        let err = CrawlError::Network {
            message: "dns failure".into(),
            status_code: None,
        };
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Network);

        let err = CrawlError::Connection("refused".into());
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Network);

        let err = CrawlError::Download(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        )));
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Network);
    }

    #[test]
    fn categorize_rate_limit() {
        let err = CrawlError::RateLimited(60);
        assert_eq!(
            CrawlErrorCategory::from(&err),
            CrawlErrorCategory::RateLimit
        );
    }

    #[test]
    fn categorize_parse_as_extraction() {
        let err = CrawlError::Parse("malformed html".into());
        assert_eq!(
            CrawlErrorCategory::from(&err),
            CrawlErrorCategory::Extraction
        );
    }

    #[test]
    fn categorize_internal_fallback() {
        let err = CrawlError::Internal("unknown".into());
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Internal);

        let err = CrawlError::Storage("corrupt".into());
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Internal);

        let err = CrawlError::Checkpoint("decode failed".into());
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Internal);

        let err = CrawlError::InvalidUrl("bad".into());
        assert_eq!(CrawlErrorCategory::from(&err), CrawlErrorCategory::Internal);
    }

    #[test]
    fn all_categories_have_unique_index() {
        let mut seen = [false; 8];
        for cat in CrawlErrorCategory::ALL {
            let idx = cat.index();
            assert!(!seen[idx], "duplicate index {idx} for {cat}");
            seen[idx] = true;
        }
        assert!(seen.iter().all(|&s| s), "all indices must be covered");
    }

    #[test]
    fn display_matches_serde_rename() {
        for cat in CrawlErrorCategory::ALL {
            let display = cat.to_string();
            let tracing = cat.tracing_field();
            assert!(
                tracing.starts_with("errors_"),
                "tracing_field must start with errors_: {tracing}"
            );
            assert!(
                tracing.contains(&display),
                "tracing_field {tracing} must contain display {display}"
            );
        }
    }
}
