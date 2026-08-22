//! Pure concurrency-derivation functions over plain [`DetectedHw`] data.
//!
//! Every function here is total, IO-free, and clock-free: it maps hardware
//! snapshots (and explicit operator overrides) onto tier newtypes. Purity is
//! what makes these derivations unit-testable across synthetic core counts
//! and safe under `cargo miri` (see task 1.9).

use std::num::NonZeroUsize;

use super::clamp::{clamp_budget, MAX_CONCURRENCY_CEILING};
use super::detector::DetectedHw;
use super::tiers::CrawlConcurrency;

/// Legacy auto-detection table from `ConcurrencyConfig::resolve()`.
///
/// Pinns TODAY'S behavior: `1-2→1`, `3-4→3`, `5-7→5`, else `min(cores−1, 8)`,
/// finally clamped through the canonical budget clamp (`≤ 16`). Shared by the
/// crawl derivation and the burst default (decision Q1 DECOUPLE) so both
/// knobs answer from ONE table.
pub(crate) fn auto_crawl_table(cores: NonZeroUsize) -> usize {
    let optimal = match cores.get() {
        1 | 2 => 1,
        3 | 4 => 3,
        5..=7 => 5,
        n => (n - 1).min(8),
    };
    clamp_budget(optimal, NonZeroUsize::MIN, MAX_CONCURRENCY_CEILING).get()
}

/// Derive the Operation-tier crawl/scrape concurrency from detected hardware.
///
/// Equivalent by construction to `ConcurrencyConfig::default().resolve()` on
/// the same machine (guarded by an equivalence sweep over cores `1..=32`).
#[must_use]
pub fn derive_auto_crawl(detected: DetectedHw) -> CrawlConcurrency {
    // The table's minimum output is 1, so the NonZero wrap cannot fail; an
    // unwrap here would be a programmer error, not a reachable state.
    let value = NonZeroUsize::new(auto_crawl_table(detected.parallelism))
        .unwrap_or_else(|| unreachable!("auto-crawl table never yields zero"));
    CrawlConcurrency::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::budget::detector::{FixedDetector, HardwareDetector};

    /// Reference implementation of TODAY'S `resolve()` math, kept verbatim
    /// from `domain/config.rs` so drift in either direction fails loudly.
    fn legacy_auto_crawl(cores: usize) -> usize {
        let optimal = match cores {
            1 | 2 => 1,
            3 | 4 => 3,
            5..=7 => 5,
            _ => (cores - 1).min(8),
        };
        clamp_budget(optimal, NonZeroUsize::MIN, MAX_CONCURRENCY_CEILING).get()
    }

    fn hw(cores: usize) -> DetectedHw {
        DetectedHw::new(
            NonZeroUsize::new(cores).unwrap_or_else(|| panic!("test core count {cores} is non-zero")),
            None,
        )
    }

    #[test]
    fn table_pins_legacy_boundaries() {
        assert_eq!(auto_crawl_table(NonZeroUsize::new(1).unwrap()), 1);
        assert_eq!(auto_crawl_table(NonZeroUsize::new(2).unwrap()), 1);
        assert_eq!(auto_crawl_table(NonZeroUsize::new(3).unwrap()), 3);
        assert_eq!(auto_crawl_table(NonZeroUsize::new(4).unwrap()), 3);
        assert_eq!(auto_crawl_table(NonZeroUsize::new(5).unwrap()), 5);
        assert_eq!(auto_crawl_table(NonZeroUsize::new(6).unwrap()), 5);
        assert_eq!(auto_crawl_table(NonZeroUsize::new(7).unwrap()), 5);
        assert_eq!(auto_crawl_table(NonZeroUsize::new(8).unwrap()), 7);
        assert_eq!(auto_crawl_table(NonZeroUsize::new(9).unwrap()), 8);
        // Ceiling: min(cores−1, 8) saturates at 8, well under clamp 16.
        assert_eq!(auto_crawl_table(NonZeroUsize::new(32).unwrap()), 8);
    }

    #[test]
    fn sweep_matches_legacy_resolve_over_1_to_32() {
        for cores in 1..=32usize {
            let detected = hw(cores);
            assert_eq!(
                derive_auto_crawl(detected).get(),
                legacy_auto_crawl(cores),
                "core count {cores} diverges from legacy resolve()"
            );
        }
    }

    #[test]
    fn live_machine_resolve_equals_derivation() {
        let detected = crate::domain::budget::detector::SystemDetector::new().detect();
        assert_eq!(
            crate::domain::config::ConcurrencyConfig::default().resolve(),
            derive_auto_crawl(detected).get(),
            "live 'auto' path diverges from the pure derivation"
        );
    }

    #[test]
    fn derivation_is_total_and_never_below_one() {
        for cores in 1..=32usize {
            assert!(derive_auto_crawl(hw(cores)).get() >= 1);
            assert!(derive_auto_crawl(hw(cores)).get() <= MAX_CONCURRENCY_CEILING);
        }
    }

    #[test]
    fn fixed_detector_drives_the_table_without_io() {
        let det = FixedDetector::with_detection(NonZeroUsize::new(6).unwrap(), Some(8));
        assert_eq!(derive_auto_crawl(*det.detected()).get(), 5);
    }
}
