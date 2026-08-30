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

use std::sync::Arc;

use futures::future::BoxFuture;
use url::Url;

use crate::application::crawler::collector::{CrawlMessage, ResultsCollector};
use crate::application::pipeline::{PipelineExecutor, ScrapedItem, StageOutcome};
use crate::domain::downloader_port::{Cookie, DownloadError, Downloader};
use crate::domain::{CrawlError, CrawlerConfig, DiscoveredUrl};
use crate::infrastructure::crawler::{extract_links, fetch_url, RobotsFetcher};

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
/// falls back to the static [`fetch_url`] helper.
pub(crate) struct ProductionPageFetcher {
    /// Optional fetch downloader for hybrid/full JS rendering.
    pub(crate) router: Option<Arc<dyn Downloader>>,
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
                let html = fetch_url(url.as_str(), config).await?;
                Ok(FetchOutcome {
                    body: html,
                    final_url: url.clone(),
                    cookies: Vec::new(),
                    status: 200,
                })
            }
        })
    }
}

/// Production [`RobotsChecker`] backed by [`RobotsFetcher`].
pub(crate) struct ProductionRobotsChecker {
    /// Shared robots.txt fetcher with per-domain cache.
    pub(crate) fetcher: Arc<RobotsFetcher>,
}

impl RobotsChecker for ProductionRobotsChecker {
    fn is_robots_allowed<'a>(&'a self, url: &'a str, domain: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { self.fetcher.is_allowed(url, domain).await })
    }
}

/// Production [`LinkExtractorPort`] delegating to the free function.
pub(crate) struct ProductionLinkExtractor;

impl LinkExtractorPort for ProductionLinkExtractor {
    fn extract_links(&self, html: &str, base_url: &str) -> Result<Vec<String>, CrawlError> {
        extract_links(html, base_url)
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

    #[test]
    fn test_fetch_outcome_preserves_final_url() {
        let requested = Url::parse("https://example.com/redirect/5").expect("valid URL");
        let final_url = Url::parse("https://example.com/final").expect("valid URL");
        let outcome = FetchOutcome {
            body: "<html></html>".to_string(),
            final_url: final_url.clone(),
            cookies: Vec::new(),
            status: 200,
        };
        assert_eq!(outcome.final_url, final_url);
        assert_ne!(outcome.final_url, requested);
        assert_eq!(outcome.body, "<html></html>");
        assert_eq!(outcome.status, 200);
    }

    // -----------------------------------------------------------------------
    // #1024 — branch observability for ProductionPageFetcher
    //
    // `fetch_page` has two branches that were previously indistinguishable to
    // the test suite. The router branch propagates `status` and `cookies` from
    // the `Downloader`; the fallback branch calls `fetch_url`, which returns
    // only a body, and then hardcodes `status: 200` and `cookies: Vec::new()`.
    //
    // That mattered concretely: sub-slice 3.B-1b made `with_js_strategy` build
    // a downloader only when a `DownloaderFactory` is injected, and four
    // `EngineOptions` literals using `..Default::default()` silently flipped to
    // the fallback while still passing. A regression that routed every crawl
    // onto the static fallback would have been invisible to CI.
    //
    // `status_code` is not cosmetic — `run_pipeline` writes it into
    // `ScrapedItem.status_code`, and `ScrapedItem` is what reaches exported
    // output, so the fallback publishes a status the server never sent.
    //
    // # Decision (#1024 AC-3): keep the fabrication, pin it, fix it separately
    //
    // The fabrication is narrower than it first looks. `fetch_url` rejects every
    // non-2xx response before the fallback can build an outcome, so `status: 200`
    // can only misreport *within* the 2xx family (201, 203, 204, 226) — it never
    // turns an error into a success. That makes it a data-correctness defect, not
    // a safety hole.
    //
    // Removing the fallback branch is not an option: `crawl_site` and
    // `crawl_site_capturing` (the batch and MCP chains) never call
    // `with_js_strategy`, so the fallback is the path most of the crawl surface
    // already runs on.
    //
    // Reporting the truth requires `fetch_url` to return the status *and* the
    // post-redirect final URL (the fallback also echoes the requested URL, so it
    // cannot observe redirects). That is a production signature change with its
    // own blast radius across every `fetch_url` caller, and it is not required by
    // #994, whose purpose is to shrink, not grow. Bundling it into a test-only
    // issue would blur review focus, so it is tracked as **#1027**.
    //
    // `fallback_branch_fabricates_status_and_drops_cookies` asserts the current
    // behaviour deliberately. When the real fix lands, that test fails and forces
    // the change to be an explicit decision instead of silent drift.
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
            .concurrency(1)
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

    /// Fallback branch against a **real** HTTP response: `fetch_url` discards
    /// everything but the body, so the outcome reports a status the server never
    /// sent and no cookies even though `Set-Cookie` was present.
    ///
    /// These assertions pin the fabrication on purpose. If the fallback is ever
    /// fixed to report the true status, this test fails and the change is forced
    /// to be a deliberate decision rather than a silent drift.
    #[tokio::test]
    async fn fallback_branch_fabricates_status_and_drops_cookies() {
        let (_server, url) = mock_203_with_cookie().await;
        let config = config_for(&url);
        let fetcher = ProductionPageFetcher { router: None };

        let outcome = fetcher
            .fetch_page(&url, &config)
            .await
            .expect("203 is 2xx, so fetch_url should succeed");

        assert_eq!(
            outcome.status, 200,
            "fallback fabricates 200 even though the server answered 203 (#1024)"
        );
        assert!(
            outcome.cookies.is_empty(),
            "fallback drops Set-Cookie entirely (#1024)"
        );
        assert_eq!(
            outcome.body, "<html>served</html>",
            "the body is the only field the fallback carries faithfully"
        );
        assert_eq!(
            outcome.final_url, url,
            "fallback echoes the requested URL, so it cannot observe redirects"
        );
    }

    /// The regression gate: both branches driven against the same server must
    /// produce **different** observable outcomes.
    ///
    /// Before #1024 no test in the repository could tell the branches apart, so
    /// routing every crawl onto the static fallback passed CI green. This pins
    /// the contract stated in `EngineOptions::downloader_factory`'s doc comment
    /// — no factory means the static fallback, and that is observable.
    #[tokio::test]
    async fn branches_diverge_on_the_same_http_response() {
        let (_server, url) = mock_203_with_cookie().await;
        let config = config_for(&url);

        let routed = ProductionPageFetcher {
            router: Some(Arc::new(RouterDouble {
                status: 203,
                cookies: vec![session_cookie()],
            })),
        };
        let fallback = ProductionPageFetcher { router: None };

        let with_router = routed
            .fetch_page(&url, &config)
            .await
            .expect("router branch should succeed");
        let without_router = fallback
            .fetch_page(&url, &config)
            .await
            .expect("fallback branch should succeed");

        assert_ne!(
            with_router.status, without_router.status,
            "status must distinguish the branches: router reports the real code, \
             fallback fabricates 200"
        );
        assert!(
            !with_router.cookies.is_empty() && without_router.cookies.is_empty(),
            "cookies must distinguish the branches: only the router branch carries them"
        );
    }
}
