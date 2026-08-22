//! Crawl task execution — per-page fetch, pipeline, and link extraction.
//!
//! Extracted from `engine.rs` (strangler fig, issue #439). Holds the free
//! functions spawned per discovered URL by `Engine::run()`:
//! `run_crawl_task` (the per-page worker) and `handle_crawl_result`
//! (task-completion bookkeeping). Both consume `Arc<CrawlTaskCtx>` and carry
//! no `Engine` state.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use tracing::{debug, instrument, warn};
use url::Url;

use super::checkpoint::BannedDomain;
use super::crawl_task_ctx::CrawlTaskCtx;
use super::ports::{waf_challenge_message, FetchOutcome};
use crate::application::pipeline::{ScrapedItem, StageOutcome};
use crate::application::url_filter::is_allowed;
use crate::domain::session_port::SessionId;
use crate::domain::{CorrelationId, CrawlError, CrawlErrorCategory, DiscoveredUrl};
use crate::infrastructure::crawler::{is_internal_link, UrlSource};
use crate::infrastructure::downloader::Cookie;
use crate::infrastructure::observability::log_scrape_error;

/// Handle result from a completed crawl task
pub(crate) fn handle_crawl_result(
    result: std::result::Result<Result<(), CrawlError>, tokio::task::JoinError>,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
) {
    match result {
        Ok(Ok(())) => {
            // Task completed successfully
        },
        Ok(Err(e)) => handle_task_error(e, error_count, error_breakdown),
        Err(e) => handle_join_error(e, error_count, error_breakdown),
    }
}

/// Bookkeeping for a task that returned an operational error.
///
/// Shutdown cancellation is a control signal, not a failure (#509), so it
/// must not inflate the crawl error counters.
fn handle_task_error(
    e: CrawlError,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
) {
    if matches!(e, CrawlError::Cancelled) {
        debug!("Task cancelled by engine shutdown");
        return;
    }
    let category = CrawlErrorCategory::from(&e);
    warn!("Task error: {}", e);
    error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    error_breakdown[category.index()].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Bookkeeping for a task that failed to join (abort or panic).
fn handle_join_error(
    e: tokio::task::JoinError,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
) {
    if e.is_cancelled() {
        // JoinSet abort (shutdown drain timeout, #509) — logged once at
        // the drain site; not counted as a panic.
        debug!("Task aborted during shutdown drain");
        return;
    }
    warn!("Task panicked: {}", e);
    error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    error_breakdown[CrawlErrorCategory::Panic.index()]
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Execute a single crawl task using shared context.
///
/// Extracted from the inline async block in `Engine::run()` to reduce
/// the per-spawn clone surface from 18 individual `Arc::clone()` calls
/// to a single `Arc<CrawlTaskCtx>` clone.
pub(crate) async fn run_crawl_task(
    ctx: Arc<CrawlTaskCtx>,
    discovered_url: DiscoveredUrl,
) -> Result<(), CrawlError> {
    match ctx
        .rate_limiter
        .until_ready_or_cancel(&ctx.cancel_token)
        .await
    {
        Ok(()) => {},
        Err(_cancelled) => {
            debug!("Rate limit wait cancelled by engine shutdown");
            return Err(CrawlError::Cancelled);
        },
    }

    let page_correlation = ctx.correlation_id.child();
    run_crawl_task_inner(ctx, discovered_url, page_correlation).await
}

/// Inner implementation of [`run_crawl_task`] — carries the per-page
/// `crawl_page` span (#519).
///
/// The `#[instrument]` span declares the per-page identity (`correlation_id`,
/// `trace_id`) AT CREATION time (#501): FileTraceLayer snapshots span fields
/// in `on_new_span`, so fields recorded later never reach the `--trace-file`
/// JSONL. The instrumented span lifecycle is also async-safe — no `enter()`
/// guard crosses an `.await` (#519).
#[instrument(
    name = "crawl_page",
    skip(ctx, page_correlation, discovered_url),
    fields(
        correlation_id = %page_correlation,
        trace_id = %page_correlation.trace_id(),
        url = %discovered_url.url,
        depth = discovered_url.depth
    )
)]
async fn run_crawl_task_inner(
    ctx: Arc<CrawlTaskCtx>,
    discovered_url: DiscoveredUrl,
    page_correlation: CorrelationId,
) -> Result<(), CrawlError> {
    let url_str = discovered_url.url.as_str().to_string();
    let url_depth = discovered_url.depth;

    // Acquire a per-domain session when the pool is enabled. `Err(())` means
    // the pool had no available session for this domain — skip the URL.
    let session_id = match acquire_session(&ctx, &url_str) {
        Ok(id) => id,
        Err(()) => return Ok(()),
    };

    debug!("Crawling: {} (depth={})", url_str, url_depth);

    let parsed_url =
        url::Url::parse(&url_str).map_err(|e| CrawlError::Internal(format!("invalid URL: {e}")))?;

    let outcome = fetch_page(&ctx, &parsed_url, &url_str, &page_correlation).await?;
    // Crash-injection: fetch completed, nothing processed yet (batch path).
    crate::cli::crash_points::hit(crate::cli::crash_points::MID_FETCH);
    // The post-redirect URL is where the content actually lives. It drives
    // output keying, deduplication, relative-link resolution, and the recorded
    // result, so aliases collapse onto one document instead of producing
    // duplicate content (#651, Bug 3).
    let final_url = outcome.final_url.clone();
    let response = outcome.body;
    let fetched_cookies = outcome.cookies;

    ingest_cookies(&ctx, &fetched_cookies);
    report_session_success(&ctx, &url_str, session_id);

    ctx.pages_crawled
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    capture_content(&ctx, final_url.as_str(), &response);

    // Crash-injection: fetched + spooled; clean/validate/extraction not run.
    crate::cli::crash_points::hit(crate::cli::crash_points::POST_FETCH_PRE_EXTRACT);
    if !run_pipeline(&ctx, final_url.as_str(), &response, outcome.status).await {
        return Ok(());
    }

    let mut result_url = discovered_url.clone();
    result_url.url = final_url.clone();
    if let Err(e) = ctx.collector.send_result(result_url).await {
        debug!("Failed to send result: {}", e);
    }

    extract_and_queue_links(&ctx, &response, final_url.as_str(), url_depth, &final_url).await;

    Ok(())
}

