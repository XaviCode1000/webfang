//! URL deduplication module.
//!
//! Lock-free, memory-efficient URL deduplication built on
//! `DashSet<u64, ahash::RandomState>`. Stores an 8-byte hash per URL instead of
//! the full normalized `String`, collapsing per-URL residency from ~150 B to
//! ~8 B (FR-2: <100 MB for 10 M URLs).
//!
//! # Design Decisions
//!
//! - `DashSet<u64, ahash::RandomState>` — lock-free concurrent check-and-insert
//! - Per-process randomized seed via `ahash::RandomState::new()` (FR-3 HashDoS
//!   resistance); deliberately NOT `RandomState::default()` (frozen keys)
//! - `try_insert` is synchronous and atomic — no `Mutex`, no `.await` in the
//!   hot loop (FR-8: no data races, no lost updates)
//! - Deterministic within a single process (FR-5): the seed is fixed for the
//!   process lifetime, so identical URLs hash to identical `u64` keys

use dashmap::DashSet;

/// Lock-free URL deduplicator.
///
/// Stores a `u64` hash (8 bytes) per seen URL rather than the full string.
/// Dedup is atomic: `try_insert` performs a single `DashSet::insert`
/// (check-and-insert in one step), so concurrent callers cannot race past each
/// other.
///
/// The hash seed is randomized per process startup (`RandomState::new()`,
/// satisfying FR-3) yet stable for the deduplicator's lifetime, so the same URL
/// always maps to the same `u64` within one process (FR-5).
///
/// # Example
///
/// ```rust
/// use rust_scraper::application::deduplicator::UrlDeduplicator;
///
/// let dedup = UrlDeduplicator::new();
/// assert!(dedup.try_insert("https://example.com"));   // newly inserted
/// assert!(!dedup.try_insert("https://example.com"));  // already seen
/// assert_eq!(dedup.len(), 1);
/// ```
pub struct UrlDeduplicator {
    seen: DashSet<u64, ahash::RandomState>,
    rs: ahash::RandomState,
}

impl UrlDeduplicator {
    /// Create a new deduplicator with a fresh per-process randomized seed.
    ///
    /// Pre-allocates capacity for ~100 URLs (mem-with-capacity).
    #[must_use]
    pub fn new() -> Self {
        let rs = ahash::RandomState::new();
        Self {
            seen: DashSet::with_capacity_and_hasher(100, rs.clone()),
            rs,
        }
    }

    /// Atomically check-and-insert a URL.
    ///
    /// Returns `true` if the URL was newly inserted, `false` if it was already
    /// present. This is a single lock-free `DashSet::insert` — no `Mutex`, no
    /// `.await` — so it is safe to call from many Tokio tasks concurrently
    /// (FR-8) without data races or lost updates.
    #[must_use]
    pub fn try_insert(&self, url: &str) -> bool {
        self.seen.insert(self.rs.hash_one(url))
    }

    /// Number of unique URLs currently tracked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.seen.len()
    }

    /// Whether no URLs have been tracked yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

impl Default for UrlDeduplicator {
    fn default() -> Self {
        // Delegate to `new()` so the seed is randomized (RandomState::new()).
        // Do NOT replace with `#[derive(Default)]`: the derived impl would use
        // `RandomState::default()` (frozen compile-time keys), violating FR-3.
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_new_deduplicator_is_empty() {
        // empty: fresh deduplicator holds nothing, first insert succeeds
        let dedup = UrlDeduplicator::new();
        assert!(dedup.is_empty());
        assert_eq!(dedup.len(), 0);
        assert!(dedup.try_insert("https://example.com"));
        assert!(!dedup.is_empty());
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn test_whitespace_url_does_not_panic() {
        // whitespace: a whitespace-only string is hashed as-is (no trimming
        // here); it must not panic and must dedup like any other key.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("   "));
        assert!(!dedup.try_insert("   "));
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn test_valid_url_insert_and_dedup() {
        // valid + Scenario: Basic dedup — same URL rejected twice
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://example.com/page")); // newly inserted
        assert!(!dedup.try_insert("https://example.com/page")); // already seen
        assert_eq!(dedup.len(), 1);
        // A different URL is accepted (Scenario: Different URLs accepted)
        assert!(dedup.try_insert("https://example.com/other"));
        assert_eq!(dedup.len(), 2);
    }

    #[test]
    fn test_no_host_url_handled() {
        // no-host: a URL without a host is hashed as a plain string — no panic,
        // normal dedup semantics.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("/relative/path"));
        assert!(!dedup.try_insert("/relative/path"));
        assert!(dedup.try_insert("javascript:void(0)"));
        assert_eq!(dedup.len(), 2);
    }

    #[test]
    fn test_deterministic_within_process() {
        // deterministic + FR-5: same URL -> same u64 within one process ->
        // consistent dedup. 100 inserts of the same URL; only the first wins.
        let dedup = UrlDeduplicator::new();
        let url = "https://example.com/deterministic";
        let mut newly_inserted = 0;
        for _ in 0..100 {
            if dedup.try_insert(url) {
                newly_inserted += 1;
            }
        }
        assert_eq!(newly_inserted, 1);
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn test_padded_urls_are_distinct() {
        // padded: try_insert does NOT trim/normalize; surrounding whitespace
        // yields a distinct hash from the bare URL. Normalization is
        // normalize_url's job, applied by callers before inserting.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://example.com"));
        assert!(dedup.try_insert(" https://example.com "));
        assert!(dedup.try_insert("https://example.com\n"));
        assert_eq!(dedup.len(), 3);
        // Re-inserting the bare URL is still a duplicate of itself
        assert!(!dedup.try_insert("https://example.com"));
    }

    #[tokio::test]
    async fn test_concurrent_inserts_unique() {
        // concurrent + Scenario: Concurrent access correctness (FR-8)
        // 1000 Tokio tasks each insert a unique URL; the set must end up with
        // exactly 1000 entries — no panics, no lost updates.
        let dedup = Arc::new(UrlDeduplicator::new());
        let mut handles = Vec::with_capacity(1000);
        for i in 0..1000u32 {
            let dedup = Arc::clone(&dedup);
            handles.push(tokio::spawn(async move {
                let url = format!("https://example.com/page/{i}");
                assert!(dedup.try_insert(&url), "unique URL must be newly inserted");
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert_eq!(dedup.len(), 1000);
    }
}
