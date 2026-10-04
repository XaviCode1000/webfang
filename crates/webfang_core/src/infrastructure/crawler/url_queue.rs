//! Concurrent URL queue for crawling
//!
//! Thread-safe URL queue with deduplication and priority scheduling.
//!
//! # Rules Applied
//!
//! - **async-no-lock-across-await**: The pending-URL `BinaryHeap` is guarded by a
//!   `tokio::sync::Mutex` acquired via `.lock().await` (never
//!   `blocking_lock()`). Each guard is held for nanoseconds across no `.await`
//!   point. The dedup `seen` set is a lock-free `DashSet`.
//! - **mem-with-capacity**: Pre-allocates internal structures.
//! - **mem-u64-dedup**: `seen` stores `u64` hashes (8 B) instead of `String`s
//!   (~150 B), keyed by the fixed deterministic `ahash::RandomState` seed
//!   (F-13 crawl order, #1237).
//! - **coll-binaryheap**: Uses `BinaryHeap` for priority queue semantics.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use dashmap::DashSet;
use tokio::sync::Mutex;
use tracing::debug;

use crate::domain::crawler_port::{UrlQueuePort, UrlSource};
use crate::domain::url_validation::{normalize_url, NormalizeConfig, RemoveQueryParameters};
use crate::domain::DiscoveredUrl;

/// Fixed `ahash` seeds pinning the frontier hash to one deterministic stream
/// (F-13 deterministic crawl order, #1237).
///
/// Nothing-up-my-sleeve numbers: the first 256 bits of the fractional part of
/// π in hexadecimal
/// (3.243F_6A88_85A3_08D3_1319_8A2E_0370_7344_A409_3822_299F_31D0_082E_FA98_EC4E_6C89…),
/// split into four `u64` lanes for [`ahash::RandomState::with_seeds`].
///
/// HashDoS tradeoff: a fixed seed forgoes per-process randomization, so a
/// hostile page could in principle feed colliding URLs at the dedup set. The
/// risk is accepted because this queue is CLI-local (the operator chooses the
/// seeds they crawl; worst case is local slowdown, bounded by `max_pages`),
/// and the fixed seed is the deterministic base the #1222 checkpoint needs:
/// same seed/config/input HTML ⇒ same push set ⇒ same pop sequence.
const FIXED_SEED_K0: u64 = 0x243F_6A88_85A3_08D3;
const FIXED_SEED_K1: u64 = 0x1319_8A2E_0370_7344;
const FIXED_SEED_K2: u64 = 0xA409_3822_299F_31D0;
const FIXED_SEED_K3: u64 = 0x082E_FA98_EC4E_6C89;

// UrlSource moved to domain::crawler_port (sub-slice 3.A). Infra re-exports
// it for backwards compatibility.

/// A URL with priority for the priority queue.
///
/// Implements `Ord` so that `BinaryHeap` yields highest-priority URLs first;
/// equal priorities yield the lexicographically smallest URL first (F-13).
#[derive(Debug, Clone)]
pub struct PrioritizedUrl {
    /// The discovered URL
    pub url: DiscoveredUrl,
    /// Priority score (higher = more important)
    pub priority: u32,
}

impl PrioritizedUrl {
    /// Create a new prioritized URL with explicit priority
    #[must_use]
    pub fn new(url: DiscoveredUrl, priority: u32) -> Self {
        Self { url, priority }
    }

    /// Create a prioritized URL from a source type and depth.
    ///
    /// Priority scoring rules:
    /// - Seed URL: 1000
    /// - Sitemap URL: 500
    /// - Link URL: 200
    /// - Depth penalty: priority -= depth * 10
    /// - Minimum priority: 1
    #[must_use]
    pub fn from_source(url: DiscoveredUrl, source: UrlSource) -> Self {
        let base_priority: u32 = match source {
            UrlSource::Seed => 1000,
            UrlSource::Sitemap => 500,
            UrlSource::Link => 200,
        };
        let depth_penalty = (url.depth as u32) * 10;
        let priority = base_priority.saturating_sub(depth_penalty).max(1);
        Self { url, priority }
    }
}

