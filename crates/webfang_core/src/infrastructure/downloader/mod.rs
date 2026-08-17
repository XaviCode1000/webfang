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

// The `Downloader` trait is dyn-compatible: `fetch` uses manual `async fn`
// desugaring to `BoxFuture` (matching the `VectorRepository` precedent in
// `domain/repository.rs`) so `&dyn Downloader` can be passed for runtime
// dispatch and test mocking.

pub mod chromiumoxide_downloader;
pub mod cookie_bridge;
pub mod hybrid_router;
pub mod obscura_downloader;
pub mod resource_governor;
pub mod spa_detector;
pub mod wreq_downloader;

use std::collections::HashMap;

use futures::future::BoxFuture;
use url::Url;

/// Downloader trait for fetching pages.
///
/// Implementations must be safe to share across threads (`Send + Sync`).
/// Each implementation owns its connection pool and request configuration.
///
/// # Examples
///
/// ```ignore
/// use webfang_core::infrastructure::downloader::wreq_downloader::WreqDownloader;
/// use webfang_core::infrastructure::downloader::Downloader;
///
/// let downloader = WreqDownloader::new(30, 10, wreq_util::Profile::Chrome145, None, 3, 1000, 10000).unwrap();
/// let page = downloader.fetch(&"https://example.com".parse().unwrap()).await.unwrap();
/// assert!(!page.html.is_empty());
/// ```
pub trait Downloader: Send + Sync {
    /// Fetch a page from the given URL.
    ///
    /// Returns the fetched page with HTML content, HTTP status, headers, and cookies.
    ///
    /// Desugared to [`BoxFuture`] (instead of native `async fn`) so the trait is
    /// dyn-compatible — `&dyn Downloader` can be passed for runtime dispatch and
    /// test mocking. Matches the `VectorRepository` precedent.
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError`] on network failure, timeout, or WAF detection.
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>>;

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
    /// Response headers (keys lowercased). Parity with domain `HttpResponse`.
    ///
    /// Used for content-type sniffing (binary detection) and filename derivation.
    /// Downloaders with limited header access (e.g. subprocess-based) may leave
    /// this empty.
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
    ///
    /// Carries the underlying error as `#[source]` (erased) so the root-cause
    /// chain survives into `CrawlError`/`ScraperError` (D4).
    #[error("network error: {0}")]
    Network(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// DNS resolution failure (NXDOMAIN, resolution error).
    ///
    /// Distinct from [`Tls`](Self::Tls) and [`Io`](Self::Io) so callers can
    /// classify a non-resolvable host as permanently fatal (#649).
    #[error("DNS error: {0}")]
    Dns(String),

    /// TLS/SSL failure (expired cert, self-signed, hostname mismatch).
    #[error("TLS error: {0}")]
    Tls(String),

    /// I/O error with preserved [`std::io::ErrorKind`] for classification.
    ///
    /// Mid-body peer drops (`UnexpectedEof`, `ConnectionReset`) reach this
    /// variant with their kind intact, so retry logic can treat them as
    /// transient instead of a bug (#649).
    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),

    /// HTTP error response (non-2xx status).
    #[error("HTTP {status}: {message}")]
    #[allow(missing_docs)] // enum variant fields can't have pub(crate) visibility
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

    /// System resources (RAM) insufficient to spawn a heavyweight downloader
    /// layer. An *expected* operational condition under memory pressure —
    /// distinct from [`Internal`](Self::Internal), which means a bug.
    #[error("insufficient resources: {0}")]
    ResourceExhausted(String),

    /// Acquisition cancelled by the engine's cancellation token (#509) —
    /// the caller was waiting for a resource permit when shutdown fired.
    #[error("operation cancelled while waiting for resources")]
    Cancelled,
}

// Manual `Clone`: `DownloadError` carries a `Box<dyn Error + Send + Sync>`
// (the `Network` variant), which is not `Clone`. We reconstruct it by
// downcasting a `std::io::Error` when possible (the common case for simulated
// failures) and otherwise preserving the displayed cause in an `Internal`
// variant. This keeps the type usable in test doubles that need an owned copy
// of the error (e.g. `StubDownloader::fails_with`).
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
            DownloadError::Cancelled => DownloadError::Cancelled,
        }
    }
}

impl From<wreq::Error> for DownloadError {
    /// Walk the transport error's cause chain to recover a typed variant.
    ///
    /// `wreq` erases the root cause behind its own error type, so DNS, TLS and
    /// I/O failures are otherwise indistinguishable (#649). The first typed
    /// `io::Error` wins (kind preserved); otherwise the cause text is matched
    /// for DNS/TLS markers before falling back to
    /// [`Network`](DownloadError::Network).
    fn from(e: wreq::Error) -> Self {
        use std::error::Error as _;

        let mut source = e.source();
        while let Some(s) = source {
            if let Some(io_err) = s.downcast_ref::<std::io::Error>() {
                return DownloadError::Io(std::io::Error::new(io_err.kind(), io_err.to_string()));
            }
            let msg = s.to_string().to_lowercase();
            if msg.contains("dns") || msg.contains("resolve") {
                return DownloadError::Dns(strip_display_marker(&s.to_string(), "dns error"));
            }
            if msg.contains("tls") || msg.contains("certificate") || msg.contains("ssl") {
                return DownloadError::Tls(strip_display_marker(&s.to_string(), "tls error"));
            }
            source = s.source();
        }
        DownloadError::Network(Box::new(e))
    }
}