/// Acquire a per-domain session from the pool, if one is configured.
///
/// Returns `Ok(Some(id))` on acquisition, `Ok(None)` when no pool is enabled,
/// and `Err(())` when the pool exists but has no available session (the caller
/// should skip the URL).
fn acquire_session(ctx: &CrawlTaskCtx, url_str: &str) -> Result<Option<SessionId>, ()> {
    let Some(ref pool) = ctx.session_pool else {
        return Ok(None);
    };
    let domain = url::Url::parse(url_str)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default();
    match pool.acquire(&domain) {
        Some(id) => Ok(Some(id)),
        None => {
            debug!("Domain {} has no available sessions, skipping", domain);
            Err(())
        },
    }
}

/// Fetch the page, handling WAF challenges (banning the domain) and logging
/// fetch failures before propagating the error.
async fn fetch_page(
    ctx: &CrawlTaskCtx,
    parsed_url: &Url,
    url_str: &str,
    page_correlation: &CorrelationId,
) -> Result<FetchOutcome, CrawlError> {
    match ctx.fetcher.fetch_page(parsed_url, &ctx.config).await {
        Ok(outcome) => Ok(outcome),
        Err(e) => {
            if let Some(waf_msg) = waf_challenge_message(&e) {
                ban_waf_domain(ctx, parsed_url, &waf_msg);
                log_scrape_error(
                    &waf_msg,
                    url_str,
                    "fetch",
                    Some(page_correlation),
                    "WAF challenge detected",
                );
            } else {
                log_scrape_error(
                    &e,
                    url_str,
                    "fetch",
                    Some(page_correlation),
                    "page fetch failed",
                );
            }
            Err(e)
        },
    }
}

/// Ban a domain that triggered a WAF challenge, unless it is already banned.
fn ban_waf_domain(ctx: &CrawlTaskCtx, parsed_url: &Url, waf_msg: &str) {
    let Some(domain) = parsed_url.host_str() else {
        return;
    };
    let banned = BannedDomain {
        domain: domain.to_string(),
        banned_until: None,
        reason: waf_msg.to_string(),
    };
    if let Ok(mut domains) = ctx.banned_domains.write() {
        if !domains.iter().any(|d| d.domain == domain) {
            domains.push(banned);
            warn!("Banned domain {} due to WAF: {}", domain, waf_msg);
        }
    }
}

/// Ingest any cookies returned by the fetch into the shared cookie bridge.
fn ingest_cookies(ctx: &CrawlTaskCtx, fetched_cookies: &[Cookie]) {
    if fetched_cookies.is_empty() {
        return;
    }
    if let Ok(mut bridge) = ctx.cookie_bridge.write() {
        for cookie in fetched_cookies {
            bridge.add(cookie.clone());
        }
    }
}

/// Report a successful fetch back to the session pool for the acquired session.
fn report_session_success(ctx: &CrawlTaskCtx, url_str: &str, session_id: Option<SessionId>) {
    let Some(ref pool) = ctx.session_pool else {
        return;
    };
    let Some(id) = session_id else {
        return;
    };
    if let Ok(parsed) = url::Url::parse(url_str) {
        if let Some(domain) = parsed.host_str() {
            pool.report_success(domain, id);
        }
    }
}

/// Hand the fetched body to the content sink when one is configured (#631).
///
/// Capture happens right after a successful fetch and before pipeline gating,
/// so a `Skip`/`Reject` outcome cannot silently drop the page from the export
/// set. The sink is synchronous and must not block.
fn capture_content(ctx: &CrawlTaskCtx, url_str: &str, response: &str) {
    let Some(ref sink) = ctx.content_sink else {
        return;
    };
    sink.capture(url_str, response);
}

