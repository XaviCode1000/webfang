//! Shared context for all crawl tasks spawned by the engine.
//!
//! Consolidates the 18 `Arc::clone()` calls that were previously done
//! per-`tokio::spawn()` into a single `Arc<CrawlTaskCtx>`, reducing
//! atomic contention, cache pollution, and the cost of adding new
//! shared resources.

use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, RwLock};

use tokio::sync::RwLock as AsyncRwLock;
use tokio_util::sync::CancellationToken;

use crate::application::crawler::checkpoint::BannedDomain;
use crate::application::crawler::content_sink::CrawlContentSink;
use crate::application::crawler::ports::{
    ContentPipeline, CrawlResultCollector, LinkExtractorPort, PageFetcher, RobotsChecker,
};
use crate::application::pipeline::OutputStage;
use crate::application::rate_limiter::SharedRateLimiter;
use crate::domain::cookie_bridge::CookieBridge;
use crate::domain::crawler_port::UrlQueuePort;
use crate::domain::session_port::SessionPort;
use crate::domain::{CorrelationId, CrawlerConfig};

/// Shared context for all crawl tasks spawned by the engine.
///
/// Instead of cloning 18 individual `Arc`s per `tokio::spawn()`,
/// we construct one `Arc<CrawlTaskCtx>` and clone only the `Arc` wrapper.
pub struct CrawlTaskCtx {
    // --- Shared config (read-only) ---
    pub(crate) config: Arc<CrawlerConfig>,
    /// Root correlation ID for the crawl — every task derives a child from it
    /// so all pages share one `trace_id` (issue #356).
    pub(crate) correlation_id: CorrelationId,
    /// Shared discovery queue — tasks push discovered links here. Erased
    /// behind the domain port (ADR-0012-B unit 8); `Arc`-shared with the
    /// scheduler so dedup state stays global.
    pub(crate) queue: Arc<dyn UrlQueuePort>,
    pub(crate) rate_limiter: SharedRateLimiter,
    /// Engine-wide cancellation token (#509) — fired on shutdown so tasks
    /// blocked on rate-limit or resource waits abort promptly.
    pub(crate) cancel_token: CancellationToken,
    pub(crate) session_pool: Option<Arc<dyn SessionPort>>,
    pub(crate) ignore_robots: bool,
    pub(crate) robots_checker: Arc<dyn RobotsChecker>,

    // --- Per-task mutable (atomics) ---
    pub(crate) error_count: Arc<AtomicUsize>,
    /// Per-category error counters indexed by `CrawlErrorCategory::index()` (issue #374).
    pub(crate) error_breakdown: Arc<[AtomicUsize; 8]>,
    pub(crate) pages_crawled: Arc<AtomicU64>,

    // --- Infrastructure (port-based) ---
    pub(crate) collector: Arc<dyn CrawlResultCollector>,
    /// Shared cookie jar — `tokio::sync::RwLock` (#1119): every acquisition
    /// happens inside async crawl-task futures, so a contended lock yields
    /// the worker instead of parking an executor thread, and the async lock
    /// cannot be poisoned.
    pub(crate) cookie_bridge: Arc<AsyncRwLock<CookieBridge>>,
    pub(crate) banned_domains: Arc<RwLock<Vec<BannedDomain>>>,
    pub(crate) fetcher: Arc<dyn PageFetcher>,
    pub(crate) link_extractor: Arc<dyn LinkExtractorPort>,

    // --- Pipeline ---
    pub(crate) pipeline: Option<Arc<dyn ContentPipeline>>,
    pub(crate) output_stages: Vec<Arc<Box<dyn OutputStage>>>,

    /// Optional sink that captures every fetched page body (#631).
    ///
    /// `CrawlResult` is metadata only, so without this the crawl discards the
    /// content and batch mode has nothing to export.
    pub(crate) content_sink: Option<Arc<dyn CrawlContentSink>>,
}
