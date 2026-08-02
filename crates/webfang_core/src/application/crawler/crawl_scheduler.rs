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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::crawler::concurrency_level::ConcurrencyLevel;

    fn url(path: &str) -> Url {
        Url::parse(&format!("https://example.com{path}")).unwrap()
    }

    fn disc(path: &str) -> DiscoveredUrl {
        let u = url(path);
        DiscoveredUrl::html(u.clone(), 0, u)
    }

    // -- record_visit --

    #[test]
    fn record_visit_first_true_duplicate_false() {
        let s = CrawlScheduler::new(4);
        assert!(s.record_visit("https://example.com/a"));
        assert!(!s.record_visit("https://example.com/a"));
        assert!(s.record_visit("https://example.com/b"));
    }

    #[test]
    fn record_visit_mirror_skips_duplicates() {
        let s = CrawlScheduler::new(4);
        assert!(s.record_visit("https://example.com/a"));
        assert!(s.record_visit("https://example.com/b"));
        assert!(!s.record_visit("https://example.com/a"));
        let snap = s.snapshot_visited();
        assert_eq!(snap.len(), 2);
        assert!(snap.contains("https://example.com/a"));
        assert!(snap.contains("https://example.com/b"));
    }

    // -- restore_visited / snapshot_visited --

    #[test]
    fn restore_visited_is_idempotent() {
        let s = CrawlScheduler::new(4);
        assert!(s.record_visit("https://example.com/a"));
        let mut set = HashSet::new();
        set.insert("https://example.com/a".to_string());
        set.insert("https://example.com/b".to_string());
        s.restore_visited(&set);
        s.restore_visited(&set);
        let snap = s.snapshot_visited();
        assert_eq!(snap.len(), 2);
        assert!(snap.contains("https://example.com/a"));
        assert!(snap.contains("https://example.com/b"));
    }

    #[test]
    fn snapshot_visited_roundtrips() {
        let s = CrawlScheduler::new(4);
        let original: HashSet<String> = [
            "https://example.com/a",
            "https://example.com/b",
            "https://example.com/c",
        ]
        .iter()
        .map(|u| (*u).to_string())
        .collect();
        s.restore_visited(&original);
        assert_eq!(s.snapshot_visited(), original);
    }

    // -- effective_concurrency / can_spawn --

    #[test]
    fn effective_concurrency_defaults_to_base() {
        assert_eq!(CrawlScheduler::new(7).effective_concurrency(), 7);
        assert_eq!(CrawlScheduler::new(1).effective_concurrency(), 1);
        assert_eq!(CrawlScheduler::new(0).effective_concurrency(), 0);
    }

    #[test]
    fn effective_concurrency_applies_autoscale() {
        let mut s = CrawlScheduler::new(10);
        let level = Arc::new(SharedConcurrencyLevel::new());
        s.set_autoscale(Arc::clone(&level));
        assert_eq!(s.effective_concurrency(), 10);
        level.set(ConcurrencyLevel::Reduced);
        assert_eq!(s.effective_concurrency(), 5);
        level.set(ConcurrencyLevel::Critical);
        assert_eq!(s.effective_concurrency(), 0);
    }

    #[test]
    fn can_spawn_boundary() {
        let s = CrawlScheduler::new(3);
        assert!(s.can_spawn(0));
        assert!(s.can_spawn(2));
        assert!(!s.can_spawn(3));
        assert!(!s.can_spawn(4));
    }

    // -- next_url --

    #[tokio::test]
    async fn next_url_none_at_limit_leaves_pending_untouched() {
        let mut s = CrawlScheduler::new(1);
        s.seed(&url("/a")).await;
        assert!(s.has_pending_work());
        assert!(s.next_url(1).is_none());
        assert!(
            s.has_pending_work(),
            "pending buffer must not be consumed at the limit"
        );
        assert!(s.next_url(0).is_some());
        assert!(!s.has_pending_work());
    }

    #[tokio::test]
    async fn next_url_skips_already_visited() {
        let mut s = CrawlScheduler::new(10);
        s.seed(&url("/a")).await;
        s.seed(&url("/b")).await;
        assert!(s.record_visit("https://example.com/a"));
        let next = s.next_url(0).unwrap();
        assert_eq!(next.url.path(), "/b");
    }

    #[tokio::test]
    async fn next_url_marks_returned_as_visited() {
        let mut s = CrawlScheduler::new(10);
        s.seed(&url("/a")).await;
        let next = s.next_url(0).unwrap();
        assert_eq!(next.url.path(), "/a");
        assert!(
            !s.record_visit("https://example.com/a"),
            "returned URL must be marked visited"
        );
        assert!(s.snapshot_visited().contains("https://example.com/a"));
    }

    #[tokio::test]
    async fn next_url_none_when_out_of_work() {
        let mut s = CrawlScheduler::new(10);
        assert!(s.next_url(0).is_none());
        s.seed(&url("/a")).await;
        assert!(s.next_url(0).is_some());
        assert!(s.next_url(0).is_none());
    }

    #[tokio::test]
    async fn next_url_returns_fifo_order() {
        let mut s = CrawlScheduler::new(10);
        s.seed(&url("/a")).await;
        s.seed(&url("/b")).await;
        s.seed(&url("/c")).await;
        assert_eq!(s.next_url(0).unwrap().url.path(), "/a");
        assert_eq!(s.next_url(0).unwrap().url.path(), "/b");
        assert_eq!(s.next_url(0).unwrap().url.path(), "/c");
        assert!(s.next_url(0).is_none());
    }

    // -- has_pending_work / seed / drain_discovered --

    #[tokio::test]
    async fn has_pending_work_tracks_buffer() {
        let mut s = CrawlScheduler::new(10);
        assert!(!s.has_pending_work());
        s.seed(&url("/a")).await;
        assert!(s.has_pending_work());
        let _ = s.next_url(0).unwrap();
        assert!(!s.has_pending_work());
    }

    #[tokio::test]
    async fn seed_pushes_to_pending_and_queue() {
        let mut s = CrawlScheduler::new(4);
        let q = s.queue();
        s.seed(&url("/seed")).await;
        assert!(s.has_pending_work());
        assert_eq!(q.len().await, 1);
        assert_eq!(s.next_url(0).unwrap().url.path(), "/seed");
    }

    #[tokio::test]
    async fn drain_discovered_moves_queue_into_pending() {
        let mut s = CrawlScheduler::new(4);
        let q = s.queue();
        assert!(q.push_prioritized(disc("/x"), UrlSource::Link).await);
        assert!(!s.has_pending_work(), "pending empty before drain");
        s.drain_discovered().await;
        assert!(s.has_pending_work());
        assert_eq!(q.len().await, 0, "queue drained");
        assert_eq!(s.next_url(0).unwrap().url.path(), "/x");
    }
}
