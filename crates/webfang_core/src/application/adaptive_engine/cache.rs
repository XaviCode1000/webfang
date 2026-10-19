//! Cache machinery for the adaptive selector repair engine.
//!
//! A TTL-bounded [`DashMap`](dashmap::DashMap) keyed by a deterministic FNV-1a
//! hash of the failed selector combined with the DOM structural hash. Eviction
//! is lazy: expired entries are purged only when the cache reaches its
//! configured capacity.

use std::time::Instant;

/// Cached repair entry with expiration.
#[derive(Debug, Clone)]
pub struct CachedEntry {
    /// The cached repair outcome.
    pub outcome: crate::application::adaptive_engine::real::AdaptiveRepairOutcome,
    /// Expiration timestamp for this cache entry.
    pub expires_at: Instant,
}

/// Minimal FNV-1a hasher for deterministic cache key computation.
#[derive(Default)]
pub struct FnvHasher(u64);

impl std::hash::Hasher for FnvHasher {
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