/// Run the content pipeline if configured. Returns `true` when processing
/// should continue (no pipeline, or `Continue`); `false` when the item was
/// skipped or rejected (the caller should stop processing this page).
async fn run_pipeline(ctx: &CrawlTaskCtx, url_str: &str, response: &str, status_code: u16) -> bool {
    let Some(ref pipeline) = ctx.pipeline else {
        return true;
    };
    let item = ScrapedItem {
        url: url_str.to_string(),
        raw_html: response.to_string(),
        text_content: None,
        metadata: std::collections::HashMap::new(),
        status_code,
        embeddings: None,
    };
    match pipeline.execute_pipeline(item).await {
        StageOutcome::Continue(processed_item) => {
            write_to_output_stages(ctx, &processed_item).await;
            true
        },
        StageOutcome::Skip => {
            debug!("Pipeline skipped item: {}", url_str);
            false
        },
        StageOutcome::Reject(reason) => {
            warn!("Pipeline rejected {}: {}", url_str, reason);
            false
        },
    }
}

/// Write a processed item to every configured output stage.
async fn write_to_output_stages(ctx: &CrawlTaskCtx, item: &ScrapedItem) {
    for stage in &ctx.output_stages {
        if let Err(e) = stage.write(item).await {
            warn!("Output stage '{}' failed: {}", stage.name(), e);
        }
    }
}

