//! Pure concurrency-derivation functions over plain [`DetectedHw`] data.
//!
//! Every function here is total, IO-free, and clock-free: it maps hardware
//! snapshots (and explicit operator overrides) onto tier newtypes. Purity is
//! what makes these derivations unit-testable across synthetic core counts
//! and safe under `cargo miri` (see task 1.9).

use std::num::{NonZeroU32, NonZeroUsize};

use super::clamp::{clamp_budget, MAX_CONCURRENCY_CEILING};
use super::detector::DetectedHw;
use super::tiers::{BurstPermits, CrawlConcurrency};

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

/// Derive the rate-limiter burst permit count (decision Q1 DECOUPLE).
///
/// * `explicit = Some(b)` → the operator's value wins verbatim.
/// * `explicit = None` → TODAY'S effective default: the auto-crawl table
///   evaluated against the DETECTOR SEAM snapshot — never re-read from the
///   configured crawler concurrency, so raising crawler concurrency does not
///   move the burst.
///
/// The explicit override surface (`WEBFANG_RATE_LIMIT_BURST` env + optional
/// CLI flag) lands in Phase 2 via `BudgetOverrides`; this function is its
/// pure core.
#[must_use]
pub fn derive_burst(explicit: Option<BurstPermits>, detected: DetectedHw) -> BurstPermits {
    match explicit {
        Some(b) => b,
        None => {
            let raw = auto_crawl_table(detected.parallelism);
            // Table output is 1..=16 by construction: both guards are
            // programmer-error traps, never reachable states.
            let permits = u32::try_from(raw)
                .unwrap_or_else(|_| unreachable!("auto-crawl table output fits u32"));
            BurstPermits::from(
                NonZeroU32::new(permits)
                    .unwrap_or_else(|| unreachable!("auto-crawl table never yields zero")),
            )
        },
    }
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
            NonZeroUsize::new(cores)
                .unwrap_or_else(|| panic!("test core count {cores} is non-zero")),
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

    // --- derive_burst (Q1 DECOUPLE) -------------------------------------

    fn burst_hw(cores: usize) -> DetectedHw {
        hw(cores)
    }

    #[test]
    fn explicit_override_wins_over_default() {
        let explicit = BurstPermits::new(2).expect("2 is non-zero");
        assert_eq!(derive_burst(Some(explicit), burst_hw(8)).get(), 2);
        // Even where the table would say something else entirely.
        let explicit_big = BurstPermits::new(64).expect("64 is non-zero");
        assert_eq!(derive_burst(Some(explicit_big), burst_hw(1)).get(), 64);
    }

    #[test]
    fn default_equals_auto_crawl_table_from_detector_seam() {
        for cores in [1usize, 3, 6, 9, 24] {
            assert_eq!(
                derive_burst(None, burst_hw(cores)).get() as usize,
                legacy_auto_crawl(cores),
                "default burst for {cores} cores must equal today's table"
            );
        }
    }

    /// THE SPEC SCENARIO: crawler concurrency raised N→M>N must leave the
    /// burst at B. The burst derivation consumes ONLY the detector seam
    /// snapshot — a different configured crawler value is not an input.
    #[test]
    fn raising_configured_crawler_concurrency_leaves_burst_unchanged() {
        let detected = burst_hw(8);
        let burst_before = derive_burst(None, detected);
        let crawl_n = derive_auto_crawl(detected); // operator at default N
                                                   // Operator raises the configured crawler concurrency to M > N.
        let m_raw = crawl_n.get() + 9;
        let crawl_m = CrawlConcurrency::from(
            NonZeroUsize::new(m_raw.min(MAX_CONCURRENCY_CEILING)).expect("M is non-zero"),
        );
        assert!(crawl_m.get() > crawl_n.get());
        assert_eq!(derive_burst(None, detected), burst_before);
    }

    #[test]
    fn burst_never_zero_across_sweep() {
        for cores in 1..=32usize {
            assert!(derive_burst(None, burst_hw(cores)).get() >= 1);
        }
    }

    #[test]
    fn zero_is_unrepresentable_as_explicit_override() {
        assert!(BurstPermits::new(0).is_err());
    }
}
