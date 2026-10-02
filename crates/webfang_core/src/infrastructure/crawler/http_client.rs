//! HTTP client with rate limiting
//!
//! Provides rate-limited HTTP client for crawling.
//!
//! # Rules Applied
//!
//! - **mem-with-capacity**: Pre-allocate when size is known
//! - **own-borrow-over-clone**: Accept references not owned values
//! - **clean-architecture**: Converts reqwest::Error → CrawlError::Network (NO reqwest in Domain)

use std::time::Duration;

use tracing::debug;
use url::Url;
use wreq::Client;
use wreq_util::Profile;

use crate::domain::crawler_port::{HttpFetchResult, StaticFetchPort};
use crate::domain::downloader_port::Cookie;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::{CrawlError, CrawlerConfig};
use crate::error::Result as ScraperResult;
use crate::infrastructure::http::create_http_client_with_config;

/// Static (non-JS) fetcher over the wreq stack — the [`StaticFetchPort`]
/// concrete named only at the composition root (ADR-0012-B unit 7).
/// The free [`fetch_url`] implementation and [`HttpFetchResult`] DTO now
/// live in `domain::crawler_port::http_fetch`; this adapter erases the
/// concrete behind the domain port.
pub struct StaticHttpFetcher;

impl StaticFetchPort for StaticHttpFetcher {
    fn fetch_url<'a>(
        &'a self,
        url: &'a str,
        config: &'a CrawlerConfig,
    ) -> futures::future::BoxFuture<'a, Result<HttpFetchResult, CrawlError>> {
        Box::pin(fetch_url(url, config))
    }
}

/// Create a rate-limited HTTP client
///
/// Delegates to the shared config-driven factory ([`create_http_client_with_config`],
/// the #299 source of truth) so the crawl client carries the same Chrome Client
/// Hints, pooled user-agent, pool tuning, compression, cookie store, and redirect
/// policy as the scrape client — with the TLS/H2 fingerprint resolved from config
/// instead of a hardcoded `Chrome145` (#312).
///
/// # Arguments
///
/// * `delay_ms` - Delay between requests in milliseconds
/// * `tls_emulation` - TLS/HTTP2 fingerprint preset applied to the client
///
/// # Returns
///
/// Configured wreq Client
///
/// # Errors
///
/// Returns an error if the underlying wreq client fails to build.
///
/// # Examples
///
/// ```
/// use webfang_core::infrastructure::crawler::create_rate_limited_client;
/// use wreq_util::Profile;
///
/// let client = create_rate_limited_client(500, Profile::Chrome145).unwrap();
/// ```
pub fn create_rate_limited_client(delay_ms: u64, tls_emulation: Profile) -> ScraperResult<Client> {
    // The connect timeout replicates the historical 10s cap of this client; the
    // per-request timeout is applied by `fetch_url` from `CrawlerConfig`.
    let http_config = HttpClientConfig {
        tls_emulation,
        connect_timeout_secs: 10,
        ..Default::default()
    };
    let client = create_http_client_with_config(&http_config)?;

    debug!(
        "Created rate-limited HTTP client with delay_ms={} tls_emulation={:?}",
        delay_ms, tls_emulation
    );

    Ok(client)
}