/// Extract links from the page and enqueue internal, allowed, robots-permitted
/// links for further crawling (subject to the configured max depth).
async fn extract_and_queue_links(
    ctx: &CrawlTaskCtx,
    response: &str,
    url_str: &str,
    url_depth: u8,
    parent_url: &Url,
) {
    if url_depth >= ctx.config.max_depth {
        return;
    }
    match ctx.link_extractor.extract_links(response, url_str) {
        Ok(links) => {
            for link in links {
                if let Ok(parsed_link) = Url::parse(&link) {
                    if let Some(seed_domain) = ctx.config.seed_url.host_str() {
                        let link_domain = parsed_link.host_str().unwrap_or("");
                        // Enqueue-time dedup is owned by the queue's `seen` set
                        // inside `push_prioritized`. The `visited` set is marked
                        // only at dispatch time (`CrawlScheduler::record_visit`),
                        // so a freshly discovered link must NOT be pre-marked
                        // here: doing so made `next_url` discard it as already
                        // visited, silently dropping every internal link (#479).
                        if is_internal_link(&link, seed_domain)
                            && is_allowed(&link, &ctx.config)
                            && (ctx.ignore_robots
                                || ctx
                                    .robots_checker
                                    .is_robots_allowed(&link, link_domain)
                                    .await)
                        {
                            let new_discovered =
                                DiscoveredUrl::html(parsed_link, url_depth + 1, parent_url.clone());
                            ctx.queue
                                .push_prioritized(new_discovered, UrlSource::Link)
                                .await;
                        }
                    }
                }
            }
        },
        Err(e) => {
            warn!("Failed to extract links from {}: {}", url_str, e);
            ctx.error_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ctx.error_breakdown[CrawlErrorCategory::Extraction.index()]
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, RwLock};
    use std::time::Duration;

    use url::Url;

    use super::*;
    use crate::application::crawler::ports::{
        ContentPipeline, CrawlResultCollector, FetchOutcome, LinkExtractorPort, PageFetcher,
        RobotsChecker,
    };
    use crate::application::pipeline::stages::output::OutputError;
    use crate::application::pipeline::OutputStage;
    use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
    use crate::domain::session_port::{SessionId, SessionPort};
    use crate::domain::CorrelationId;
    use crate::infrastructure::crawler::UrlQueue;
    use crate::infrastructure::downloader::cookie_bridge::CookieBridge;
    use crate::infrastructure::downloader::Cookie;
    use tokio_util::sync::CancellationToken;

    // ── Mock implementations ──

    enum FetchBehavior {
        Success(String, Vec<Cookie>),
        WafError(String),
        NetworkError(String),
    }

    struct MockPageFetcher {
        behavior: FetchBehavior,
        final_url: Option<Url>,
    }

    impl PageFetcher for MockPageFetcher {
        fn fetch_page<'a>(
            &'a self,
            _url: &'a Url,
            _config: &'a crate::domain::CrawlerConfig,
        ) -> Pin<Box<dyn Future<Output = Result<FetchOutcome, CrawlError>> + Send + 'a>> {
            Box::pin(async move {
                match &self.behavior {
                    FetchBehavior::Success(html, cookies) => Ok(FetchOutcome {
                        body: html.clone(),
                        final_url: self.final_url.clone().unwrap_or_else(|| _url.clone()),
                        cookies: cookies.clone(),
                        status: 200,
                    }),
                    FetchBehavior::WafError(msg) => Err(CrawlError::WafChallenge {
                        provider: msg.clone(),
                        kind: crate::domain::error::WafDetectionKind::BodySignature,
                        url: String::new(),
                    }),
                    FetchBehavior::NetworkError(msg) => Err(CrawlError::Network {
                        message: msg.clone(),
                        status_code: None,
                    }),
                }
            })
        }
    }

    struct MockRobotsChecker {
        allowed: bool,
        call_count: Arc<AtomicUsize>,
    }

    impl RobotsChecker for MockRobotsChecker {
        fn is_robots_allowed<'a>(
            &'a self,
            _url: &'a str,
            _domain: &'a str,
        ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
            Box::pin(async move {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                self.allowed
            })
        }
    }

    enum ExtractBehavior {
        Links(Vec<String>),
        Error(String),
    }

    struct MockLinkExtractor {
        behavior: ExtractBehavior,
    }

    impl LinkExtractorPort for MockLinkExtractor {
        fn extract_links(&self, _html: &str, _base_url: &str) -> Result<Vec<String>, CrawlError> {
            match &self.behavior {
                ExtractBehavior::Links(links) => Ok(links.clone()),
                ExtractBehavior::Error(msg) => Err(CrawlError::Parse(msg.clone())),
            }
        }
    }

    enum PipelineBehavior {
        Continue,
        Skip,
        Reject(String),
    }

    struct MockPipeline {
        behavior: PipelineBehavior,
    }

    impl ContentPipeline for MockPipeline {
        fn execute_pipeline<'a>(
            &'a self,
            item: ScrapedItem,
        ) -> Pin<Box<dyn Future<Output = StageOutcome> + Send + 'a>> {
            Box::pin(async move {
                match &self.behavior {
                    PipelineBehavior::Continue => StageOutcome::Continue(item),
                    PipelineBehavior::Skip => StageOutcome::Skip,
                    PipelineBehavior::Reject(reason) => StageOutcome::Reject(reason.clone()),
                }
            })
        }
    }

    struct MockCollector {
        sent: Arc<Mutex<Vec<DiscoveredUrl>>>,
    }

    impl CrawlResultCollector for MockCollector {
        fn send_result<'a>(
            &'a self,
            url: DiscoveredUrl,
        ) -> Pin<Box<dyn Future<Output = Result<(), CrawlError>> + Send + 'a>> {
            Box::pin(async move {
                self.sent
                    .lock()
                    .map_err(|e| CrawlError::Internal(e.to_string()))?
                    .push(url);
                Ok(())
            })
        }
    }

    struct MockSessionPool {
        acquire_result: Option<SessionId>,
        success_calls: Arc<Mutex<Vec<(String, SessionId)>>>,
    }

    impl SessionPort for MockSessionPool {
        fn acquire(&self, _domain: &str) -> Option<SessionId> {
            self.acquire_result
        }
        fn report_success(&self, domain: &str, session: SessionId) {
            if let Ok(mut calls) = self.success_calls.lock() {
                calls.push((domain.to_string(), session));
            }
        }
        fn report_failure(&self, _domain: &str, _session: SessionId, _status: u16) {}
    }

    struct MockOutputStage {
        written: Arc<Mutex<Vec<ScrapedItem>>>,
    }

    impl OutputStage for MockOutputStage {
        fn name(&self) -> &str {
            "mock_output"
        }
        fn write<'a>(
            &'a self,
            item: &'a ScrapedItem,
        ) -> Pin<Box<dyn Future<Output = Result<(), OutputError>> + Send + 'a>> {
            Box::pin(async move {
                self.written
                    .lock()
                    .map_err(|e| OutputError::Backend(e.to_string()))?
                    .push(item.clone());
                Ok(())
            })
        }
    }

    // ── Test helpers ──

    struct TestCtxBuilder {
        fetcher: Arc<dyn PageFetcher>,
        robots: Arc<dyn RobotsChecker>,
        link_extractor: Arc<dyn LinkExtractorPort>,
        pipeline: Option<Arc<dyn ContentPipeline>>,
        collector: Arc<dyn CrawlResultCollector>,
        session_pool: Option<Arc<dyn SessionPort>>,
        output_stages: Vec<Arc<Box<dyn OutputStage>>>,
        rate_limiter: Option<SharedRateLimiter>,
        cancel_token: CancellationToken,
        ignore_robots: bool,
        max_depth: u8,
    }

    impl TestCtxBuilder {
        fn new(collector: Arc<dyn CrawlResultCollector>) -> Self {
            Self {
                fetcher: Arc::new(MockPageFetcher {
                    behavior: FetchBehavior::Success(
                        "<html><body>hello</body></html>".to_string(),
                        Vec::new(),
                    ),
                    final_url: None,
                }),
                robots: Arc::new(MockRobotsChecker {
                    allowed: true,
                    call_count: Arc::new(AtomicUsize::new(0)),
                }),
                link_extractor: Arc::new(MockLinkExtractor {
                    behavior: ExtractBehavior::Links(Vec::new()),
                }),
                pipeline: None,
                collector,
                session_pool: None,
                output_stages: Vec::new(),
                rate_limiter: None,
                cancel_token: CancellationToken::new(),
                ignore_robots: false,
                max_depth: 3,
            }
        }

        fn fetcher(mut self, f: Arc<dyn PageFetcher>) -> Self {
            self.fetcher = f;
            self
        }
        fn robots(mut self, r: Arc<dyn RobotsChecker>) -> Self {
            self.robots = r;
            self
        }
        fn link_extractor(mut self, le: Arc<dyn LinkExtractorPort>) -> Self {
            self.link_extractor = le;
            self
        }
        fn pipeline(mut self, p: Arc<dyn ContentPipeline>) -> Self {
            self.pipeline = Some(p);
            self
        }
        fn session_pool(mut self, sp: Arc<dyn SessionPort>) -> Self {
            self.session_pool = Some(sp);
            self
        }
        fn output_stage(mut self, s: Arc<Box<dyn OutputStage>>) -> Self {
            self.output_stages.push(s);
            self
        }
        fn rate_limiter(mut self, rl: SharedRateLimiter) -> Self {
            self.rate_limiter = Some(rl);
            self
        }
        fn cancel_token(mut self, token: CancellationToken) -> Self {
            self.cancel_token = token;
            self
        }
        fn ignore_robots(mut self, v: bool) -> Self {
            self.ignore_robots = v;
            self
        }
        fn max_depth(mut self, d: u8) -> Self {
            self.max_depth = d;
            self
        }

        fn build(self) -> Arc<CrawlTaskCtx> {
            let seed_url = Url::parse("https://example.com").expect("valid seed URL");
            let mut config = crate::domain::CrawlerConfig::new(seed_url);
            config.max_depth = self.max_depth;

            let rate_limiter = self.rate_limiter.unwrap_or_else(|| {
                SharedRateLimiter::new(&RateLimiterConfig::new(1, 100))
                    .expect("valid rate limiter config")
            });

            Arc::new(CrawlTaskCtx {
                config: Arc::new(config),
                correlation_id: CorrelationId::new(),
                queue: Arc::new(UrlQueue::new()),
                rate_limiter,
                cancel_token: self.cancel_token,
                session_pool: self.session_pool,
                ignore_robots: self.ignore_robots,
                robots_checker: self.robots,
                error_count: Arc::new(AtomicUsize::new(0)),
                error_breakdown: Arc::new([
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                ]),
                pages_crawled: Arc::new(AtomicU64::new(0)),
                collector: self.collector,
                cookie_bridge: Arc::new(RwLock::new(CookieBridge::new())),
                banned_domains: Arc::new(RwLock::new(Vec::new())),
                fetcher: self.fetcher,
                link_extractor: self.link_extractor,
                pipeline: self.pipeline,
                output_stages: self.output_stages,
                content_sink: None,
            })
        }
    }

    fn test_url(url: &str, depth: u8) -> DiscoveredUrl {
        DiscoveredUrl::html(
            Url::parse(url).expect("valid test URL"),
            depth,
            Url::parse("https://example.com").expect("valid parent URL"),
        )
    }

    fn mock_collector() -> (
        Arc<dyn CrawlResultCollector>,
        Arc<Mutex<Vec<DiscoveredUrl>>>,
    ) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let collector = Arc::new(MockCollector {
            sent: Arc::clone(&sent),
        });
        (collector, sent)
    }

    // ── Tests: fetch success path ──

    #[tokio::test]
    async fn test_fetch_success_sends_to_collector() {
        let (collector, sent) = mock_collector();
        let ctx = TestCtxBuilder::new(collector).build();
        let url = test_url("https://example.com/page1", 0);

        let result = run_crawl_task(ctx, url).await;
        assert!(result.is_ok());
        let sent = sent.lock().expect("lock not poisoned");
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].url.as_str(), "https://example.com/page1");
    }

    #[tokio::test]
    async fn test_redirect_propagates_final_url_to_collector() {
        // Bug 3 (#651): a redirect /redirect/5 -> /final must be recorded under
        // /final, not the requested /redirect/5, or aliases collapse into
        // duplicate content.
        let (collector, sent) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .fetcher(Arc::new(MockPageFetcher {
                behavior: FetchBehavior::Success("<html></html>".to_string(), Vec::new()),
                final_url: Some(Url::parse("https://example.com/final").expect("valid URL")),
            }))
            .build();
        let url = test_url("https://example.com/redirect/5", 0);

        let result = run_crawl_task(ctx, url).await;
        assert!(
            result.is_ok(),
            "redirect crawl should succeed, got: {:?}",
            result.err()
        );
        let sent = sent.lock().expect("lock not poisoned");
        assert_eq!(
            sent.len(),
            1,
            "exactly one result must reach the collector after a redirect"
        );
        assert_eq!(sent[0].url.as_str(), "https://example.com/final");
    }

    #[tokio::test]
    async fn test_fetch_success_increments_pages_crawled() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector).build();
        let pages = Arc::clone(&ctx.pages_crawled);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        assert_eq!(pages.load(Ordering::Relaxed), 1);
    }

    // ── Tests: WAF error handling ──

    #[tokio::test]
    async fn test_fetch_waf_error_bans_domain() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .fetcher(Arc::new(MockPageFetcher {
                behavior: FetchBehavior::WafError("Cloudflare".to_string()),
                final_url: None,
            }))
            .build();
        let banned = Arc::clone(&ctx.banned_domains);

        let result = run_crawl_task(ctx, test_url("https://protected.com/page", 0)).await;
        assert!(result.is_err());
        let domains = banned.read().expect("lock not poisoned");
        assert_eq!(domains.len(), 1);
        assert_eq!(domains[0].domain, "protected.com");
    }

    #[tokio::test]
    async fn test_fetch_waf_error_returns_err() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .fetcher(Arc::new(MockPageFetcher {
                behavior: FetchBehavior::WafError("Cloudflare".to_string()),
                final_url: None,
            }))
            .build();

        let result = run_crawl_task(ctx, test_url("https://protected.com/", 0)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_fetch_generic_error_returns_err_no_ban() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .fetcher(Arc::new(MockPageFetcher {
                behavior: FetchBehavior::NetworkError("connection refused".to_string()),
                final_url: None,
            }))
            .build();
        let banned = Arc::clone(&ctx.banned_domains);

        let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
        assert!(result.is_err());
        let domains = banned.read().expect("lock not poisoned");
        assert!(domains.is_empty());
    }

    // ── Tests: session pool ──

    #[tokio::test]
    async fn test_session_pool_acquire_none_skips_url() {
        let (collector, sent) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .session_pool(Arc::new(MockSessionPool {
                acquire_result: None,
                success_calls: Arc::new(Mutex::new(Vec::new())),
            }))
            .build();

        let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
        assert!(result.is_ok());
        let sent = sent.lock().expect("lock not poisoned");
        assert!(sent.is_empty());
    }

    #[tokio::test]
    async fn test_session_pool_acquire_some_continues() {
        let (collector, sent) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .session_pool(Arc::new(MockSessionPool {
                acquire_result: Some(SessionId(0)),
                success_calls: Arc::new(Mutex::new(Vec::new())),
            }))
            .build();

        let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
        assert!(result.is_ok());
        let sent = sent.lock().expect("lock not poisoned");
        assert_eq!(sent.len(), 1);
    }

    #[tokio::test]
    async fn test_session_pool_report_success_called() {
        let (collector, _) = mock_collector();
        let success_calls = Arc::new(Mutex::new(Vec::new()));
        let ctx = TestCtxBuilder::new(collector)
            .session_pool(Arc::new(MockSessionPool {
                acquire_result: Some(SessionId(0)),
                success_calls: Arc::clone(&success_calls),
            }))
            .build();

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        let calls = success_calls.lock().expect("lock not poisoned");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "example.com");
        assert_eq!(calls[0].1, SessionId(0));
    }

    // ── Tests: pipeline ──

    #[tokio::test]
    async fn test_pipeline_continue_runs_output_stages() {
        let (collector, _) = mock_collector();
        let written = Arc::new(Mutex::new(Vec::new()));
        let stage: Arc<Box<dyn OutputStage>> = Arc::new(Box::new(MockOutputStage {
            written: Arc::clone(&written),
        }));
        let ctx = TestCtxBuilder::new(collector)
            .pipeline(Arc::new(MockPipeline {
                behavior: PipelineBehavior::Continue,
            }))
            .output_stage(stage)
            .build();

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        let items = written.lock().expect("lock not poisoned");
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn test_pipeline_skip_returns_early() {
        let (collector, sent) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .pipeline(Arc::new(MockPipeline {
                behavior: PipelineBehavior::Skip,
            }))
            .build();

        let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
        assert!(result.is_ok());
        let sent = sent.lock().expect("lock not poisoned");
        assert!(sent.is_empty());
    }

    #[tokio::test]
    async fn test_pipeline_reject_returns_early() {
        let (collector, sent) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .pipeline(Arc::new(MockPipeline {
                behavior: PipelineBehavior::Reject("bad content".to_string()),
            }))
            .build();

        let result = run_crawl_task(ctx, test_url("https://example.com/", 0)).await;
        assert!(result.is_ok());
        let sent = sent.lock().expect("lock not poisoned");
        assert!(sent.is_empty());
    }

    #[tokio::test]
    async fn test_no_pipeline_skips_processing() {
        let (collector, sent) = mock_collector();
        let ctx = TestCtxBuilder::new(collector).build();

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        let sent = sent.lock().expect("lock not poisoned");
        assert_eq!(sent.len(), 1);
    }

    // ── Tests: link extraction and depth ──

    #[tokio::test]
    async fn test_depth_below_max_extracts_links() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .max_depth(3)
            .link_extractor(Arc::new(MockLinkExtractor {
                behavior: ExtractBehavior::Links(vec!["https://example.com/page2".to_string()]),
            }))
            .build();
        let queue = Arc::clone(&ctx.queue);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        assert!(!queue.is_empty().await);
    }

    #[tokio::test]
    async fn test_depth_at_max_skips_extraction() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .max_depth(1)
            .link_extractor(Arc::new(MockLinkExtractor {
                behavior: ExtractBehavior::Links(vec!["https://example.com/page2".to_string()]),
            }))
            .build();
        let queue = Arc::clone(&ctx.queue);

        run_crawl_task(ctx, test_url("https://example.com/", 1))
            .await
            .expect("crawl should succeed");
        assert!(queue.is_empty().await);
    }

    // ── Tests: robots.txt ──

    #[tokio::test]
    async fn test_robots_denied_link_not_queued() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .robots(Arc::new(MockRobotsChecker {
                allowed: false,
                call_count: Arc::new(AtomicUsize::new(0)),
            }))
            .link_extractor(Arc::new(MockLinkExtractor {
                behavior: ExtractBehavior::Links(vec!["https://example.com/blocked".to_string()]),
            }))
            .build();
        let queue = Arc::clone(&ctx.queue);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        assert!(queue.is_empty().await);
    }

    #[tokio::test]
    async fn test_robots_allowed_link_queued() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .robots(Arc::new(MockRobotsChecker {
                allowed: true,
                call_count: Arc::new(AtomicUsize::new(0)),
            }))
            .link_extractor(Arc::new(MockLinkExtractor {
                behavior: ExtractBehavior::Links(vec!["https://example.com/allowed".to_string()]),
            }))
            .build();
        let queue = Arc::clone(&ctx.queue);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        assert!(!queue.is_empty().await);
    }

    #[tokio::test]
    async fn test_ignore_robots_bypasses_check() {
        let (collector, _) = mock_collector();
        let call_count = Arc::new(AtomicUsize::new(0));
        let ctx = TestCtxBuilder::new(collector)
            .ignore_robots(true)
            .robots(Arc::new(MockRobotsChecker {
                allowed: false,
                call_count: Arc::clone(&call_count),
            }))
            .link_extractor(Arc::new(MockLinkExtractor {
                behavior: ExtractBehavior::Links(vec!["https://example.com/page".to_string()]),
            }))
            .build();
        let queue = Arc::clone(&ctx.queue);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        assert!(!queue.is_empty().await);
        assert_eq!(call_count.load(Ordering::SeqCst), 0);
    }

    // ── Tests: deduplication ──

    #[tokio::test]
    async fn test_duplicate_link_not_queued() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .link_extractor(Arc::new(MockLinkExtractor {
                behavior: ExtractBehavior::Links(vec!["https://example.com/dup".to_string()]),
            }))
            .build();
        let queue = Arc::clone(&ctx.queue);
        // Pre-enqueue the URL so the queue's dedup set (`seen`) already contains
        // it. Enqueue-time dedup lives in `push_prioritized`, not in the `visited`
        // set (which is reserved for dispatch-time dedup — see #479).
        let seed = test_url("https://example.com/", 0);
        let dup = DiscoveredUrl::html(
            Url::parse("https://example.com/dup").expect("valid test URL"),
            1,
            seed.url.clone(),
        );
        assert!(queue.push_prioritized(dup, UrlSource::Link).await);

        run_crawl_task(ctx, seed)
            .await
            .expect("crawl should succeed");
        // Only the pre-seeded entry remains — the task's duplicate was rejected.
        assert_eq!(queue.len().await, 1);
    }

    // ── Tests: external links ──

    #[tokio::test]
    async fn test_external_link_not_queued() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .link_extractor(Arc::new(MockLinkExtractor {
                behavior: ExtractBehavior::Links(vec!["https://other.com/page".to_string()]),
            }))
            .build();
        let queue = Arc::clone(&ctx.queue);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        assert!(queue.is_empty().await);
    }

    // ── Tests: link extraction error ──

    #[tokio::test]
    async fn test_link_extraction_error_increments_error_count() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .link_extractor(Arc::new(MockLinkExtractor {
                behavior: ExtractBehavior::Error("parse failed".to_string()),
            }))
            .build();
        let error_count = Arc::clone(&ctx.error_count);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed despite extraction error");
        assert_eq!(error_count.load(Ordering::SeqCst), 1);
    }

    // ── Tests: cookies ──

    #[tokio::test]
    async fn test_cookies_ingested_into_bridge() {
        let (collector, _) = mock_collector();
        let cookie = Cookie {
            name: "session".to_string(),
            value: "abc123".to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            http_only: true,
            secure: true,
        };
        let ctx = TestCtxBuilder::new(collector)
            .fetcher(Arc::new(MockPageFetcher {
                behavior: FetchBehavior::Success("<html></html>".to_string(), vec![cookie]),
                final_url: None,
            }))
            .build();
        let bridge = Arc::clone(&ctx.cookie_bridge);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        let b = bridge.read().expect("lock not poisoned");
        assert_eq!(b.len(), 1);
    }

    #[tokio::test]
    async fn test_no_cookies_skips_bridge() {
        let (collector, _) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .fetcher(Arc::new(MockPageFetcher {
                behavior: FetchBehavior::Success("<html></html>".to_string(), Vec::new()),
                final_url: None,
            }))
            .build();
        let bridge = Arc::clone(&ctx.cookie_bridge);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        let b = bridge.read().expect("lock not poisoned");
        assert_eq!(b.len(), 0);
    }

    // --- handle_crawl_result unit tests (PR #476) ---

    fn counters() -> (Arc<AtomicUsize>, Arc<[AtomicUsize; 8]>) {
        (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(std::array::from_fn(|_| AtomicUsize::new(0))),
        )
    }

    #[test]
    fn success_leaves_counters_unchanged() {
        let (error_count, breakdown) = counters();
        handle_crawl_result(Ok(Ok(())), &error_count, &breakdown);
        assert_eq!(error_count.load(Ordering::SeqCst), 0);
        assert!(breakdown.iter().all(|c| c.load(Ordering::SeqCst) == 0));
    }

    #[test]
    fn crawl_error_increments_count_and_own_category() {
        let (error_count, breakdown) = counters();
        handle_crawl_result(Ok(Err(CrawlError::Timeout)), &error_count, &breakdown);
        assert_eq!(error_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            breakdown[CrawlErrorCategory::Timeout.index()].load(Ordering::SeqCst),
            1
        );
        for cat in CrawlErrorCategory::ALL {
            if cat != CrawlErrorCategory::Timeout {
                assert_eq!(breakdown[cat.index()].load(Ordering::SeqCst), 0);
            }
        }
    }

    #[test]
    fn http_403_increments_waf_category() {
        // Issue #603: a 403 must be counted as errors_waf, not network/http.
        let (error_count, breakdown) = counters();
        handle_crawl_result(
            Ok(Err(CrawlError::Http {
                status: 403,
                url: "https://example.com".into(),
            })),
            &error_count,
            &breakdown,
        );
        assert_eq!(error_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            breakdown[CrawlErrorCategory::Waf.index()].load(Ordering::SeqCst),
            1
        );
        for cat in CrawlErrorCategory::ALL {
            if cat != CrawlErrorCategory::Waf {
                assert_eq!(
                    breakdown[cat.index()].load(Ordering::SeqCst),
                    0,
                    "category {cat} must remain zero for a 403"
                );
            }
        }
    }

    #[test]
    fn crawl_error_category_is_derived_from_error() {
        let (error_count, breakdown) = counters();
        handle_crawl_result(
            Ok(Err(CrawlError::Parse("bad html".into()))),
            &error_count,
            &breakdown,
        );
        assert_eq!(error_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            breakdown[CrawlErrorCategory::Extraction.index()].load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            breakdown[CrawlErrorCategory::Timeout.index()].load(Ordering::SeqCst),
            0,
            "category must follow the error, not a fixed index"
        );
    }

    #[tokio::test]
    async fn join_error_increments_panic_category() {
        let (error_count, breakdown) = counters();
        let join_err = tokio::spawn(async { panic!("boom") }).await.unwrap_err();
        handle_crawl_result(Err(join_err), &error_count, &breakdown);
        assert_eq!(error_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            breakdown[CrawlErrorCategory::Panic.index()].load(Ordering::SeqCst),
            1
        );
        for cat in CrawlErrorCategory::ALL {
            if cat != CrawlErrorCategory::Panic {
                assert_eq!(breakdown[cat.index()].load(Ordering::SeqCst), 0);
            }
        }
    }

    // ========================================================================
    // Cancellation tests (#509)
    // ========================================================================

    #[test]
    fn cancelled_result_is_not_counted_as_error() {
        let (error_count, breakdown) = counters();
        handle_crawl_result(Ok(Err(CrawlError::Cancelled)), &error_count, &breakdown);
        assert_eq!(
            error_count.load(Ordering::SeqCst),
            0,
            "shutdown cancellation must not inflate the error count"
        );
        assert!(breakdown.iter().all(|c| c.load(Ordering::SeqCst) == 0));
    }

    #[tokio::test]
    async fn aborted_join_result_is_not_counted_as_panic() {
        let (error_count, breakdown) = counters();
        let handle = tokio::spawn(std::future::pending::<Result<(), CrawlError>>());
        handle.abort();
        let join_err = handle.await.unwrap_err();
        assert!(join_err.is_cancelled());

        handle_crawl_result(Err(join_err), &error_count, &breakdown);
        assert_eq!(
            error_count.load(Ordering::SeqCst),
            0,
            "drain-time aborts are a control signal, not panics"
        );
        assert!(breakdown.iter().all(|c| c.load(Ordering::SeqCst) == 0));
    }

    #[tokio::test]
    async fn run_crawl_task_cancelled_during_rate_limit_wait() {
        // 60s refill with burst 1: consuming the burst forces the task into
        // the rate-limit wait, where only cancellation can free it.
        let limiter = SharedRateLimiter::new(&RateLimiterConfig::new(60_000, 1))
            .expect("valid rate limiter config");
        limiter.until_ready().await;

        let cancel = CancellationToken::new();
        let (collector, sent) = mock_collector();
        let ctx = TestCtxBuilder::new(collector)
            .rate_limiter(limiter)
            .cancel_token(cancel.clone())
            .build();

        cancel.cancel();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            run_crawl_task(ctx, test_url("https://example.com/page", 0)),
        )
        .await;

        assert!(
            matches!(result, Ok(Err(CrawlError::Cancelled))),
            "worker parked on the rate limiter must abort within the bound"
        );
        assert!(
            sent.lock().map(|s| s.is_empty()).unwrap_or(false),
            "cancelled task must not emit results"
        );
    }
}
