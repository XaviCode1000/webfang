//! Error mapping — `HttpError` to domain/scraper error conversion.
//!
//! This module is the application-layer half of the `HttpError` → `CrawlError`
//! mapping; the domain half lives in `domain/error/crawl_error.rs`
//! (`From<HttpError> for CrawlError`). The two sites MUST stay in sync — see
//! `tests::test_request_failed_dual_site_agreement`.

use crate::application::http_client::HttpError;
use crate::error::ScraperError;

/// Convert an [`HttpError`] into a domain [`CrawlError`] with the URL context.
///
/// This is the application-layer half of the `HttpError` → `CrawlError`
/// mapping; the domain half lives in `domain/error/crawl_error.rs`
/// (`From<HttpError> for CrawlError`). The two sites MUST stay in sync —
/// see `tests::test_request_failed_dual_site_agreement`.
fn crawl_error_from_http(err: HttpError, url: &str) -> crate::domain::error::CrawlError {
    use crate::domain::error::CrawlError;
    match err {
        HttpError::ClientError(code) | HttpError::ServerError(code) => CrawlError::Http {
            status: code,
            url: url.to_string(),
        },
        HttpError::Forbidden => CrawlError::Http {
            status: 403,
            url: url.to_string(),
        },
        HttpError::RateLimited(retry_after) => CrawlError::RateLimited(retry_after),
        HttpError::Timeout => CrawlError::Timeout,
        HttpError::Connection(msg) => CrawlError::Connection(msg),
        HttpError::Request(msg) => CrawlError::RequestFailed(msg),
        HttpError::WafChallenge(provider) => CrawlError::WafChallenge {
            provider,
            kind: crate::domain::error::WafDetectionKind::BodySignature,
            url: url.to_string(),
        },
        HttpError::DomainBanned(domain) => {
            CrawlError::SessionPool(format!("domain banned: {domain}"))
        },
    }
}

/// Convert an [`HttpError`] into a [`ScraperError`] with the URL context.
pub(crate) fn scraper_error_from_http(err: HttpError, url: &str) -> ScraperError {
    ScraperError::from(crawl_error_from_http(err, url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::CrawlError;
    use crate::error::ErrorClass;

    const URL: &str = "https://example.com";

    /// EC-REQUEST-FAILED: both `HttpError::Request` conversion sites must agree.
    ///
    /// Site 1: `application::error_mapping::crawl_error_from_http`
    /// Site 2: `domain/error/crawl_error.rs` `From<HttpError>`
    ///
    /// Both must yield `CrawlError::RequestFailed(msg)` for the same input —
    /// no divergence between the application and domain conversion paths.
    #[test]
    fn test_request_failed_dual_site_agreement() {
        let msg = "build failed";

        // Site 1 (application layer)
        let via_service =
            crawl_error_from_http(HttpError::Request(msg.to_string()), "https://example.com");
        // Site 2 (domain layer)
        let via_domain: CrawlError = HttpError::Request(msg.to_string()).into();

        assert!(
            matches!(&via_service, CrawlError::RequestFailed(m) if m == msg),
            "service site must produce RequestFailed, got: {via_service:?}"
        );
        assert!(
            matches!(&via_domain, CrawlError::RequestFailed(m) if m == msg),
            "domain site must produce RequestFailed, got: {via_domain:?}"
        );

        // Classification is preserved end-to-end: RequestFailed → Internal → InternalFatal.
        let scraper_err = ScraperError::from(via_service);
        assert!(
            matches!(&scraper_err, ScraperError::Internal(m) if m == msg),
            "RequestFailed must map to ScraperError::Internal, got: {scraper_err}"
        );
        assert_eq!(scraper_err.classify(), ErrorClass::InternalFatal);
    }

    #[test]
    fn test_client_error_maps_to_http_with_status() {
        let err = crawl_error_from_http(HttpError::ClientError(404), URL);
        assert!(
            matches!(&err, CrawlError::Http { status: 404, url } if url == URL),
            "expected Http 404 with url, got: {err:?}"
        );
    }

    #[test]
    fn test_server_error_maps_to_http_with_status() {
        let err = crawl_error_from_http(HttpError::ServerError(503), URL);
        assert!(
            matches!(&err, CrawlError::Http { status: 503, url } if url == URL),
            "expected Http 503 with url, got: {err:?}"
        );
    }

    #[test]
    fn test_forbidden_maps_to_http_403() {
        let err = crawl_error_from_http(HttpError::Forbidden, URL);
        assert!(
            matches!(&err, CrawlError::Http { status: 403, url } if url == URL),
            "expected Http 403, got: {err:?}"
        );
    }

    #[test]
    fn test_rate_limited_preserves_retry_after() {
        let err = crawl_error_from_http(HttpError::RateLimited(60), URL);
        assert!(
            matches!(err, CrawlError::RateLimited(60)),
            "expected RateLimited(60), got: {err:?}"
        );
    }

    #[test]
    fn test_timeout_maps_to_timeout() {
        let err = crawl_error_from_http(HttpError::Timeout, URL);
        assert!(matches!(err, CrawlError::Timeout), "got: {err:?}");
    }

    #[test]
    fn test_connection_preserves_message() {
        let err = crawl_error_from_http(HttpError::Connection("refused".to_string()), URL);
        assert!(
            matches!(&err, CrawlError::Connection(m) if m == "refused"),
            "got: {err:?}"
        );
    }

    #[test]
    fn test_waf_challenge_maps_with_body_signature_kind() {
        use crate::domain::error::WafDetectionKind;
        let err = crawl_error_from_http(HttpError::WafChallenge("cloudflare".to_string()), URL);
        assert!(
            matches!(&err, CrawlError::WafChallenge { provider, kind, url }
                if provider == "cloudflare"
                    && matches!(kind, WafDetectionKind::BodySignature)
                    && url == URL),
            "got: {err:?}"
        );
    }

    #[test]
    fn test_domain_banned_maps_to_session_pool() {
        let err = crawl_error_from_http(HttpError::DomainBanned("example.com".to_string()), URL);
        assert!(
            matches!(&err, CrawlError::SessionPool(m) if m == "domain banned: example.com"),
            "got: {err:?}"
        );
    }

    #[test]
    fn test_scraper_error_from_http_wraps_domain_error() {
        let msg = "build failed";
        let err = scraper_error_from_http(HttpError::Request(msg.to_string()), URL);
        // RequestFailed → ScraperError::Internal (same invariant as the dual-site test).
        assert!(
            matches!(&err, ScraperError::Internal(m) if m == msg),
            "expected ScraperError::Internal, got: {err}"
        );
        assert_eq!(err.classify(), ErrorClass::InternalFatal);
    }
}
