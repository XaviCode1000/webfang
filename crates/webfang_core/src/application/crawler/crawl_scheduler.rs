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
//!
//! # D6 lock-across-await audit (task 2.3, change stabilization-concurrency-budget)
//!
//! Functions rewired by commit 20806c7a (scheduler gating):
//!
//! | Function | `.await` points | Guard discipline | Verdict |
//! |---|---|---|---|
//! | `CrawlScheduler::new` | none (sync fn) | constructs `RwLock`/dedup state by value; no guard held past initialization expressions | PASS |
//! | `CrawlScheduler::effective_concurrency` | none (sync fn) | reads `SharedConcurrencyLevel` atomics only; no lock guard taken | PASS |
//!
//! Async callers (`snapshot_pending`, queue drain) touch the shared [`UrlQueue`](crate::infrastructure::crawler::UrlQueue)
//! whose sole `tokio::sync::Mutex` is confined to documented sync-only sections
//! (`infrastructure/crawler/url_queue.rs`, invariant AL-2).
//!
//! Enforcement: `#![deny(clippy::await_holding_lock)]` below fails the build if
//! a future edit ever holds a `std` lock guard across an `.await` in this module.

#![deny(clippy::await_holding_lock)]

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, RwLock};

use url::Url;

use super::concurrency_level::SharedConcurrencyLevel;
use crate::application::deduplicator::UrlDeduplicator;
use crate::domain::budget::CrawlConcurrency;
use crate::domain::crawler_port::UrlQueuePort;
use crate::domain::crawler_port::UrlSource;
use crate::domain::DiscoveredUrl;

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
    /// Shared discovery queue — tasks push discovered links here. Erased
    /// behind the domain port (ADR-0012-B unit 8); `Arc`-shared with the
    /// per-page tasks so enqueue-time dedup stays global.
    queue: Arc<dyn UrlQueuePort>,
    /// Local work buffer drained from `queue`; the scheduler pops from here.
    pending: VecDeque<DiscoveredUrl>,
    /// Base concurrency — the budget model's `Operation.crawl` tier.
    /// A plain `usize` would let zero or unclamped values in; the tier
    /// newtype makes an invalid bound unrepresentable (design D4).
    base_concurrency: CrawlConcurrency,
    /// Optional autoscale level for RAM-aware concurrency adjustment.
    autoscale_level: Option<Arc<SharedConcurrencyLevel>>,
}

impl CrawlScheduler {
    /// Create a scheduler with fresh scheduling state, gated by the budget
    /// model's crawl tier.
    ///
    /// Builds the visited set, its checkpoint string mirror, and the shared
    /// discovery queue internally; these are `Arc`-shared with the per-page
    /// tasks via the accessors below.
    pub(crate) fn new(base_concurrency: CrawlConcurrency) -> Self {
        Self {
            visited: Arc::new(UrlDeduplicator::new()),
            visited_urls: Arc::new(RwLock::new(Vec::new())),
            queue: crate::application::container::build_url_queue(),
            pending: VecDeque::new(),
            base_concurrency,
            autoscale_level: None,
        }
    }

    /// Enable RAM-aware autoscaling of the concurrency limit.
    pub(crate) fn set_autoscale(&mut self, level: Arc<SharedConcurrencyLevel>) {
        self.autoscale_level = Some(level);
    }

    /// Borrow the autoscale level handle (if any) — used by engine integration
    /// tests to assert the background poller has moved the level.
    #[cfg(test)]
    pub(crate) fn autoscale_level(&self) -> Option<&Arc<SharedConcurrencyLevel>> {
        self.autoscale_level.as_ref()
    }

    /// Clone the discovery queue for the shared task context.
    pub(crate) fn queue(&self) -> Arc<dyn UrlQueuePort> {
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
            // LCOV_EXCL_LINE defensive: rwlock-poisoning — a poisoned lock leaves process state undefined
            .expect("visited_urls RwLock poisoned");
        urls.iter().cloned().collect()
    }

    /// Snapshot URLs that are pending (queued but not yet visited) for
    /// checkpoint persistence.
    ///
    /// Combines the shared discovery queue (links pushed by in-flight tasks but
    /// not yet drained) with the scheduler's local pending buffer. The engine
    /// persists this so a resume re-enqueues exactly what was left to crawl —
    /// without it, `save_checkpoint` used to write an empty queue and a resume
    /// after the seed was already visited would crawl nothing (#517).
    pub(crate) async fn snapshot_pending(&self) -> Vec<String> {
        let mut urls: Vec<String> = self.pending.iter().map(|d| d.url.to_string()).collect();
        urls.extend(self.queue.snapshot_urls().await);
        urls
    }

