//! Application-layer ports for crawl task dependencies.
//!
//! These traits decouple `run_crawl_task` from concrete infrastructure,
//! enabling inline unit tests that kill mutants under `cargo mutants --lib`.
//!
//! # Async desugaring
//!
//! Async methods use manual `Pin<Box<dyn Future>>` desugaring (BoxFuture)
//! instead of the `async_trait` crate, matching the frozen decision #1
//! established in [`crate::domain::repository::VectorRepository`] and
//! [`crate::domain::downloader_port::Downloader`].
//!
//! rust-analyzer caveat (#1034): the manual `Box::pin(async move { ... })`
//! form can produce a spurious `E0308` mismatch against an `BoxFuture<'a, ...>`
//! return type in editor diagnostics, even when `cargo check --all-features`
//! accepts the file as well-typed. Treat the compiler as the source of truth
//! for these methods; the rust-analyzer overlay is not.

use std::sync::Arc;

use futures::future::BoxFuture;
use url::Url;

use crate::application::crawler::collector::{CrawlMessage, ResultsCollector};
use crate::application::pipeline::{PipelineExecutor, ScrapedItem, StageOutcome};
use crate::domain::crawler_port::RobotsPort;
use crate::domain::crawler_port::StaticFetchPort;
use crate::domain::downloader_port::{Cookie, DownloadError, Downloader};
use crate::domain::link_extractor::LinkExtractor;
use crate::domain::{CrawlError, CrawlerConfig, DiscoveredUrl};

/// Outcome of fetching a single page.
///
/// Carries the response body, the final URL after any redirects, the cookies
/// set by the server, and the HTTP status code. Propagating `final_url`
/// (rather than the requested URL) lets the crawl key deduplication and output
/// on the document's true location, avoiding duplicate content under multiple
/// redirect aliases (#651, Bug 3).
pub(crate) struct FetchOutcome {
    /// Raw HTML body of the page.
    pub body: String,
    /// Final URL after redirects — where the content actually lives.
    pub final_url: Url,
    /// Cookies set by the server during the request.
    pub cookies: Vec<Cookie>,
    /// HTTP status code.
    pub status: u16,
}

/// Fetches a web page. Unifies the dynamic [`Downloader`] and static
/// `fetch_url()` paths.
///
/// Returns a [`FetchOutcome`] carrying the final post-redirect URL. The WAF
/// variant is preserved in the error so `run_crawl_task` can apply domain-banning
/// logic.
pub(crate) trait PageFetcher: Send + Sync {
    /// Fetch the page at `url` using the given crawl configuration.
    ///
    /// # Errors
    ///
    /// Returns [`CrawlError`] on network failure, WAF detection, or timeout.
    fn fetch_page<'a>(
        &'a self,
        url: &'a Url,
        config: &'a CrawlerConfig,
    ) -> BoxFuture<'a, Result<FetchOutcome, CrawlError>>;
}

/// Checks robots.txt rules for a URL.
pub(crate) trait RobotsChecker: Send + Sync {
    /// Returns `true` if the URL is allowed by the domain's robots.txt.
    fn is_robots_allowed<'a>(&'a self, url: &'a str, domain: &'a str) -> BoxFuture<'a, bool>;
}

/// Extracts links from HTML content.
pub(crate) trait LinkExtractorPort: Send + Sync {
    /// Extract and normalize all `<a href>` links from `html`.
    ///
    /// # Errors
    ///
    /// Returns [`CrawlError`] on selector or base-URL parse failure.
    fn extract_links(&self, html: &str, base_url: &str) -> Result<Vec<String>, CrawlError>;
}

/// Executes the content processing pipeline.
pub(crate) trait ContentPipeline: Send + Sync {
    /// Run all pipeline stages on `item` and return the outcome.
    fn execute_pipeline<'a>(&'a self, item: ScrapedItem) -> BoxFuture<'a, StageOutcome>;
}

