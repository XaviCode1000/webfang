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
//! - **Canonical-key dedup (#517)**: `try_insert` normalizes the URL through
//!   the domain `normalize_url` (strip_www, strip hash/query, collapse
//!   `/index.html`) BEFORE hashing, so two spellings of the same document
//!   (`https://www.example.com/page`, `https://example.com/page#top`) collapse
//!   to one key. This is a deliberate contract change from the earlier design
//!   where callers were expected to normalize first — the queue cannot rely on
//!   every producer (seed, links, sitemap) doing so.

use dashmap::DashSet;

use crate::domain::url_validation::{normalize_url, NormalizeConfig, RemoveQueryParameters};

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
/// URLs are normalized to a canonical form before hashing (#517), so equivalent
/// spellings of the same document deduplicate as one key.
///
/// # Example
///
/// ```rust
/// use webfang_core::application::deduplicator::UrlDeduplicator;
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
    ///
    /// The URL is normalized via the canonical domain normalizer before
    /// hashing (#517), so `https://www.example.com/page` and
    /// `https://example.com/page#top` collide on one key.
    #[must_use]
    pub fn try_insert(&self, url: &str) -> bool {
        let canonical = normalize_url(
            url,
            &NormalizeConfig {
                strip_www: true,
                query_policy: RemoveQueryParameters::All,
            },
        );
        self.seen.insert(self.rs.hash_one(&canonical))
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
    fn test_padded_urls_are_canonicalized() {
        // padded + canonical-key (#517): surrounding whitespace is removed by
        // url-normalize's WHATWG preprocessing, so padded and bare spellings
        // of the same URL collapse to ONE key. This is the contract change
        // from the earlier design where try_insert hashed raw strings.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://example.com"));
        assert!(!dedup.try_insert(" https://example.com "));
        assert!(!dedup.try_insert("https://example.com\n"));
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn test_cross_normalization_www_collapses() {
        // www-vs-bare (#517): the same document reached via www and bare host
        // must deduplicate to a single key.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://www.example.com/page"));
        assert!(!dedup.try_insert("https://example.com/page"));
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn test_cross_normalization_fragment_collapses() {
        // fragments (#517): a fragment is not part of the document identity.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://example.com/page#section"));
        assert!(!dedup.try_insert("https://example.com/page#other"));
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn test_cross_normalization_query_order_collapses() {
        // query ordering + full query strip (#517): query parameters are
        // removed entirely for dedup (remove_query_parameters: All), so order
        // and presence of query strings never splits a document.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://example.com/page?a=1&b=2"));
        assert!(!dedup.try_insert("https://example.com/page?b=2&a=1"));
        assert!(!dedup.try_insert("https://example.com/page?utm_source=x"));
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn test_cross_normalization_host_case_collapses() {
        // host casing (#517): WHATWG URL normalization lowercases the host.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://Example.COM/page"));
        assert!(!dedup.try_insert("https://example.com/page"));
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn test_cross_normalization_default_port_collapses() {
        // default ports (#517): :443 on https is redundant.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://example.com/page"));
        assert!(!dedup.try_insert("https://example.com:443/page"));
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn test_cross_normalization_index_html_collapses() {
        // /index.html (#344/#517): the same document served at / and
        // /index.html is one key.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://example.com/"));
        assert!(!dedup.try_insert("https://example.com/index.html"));
        assert_eq!(dedup.len(), 1);
    }

    #[test]
    fn test_cross_normalization_distinct_documents_stay_distinct() {
        // distinct documents must NOT collide (#517): normalization collapses
        // equivalent spellings, never different pages.
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://example.com/page"));
        assert!(dedup.try_insert("https://example.com/other"));
        assert!(dedup.try_insert("https://example.com/page/"));
        assert_eq!(dedup.len(), 3);
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

    #[test]
    fn test_discovered_url_dedup_via_url_deduplicator() {
        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://example.com/page"));
        assert!(!dedup.try_insert("https://example.com/page")); // duplicate
        assert!(dedup.try_insert("https://example.com/other")); // different URL
        assert_eq!(dedup.len(), 2);
    }
}
