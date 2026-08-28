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
//! [`crate::infrastructure::downloader::Downloader`].

use std::sync::Arc;

use futures::future::BoxFuture;
use url::Url;

use crate::application::crawler::collector::{CrawlMessage, ResultsCollector};
use crate::application::crawler::fetch_router::FetchRouter;
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

/// Fetches a web page. Unifies the `FetchRouter` and static `fetch_url()` paths.
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

/// Production [`PageFetcher`] that wraps an optional [`FetchRouter`].
///
/// When a router is present, delegates to [`Downloader::fetch`]; otherwise
/// falls back to the static [`fetch_url`] helper.
pub(crate) struct ProductionPageFetcher {
    /// Optional fetch router for hybrid/full JS rendering.
    pub(crate) router: Option<FetchRouter>,
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
}