impl PartialEq for PrioritizedUrl {
    fn eq(&self, other: &Self) -> bool {
        // Kept consistent with `Ord` (F-13): equality is the full
        // (priority, URL) identity, so `==` and `cmp` never disagree.
        self.priority == other.priority && self.url.url.as_str() == other.url.url.as_str()
    }
}

impl Eq for PrioritizedUrl {}

impl Ord for PrioritizedUrl {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap: higher priority pops first. The URL
        // tie-break is REVERSED so the lexicographically smallest URL
        // compares greatest and pops first (F-13 total order, #1237).
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.url.url.as_str().cmp(self.url.url.as_str()))
    }
}

impl PartialOrd for PrioritizedUrl {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Thread-safe URL queue with deduplication and priority scheduling
///
/// Following **async-no-lock-across-await**: uses `tokio::sync::Mutex` with
/// `.lock().await` for the pending-URL buffer (held across no `.await`), and a
/// lock-free `DashSet<u64, ahash::RandomState>` for the seen set.
/// Following **mem-with-capacity**: pre-allocates internal storage.
/// Following **coll-binaryheap**: uses `BinaryHeap` for priority queue semantics.
pub struct UrlQueue {
    /// Pending URLs to crawl — the only `tokio::sync::Mutex`, held briefly.
    queue: Mutex<BinaryHeap<PrioritizedUrl>>,
    /// Set of URL hashes already enqueued or visited (for deduplication).
    /// `u64` keys (8 B) instead of `String` (~150 B).
    seen: DashSet<u64, ahash::RandomState>,
    /// Fixed deterministic hash seed (F-13, #1237). Cloned into `seen` at
    /// construction so both use identical keys.
    rs: ahash::RandomState,
}

impl UrlQueue {
    /// Create a new URL queue
    ///
    /// Following **mem-with-capacity**: pre-allocates internal structures.
    /// The hash seed is FIXED (`FIXED_SEED_K0..K3`, F-13) so every queue
    /// built from the same input drains in the same order in every process;
    /// see the `FIXED_SEED_*` docs for the accepted HashDoS tradeoff.
    #[must_use]
    pub fn new() -> Self {
        let rs = ahash::RandomState::with_seeds(
            FIXED_SEED_K0,
            FIXED_SEED_K1,
            FIXED_SEED_K2,
            FIXED_SEED_K3,
        );
        Self {
            queue: Mutex::new(BinaryHeap::with_capacity(100)),
            seen: DashSet::with_capacity_and_hasher(100, rs.clone()),
            rs,
        }
    }

    /// Push a URL to the queue with a specific source type.
    ///
    /// The source determines the base priority:
    /// - `UrlSource::Seed`: priority 1000
    /// - `UrlSource::Sitemap`: priority 500
    /// - `UrlSource::Link`: priority 200
    ///
    /// Depth penalty is applied automatically from `url.depth`.
    ///
    /// Returns `false` if the URL was already seen (duplicate).
    ///
    /// # Arguments
    ///
    /// * `url` - URL to add
    /// * `source` - Source type for priority scoring
    ///
    /// # Returns
    ///
    /// `true` if added, `false` if duplicate
    pub async fn push_prioritized(&self, url: DiscoveredUrl, source: UrlSource) -> bool {
        // Lock-free, atomic check-and-insert via DashSet::insert (no Mutex, no
        // .await). Prevents the race where two tasks both pass contains() and
        // both insert. The hash uses the fixed deterministic seed (F-13, #1237).
        //
        // The URL is normalized to its canonical form BEFORE hashing (#517):
        // the seed enters the queue raw while links arrive already normalized,
        // so a raw/equivalently-spelled duplicate must still be rejected here.
        let canonical = normalize_url(
            url.url.as_str(),
            &NormalizeConfig {
                strip_www: true,
                query_policy: RemoveQueryParameters::All,
            },
        );
        let hash = self.rs.hash_one(&canonical);
        if !self.seen.insert(hash) {
            debug!("Duplicate URL in queue: {}", url.url);
            return false;
        }

        let prioritized = PrioritizedUrl::from_source(url, source);

        // Pending-URL BinaryHeap is the only tokio::sync::Mutex; the guard is held
        // across no .await (AL-2) — acquired, pushed, dropped.
        let mut queue = self.queue.lock().await;
        queue.push(prioritized);

        true
    }

