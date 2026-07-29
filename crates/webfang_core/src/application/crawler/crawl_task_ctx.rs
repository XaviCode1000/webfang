//! Shared context for all crawl tasks spawned by the engine.
//!
//! Consolidates the 18 `Arc::clone()` calls that were previously done
//! per-`tokio::spawn()` into a single `Arc<CrawlTaskCtx>`, reducing
//! atomic contention, cache pollution, and the cost of adding new
//! shared resources.

use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, RwLock};

use crate::application::crawler::checkpoint::BannedDomain;
use crate::application::crawler::collector::ResultsCollector;
use crate::application::crawler::engine::FetchRouter;
use crate::application::deduplicator::UrlDeduplicator;
use crate::application::pipeline::{OutputStage, PipelineExecutor};
use crate::application::rate_limiter::SharedRateLimiter;
use crate::domain::CrawlerConfig;
use crate::infrastructure::crawler::RobotsFetcher;
use crate::infrastructure::crawler::UrlQueue;
use crate::infrastructure::downloader::cookie_bridge::CookieBridge;
use crate::infrastructure::network::session_pool::DomainSessionPool;

/// Shared context for all crawl tasks spawned by the engine.
///
/// Instead of cloning 18 individual `Arc`s per `tokio::spawn()`,
/// we construct one `Arc<CrawlTaskCtx>` and clone only the `Arc` wrapper.
pub struct CrawlTaskCtx {
    // --- Shared config (read-only) ---
    pub(crate) config: Arc<CrawlerConfig>,
    pub(crate) visited: Arc<UrlDeduplicator>,
    pub(crate) visited_urls: Arc<RwLock<Vec<String>>>,
    pub(crate) queue: Arc<UrlQueue>,
    pub(crate) rate_limiter: SharedRateLimiter,
    pub(crate) session_pool: Option<DomainSessionPool>,
    pub(crate) ignore_robots: bool,
    pub(crate) robots_fetcher: Arc<RobotsFetcher>,

    // --- Per-task mutable (atomics) ---
    pub(crate) error_count: Arc<AtomicUsize>,
    pub(crate) pages_crawled: Arc<AtomicU64>,

    // --- Infrastructure ---
    pub(crate) collector: ResultsCollector,
    pub(crate) cookie_bridge: Arc<RwLock<CookieBridge>>,
    pub(crate) banned_domains: Arc<RwLock<Vec<BannedDomain>>>,
    pub(crate) fetch_router: Option<FetchRouter>,

    // --- Pipeline ---
    pub(crate) pipeline: Option<Arc<PipelineExecutor>>,
    pub(crate) output_stages: Vec<Arc<Box<dyn OutputStage>>>,
}
