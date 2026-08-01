//! Cache machinery for the adaptive selector repair engine.
//!
//! A TTL-bounded [`DashMap`](dashmap::DashMap) keyed by a deterministic FNV-1a
//! hash of the failed selector combined with the DOM structural hash. Eviction
//! is lazy: expired entries are purged only when the cache reaches its
//! configured capacity.

use std::hash::{Hash, Hasher};
use std::time::Instant;

use super::{AdaptiveRepairOutcome, AdaptiveSelectorEngine};

/// Cached repair entry with expiration.
pub(super) struct CachedEntry {
    outcome: AdaptiveRepairOutcome,
    expires_at: Instant,
}

impl AdaptiveSelectorEngine {
    /// Compute cache key from selector + structural hash.
    pub(super) fn cache_key(&self, selector: &str, structural_hash: u64) -> u64 {
        let mut hasher = FnvHasher::default();
        selector.hash(&mut hasher);
        structural_hash.hash(&mut hasher);
        hasher.finish()
    }

    /// Fetch a non-expired cached outcome, if present.
    ///
    /// Returns a clone of the stored outcome without mutating its trace; callers
    /// decide whether to flag the result as a cache hit.
    pub(super) fn cache_get(&self, key: u64) -> Option<AdaptiveRepairOutcome> {
        let entry = self.cache.get(&key)?;
        if entry.expires_at > Instant::now() {
            Some(entry.outcome.clone())
        } else {
            None
        }
    }

    /// Insert into cache with lazy eviction when at capacity.
    pub(super) fn cache_insert(&self, key: u64, outcome: AdaptiveRepairOutcome) {
        // Lazy eviction: if at capacity, remove expired entries
        if self.cache.len() >= self.options.max_cache_entries {
            let now = Instant::now();
            self.cache.retain(|_, entry| entry.expires_at > now);
        }

        self.cache.insert(
            key,
            CachedEntry {
                outcome,
                expires_at: Instant::now() + self.options.cache_ttl,
            },
        );
    }

    /// Get the number of cached entries (for monitoring).
    #[must_use]
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

/// Minimal FNV-1a hasher for deterministic cache key computation.
#[derive(Default)]
pub(super) struct FnvHasher(u64);

impl Hasher for FnvHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        self.0 = h;
    }
}
