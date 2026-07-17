//! Autoscaled concurrency — dynamically adjusts task concurrency based on RAM usage.
//!
//! Uses `sysinfo` via [`ResourceGovernor`] to monitor system memory and
//! adaptively scales the effective concurrency limit:
//!
//! | RAM Usage | Level      | Effect                          |
//! |-----------|------------|---------------------------------|
//! | < 80%     | Normal     | Full base concurrency           |
//! | 80–90%    | Reduced    | 50% of base concurrency         |
//! | > 90%     | Critical   | 0 (pause new task spawning)     |
//!
//! The level is shared atomically between a background poller task and the
//! engine's spawn loop — no locks required.

use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::AUTOSCALE_LEVEL_TRANSITIONS;

/// Memory-pressure concurrency level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConcurrencyLevel {
    /// RAM < 80% — full base concurrency.
    Normal,
    /// RAM 80–90% — 50% of base concurrency.
    Reduced,
    /// RAM > 90% — pause new task spawning.
    Critical,
}

/// Atomic wrapper for sharing [`ConcurrencyLevel`] between the background
/// poller and the engine without locks.
///
/// Uses `AtomicUsize` with `Ordering::Relaxed` — no cross-thread ordering
/// constraints needed because the poller and engine only need eventual
/// consistency (a 5s stale read is acceptable).
pub struct SharedConcurrencyLevel {
    level: AtomicUsize,
}

impl SharedConcurrencyLevel {
    /// Create a new instance at [`ConcurrencyLevel::Normal`].
    pub fn new() -> Self {
        Self {
            level: AtomicUsize::new(0),
        }
    }

    /// Update the current concurrency level.
    pub fn set(&self, level: ConcurrencyLevel) {
        let val = match level {
            ConcurrencyLevel::Normal => 0,
            ConcurrencyLevel::Reduced => 1,
            ConcurrencyLevel::Critical => 2,
        };

        #[cfg(feature = "otel-metrics")]
        {
            let old = self.level.swap(val, Ordering::Relaxed);
            if old != val {
                let direction = match (old, val) {
                    (0, 1) | (0, 2) | (1, 2) => "down",
                    (1, 0) | (2, 0) | (2, 1) => "up",
                    _ => "same",
                };
                AUTOSCALE_LEVEL_TRANSITIONS
                    .add(1, &[opentelemetry::KeyValue::new("direction", direction)]);
            }
        }

        #[cfg(not(feature = "otel-metrics"))]
        self.level.store(val, Ordering::Relaxed);
    }

    /// Read the current concurrency level.
    pub fn get(&self) -> ConcurrencyLevel {
        match self.level.load(Ordering::Relaxed) {
            0 => ConcurrencyLevel::Normal,
            1 => ConcurrencyLevel::Reduced,
            _ => ConcurrencyLevel::Critical,
        }
    }

    /// Compute effective concurrency from a base value and the current level.
    pub fn effective_concurrency(&self, base: usize) -> usize {
        match self.get() {
            ConcurrencyLevel::Normal => base,
            ConcurrencyLevel::Reduced => base / 2,
            ConcurrencyLevel::Critical => 0,
        }
    }
}

impl Default for SharedConcurrencyLevel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_normal() {
        let scl = SharedConcurrencyLevel::new();
        assert_eq!(scl.get(), ConcurrencyLevel::Normal);
    }

    #[test]
    fn test_set_get_roundtrip() {
        let scl = SharedConcurrencyLevel::new();

        scl.set(ConcurrencyLevel::Reduced);
        assert_eq!(scl.get(), ConcurrencyLevel::Reduced);

        scl.set(ConcurrencyLevel::Critical);
        assert_eq!(scl.get(), ConcurrencyLevel::Critical);

        scl.set(ConcurrencyLevel::Normal);
        assert_eq!(scl.get(), ConcurrencyLevel::Normal);
    }

    #[test]
    fn test_effective_concurrency_normal() {
        let scl = SharedConcurrencyLevel::new();
        // Normal → full base
        assert_eq!(scl.effective_concurrency(10), 10);
        assert_eq!(scl.effective_concurrency(1), 1);
        assert_eq!(scl.effective_concurrency(0), 0);
    }

    #[test]
    fn test_effective_concurrency_reduced() {
        let scl = SharedConcurrencyLevel::new();
        scl.set(ConcurrencyLevel::Reduced);
        // Reduced → base / 2
        assert_eq!(scl.effective_concurrency(10), 5);
        assert_eq!(scl.effective_concurrency(9), 4);
        assert_eq!(scl.effective_concurrency(1), 0);
    }

    #[test]
    fn test_effective_concurrency_critical() {
        let scl = SharedConcurrencyLevel::new();
        scl.set(ConcurrencyLevel::Critical);
        // Critical → 0 (pause)
        assert_eq!(scl.effective_concurrency(10), 0);
        assert_eq!(scl.effective_concurrency(1), 0);
        assert_eq!(scl.effective_concurrency(0), 0);
    }

    #[test]
    fn test_transitions_normal_to_critical() {
        let scl = SharedConcurrencyLevel::new();
        let base = 20;

        // Start normal
        assert_eq!(scl.effective_concurrency(base), 20);

        // RAM climbs → reduced
        scl.set(ConcurrencyLevel::Reduced);
        assert_eq!(scl.effective_concurrency(base), 10);

        // RAM spikes → critical
        scl.set(ConcurrencyLevel::Critical);
        assert_eq!(scl.effective_concurrency(base), 0);

        // RAM drops back → normal
        scl.set(ConcurrencyLevel::Normal);
        assert_eq!(scl.effective_concurrency(base), 20);
    }

    #[test]
    fn test_atomic_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let scl = Arc::new(SharedConcurrencyLevel::new());
        let mut handles = vec![];

        for i in 0..10 {
            let scl = Arc::clone(&scl);
            handles.push(thread::spawn(move || {
                scl.set(ConcurrencyLevel::Reduced);
                let _ = scl.effective_concurrency(i * 10);
                scl.set(ConcurrencyLevel::Normal);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(scl.get(), ConcurrencyLevel::Normal);
    }
}

#[cfg(test)]
#[cfg(feature = "otel-metrics")]
mod metrics_tests {
    use super::*;

    #[test]
    fn test_autoscale_transition_instrument_init() {
        let _ = &*AUTOSCALE_LEVEL_TRANSITIONS;
    }

    #[test]
    fn test_set_records_direction_down() {
        let scl = SharedConcurrencyLevel::new();
        // Normal -> Reduced = "down"
        scl.set(ConcurrencyLevel::Reduced);
        assert_eq!(scl.get(), ConcurrencyLevel::Reduced);
    }

    #[test]
    fn test_set_records_direction_up() {
        let scl = SharedConcurrencyLevel::new();
        scl.set(ConcurrencyLevel::Critical);
        // Critical -> Normal = "up"
        scl.set(ConcurrencyLevel::Normal);
        assert_eq!(scl.get(), ConcurrencyLevel::Normal);
    }

    #[test]
    fn test_same_level_no_duplicate_metric() {
        let scl = SharedConcurrencyLevel::new();
        // Setting same level should not panic (no-op)
        scl.set(ConcurrencyLevel::Normal);
        assert_eq!(scl.get(), ConcurrencyLevel::Normal);
    }
}
