//! Crawl task execution — per-page fetch, pipeline, and link extraction.
//!
//! Extracted from `engine.rs` (strangler fig, issue #439). Holds the free
//! functions spawned per discovered URL by `Engine::run()`:
//! `run_crawl_task` (the per-page worker) and `handle_crawl_result`
//! (task-completion bookkeeping). Both consume `Arc<CrawlTaskCtx>` and carry
//! no `Engine` state.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use tracing::{debug, span, warn, Level};
use url::Url;

use super::checkpoint::BannedDomain;
use super::crawl_task_ctx::CrawlTaskCtx;
use super::ports::waf_challenge_message;
use crate::application::pipeline::{ScrapedItem, StageOutcome};
use crate::application::url_filter::is_allowed;
use crate::domain::{CrawlError, CrawlErrorCategory, DiscoveredUrl};
use crate::infrastructure::crawler::{is_internal_link, UrlSource};
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
        Ok(Err(e)) => {
            let category = CrawlErrorCategory::from(&e);
            warn!("Task error: {}", e);
            error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            error_breakdown[category.index()].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
        Err(e) => {
            warn!("Task panicked: {}", e);
            error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            error_breakdown[CrawlErrorCategory::Panic.index()]
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
    }
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
    ctx.rate_limiter.until_ready().await;

    let url_str = discovered_url.url.as_str().to_string();
    let url_depth = discovered_url.depth;
    let parent_url = discovered_url.url.clone();

    let page_correlation = ctx.correlation_id.child();
    let page_span = span!(
        Level::DEBUG,
        "crawl_page",
        correlation_id = %page_correlation,
        trace_id = %page_correlation.trace_id(),
        url = %url_str,
        depth = url_depth
    );
    let _page_guard = page_span.enter();

    let mut session_id = None;
    if let Some(ref pool) = ctx.session_pool {
        let domain = url::Url::parse(&url_str)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_default();
        match pool.acquire(&domain) {
            Some(id) => {
                session_id = Some(id);
            },
            None => {
                debug!("Domain {} has no available sessions, skipping", domain);
                return Ok(());
            },
        }
    }

    debug!("Crawling: {} (depth={})", url_str, url_depth);

    let parsed_url =
        url::Url::parse(&url_str).map_err(|e| CrawlError::Internal(format!("invalid URL: {e}")))?;

    let (response, fetched_cookies) = match ctx.fetcher.fetch_page(&parsed_url, &ctx.config).await {
        Ok((html, cookies)) => (html, cookies),
        Err(e) => {
            if let Some(waf_msg) = waf_challenge_message(&e) {
                if let Some(domain) = parsed_url.host_str() {
                    let banned = BannedDomain {
                        domain: domain.to_string(),
                        banned_until: None,
                        reason: waf_msg.clone(),
                    };
                    if let Ok(mut domains) = ctx.banned_domains.write() {
                        if !domains.iter().any(|d| d.domain == domain) {
                            domains.push(banned);
                            warn!("Banned domain {} due to WAF: {}", domain, waf_msg);
                        }
                    }
                }
                log_scrape_error(
                    &waf_msg,
                    &url_str,
                    "fetch",
                    Some(&page_correlation),
                    "WAF challenge detected",
                );
            } else {
                log_scrape_error(
                    &e,
                    &url_str,
                    "fetch",
                    Some(&page_correlation),
                    "page fetch failed",
                );
            }
            return Err(e);
        },
    };

    if !fetched_cookies.is_empty() {
        if let Ok(mut bridge) = ctx.cookie_bridge.write() {
            for cookie in &fetched_cookies {
                bridge.add(cookie.clone());
            }
        }
    }

    if let Some(ref pool) = ctx.session_pool {
        if let Some(id) = session_id {
            if let Ok(parsed) = url::Url::parse(&url_str) {
                if let Some(domain) = parsed.host_str() {
                    pool.report_success(domain, id);
                }
            }
        }
    }

    ctx.pages_crawled
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    if let Some(ref pipeline) = ctx.pipeline {
        let item = ScrapedItem {
            url: url_str.clone(),
            raw_html: response.clone(),
            text_content: None,
            metadata: std::collections::HashMap::new(),
            status_code: 200,
            embeddings: None,
        };

        match pipeline.execute_pipeline(item).await {
            StageOutcome::Continue(processed_item) => {
                for stage in &ctx.output_stages {
                    if let Err(e) = stage.write(&processed_item).await {
                        warn!("Output stage '{}' failed: {}", stage.name(), e);
                    }
                }
            },
            StageOutcome::Skip => {
                debug!("Pipeline skipped item: {}", url_str);
                return Ok(());
            },
            StageOutcome::Reject(reason) => {
                warn!("Pipeline rejected {}: {}", url_str, reason);
                return Ok(());
            },
        }
    }

    if let Err(e) = ctx.collector.send_result(discovered_url).await {
        debug!("Failed to send result: {}", e);
    }

    if url_depth < ctx.config.max_depth {
        match ctx.link_extractor.extract_links(&response, &url_str) {
            Ok(links) => {
                for link in links {
                    if let Ok(parsed_link) = Url::parse(&link) {
                        if let Some(seed_domain) = ctx.config.seed_url.host_str() {
                            let link_domain = parsed_link.host_str().unwrap_or("");
                            if is_internal_link(&link, seed_domain)
                                && is_allowed(&link, &ctx.config)
                                && (ctx.ignore_robots
                                    || ctx
                                        .robots_checker
                                        .is_robots_allowed(&link, link_domain)
                                        .await)
                                && ctx.visited.try_insert(&link)
                            {
                                if let Ok(mut urls) = ctx.visited_urls.write() {
                                    urls.push(link.clone());
                                }

                                let new_discovered = DiscoveredUrl::html(
                                    parsed_link,
                                    url_depth + 1,
                                    parent_url.clone(),
                                );
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

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, RwLock};

    use url::Url;

    use super::*;
    use crate::application::crawler::ports::{
        ContentPipeline, CrawlResultCollector, LinkExtractorPort, PageFetcher, RobotsChecker,
    };
    use crate::application::deduplicator::UrlDeduplicator;
    use crate::application::pipeline::stages::output::OutputError;
    use crate::application::pipeline::OutputStage;
    use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
    use crate::domain::session_port::{SessionId, SessionPort};
    use crate::domain::CorrelationId;
    use crate::infrastructure::crawler::UrlQueue;
    use crate::infrastructure::downloader::cookie_bridge::CookieBridge;
    use crate::infrastructure::downloader::Cookie;

    // ── Mock implementations ──

    enum FetchBehavior {
        Success(String, Vec<Cookie>),
        WafError(String),
        NetworkError(String),
    }

    struct MockPageFetcher {
        behavior: FetchBehavior,
    }

    impl PageFetcher for MockPageFetcher {
        fn fetch_page<'a>(
            &'a self,
            _url: &'a Url,
            _config: &'a crate::domain::CrawlerConfig,
        ) -> Pin<Box<dyn Future<Output = Result<(String, Vec<Cookie>), CrawlError>> + Send + 'a>>
        {
            Box::pin(async move {
                match &self.behavior {
                    FetchBehavior::Success(html, cookies) => Ok((html.clone(), cookies.clone())),
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

            let rate_limiter = SharedRateLimiter::new(&RateLimiterConfig::new(1, 100))
                .expect("valid rate limiter config");

            Arc::new(CrawlTaskCtx {
                config: Arc::new(config),
                correlation_id: CorrelationId::new(),
                visited: Arc::new(UrlDeduplicator::new()),
                visited_urls: Arc::new(RwLock::new(Vec::new())),
                queue: Arc::new(UrlQueue::new()),
                rate_limiter,
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
        assert!(ctx.visited.try_insert("https://example.com/dup"));
        let queue = Arc::clone(&ctx.queue);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        assert!(queue.is_empty().await);
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
            }))
            .build();
        let bridge = Arc::clone(&ctx.cookie_bridge);

        run_crawl_task(ctx, test_url("https://example.com/", 0))
            .await
            .expect("crawl should succeed");
        let b = bridge.read().expect("lock not poisoned");
        assert_eq!(b.len(), 0);
    }
}