/// Collects crawl results via channel.
pub(crate) trait CrawlResultCollector: Send + Sync {
    /// Send a successfully crawled URL to the results channel.
    ///
    /// # Errors
    ///
    /// Returns [`CrawlError`] if the channel is closed.
    fn send_result<'a>(&'a self, url: DiscoveredUrl) -> BoxFuture<'a, Result<(), CrawlError>>;
}

// ---------------------------------------------------------------------------
// Production implementations
// ---------------------------------------------------------------------------

/// Production [`PageFetcher`] that wraps an optional [`Downloader`].
///
/// When a downloader is present, delegates to [`Downloader::fetch`]; otherwise
/// falls back to the composition-root-injected [`StaticFetchPort`].
pub(crate) struct ProductionPageFetcher {
    /// Optional fetch downloader for hybrid/full JS rendering.
    pub(crate) router: Option<Arc<dyn Downloader>>,
    /// Static-fetch fallback (no JS rendering); built at the composition
    /// root (ADR-0012-B unit 7).
    pub(crate) fallback: Arc<dyn StaticFetchPort>,
}

impl PageFetcher for ProductionPageFetcher {
    fn fetch_page<'a>(
        &'a self,
        url: &'a Url,
        config: &'a CrawlerConfig,
    ) -> BoxFuture<'a, Result<FetchOutcome, CrawlError>> {
        Box::pin(async move {
            if let Some(ref router) = self.router {
                match router.fetch(url).await {
                    Ok(page) => Ok(FetchOutcome {
                        body: page.html,
                        final_url: page.url,
                        cookies: page.cookies,
                        status: page.status,
                    }),
                    Err(e) => Err(e.into()),
                }
            } else {
                let result = self.fallback.fetch_url(url.as_str(), config).await?;
                Ok(FetchOutcome {
                    body: result.body,
                    final_url: result.final_url,
                    cookies: result.cookies,
                    status: result.status,
                })
            }
        })
    }
}

/// Production [`RobotsChecker`] delegating to the domain [`RobotsPort`] seam.
/// The concrete `RobotsFetcher` behind the port is built by the
/// composition-root helper `application::container::build_robots_fetcher`.
pub(crate) struct ProductionRobotsChecker {
    /// Shared robots.txt port with per-domain cache.
    pub(crate) fetcher: Arc<dyn RobotsPort>,
}

impl RobotsChecker for ProductionRobotsChecker {
    fn is_robots_allowed<'a>(&'a self, url: &'a str, domain: &'a str) -> BoxFuture<'a, bool> {
        // The domain port already returns a boxed future — delegate directly.
        self.fetcher.is_allowed(url, domain)
    }
}

/// Production [`LinkExtractorPort`] delegating to the composition-root
/// [`LinkExtractor`] seam (ADR-0012-B unit 6): the scraper-backed concrete
/// stays in infrastructure; this wrapper erases it behind the domain
/// trait object built by `application::container::build_link_extractor`.
pub(crate) struct ProductionLinkExtractor {
    inner: std::sync::Arc<dyn LinkExtractor>,
}

impl ProductionLinkExtractor {
    /// Wrap a domain link-extractor port object.
    pub(crate) fn new(inner: std::sync::Arc<dyn LinkExtractor>) -> Self {
        Self { inner }
    }
}

impl LinkExtractorPort for ProductionLinkExtractor {
    fn extract_links(&self, html: &str, base_url: &str) -> Result<Vec<String>, CrawlError> {
        self.inner.extract_links(html, base_url)
    }
}

/// Production [`ContentPipeline`] backed by [`PipelineExecutor`].
pub(crate) struct ProductionPipeline {
    /// The pipeline executor with registered stages.
    pub(crate) executor: Arc<PipelineExecutor>,
}

impl ContentPipeline for ProductionPipeline {
    fn execute_pipeline<'a>(&'a self, item: ScrapedItem) -> BoxFuture<'a, StageOutcome> {
        Box::pin(async move { self.executor.execute(item).await })
    }
}