/// Fetch a URL and return the response plus its final URL, status, and cookies
///
/// Following **own-borrow-over-clone**: Accepts `&str` and `&CrawlerConfig`.
/// Following **clean-architecture**: Converts reqwest::Error → CrawlError::Network
///
/// # Arguments
///
/// * `url` - URL to fetch
/// * `config` - Crawler configuration
///
/// # Returns
///
/// * `Ok(HttpFetchResult)` - Body, status, post-redirect `final_url`, and cookies
/// * `Err(CrawlError)` - Error during fetch (transport, non-2xx status, body read)
///   or a forbidden SSRF literal target (below)
///
/// # Note on final URL
///
/// `final_url` reflects the URL after wreq follows redirects. When the response
/// `Uri` cannot be parsed back into a `url::Url` (e.g. an opaque final URI), the
/// caller-supplied URL is reused as a conservative fallback so callers can keep
/// keying by URL without special-casing the rare failure path.
///
/// # SSRF layer 2 lives here, not at the call sites (#1615, F11)
///
/// This is the one place every [`StaticFetchPort`] consumer passes through:
/// `application::crawler::ports::ProductionPageFetcher` calls it whenever no
/// JS-rendering `Downloader` is injected. Before this change the fallback
/// applied the client-stack layers -- timeout, redirect policy, connect-time
/// validating resolver -- but never the LITERAL-IP entry guard, so a target
/// like `http://169.254.169.254/` was dialed: the address the validating
/// resolver checks is itself the forbidden address, and a literal needs no
/// resolution in the first place. The JS arm never had this problem, because
/// `FetchRouter::fetch` applies `reject_forbidden_literal_url` to every
/// strategy arm. That asymmetry is the finding.
///
/// Putting the check at the PORT rather than at each call site is the issue's
/// own remedy, and it is the one that survives: a future consumer of the port
/// inherits the guard by existing, where a call-site check is a check someone
/// has to remember to add. The check runs before the client is built, so a
/// refused target never opens a socket and never constructs a TLS stack.
///
/// Ordering is the guard chain in `AGENTS.md` and nothing here is reordered:
/// entry validation, then pacing at the call site, then per attempt timeout ->
/// redirect -> SSRF-at-dial. The literal guard is a pre-dial refinement of
/// stage 3c, not a new stage.
///
/// # Errors
///
/// [`CrawlError::InvalidUrl`] when the host is a forbidden IP literal, carrying
/// the guard's Spanish message verbatim -- the same text the CLI and MCP entry
/// points produce, because it is the same guard. `InvalidUrl` is terminal: the
/// engine does not retry it and does not escalate to the JS downloader, so a
/// refused target stays refused.
pub async fn fetch_url(url: &str, config: &CrawlerConfig) -> Result<HttpFetchResult, CrawlError> {
    debug!("Fetching URL: {}", url);

    let requested = Url::parse(url).map_err(|e| CrawlError::Network {
        message: format!("invalid URL {url}: {e}"),
        status_code: None,
    })?;

    // SSRF layer 2 (F-06 + F-32 #1217), applied here so every consumer of the
    // port inherits it (F11). Same choke point, same deny list, same
    // exact-"1" hatch as the CLI and MCP entry paths: sharing
    // `reject_forbidden_literal_url` is what makes the entry points unable to
    // drift apart.
    if let Err(rejection) = crate::domain::ssrf_guard::reject_forbidden_literal_url(&requested) {
        return Err(CrawlError::InvalidUrl(rejection.to_string()));
    }

    let client = create_rate_limited_client(config.delay_ms, config.tls_emulation)
        // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
        .map_err(|e| CrawlError::Internal(format!("Failed to create HTTP client: {e}")))?;

    let response = client
        .get(url)
        .timeout(Duration::from_secs(config.timeout_secs))
        .send()
        .await
        .map_err(|e| CrawlError::Network {
            message: e.to_string(),
            status_code: e.status().map(|s| s.as_u16()),
        })?;

    let status = response.status().as_u16();

    // The final URL after redirects — wreq's `Response::uri()` returns the
    // post-redirect URI that hyper tracks through the redirect chain. Fall back
    // to the requested URL when the URI cannot be round-tripped through `url::Url`
    // (e.g. an opaque scheme or a fragment-only URI), matching the conservative
    // behaviour of the legacy fallback path.
    let final_url = Url::parse(&response.uri().to_string()).unwrap_or_else(|_| requested.clone());

    // Extract cookies BEFORE consuming the body. wreq's `Response::cookies()`
    // yields `wreq::cookie::Cookie<'_>` items parsed from `Set-Cookie` headers;
    // we project them into the domain `Cookie` DTO to match every other
    // downloader in the codebase (see `WreqDownloader::extract_cookies`).
    let cookies = response
        .cookies()
        .map(|c| Cookie {
            name: c.name().to_string(),
            value: c.value().to_string(),
            domain: c.domain().unwrap_or("").to_string(),
            path: c.path().unwrap_or("/").to_string(),
            http_only: c.http_only(),
            secure: c.secure(),
        })
        .collect();

    // Check for successful status
    if !response.status().is_success() {
        // Convert HTTP error to CrawlError::Network
        return Err(CrawlError::Network {
            message: format!("HTTP error: {}", response.status()),
            status_code: Some(status),
        });
    }

    let text = response.text().await.map_err(|e| CrawlError::Network {
        message: e.to_string(),
        status_code: None,
    })?;

    Ok(HttpFetchResult {
        body: text,
        status,
        final_url,
        cookies,
    })
}

#[cfg(test)]
#[cfg(not(miri))] // all tests create wreq::Client with btls-sys FFI (unsupported by Miri)
mod tests {
    use super::*;

    #[test]
    fn test_create_rate_limited_client() {
        let client = create_rate_limited_client(500, Profile::Chrome145);
        assert!(client.is_ok());
    }

    #[test]
    fn test_create_rate_limited_client_zero_delay() {
        let client = create_rate_limited_client(0, Profile::Chrome145);
        assert!(client.is_ok());
    }

    #[test]
    fn test_create_rate_limited_client_honors_profile_param() {
        // The profile parameter must be accepted and threaded into the client
        // builder for every preset, not just the Chrome145 default (#312).
        for profile in [Profile::Chrome145, Profile::Chrome131, Profile::Firefox135] {
            assert!(
                create_rate_limited_client(100, profile).is_ok(),
                "client build should succeed for profile {profile:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_fetch_url_with_custom_profile_succeeds() {
        // #1615 (F11 / G-3 / G-4 / G-5): this path now applies the
        // literal-IP entry guard, which refuses wiremock's 127.0.0.1.
        // The named constructor is the repo's rule — never spell the
        // variable out at the call site.
        let _entry_off = webfang_test_utils::EnvGuard::entry_guard_off();
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>ok</html>"))
            .mount(&server)
            .await;

        let seed = url::Url::parse(&server.uri()).unwrap();
        let config = CrawlerConfig::builder(seed)
            .tls_emulation(Profile::Chrome131)
            .build();

        let html = fetch_url(&server.uri(), &config)
            .await
            .expect("fetch_url should succeed with a custom TLS profile")
            .body;
        assert_eq!(html, "<html>ok</html>");
    }
}
