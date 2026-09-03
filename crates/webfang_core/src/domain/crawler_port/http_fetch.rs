//! Static HTTP fetch port — domain seam for the non-JS fetch fallback
//! (ADR-0012-B unit 7).
//!
//! `application::crawler::ports::ProductionPageFetcher` fell back to the free
//! fn `infrastructure::crawler::http_client::fetch_url` when no JS-rendering
//! downloader is injected — an `application→infrastructure` edge absorbed by
//! an allowlist entry. This port inverts it (asset_downloader_factory
//! precedent): application consumes [`StaticFetchPort`], the wreq concrete
//! stays in infrastructure per ADR-0012-B §2.1, and the composition root
//! (`application::container::build_static_fetcher`) names the concrete.
//!
//! [`HttpFetchResult`] moved here verbatim: it is pure domain vocabulary
//! (`String` body, status, `url::Url` final URL, domain [`Cookie`] cookies).
//! The infrastructure layer remains the sole constructor; consumers read the
//! fields outward.
//!
//! # Async desugaring
//!
//! Manual `BoxFuture` desugaring per the repo's frozen decision #1 (see
//! [`crate::domain::downloader_port::Downloader`]).

use futures::future::BoxFuture;
use url::Url;

use crate::domain::downloader_port::Cookie;
use crate::domain::{CrawlError, CrawlerConfig};

/// Result of a plain HTTP fetch via the static-fetch fallback.
///
/// Carries the body plus the response metadata the static-fetch fallback
/// must propagate — status, post-redirect final URL and cookies — so the
/// crawl fallback reports the real response instead of fabricating values
/// (#1027).
///
/// `cookies` reuses the domain [`Cookie`] type rather than wreq's internal
/// `wreq::cookie::Cookie<'a>` to keep the lifetime out of this DTO and align
/// with what every other downloader in the codebase already returns.
#[derive(Debug, Clone)]
pub struct HttpFetchResult {
    /// Decoded response body.
    pub body: String,
    /// HTTP status code, observed before any further processing.
    pub status: u16,
    /// Final URL after redirects. Falls back to the requested URL when wreq
    /// cannot parse the final `Uri` back into a `url::Url` (rare; e.g. when
    /// a redirect chain ends at an opaque URI).
    pub final_url: Url,
    /// Cookies set by the server during this request.
    pub cookies: Vec<Cookie>,
}

/// Static (non-JS) HTTP fetch surface.
///
/// Implemented in `infrastructure::crawler::http_client` over the wreq
/// client stack (rate-limited, TLS-fingerprinted); consumed by
/// `application::crawler::ports::ProductionPageFetcher` as the fallback when
/// no dynamic [`Downloader`](crate::domain::downloader_port::Downloader) is
/// injected.
pub trait StaticFetchPort: Send + Sync {
    /// Fetch `url` with a rate-limited wreq client under `config` policy.
    ///
    /// # Errors
    ///
    /// Returns [`CrawlError`] on URL parse failure, network error, or
    /// timeout — identical to the pre-port free-fn contract.
    fn fetch_url<'a>(
        &'a self,
        url: &'a str,
        config: &'a CrawlerConfig,
    ) -> BoxFuture<'a, Result<HttpFetchResult, CrawlError>>;
}