/// Strip a leading marker that the variant's `Display` already prints.
///
/// `wreq`'s erased source text often IS the bare marker (e.g. `"dns error"`),
/// which would render as `"DNS error: dns error"` (#761). Removing the
/// redundant prefix keeps the message informative. The variant's `Display`
/// prefix still carries the marker, so downstream text-based classification
/// (`error.rs` heuristics match the lowercased full chain) is unaffected.
/// If stripping leaves nothing, substitute a short description so the
/// message never ends in a dangling colon.
fn strip_display_marker(payload: &str, marker: &str) -> String {
    let stripped = if payload.to_lowercase().starts_with(marker) {
        payload[marker.len()..]
            .trim_start_matches([':', ' '])
            .to_string()
    } else {
        payload.to_string()
    };
    if stripped.is_empty() {
        // Bare marker only — no extra detail survived the erasure.
        if marker.starts_with("dns") {
            "name resolution failed".to_string()
        } else {
            "handshake failed".to_string()
        }
    } else {
        stripped
    }
}

impl DownloadError {
    /// Classify this download error by operational severity.
    ///
    /// Mirrors [`crate::error::ScraperError::classify`] but operates on the
    /// typed variants, so no string-matching heuristic is needed (#649).
    #[must_use]
    pub fn classify(&self) -> crate::error::ErrorClass {
        use crate::error::ErrorClass;

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
            Self::Cancelled => ErrorClass::InternalFatal,
        }
    }
}

/// Whether an I/O failure is a recoverable transport hiccup.
///
/// `UnexpectedEof` covers the mid-body peer drop that previously surfaced as
/// `InternalFatal` (exit 3) instead of a retriable transport error (#649).
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
        let err = DownloadError::Network(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        )));
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

    /// #761: the erased wreq source text is often the bare marker
    /// (`"dns error"`), which used to render as `"DNS error: dns error"`.
    /// `strip_display_marker` removes the redundant prefix; a bare marker
    /// becomes a short description instead of a dangling colon.
    #[test]
    fn test_strip_display_marker_dedupes_redundant_prefix() {
        // Bare marker → substituted description, no dangling colon.
        assert_eq!(
            super::strip_display_marker("dns error", "dns error"),
            "name resolution failed"
        );
        // Marker + detail → detail survives.
        assert_eq!(
            super::strip_display_marker("dns error: NXDOMAIN", "dns error"),
            "NXDOMAIN"
        );
        // No marker prefix → payload untouched.
        assert_eq!(
            super::strip_display_marker("failed to lookup address", "dns error"),
            "failed to lookup address"
        );
        // TLS variant shares the same behavior.
        assert_eq!(
            super::strip_display_marker("tls error", "tls error"),
            "handshake failed"
        );
    }

    #[test]
    fn test_download_error_classify_variants() {
        use crate::error::ErrorClass;

        // Table of (error, expected class, label) — keeps classification rules
        // in one place instead of N near-identical `assert_eq!` tests.
        let cases: &[(DownloadError, ErrorClass)] = &[
            (
                DownloadError::Dns("NXDOMAIN".into()),
                ErrorClass::PermanentFatal,
            ),
            (
                DownloadError::Tls("certificate expired".into()),
                ErrorClass::PermanentFatal,
            ),
            (
                DownloadError::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "peer",
                )),
                ErrorClass::TransientRetriable,
            ),
            (
                DownloadError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "trunc",
                )),
                ErrorClass::TransientRetriable,
            ),
            (
                DownloadError::Io(std::io::Error::other("bug")),
                ErrorClass::InternalFatal,
            ),
            (DownloadError::Timeout(1), ErrorClass::TransientBackoff),
            (
                DownloadError::Http {
                    status: 503,
                    message: "x".into(),
                },
                ErrorClass::TransientRetriable,
            ),
            (
                DownloadError::Http {
                    status: 429,
                    message: "x".into(),
                },
                ErrorClass::TransientBackoff,
            ),
            (
                DownloadError::Http {
                    status: 404,
                    message: "x".into(),
                },
                ErrorClass::PermanentFatal,
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.classify(), *expected, "variant classification mismatch");
        }
    }

    #[test]
    fn test_scraper_error_delegates_download_classification() {
        use crate::error::{ErrorClass, ScraperError};

        let dns: ScraperError = DownloadError::Dns("NXDOMAIN".into()).into();
        assert_eq!(dns.classify(), ErrorClass::PermanentFatal);

        let transient: ScraperError = DownloadError::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        ))
        .into();
        assert_eq!(transient.classify(), ErrorClass::TransientRetriable);
    }

    #[test]
    fn test_download_error_clone_preserves_new_variants() {
        let dns = DownloadError::Dns("NXDOMAIN".into()).clone();
        assert!(matches!(dns, DownloadError::Dns(ref s) if s == "NXDOMAIN"));

        let tls = DownloadError::Tls("expired".into()).clone();
        assert!(matches!(tls, DownloadError::Tls(ref s) if s == "expired"));

        let io = DownloadError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "truncated",
        ))
        .clone();
        match io {
            DownloadError::Io(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
                assert!(e.to_string().contains("truncated"));
            },
            other => panic!("expected Io variant, got {other:?}"),
        }
    }

    #[test]
    fn test_download_error_into_crawl_error() {
        let err = DownloadError::Network(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        )));
        let crawl_err: crate::domain::CrawlError = err.into();
        assert!(crawl_err.to_string().contains("download error"));
        assert!(crawl_err.to_string().contains("reset"));
    }
}