    /// Restore pending (queued-but-unvisited) URLs from a checkpoint.
    ///
    /// Re-enqueues them into the local pending buffer as depth-0 HTML links.
    /// The visited set is restored separately and takes precedence: `next_url`
    /// skips any restored URL that was actually already visited, so a queued
    /// entry that raced into `visited` never gets re-crawled (#517).
    pub(crate) fn restore_pending(&mut self, urls: &[String]) {
        for raw in urls {
            if let Ok(url) = Url::parse(raw) {
                let discovered = DiscoveredUrl::html(url.clone(), 0, url);
                self.pending.push_back(discovered);
            }
        }
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
    ///
    /// Autoscale levels stay WITHIN the model's `Operation.crawl` ceiling:
    /// Normal → full tier value, Reduced → half, Critical → 0 (pause). No
    /// re-clamp needed — the tier is already ≤ the global ceiling and every
    /// scaled-down result is bounded by it.
    #[must_use]
    pub(crate) fn effective_concurrency(&self) -> usize {
        match self.autoscale_level.as_ref() {
            Some(level) => level.effective_concurrency(self.base_concurrency.get()),
            None => self.base_concurrency.get(),
        }
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

    /// Test constructor wrapping the NonZero-gated newtype: a raw `usize`
    /// bound is no longer representable (task 2.2b — D4 tier newtype).
    fn sched(n: usize) -> CrawlScheduler {
        CrawlScheduler::new(CrawlConcurrency::new(n).expect("test concurrency non-zero"))
    }

    // -- record_visit --

    #[test]
    fn record_visit_first_true_duplicate_false() {
        let s = sched(4);
        assert!(s.record_visit("https://example.com/a"));
        assert!(!s.record_visit("https://example.com/a"));
        assert!(s.record_visit("https://example.com/b"));
    }

    #[test]
    fn record_visit_mirror_skips_duplicates() {
        let s = sched(4);
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
        let s = sched(4);
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
        let s = sched(4);
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

    /// Task 2.2(b): the scheduler's spawn bound follows the injected
    /// [`BudgetModel`]'s `Operation.crawl` tier — never a raw configured
    /// concurrency value.
    #[test]
    fn scheduler_bound_follows_injected_budget_model() {
        use crate::domain::budget::detector::FixedDetector;
        use crate::domain::budget::{BudgetModel, BudgetOverrides};

        fn scheduler_from_cores(cores: usize) -> CrawlScheduler {
            let detector = FixedDetector::with_detection(
                std::num::NonZeroUsize::new(cores).expect("test cores non-zero"),
                None,
            );
            let model = BudgetModel::build(BudgetOverrides::default(), &detector);
            CrawlScheduler::new(model.crawl())
        }

        // 4-core auto table → crawl 3.
        let s = scheduler_from_cores(4);
        assert!(s.can_spawn(2));
        assert!(!s.can_spawn(3));

        // 16-core auto table → crawl min(15, 8) = 8.
        let s = scheduler_from_cores(16);
        assert!(s.can_spawn(7));
        assert!(!s.can_spawn(8));
    }

    #[test]
    fn effective_concurrency_defaults_to_base() {
        // Zero is unrepresentable since 2.2b: `CrawlConcurrency` rejects it
        // at construction (NonZero guard).
        assert_eq!(sched(7).effective_concurrency(), 7);
        assert_eq!(sched(1).effective_concurrency(), 1);
    }

    #[test]
    fn effective_concurrency_applies_autoscale() {
        let mut s = sched(10);
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
        let s = sched(3);
        assert!(s.can_spawn(0));
        assert!(s.can_spawn(2));
        assert!(!s.can_spawn(3));
        assert!(!s.can_spawn(4));
    }

    // -- next_url --

    #[tokio::test]
    async fn next_url_none_at_limit_leaves_pending_untouched() {
        let mut s = sched(1);
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
        let mut s = sched(10);
        s.seed(&url("/a")).await;
        s.seed(&url("/b")).await;
        assert!(s.record_visit("https://example.com/a"));
        let next = s.next_url(0).unwrap();
        assert_eq!(next.url.path(), "/b");
    }

    #[tokio::test]
    async fn next_url_marks_returned_as_visited() {
        let mut s = sched(10);
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
        let mut s = sched(10);
        assert!(s.next_url(0).is_none());
        s.seed(&url("/a")).await;
        assert!(s.next_url(0).is_some());
        assert!(s.next_url(0).is_none());
    }

    #[tokio::test]
    async fn next_url_returns_fifo_order() {
        let mut s = sched(10);
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
        let mut s = sched(10);
        assert!(!s.has_pending_work());
        s.seed(&url("/a")).await;
        assert!(s.has_pending_work());
        let _ = s.next_url(0).unwrap();
        assert!(!s.has_pending_work());
    }

    #[tokio::test]
    async fn seed_pushes_to_pending_and_queue() {
        let mut s = sched(4);
        let q = s.queue();
        s.seed(&url("/seed")).await;
        assert!(s.has_pending_work());
        assert_eq!(q.len().await, 1);
        assert_eq!(s.next_url(0).unwrap().url.path(), "/seed");
    }

    #[tokio::test]
    async fn drain_discovered_moves_queue_into_pending() {
        let mut s = sched(4);
        let q = s.queue();
        assert!(q.push_prioritized(disc("/x"), UrlSource::Link).await);
        assert!(!s.has_pending_work(), "pending empty before drain");
        s.drain_discovered().await;
        assert!(s.has_pending_work());
        assert_eq!(q.len().await, 0, "queue drained");
        assert_eq!(s.next_url(0).unwrap().url.path(), "/x");
    }

    // -- snapshot_pending / restore_pending (#517) --

    #[tokio::test]
    async fn snapshot_pending_captures_pending_and_queue() {
        let mut s = sched(4);
        // One URL sits in the pending buffer, one still in the shared queue.
        s.restore_pending(&["https://example.com/buffered".into()]);
        s.queue()
            .push_prioritized(disc("/queued"), UrlSource::Link)
            .await;
        let snap = s.snapshot_pending().await;
        assert_eq!(snap.len(), 2, "both sources must be captured: {snap:?}");
        assert!(snap.iter().any(|u| u.ends_with("/buffered")));
        assert!(snap.iter().any(|u| u.ends_with("/queued")));
    }

    #[tokio::test]
    async fn snapshot_pending_is_nondestructive() {
        let mut s = sched(4);
        s.restore_pending(&["https://example.com/buffered".into()]);
        s.queue()
            .push_prioritized(disc("/queued"), UrlSource::Link)
            .await;
        let _ = s.snapshot_pending().await;
        assert!(s.has_pending_work(), "pending buffer must survive snapshot");
        assert_eq!(s.queue().len().await, 1, "queue must survive snapshot");
    }

    #[test]
    fn restore_pending_reenqueues_for_next_url() {
        let mut s = sched(4);
        s.restore_pending(&[
            "https://example.com/p1".into(),
            "https://example.com/p2".into(),
        ]);
        assert!(s.has_pending_work());
        assert_eq!(s.next_url(0).unwrap().url.path(), "/p1");
        assert_eq!(s.next_url(0).unwrap().url.path(), "/p2");
        assert!(!s.has_pending_work());
    }

    #[test]
    fn restore_pending_skips_unparseable_urls() {
        let mut s = sched(4);
        s.restore_pending(&["not-a-url".into(), "https://example.com/ok".into()]);
        assert_eq!(s.next_url(0).unwrap().url.path(), "/ok");
        assert!(!s.has_pending_work());
    }

    #[tokio::test]
    async fn restore_pending_then_next_url_skips_visited() {
        let mut s = sched(4);
        s.record_visit("https://example.com/done");
        s.restore_pending(&[
            "https://example.com/done".into(),
            "https://example.com/fresh".into(),
        ]);
        assert_eq!(
            s.next_url(0).unwrap().url.path(),
            "/fresh",
            "restored URL already visited must be skipped"
        );
        assert!(!s.has_pending_work());
    }

    #[tokio::test]
    async fn snapshot_restore_pending_roundtrips() {
        let mut s = sched(4);
        s.restore_pending(&["https://example.com/buffered".into()]);
        s.queue()
            .push_prioritized(disc("/queued"), UrlSource::Link)
            .await;
        let snap = s.snapshot_pending().await;

        let mut s2 = sched(4);
        s2.restore_pending(&snap);
        let mut paths: Vec<String> = Vec::new();
        while let Some(d) = s2.next_url(0) {
            paths.push(d.url.path().to_string());
        }
        assert_eq!(
            paths.len(),
            2,
            "all pending URLs must be restored: {paths:?}"
        );
        assert!(paths.contains(&"/buffered".into()));
        assert!(paths.contains(&"/queued".into()));
    }
}

// ============================================================================
// Task 5.1 memory probe — visited_urls string mirror growth (BEFORE numbers).
// Writes one line to WEBFANG_MEMORY_REPORT_PATH; no byte assertions by design
// (Q3 MEASURE FIRST — data decides, not preferences).
// ============================================================================
#[cfg(test)]
mod memory_probe_tests {
    use super::*;
    use crate::infrastructure::observability::memory_probe;

    #[test]
    fn probe_visited_mirror_growth_200k_urls() {
        const N: usize = 200_000;
        let scheduler =
            CrawlScheduler::new(crate::domain::budget::CrawlConcurrency::new(8).expect("8 > 0"));
        let before = memory_probe::rss_bytes();

        for i in 0..N {
            let url = format!("https://probe.example.com/section-{}/page/{i}", i % 64);
            assert!(scheduler.record_visit(&url), "probe URL must be new");
        }

        let after = memory_probe::rss_bytes();
        let mirror_len = scheduler
            .visited_urls
            .read()
            .map(|urls| urls.len())
            .unwrap_or(usize::MAX);
        assert_eq!(mirror_len, N, "mirror holds exactly one entry per URL");

        memory_probe::append_report(
            "BEFORE — visited_urls mirror",
            &format!(
                "entries={mirror_len} rss_before={} rss_after={} delta={}",
                memory_probe::fmt_rss(before),
                memory_probe::fmt_rss(after),
                memory_probe::fmt_rss(after.and_then(|a| before.map(|b| a.saturating_sub(b)))),
            ),
        );
    }
}