/// Production [`CrawlResultCollector`] backed by the mpsc [`ResultsCollector`].
pub(crate) struct ProductionCollector {
    /// The mpsc-based results collector.
    pub(crate) collector: ResultsCollector,
}

impl CrawlResultCollector for ProductionCollector {
    fn send_result<'a>(&'a self, url: DiscoveredUrl) -> BoxFuture<'a, Result<(), CrawlError>> {
        Box::pin(async move {
            self.collector
                .send(CrawlMessage::success(url))
                .await
                // LCOV_EXCL_LINE defensive: mpsc-send — send fails only when the engine collector was dropped, a shutdown bug
                .map_err(|e| CrawlError::Internal(e.to_string()))
        })
    }
}

/// Check whether a [`CrawlError`] represents a WAF challenge.
///
/// Inspects the [`CrawlError::Download`] source chain for
/// [`DownloadError::WafChallenge`], the domain-level
/// [`CrawlError::WafChallenge`] variant, and the legacy string heuristic
/// used by the static `fetch_url` fallback path.
pub(crate) fn waf_challenge_message(err: &CrawlError) -> Option<String> {
    match err {
        CrawlError::WafChallenge { provider, .. } => Some(provider.clone()),
        CrawlError::Download(source) => {
            source
                .downcast_ref::<DownloadError>()
                .and_then(|de| match de {
                    DownloadError::WafChallenge(msg) => Some(msg.clone()),
                    _ => None,
                })
        },
        other => {
            let msg = other.to_string();
            if msg.contains("WAF") {
                Some(msg)
            } else {
                None
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::downloader_port::FetchedPage;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // -----------------------------------------------------------------------
    // Branch observability for ProductionPageFetcher (#1024 → #1027 → #1030)
    //
    // `fetch_page` has two branches: a router branch that delegates to a
    // [`Downloader`] and a fallback branch that calls the static
    // [`fetch_url`] helper. Both must report the same observable truth about
    // a response — same status, same cookies, same final URL after redirects.
    //
    // Before #1027 the fallback fabricated `status: 200` and `cookies:
    // Vec::new()`, diverging from the real HTTP response. `fetch_url` rejected
    // every non-2xx before the fallback saw them, so the fabrication could
    // only misreport *within* the 2xx family (201, 203, 204, 226). That made
    // it a data-correctness defect (the published `ScrapedItem.status_code`
    // lied) but not a safety hole (no error turned into a success).
    //
    // #1027 changed `fetch_url` to return `HttpFetchResult { body, status,
    // final_url, cookies }` so the fallback could propagate the real values.
    // The two tests below are the tripwire: any regression that re-introduces
    // fabrication would surface here as a status / cookie mismatch.
    // -----------------------------------------------------------------------

    /// A [`Downloader`] double reporting a status and cookies the fallback
    /// branch cannot produce. Any assertion satisfied through it proves the
    /// router branch ran rather than the static fallback.
    struct RouterDouble {
        status: u16,
        cookies: Vec<Cookie>,
    }

    impl Downloader for RouterDouble {
        fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
            Box::pin(async move {
                Ok(FetchedPage {
                    url: url.clone(),
                    html: "<html>rendered</html>".to_string(),
                    status: self.status,
                    headers: HashMap::new(),
                    cookies: self.cookies.clone(),
                })
            })
        }

        fn supports_interactions(&self) -> bool {
            true
        }

        fn memory_cost(&self) -> usize {
            0
        }
    }

    fn session_cookie() -> Cookie {
        Cookie {
            name: "wf_session".to_string(),
            value: "abc123".to_string(),
            domain: "127.0.0.1".to_string(),
            path: "/".to_string(),
            http_only: false,
            secure: false,
        }
    }

    fn config_for(url: &Url) -> CrawlerConfig {
        CrawlerConfig::builder(url.clone())
            .max_depth(0)
            .max_pages(1)
            .delay_ms(1)
            .concurrency(std::num::NonZeroUsize::new(1).expect("1 is non-zero"))
            .timeout_secs(5)
            .build()
    }

    /// A mock server answering 203 with a `Set-Cookie` header.
    ///
    /// 203 is deliberate: `fetch_url` rejects only non-2xx responses before the
    /// caller ever sees a status, so 203 reaches the fallback's success path and
    /// exposes the fabricated `status: 200` without the test depending on an error
    /// being turned into a success.
    async fn mock_203_with_cookie() -> (MockServer, Url) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(
                ResponseTemplate::new(203)
                    .set_body_string("<html>served</html>")
                    .insert_header("set-cookie", "wf_session=abc123; Path=/"),
            )
            .mount(&server)
            .await;
        let url = Url::parse(&format!("{}/page", server.uri())).expect("valid mock URL");
        (server, url)
    }

    /// Router branch: the `Downloader`'s status and cookies reach the caller
    /// untouched.
    #[tokio::test]
    async fn router_branch_propagates_status_and_cookies() {
        let url = Url::parse("https://example.test/page").expect("valid URL");
        let config = config_for(&url);
        let fetcher = ProductionPageFetcher {
            router: Some(Arc::new(RouterDouble {
                status: 203,
                cookies: vec![session_cookie()],
            })),
            fallback: crate::application::container::build_static_fetcher(),
        };

        let outcome = fetcher
            .fetch_page(&url, &config)
            .await
            .expect("router branch should succeed");

        assert_eq!(
            outcome.status, 203,
            "router branch must propagate the downloader's real status"
        );
        assert_eq!(
            outcome.cookies.len(),
            1,
            "router branch must propagate the downloader's cookies"
        );
        assert_eq!(outcome.cookies[0].name, "wf_session");
        assert_eq!(outcome.body, "<html>rendered</html>");
    }

    /// Fallback branch against a **real** HTTP response: after #1027, the
    /// fallback propagates the server's true status and `Set-Cookie` instead of
    /// fabricating them.
    ///
    /// 203 is deliberate: `fetch_url` rejects only non-2xx responses before the
    /// caller ever sees a status, so 203 reaches the fallback's success path and
    /// exercises the propagation without the test depending on an error being
    /// turned into a success.
    ///
    /// This is the tripwire from #1024 → #1027. A regression that re-introduced
    /// fabrication (e.g. reverting the fallback to `status: 200` / `Vec::new()`)
    /// would surface here as a status or cookie mismatch.
    #[tokio::test]
    async fn fallback_branch_propagates_status_and_cookies() {
        let (_server, url) = mock_203_with_cookie().await;
        let config = config_for(&url);
        let fetcher = ProductionPageFetcher {
            router: None,
            fallback: crate::application::container::build_static_fetcher(),
        };

        let outcome = fetcher
            .fetch_page(&url, &config)
            .await
            .expect("203 is 2xx, so fetch_url should succeed");

        assert_eq!(
            outcome.status, 203,
            "fallback must propagate the server's 203, not fabricate 200 (#1027)"
        );
        assert_eq!(
            outcome.cookies.len(),
            1,
            "fallback must propagate the Set-Cookie set by the server (#1027)"
        );
        assert_eq!(outcome.cookies[0].name, "wf_session");
        assert_eq!(
            outcome.body, "<html>served</html>",
            "body is the only field the fallback already carried faithfully"
        );
        assert_eq!(
            outcome.final_url, url,
            "no redirect was configured, so final_url equals the requested URL"
        );
    }

    /// The router branch and the fallback branch must agree on the real status
    /// of the same HTTP response (#1027). Before the fix the branches diverged
    /// (router=203, fallback=200) because the fallback fabricated 200; after the
    /// fix they converge on the truth (both = 203).
    ///
    /// `cookies` may still differ in shape (the router's `RouterDouble` hardcodes
    /// a `127.0.0.1` domain; the fallback parses the raw `Set-Cookie` header),
    /// which is expected — what matters is that both branches report the
    /// server-observed status faithfully. A regression that re-introduced
    /// fabrication would surface here as `with_router.status != without_router.status`.
    #[tokio::test]
    async fn branches_agree_on_status_after_fix() {
        let (_server, url) = mock_203_with_cookie().await;
        let config = config_for(&url);

        let routed = ProductionPageFetcher {
            router: Some(Arc::new(RouterDouble {
                status: 203,
                cookies: vec![session_cookie()],
            })),
            fallback: crate::application::container::build_static_fetcher(),
        };
        let fallback = ProductionPageFetcher {
            router: None,
            fallback: crate::application::container::build_static_fetcher(),
        };

        let with_router = routed
            .fetch_page(&url, &config)
            .await
            .expect("router branch should succeed");
        let without_router = fallback
            .fetch_page(&url, &config)
            .await
            .expect("fallback branch should succeed");

        assert_eq!(
            with_router.status, without_router.status,
            "both branches must report the server's real status, not fabricate 200 (#1027)"
        );
        assert_eq!(with_router.status, 203);
        // Body may legitimately differ: the router branch uses `RouterDouble`'s
        // canned body ("<html>rendered</html>") while the fallback branch reads
        // the mock server's body ("<html>served</html>"). What the tripwire
        // cares about is status and cookie provenance, not body equality.
        assert!(
            with_router.cookies[0].name == "wf_session"
                && without_router.cookies[0].name == "wf_session",
            "both branches must surface the Set-Cookie set by the server (#1027)"
        );
    }

    /// Fallback branch follows a redirect and exposes the post-redirect URL.
    ///
    /// Before #1027 the fallback echoed the *requested* URL as `final_url`,
    /// making it impossible to deduplicate crawl output across redirect aliases
    /// (#651, Bug 3). After #1027, `fetch_url` returns the post-redirect URI
    /// wreq observed through the chain.
    ///
    /// `WEBFANG_DISABLE_SSRF_REDIRECT_GUARD=1` is set because wiremock binds
    /// 127.0.0.1 and the SSRF redirect guard (#703) blocks redirect targets on
    /// literal IPs. The guard exists for production traffic; this test bypasses
    /// it explicitly. Same pattern as
    /// `WreqDownloader::test_fetch_returns_final_url`.
    #[tokio::test]
    async fn fallback_branch_propagates_final_url_after_redirect() {
        let _guard = webfang_test_utils::EnvGuard::with(&[(
            crate::infrastructure::ssrf::DISABLE_REDIRECT_GUARD_ENV,
            "1",
        )]);

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/redirect"))
            .respond_with(ResponseTemplate::new(301).insert_header("location", "/target"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/target"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html>landed</html>")
                    .insert_header("set-cookie", "wf_session=postredirect; Path=/"),
            )
            .mount(&server)
            .await;

        let requested = Url::parse(&format!("{}/redirect", server.uri())).expect("valid mock URL");
        let config = config_for(&requested);
        let fetcher = ProductionPageFetcher {
            router: None,
            fallback: crate::application::container::build_static_fetcher(),
        };

        let outcome = fetcher
            .fetch_page(&requested, &config)
            .await
            .expect("redirect chain should resolve to a 2xx response");

        assert_ne!(
            outcome.final_url, requested,
            "fallback must expose the post-redirect URL, not the requested one (#1027, #651)"
        );
        let expected_final =
            Url::parse(&format!("{}/target", server.uri())).expect("valid mock URL");
        assert_eq!(
            outcome.final_url, expected_final,
            "final_url must point at the resolved target"
        );
        assert_eq!(outcome.status, 200);
        assert_eq!(outcome.body, "<html>landed</html>");
        assert_eq!(outcome.cookies.len(), 1);
        assert_eq!(outcome.cookies[0].name, "wf_session");
    }
}