    /// Push a URL to the queue (defaults to Link source, priority 200)
    ///
    /// Returns `false` if the URL was already seen (duplicate).
    ///
    /// # Arguments
    ///
    /// * `url` - URL to add
    ///
    /// # Returns
    ///
    /// `true` if added, `false` if duplicate
    pub async fn push(&self, url: DiscoveredUrl) -> bool {
        self.push_prioritized(url, UrlSource::Link).await
    }

    /// Pop the highest-priority URL from the queue
    ///
    /// # Returns
    ///
    /// `Some(DiscoveredUrl)` if queue has URLs, `None` if empty
    pub async fn pop(&self) -> Option<DiscoveredUrl> {
        let mut queue = self.queue.lock().await;
        queue.pop().map(|p| p.url)
    }

    /// Drain all pending URLs from the internal queue into a VecDeque.
    ///
    /// Used to transfer discovered links from the deduplicated `UrlQueue` to
    /// the main crawl loop's `VecDeque` work queue.
    ///
    /// URLs are drained in priority order (highest first) from the BinaryHeap.
    ///
    /// # Returns
    ///
    /// VecDeque of all pending URLs (queue is emptied)
    pub async fn drain_all(&self) -> std::collections::VecDeque<DiscoveredUrl> {
        let mut queue = self.queue.lock().await;
        let mut heap = std::mem::take(&mut *queue);
        let mut result = std::collections::VecDeque::with_capacity(heap.len());
        while let Some(prioritized) = heap.pop() {
            result.push_back(prioritized.url);
        }
        result
    }

    /// Snapshot pending URLs as strings WITHOUT consuming them.
    ///
    /// Used by checkpoint persistence (#517): the queue must be serializable at
    /// any point so a resume can re-enqueue what was pending when the crawl
    /// stopped. The `seen` set is untouched, so re-pushing these URLs later
    /// would be rejected as duplicates — callers restoring a checkpoint must
    /// bypass the dedup set for exactly the restored URLs.
    ///
    /// # Returns
    ///
    /// All URLs currently in the queue (heap order, not priority order).
    #[must_use]
    pub async fn snapshot_urls(&self) -> Vec<String> {
        let queue = self.queue.lock().await;
        queue.iter().map(|p| p.url.url.to_string()).collect()
    }

    /// The `n` highest-priority URLs currently queued, in the order the crawler
    /// would actually take them.
    ///
    /// `BinaryHeap::iter()` yields in internal array order, NOT priority order,
    /// so truncating a plain snapshot would discard an arbitrary subset of the
    /// frontier. This pops up to `n` items and pushes them back while holding
    /// the lock (no `.await` inside — AL-2): the queue is not consumed and
    /// nothing proportional to the whole frontier is cloned — O(n log n) time,
    /// O(n) transient space. `PrioritizedUrl`'s `Ord` is a total order over
    /// (priority, reversed URL) and the `seen` set guarantees distinct URLs,
    /// so the popped sequence is exactly the priority order the crawl itself
    /// follows regardless of heap layout, and re-pushing the popped items
    /// restores the identical multiset. Used by the bounded checkpoint
    /// frontier (#1234 / F-39).
    pub async fn snapshot_urls_bounded(&self, n: usize) -> Vec<String> {
        let mut queue = self.queue.lock().await;
        let mut heap = std::mem::take(&mut *queue);
        let capacity = n.min(heap.len());
        let mut popped = Vec::with_capacity(capacity);
        let mut urls = Vec::with_capacity(capacity);
        while urls.len() < n {
            match heap.pop() {
                Some(prioritized) => {
                    urls.push(prioritized.url.url.to_string());
                    popped.push(prioritized);
                },
                None => break,
            }
        }
        for prioritized in popped {
            heap.push(prioritized);
        }
        *queue = heap;
        urls
    }

    /// Get the current queue length
    ///
    /// # Returns
    ///
    /// Number of URLs in the queue
    pub async fn len(&self) -> usize {
        self.queue.lock().await.len()
    }

