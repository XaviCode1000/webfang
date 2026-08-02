//! Crawl scheduling policy — visited tracking, work queue, and concurrency limits.
//!
//! Extracted from `engine.rs` (strangler fig Corte C, issue #440). Encapsulates
//! the *scheduling decisions* that were previously inlined in `Engine::run()`:
//! which URL to crawl next, deduplication of visited URLs, the pending-work
//! buffer drained from the shared [`UrlQueue`], and the autoscale-aware
//! concurrency limit.
//!
//! The `Engine` remains the *orchestration mechanism*: it owns the `JoinSet`,
//! spawns tasks via the `crawl_task` module, coordinates signal-driven shutdown,
//! and persists checkpoints. `CrawlScheduler` carries no task, shutdown, or
//! checkpoint state.
//!
//! # Shared state
//!
//! `visited`, `visited_urls`, and `queue` are `Arc`-shared with the spawned
//! per-page tasks (via `CrawlTaskCtx`): tasks dedup discovered links and push
//! them onto `queue`, while the scheduler drains `queue` into its local
//! `pending` buffer and decides what to crawl next. The scheduler owns the
//! canonical `Arc`s and exposes accessors so the `Engine` clones them into the
//! task context exactly once.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, RwLock};

use url::Url;

use super::concurrency_level::SharedConcurrencyLevel;
use crate::application::deduplicator::UrlDeduplicator;
use crate::domain::DiscoveredUrl;
use crate::infrastructure::crawler::{UrlQueue, UrlSource};

/// Scheduling policy for a crawl: visited dedup, pending-work buffer, and
/// concurrency limits.
///
/// Owns the canonical `Arc`s for the visited set ([`UrlDeduplicator`]), the
/// checkpoint string mirror (`visited_urls`), and the shared discovery queue
/// ([`UrlQueue`]). These are `Arc`-shared with the per-page tasks, which push
/// discovered links onto `queue`; the scheduler drains that queue into a local
/// `pending` buffer and hands out the next URL to crawl.
pub(crate) struct CrawlScheduler {
    /// Lock-free dedup of visited URLs (hash set).
    visited: Arc<UrlDeduplicator>,
    /// String mirror of visited URLs for checkpoint persistence.
    visited_urls: Arc<RwLock<Vec<String>>>,
    /// Shared discovery queue — tasks push discovered links here.
    queue: Arc<UrlQueue>,
    /// Local work buffer drained from `queue`; the scheduler pops from here.
    pending: VecDeque<DiscoveredUrl>,
    /// Base concurrency from `CrawlerConfig::concurrency`.
    base_concurrency: usize,
    /// Optional autoscale level for RAM-aware concurrency adjustment.
    autoscale_level: Option<Arc<SharedConcurrencyLevel>>,
}

impl CrawlScheduler {
    /// Create a scheduler with fresh scheduling state.
    ///
    /// Builds the visited set, its checkpoint string mirror, and the shared
    /// discovery queue internally; these are `Arc`-shared with the per-page
    /// tasks via the accessors below.
    pub(crate) fn new(base_concurrency: usize) -> Self {
        Self {
            visited: Arc::new(UrlDeduplicator::new()),
            visited_urls: Arc::new(RwLock::new(Vec::new())),
            queue: Arc::new(UrlQueue::new()),
            pending: VecDeque::new(),
            base_concurrency,
            autoscale_level: None,
        }
    }

    /// Enable RAM-aware autoscaling of the concurrency limit.
    pub(crate) fn set_autoscale(&mut self, level: Arc<SharedConcurrencyLevel>) {
        self.autoscale_level = Some(level);
    }

    /// Clone the visited deduplicator for the shared task context.
    pub(crate) fn visited(&self) -> Arc<UrlDeduplicator> {
        Arc::clone(&self.visited)
    }

    /// Clone the visited-URL string mirror for the shared task context.
    pub(crate) fn visited_urls(&self) -> Arc<RwLock<Vec<String>>> {
        Arc::clone(&self.visited_urls)
    }

    /// Clone the discovery queue for the shared task context.
    pub(crate) fn queue(&self) -> Arc<UrlQueue> {
        Arc::clone(&self.queue)
    }

    /// Record a URL as visited (hash dedup + string mirror).
    ///
    /// Returns `true` if newly inserted, `false` if already visited.
    pub(crate) fn record_visit(&self, url: &str) -> bool {
        if self.visited.try_insert(url) {
            if let Ok(mut urls) = self.visited_urls.write() {
                urls.push(url.to_string());
            }
            true
        } else {
            false
        }
    }

    /// Restore visited URLs from a checkpoint (idempotent via dedup).
    pub(crate) fn restore_visited(&self, urls: &HashSet<String>) {
        for url in urls {
            self.record_visit(url);
        }
    }

    /// Snapshot the visited-URL string mirror for checkpoint persistence.
    #[allow(clippy::expect_used)]
    pub(crate) fn snapshot_visited(&self) -> HashSet<String> {
        let urls = self
            .visited_urls
            .read()
            .expect("visited_urls RwLock poisoned");
        urls.iter().cloned().collect()
    }

    /// Seed the crawl: push the seed onto the discovery queue (highest priority)
    /// and onto the local pending buffer.
    pub(crate) async fn seed(&mut self, seed_url: &Url) {
        let discovered = DiscoveredUrl::html(seed_url.clone(), 0, seed_url.clone());
        self.queue
            .push_prioritized(discovered.clone(), UrlSource::Seed)
            .await;
        self.pending.push_back(discovered);
    }

    /// Drain discovered links from the shared queue into the pending buffer.
    pub(crate) async fn drain_discovered(&mut self) {
        self.pending.append(&mut self.queue.drain_all().await);
    }

    /// Whether there are URLs left to schedule.
    #[must_use]
    pub(crate) fn has_pending_work(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Effective concurrency limit (autoscale-aware).
    #[must_use]
    pub(crate) fn effective_concurrency(&self) -> usize {
        self.autoscale_level
            .as_ref()
            .map(|level| level.effective_concurrency(self.base_concurrency))
            .unwrap_or(self.base_concurrency)
    }

    /// Whether another task may be spawned given `in_flight` running tasks.
    #[must_use]
    pub(crate) fn can_spawn(&self, in_flight: usize) -> bool {
        in_flight < self.effective_concurrency()
    }

    /// Next URL to crawl, or `None` if at the concurrency limit or out of work.
    ///
    /// Checks the concurrency limit *before* popping (so the pending buffer is
    /// left untouched when no task may spawn), then skips already-visited URLs,
    /// marking the returned URL as visited.
    pub(crate) fn next_url(&mut self, in_flight: usize) -> Option<DiscoveredUrl> {
        if !self.can_spawn(in_flight) {
            return None;
        }
        while let Some(discovered) = self.pending.pop_front() {
            if self.record_visit(discovered.url.as_str()) {
                return Some(discovered);
            }
        }
        None
    }
}
