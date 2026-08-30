//! Downloader abstraction — shim re-exporting domain::downloader_port (ADR-0010).
//!
//! The `Downloader` trait and DTOs now live in `crate::domain::downloader_port`
//! so `application::*` can depend on `domain::*` without
//! `application→infrastructure`. The concrete `wreq`-based impls stay here
//! and implement the domain trait; the `wreq::Error → DownloadError` mapping
//! and `strip_display_marker` helper remain infrastructure-owned (they would be
//! an outward `domain→wreq` dependency if they lived in `domain`).

pub mod chromiumoxide_downloader;
pub mod cookie_bridge;
pub mod fetch_router;
pub mod hybrid_router;
pub mod obscura_downloader;
pub mod resource_governor;
pub mod spa_detector;
pub mod wreq_downloader;

// Domain-owned port — re-exported so `crate::infrastructure::downloader::Downloader`
// still resolves (shim for `webfang_core::infrastructure::downloader` path).
pub use crate::domain::downloader_port::{Cookie, DownloadError, Downloader, FetchedPage};

impl From<wreq::Error> for DownloadError {
    /// Walk the transport error's cause chain to recover a typed variant.
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
fn strip_display_marker(payload: &str, marker: &str) -> String {
    let stripped = if payload.to_lowercase().starts_with(marker) {
        payload[marker.len()..]
            .trim_start_matches([':', ' '])
            .to_string()
    } else {
        payload.to_string()
    };
    if stripped.is_empty() {
        if marker.starts_with("dns") {
            "name resolution failed".to_string()
        } else {
            "handshake failed".to_string()
        }
    } else {
        stripped
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

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

    #[test]
    fn test_strip_display_marker_dedupes_redundant_prefix() {
        assert_eq!(
            super::strip_display_marker("dns error", "dns error"),
            "name resolution failed"
        );
        assert_eq!(
            super::strip_display_marker("dns error: NXDOMAIN", "dns error"),
            "NXDOMAIN"
        );
        assert_eq!(
            super::strip_display_marker("failed to lookup address", "dns error"),
            "failed to lookup address"
        );
        assert_eq!(
            super::strip_display_marker("tls error", "tls error"),
            "handshake failed"
        );
    }

    #[test]
    fn test_download_error_classify_variants() {
        use crate::error::ErrorClass;

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
    fn test_download_error_into_crawl_error() {
        let err = DownloadError::Network(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        )));
        let crawl_err: crate::domain::CrawlError = err.into();
        assert!(crawl_err.to_string().contains("download error"));
        assert!(crawl_err.to_string().contains("reset"));
    }

    #[test]
    fn test_download_error_feature_gated_clone_and_classify() {
        let original = DownloadError::FeatureGated("chromium disabled".to_string());
        let cloned = original.clone();
        assert!(
            matches!(cloned, DownloadError::FeatureGated(ref s) if s == "chromium disabled"),
            "FeatureGated clone must preserve String, got: {cloned:?}"
        );
        assert_eq!(cloned.classify(), crate::error::ErrorClass::PermanentFatal,);
        assert!(cloned.to_string().contains("chromium disabled"));
        assert!(
            cloned.to_string().contains("funcionalidad no disponible"),
            "FeatureGated Display must be Spanish, got: {cloned}"
        );
    }
}