    /// Peek at the highest-priority URL without removing it
    ///
    /// # Returns
    ///
    /// `Some(&DiscoveredUrl)` if queue has URLs, `None` if empty
    pub async fn peek(&self) -> Option<DiscoveredUrl> {
        self.queue.lock().await.peek().map(|p| p.url.clone())
    }

    /// Check if the queue is empty
    ///
    /// # Returns
    ///
    /// `true` if queue is empty
    #[must_use]
    pub async fn is_empty(&self) -> bool {
        self.queue.lock().await.is_empty()
    }

    /// Get the number of seen URLs
    ///
    /// Reads the lock-free `DashSet` directly — no mutex acquisition, so this
    /// stays synchronous (AL-3 applies only to methods that acquire the lock).
    ///
    /// # Returns
    ///
    /// Number of URLs that have been seen (added or visited)
    #[must_use]
    pub fn seen_count(&self) -> usize {
        self.seen.len()
    }

    /// Clear the queue (but not the seen set)
    pub async fn clear(&self) {
        self.queue.lock().await.clear();
    }

    /// Get all URLs from the queue (for debugging)
    ///
    /// # Returns
    ///
    /// Vec of all URLs currently in the queue (order may not match priority)
    #[cfg(test)]
    pub async fn get_all(&self) -> Vec<DiscoveredUrl> {
        self.queue
            .lock()
            .await
            .iter()
            .map(|p| p.url.clone())
            .collect()
    }
}

impl Default for UrlQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Port-slice adapter (ADR-0012-B unit 8): the concrete's scheduling surface
/// exposed as the domain [`UrlQueuePort`]. Dedup state, priority ordering,
/// and the `DashSet`/heap machinery stay here; the composition root erases
/// the concrete so scheduler and per-page tasks share the port object.
impl UrlQueuePort for UrlQueue {
    fn push_prioritized<'a>(
        &'a self,
        url: DiscoveredUrl,
        source: UrlSource,
    ) -> futures::future::BoxFuture<'a, bool> {
        Box::pin(UrlQueue::push_prioritized(self, url, source))
    }

    fn snapshot_urls<'a>(&'a self) -> futures::future::BoxFuture<'a, Vec<String>> {
        Box::pin(UrlQueue::snapshot_urls(self))
    }

    fn snapshot_urls_bounded<'a>(
        &'a self,
        n: usize,
    ) -> futures::future::BoxFuture<'a, Vec<String>> {
        Box::pin(UrlQueue::snapshot_urls_bounded(self, n))
    }

    fn drain_all<'a>(
        &'a self,
    ) -> futures::future::BoxFuture<'a, std::collections::VecDeque<DiscoveredUrl>> {
        Box::pin(UrlQueue::drain_all(self))
    }

    fn len<'a>(&'a self) -> futures::future::BoxFuture<'a, usize> {
        Box::pin(UrlQueue::len(self))
    }

    fn is_empty<'a>(&'a self) -> futures::future::BoxFuture<'a, bool> {
        Box::pin(UrlQueue::is_empty(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn create_test_url(path: &str) -> DiscoveredUrl {
        let url = Url::parse(&format!("https://example.com{path}")).unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        DiscoveredUrl::html(url, 0, parent)
    }

    fn create_test_url_with_depth(path: &str, depth: u8) -> DiscoveredUrl {
        let url = Url::parse(&format!("https://example.com{path}")).unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        DiscoveredUrl::html(url, depth, parent)
    }

    /// Build a DiscoveredUrl from an arbitrary absolute URL string.
    fn push_with_url(raw: &str) -> DiscoveredUrl {
        let url = Url::parse(raw).unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        DiscoveredUrl::html(url, 0, parent)
    }

    #[tokio::test]
    async fn test_url_queue_new() {
        let queue = UrlQueue::new();
        assert!(queue.is_empty().await);
        assert_eq!(queue.len().await, 0);
        assert_eq!(queue.seen_count(), 0);
    }

    #[tokio::test]
    async fn test_url_queue_push_pop() {
        let queue = UrlQueue::new();

        let url1 = create_test_url("/page1");
        let url2 = create_test_url("/page2");

        assert!(queue.push(url1).await);
        assert!(queue.push(url2).await);

        assert_eq!(queue.len().await, 2);
        assert_eq!(queue.seen_count(), 2);

        let popped = queue.pop().await;
        assert!(popped.is_some());
        // Both have same priority (Link=200), order is undefined but both are valid

        assert_eq!(queue.len().await, 1);
    }

    #[tokio::test]
    async fn test_url_queue_duplicate_detection() {
        let queue = UrlQueue::new();

        let url1 = create_test_url("/page1");
        let url2 = create_test_url("/page1"); // Same URL

        assert!(queue.push(url1).await);
        assert!(!queue.push(url2).await); // Duplicate

        assert_eq!(queue.len().await, 1);
        assert_eq!(queue.seen_count(), 1);
    }

    #[tokio::test]
    async fn test_url_queue_cross_normalization_www_collapses() {
        // www-vs-bare (#517): same document reached via www and bare host is
        // one queue entry.
        let queue = UrlQueue::new();
        assert!(queue.push(create_test_url("/page")).await);
        assert!(
            !queue
                .push(push_with_url("https://www.example.com/page"))
                .await
        );
        assert_eq!(queue.len().await, 1);
    }

    #[tokio::test]
    async fn test_url_queue_cross_normalization_fragment_collapses() {
        // fragments (#517): a fragment is not part of the document identity.
        let queue = UrlQueue::new();
        assert!(queue.push(create_test_url("/page")).await);
        assert!(
            !queue
                .push(push_with_url("https://example.com/page#section"))
                .await
        );
        assert_eq!(queue.len().await, 1);
    }

    #[tokio::test]
    async fn test_url_queue_cross_normalization_query_collapses() {
        // query strings (#517): query parameters are removed entirely for
        // dedup (remove_query_parameters: All), so a seed with a tracking
        // query is the same document as the bare link.
        let queue = UrlQueue::new();
        assert!(queue.push(create_test_url("/page")).await);
        assert!(
            !queue
                .push(push_with_url("https://example.com/page?a=1&b=2"))
                .await
        );
        assert_eq!(queue.len().await, 1);
    }

    #[tokio::test]
    async fn test_url_queue_empty_pop() {
        let queue = UrlQueue::new();
        assert!(queue.pop().await.is_none());
    }

    #[tokio::test]
    async fn test_url_queue_clear() {
        let queue = UrlQueue::new();

        queue.push(create_test_url("/page1")).await;
        queue.push(create_test_url("/page2")).await;

        assert_eq!(queue.len().await, 2);

        queue.clear().await;

        assert_eq!(queue.len().await, 0);
        assert_eq!(queue.seen_count(), 2); // Seen set not cleared
    }

    #[tokio::test]
    async fn test_url_queue_multiple_urls() {
        let queue = UrlQueue::new();

        for i in 0..10 {
            let url = create_test_url(&format!("/page{i}"));
            assert!(queue.push(url).await);
        }

        assert_eq!(queue.len().await, 10);
        assert_eq!(queue.seen_count(), 10);

        // Pop all
        for _ in 0..10 {
            assert!(queue.pop().await.is_some());
        }

        assert!(queue.is_empty().await);
    }

    #[tokio::test]
    async fn test_url_queue_drain_all() {
        let queue = UrlQueue::new();

        queue.push(create_test_url("/page1")).await;
        queue.push(create_test_url("/page2")).await;
        queue.push(create_test_url("/page3")).await;

        assert_eq!(queue.len().await, 3);

        let drained = queue.drain_all().await;

        assert_eq!(drained.len(), 3);
        assert!(queue.is_empty().await);

        // Re-pushing same URLs should fail (dedup via seen set)
        assert!(!queue.push(create_test_url("/page1")).await);
        assert!(!queue.push(create_test_url("/page2")).await);
        assert!(!queue.push(create_test_url("/page3")).await);
    }

    // Priority scheduling tests

    #[tokio::test]
    async fn test_sitemap_higher_priority_than_link() {
        let queue = UrlQueue::new();

        // Push a link URL first
        let link_url = create_test_url("/link-page");
        assert!(queue.push_prioritized(link_url, UrlSource::Link).await);

        // Push a sitemap URL second
        let sitemap_url = create_test_url("/sitemap-page");
        assert!(
            queue
                .push_prioritized(sitemap_url, UrlSource::Sitemap)
                .await
        );

        // Sitemap should come out first (higher priority)
        let popped = queue.pop().await.unwrap();
        assert_eq!(popped.url.path(), "/sitemap-page");
    }

    #[tokio::test]
    async fn test_seed_highest_priority() {
        let queue = UrlQueue::new();

        // Push link, sitemap, and seed URLs
        assert!(
            queue
                .push_prioritized(create_test_url("/link"), UrlSource::Link)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/sitemap"), UrlSource::Sitemap)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/seed"), UrlSource::Seed)
                .await
        );

        // Seed should come first
        let popped = queue.pop().await.unwrap();
        assert_eq!(popped.url.path(), "/seed");

        // Sitemap should come second
        let popped = queue.pop().await.unwrap();
        assert_eq!(popped.url.path(), "/sitemap");

        // Link should come last
        let popped = queue.pop().await.unwrap();
        assert_eq!(popped.url.path(), "/link");
    }

    #[tokio::test]
    async fn test_depth_reduces_priority() {
        let queue = UrlQueue::new();

        // Push a deep link URL (depth 5)
        let deep_url = create_test_url_with_depth("/deep-page", 5);
        assert!(queue.push_prioritized(deep_url, UrlSource::Link).await);

        // Push a shallow link URL (depth 1)
        let shallow_url = create_test_url_with_depth("/shallow-page", 1);
        assert!(queue.push_prioritized(shallow_url, UrlSource::Link).await);

        // Shallow should come first (higher priority due to less depth penalty)
        let popped = queue.pop().await.unwrap();
        assert_eq!(popped.url.path(), "/shallow-page");
    }

    #[tokio::test]
    async fn test_priority_ordering() {
        let queue = UrlQueue::new();

        // Push URLs with different priorities
        assert!(
            queue
                .push_prioritized(create_test_url("/low"), UrlSource::Link)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/high"), UrlSource::Seed)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/mid"), UrlSource::Sitemap)
                .await
        );

        // Pop should return in priority order: high > mid > low
        assert_eq!(queue.pop().await.unwrap().url.path(), "/high");
        assert_eq!(queue.pop().await.unwrap().url.path(), "/mid");
        assert_eq!(queue.pop().await.unwrap().url.path(), "/low");
    }

    #[tokio::test]
    async fn test_drain_all_priority_order() {
        let queue = UrlQueue::new();

        // Push URLs with different priorities
        assert!(
            queue
                .push_prioritized(create_test_url("/low"), UrlSource::Link)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/high"), UrlSource::Seed)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/mid"), UrlSource::Sitemap)
                .await
        );

        // Drain should return in priority order
        let drained = queue.drain_all().await;
        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].url.path(), "/high");
        assert_eq!(drained[1].url.path(), "/mid");
        assert_eq!(drained[2].url.path(), "/low");
    }

    // -- snapshot_urls_bounded (#1823) --

    #[tokio::test]
    async fn test_snapshot_urls_bounded_top_n_in_priority_order() {
        let queue = UrlQueue::new();

        assert!(
            queue
                .push_prioritized(create_test_url("/low"), UrlSource::Link)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/high"), UrlSource::Seed)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/mid"), UrlSource::Sitemap)
                .await
        );

        // The bounded snapshot must yield the same order the crawler pops.
        let urls = queue.snapshot_urls_bounded(2).await;
        assert_eq!(urls.len(), 2);
        assert!(urls[0].ends_with("/high"), "got {urls:?}");
        assert!(urls[1].ends_with("/mid"), "got {urls:?}");
    }

    #[tokio::test]
    async fn test_snapshot_urls_bounded_reversed_url_tiebreak() {
        let queue = UrlQueue::new();

        // Equal priorities: the total order breaks ties by the REVERSED URL,
        // so the lexicographically smallest URL pops first (F-13).
        assert!(queue.push(create_test_url("/b")).await);
        assert!(queue.push(create_test_url("/a")).await);

        let urls = queue.snapshot_urls_bounded(2).await;
        assert_eq!(urls.len(), 2);
        assert!(urls[0].ends_with("/a"), "got {urls:?}");
        assert!(urls[1].ends_with("/b"), "got {urls:?}");
    }

    #[tokio::test]
    async fn test_snapshot_urls_bounded_does_not_consume() {
        let queue = UrlQueue::new();

        for i in 0..3 {
            assert!(queue.push(create_test_url(&format!("/page{i}"))).await);
        }

        let urls = queue.snapshot_urls_bounded(2).await;
        assert_eq!(urls.len(), 2);
        assert_eq!(queue.len().await, 3, "snapshot must not consume the queue");

        // A later drain still yields everything.
        let drained = queue.drain_all().await;
        assert_eq!(drained.len(), 3);
    }

    #[tokio::test]
    async fn test_snapshot_urls_bounded_larger_than_queue_returns_all() {
        let queue = UrlQueue::new();

        assert!(queue.push(create_test_url("/one")).await);
        assert!(queue.push(create_test_url("/two")).await);

        let urls = queue.snapshot_urls_bounded(10).await;
        assert_eq!(urls.len(), 2);
        assert_eq!(queue.len().await, 2, "oversized n must not consume either");
    }

    #[tokio::test]
    async fn test_snapshot_urls_bounded_zero_returns_empty() {
        let queue = UrlQueue::new();

        assert!(queue.push(create_test_url("/one")).await);

        let urls = queue.snapshot_urls_bounded(0).await;
        assert!(urls.is_empty());
        assert_eq!(queue.len().await, 1);
    }

    #[tokio::test]
    async fn test_snapshot_urls_bounded_empty_queue_returns_empty() {
        let queue = UrlQueue::new();
        assert!(queue.snapshot_urls_bounded(5).await.is_empty());
    }

    #[tokio::test]
    async fn test_snapshot_urls_bounded_matches_drain_all_order() {
        let queue = UrlQueue::new();

        assert!(
            queue
                .push_prioritized(create_test_url("/low"), UrlSource::Link)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/high"), UrlSource::Seed)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/mid"), UrlSource::Sitemap)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/another-low"), UrlSource::Link)
                .await
        );

        // Snapshot first (non-consuming), then drain: the bounded snapshot
        // must equal the first n of the true pop order.
        let snapshot = queue.snapshot_urls_bounded(3).await;
        let drained = queue.drain_all().await;
        let expected: Vec<String> = drained.iter().take(3).map(|d| d.url.to_string()).collect();

        assert_eq!(snapshot, expected);
    }

    #[tokio::test]
    async fn test_peek_shows_highest_priority() {
        let queue = UrlQueue::new();

        assert!(
            queue
                .push_prioritized(create_test_url("/link"), UrlSource::Link)
                .await
        );
        assert!(
            queue
                .push_prioritized(create_test_url("/seed"), UrlSource::Seed)
                .await
        );

        let peeked = queue.peek().await.unwrap();
        assert_eq!(peeked.url.path(), "/seed");

        // Queue should not be modified
        assert_eq!(queue.len().await, 2);
    }

    #[tokio::test]
    async fn test_minimum_priority_floor() {
        // Test that very deep URLs still have minimum priority of 1
        let url = create_test_url_with_depth("/very-deep", 255);
        let prioritized = PrioritizedUrl::from_source(url, UrlSource::Link);
        assert_eq!(prioritized.priority, 1);
    }

    #[test]
    fn test_prioritized_url_ordering() {
        let url = create_test_url("/test");
        let high = PrioritizedUrl::new(url.clone(), 1000);
        let low = PrioritizedUrl::new(url.clone(), 100);
        let mid = PrioritizedUrl::new(url.clone(), 500);

        assert!(high > low);
        assert!(high > mid);
        assert!(mid > low);
        assert_eq!(
            high.cmp(&PrioritizedUrl::new(url.clone(), 1000)),
            Ordering::Equal
        );
    }
}
