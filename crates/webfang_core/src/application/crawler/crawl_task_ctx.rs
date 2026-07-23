//! Shared context for all crawl tasks spawned by the engine.
//!
//! Consolidates the 18 `Arc::clone()` calls that were previously done
//! per-`tokio::spawn()` into a single `Arc<CrawlTaskCtx>`, reducing
//! atomic contention, cache pollution, and the cost of adding new
//! shared resources.

use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, RwLock};

use crate::application::crawler::collector::ResultsCollector;
use crate::application::crawler::engine::FetchRouter;
use crate::application::deduplicator::UrlDeduplicator;
use crate::application::pipeline::{OutputStage, PipelineExecutor};
use crate::application::rate_limiter::SharedRateLimiter;
use crate::domain::CrawlerConfig;
use crate::infrastructure::checkpoint::store::BannedDomain;
use crate::infrastructure::crawler::RobotsCache;
use crate::infrastructure::crawler::UrlQueue;
use crate::infrastructure::downloader::cookie_bridge::CookieBridge;
use crate::infrastructure::network::session_pool::DomainSessionPool;

/// Shared context for all crawl tasks spawned by the engine.
///
/// Instead of cloning 18 individual `Arc`s per `tokio::spawn()`,
/// we construct one `Arc<CrawlTaskCtx>` and clone only the `Arc` wrapper.
pub struct CrawlTaskCtx {
    // --- Shared config (read-only) ---
    pub config: Arc<CrawlerConfig>,
    pub visited: Arc<UrlDeduplicator>,
    pub visited_urls: Arc<RwLock<Vec<String>>>,
    pub queue: Arc<UrlQueue>,
    pub rate_limiter: SharedRateLimiter,
    pub session_pool: Option<DomainSessionPool>,
    pub ignore_robots: bool,
    pub robots_cache: RobotsCache,

    // --- Per-task mutable (atomics) ---
    pub error_count: Arc<AtomicUsize>,
    pub pages_crawled: Arc<AtomicU64>,

    // --- Infrastructure ---
    pub collector: ResultsCollector,
    pub cookie_bridge: Arc<RwLock<CookieBridge>>,
    pub banned_domains: Arc<RwLock<Vec<BannedDomain>>>,
    pub(crate) fetch_router: Option<FetchRouter>,

    // --- Pipeline ---
    pub pipeline: Option<Arc<PipelineExecutor>>,
    pub output_stages: Vec<Arc<Box<dyn OutputStage>>>,
}
