//! Pure concurrency-derivation functions over plain
//! [`DetectedHw`](detector::DetectedHw) data.
//!
//! Every function here is total, IO-free, and clock-free: it maps hardware
//! snapshots (and explicit operator overrides) onto tier newtypes. Purity is
//! what makes these derivations unit-testable across synthetic core counts
//! and safe under `cargo miri` (see task 1.9).

use std::num::{NonZeroU32, NonZeroUsize};

use super::clamp::{clamp_budget, MAX_CONCURRENCY_CEILING};
use super::detector::DetectedHw;
use super::tiers::{BurstPermits, CrawlConcurrency, MaxChromeInstances};

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

/// Derive the Operation-tier crawl concurrency with operator override support.
///
/// * `explicit = Some(v)` → the operator's explicit value wins verbatim
///   (`--concurrency` / TOML / TUI when not "auto", mapped by preflight into
///   [`super::BudgetOverrides::crawl`]).
/// * `explicit = None` → the auto-crawl table over the detector seam —
///   today's default, behavior-identical to `ConcurrencyConfig::default()`.
#[must_use]
pub fn derive_crawl(explicit: Option<CrawlConcurrency>, detected: DetectedHw) -> CrawlConcurrency {
    explicit.unwrap_or_else(|| derive_auto_crawl(detected))
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

// --- Governor RAM formula (task 1.7) ---------------------------------

/// Approximate RAM cost of one Chrome instance (200 MB), mirroring the
/// resource governor's legacy `CHROME_INSTANCE_COST` constant.
pub(crate) const CHROME_INSTANCE_COST_BYTES: u64 = 200_000_000;

/// Fraction of total RAM budgeted to Chrome instances; mirrors the
/// governor's legacy `RAM_BUDGET_FRACTION`. Kept as `f64` so derived
/// outputs stay identical to today's governor math.
const RAM_BUDGET_FRACTION: f64 = 0.6;

/// RAM-usage thresholds for the Chrome-instance budget.
///
/// * at/above [`RamThresholds::warning_percent`] → budget halved,
/// * at/above [`RamThresholds::critical_percent`] → new instances denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RamThresholds {
    /// Usage percentage at/above which the instance budget is halved.
    pub warning_percent: u8,
    /// Usage percentage at/above which new Chrome instances are denied.
    pub critical_percent: u8,
}

impl RamThresholds {
    /// Canonical default warning percentage. Consumers that need the value
    /// in `const` context (e.g. the governor's threshold constants) derive
    /// from these associated consts so the model stays the single source of
    /// truth (#897 item 4).
    pub const DEFAULT_WARNING_PERCENT: u8 = 80;
    /// Canonical default critical percentage; see
    /// [`Self::DEFAULT_WARNING_PERCENT`].
    pub const DEFAULT_CRITICAL_PERCENT: u8 = 90;
}

impl Default for RamThresholds {
    fn default() -> Self {
        Self {
            warning_percent: Self::DEFAULT_WARNING_PERCENT,
            critical_percent: Self::DEFAULT_CRITICAL_PERCENT,
        }
    }
}

/// Explicit outcome of the RAM-based Chrome-instance derivation.
///
/// Denial is a first-class variant — never a zero permit count — because
/// [`MaxChromeInstances`] cannot
/// represent 0 by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxChromeDecision {
    /// Permit acquisition allowed with this instance ceiling.
    Allow(MaxChromeInstances),
    /// RAM usage at/above the critical threshold; new instances denied.
    Deny,
}

/// Derive the ResourceGovernor's Chrome-instance budget from RAM.
///
/// Pure plain-data function (Miri-safe): reproduces TODAY'S governor math
/// — `(total_ram_bytes * 0.6) / 200 MB`, floored at 1 — and layers the
/// usage-pressure tiers on top:
///
/// * usage `< warning`  → full budget,
/// * `warning ≤ usage < critical` → budget halved (floored at 1),
/// * usage `≥ critical` → [`MaxChromeDecision::Deny`].
///
/// Usage is computed from `used_ram_bytes` over `total_ram_bytes` exactly
/// like the governor's integer `used * 100 / total`; values above 100%
/// clamp to 100%.
#[must_use]
pub fn derive_max_instances(
    total_ram_bytes: u64,
    used_ram_bytes: u64,
    thresholds: RamThresholds,
) -> MaxChromeDecision {
    // Integer usage percentage, identical to the governor's
    // `used * 100 / total`; over-total usage clamps to 100%.
    let used = used_ram_bytes.min(total_ram_bytes);
    let usage_percent = if total_ram_bytes == 0 {
        0
    } else {
        used.saturating_mul(100) / total_ram_bytes
    } as u8;

    if usage_percent >= thresholds.critical_percent {
        return MaxChromeDecision::Deny;
    }

    // Legacy governor math kept verbatim (`compute_max_instances`): the
    // float multiply is intentional so outputs stay bit-identical.
    let budget = (total_ram_bytes as f64 * RAM_BUDGET_FRACTION) as u64;
    let full = (budget / CHROME_INSTANCE_COST_BYTES).max(1);

    let effective = if usage_percent >= thresholds.warning_percent {
        // Governor halves the available permits at/above the warning
        // threshold; floor at 1 so a permit count of 0 is unrepresentable.
        (full / 2).max(1)
    } else {
        full
    };

    let instances =
        usize::try_from(effective).unwrap_or_else(|_| unreachable!("instance budget fits usize"));
    MaxChromeDecision::Allow(
        MaxChromeInstances::new(instances)
            .unwrap_or_else(|_| unreachable!("instance budget never yields zero")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::budget::detector::{FixedDetector, HardwareDetector};

    // --- derive_max_instances (governor RAM formula, task 1.7) -----------

    /// Verbatim replica of TODAY'S `ResourceGovernor::compute_max_instances`
    /// math so any drift in either direction fails loudly.
    fn legacy_compute_max_instances(total_ram_bytes: u64) -> u64 {
        const CHROME_INSTANCE_COST: u64 = 200_000_000;
        let budget = (total_ram_bytes as f64 * 0.6) as u64;
        (budget / CHROME_INSTANCE_COST).max(1)
    }

    const GB: u64 = 1_000_000_000;

    #[test]
    fn full_budget_matches_governor_formula() {
        let decision = derive_max_instances(16 * GB, 0, RamThresholds::default());
        assert_eq!(
            decision,
            MaxChromeDecision::Allow(
                MaxChromeInstances::try_from(legacy_compute_max_instances(16 * GB) as usize)
                    .expect("formula output is non-zero")
            )
        );
    }

    #[test]
    fn tiny_or_absent_ram_floors_at_one() {
        for total in [0_u64, 1, 100_000_000] {
            let decision = derive_max_instances(total, 0, RamThresholds::default());
            assert_eq!(
                decision,
                MaxChromeDecision::Allow(MaxChromeInstances::new(1).expect("1 is non-zero")),
                "total {total} must floor the budget at one instance"
            );
        }
    }

    #[test]
    fn usage_below_warning_keeps_full_budget() {
        // 79% of 16 GB.
        let used = 16 * GB * 79 / 100;
        let decision = derive_max_instances(16 * GB, used, RamThresholds::default());
        assert_eq!(
            decision,
            MaxChromeDecision::Allow(
                MaxChromeInstances::try_from(legacy_compute_max_instances(16 * GB) as usize)
                    .expect("non-zero")
            )
        );
    }

    #[test]
    fn warning_threshold_halves_budget() {
        // Exactly 80% of 16 GB → halved from 48 to 24.
        let used = 16 * GB * 80 / 100;
        let decision = derive_max_instances(16 * GB, used, RamThresholds::default());
        let full = legacy_compute_max_instances(16 * GB);
        assert_eq!(full, 48);
        assert_eq!(
            decision,
            MaxChromeDecision::Allow(MaxChromeInstances::new((full / 2) as usize).expect("24"))
        );
    }

    #[test]
    fn critical_threshold_denies_explicitly() {
        // Exactly 90% of 16 GB → deny signal, never a zero permit count.
        let used = 16 * GB * 90 / 100;
        let decision = derive_max_instances(16 * GB, used, RamThresholds::default());
        assert_eq!(decision, MaxChromeDecision::Deny);
    }

    #[test]
    fn just_below_critical_still_halves_not_denies() {
        let used = 16 * GB * 89 / 100;
        let full = legacy_compute_max_instances(16 * GB);
        let decision = derive_max_instances(16 * GB, used, RamThresholds::default());
        assert_eq!(
            decision,
            MaxChromeDecision::Allow(MaxChromeInstances::new((full / 2) as usize).expect("24"))
        );
    }

    #[test]
    fn usage_above_total_clamps_to_full_pressure() {
        let decision = derive_max_instances(GB, 2 * GB, RamThresholds::default());
        assert_eq!(decision, MaxChromeDecision::Deny);
    }

    #[test]
    fn custom_thresholds_are_honored() {
        let thresholds = RamThresholds {
            warning_percent: 50,
            critical_percent: 60,
        };
        let full = legacy_compute_max_instances(16 * GB);
        // 55% → between custom thresholds → halved.
        let used = 16 * GB * 55 / 100;
        assert_eq!(
            derive_max_instances(16 * GB, used, thresholds),
            MaxChromeDecision::Allow(MaxChromeInstances::new((full / 2) as usize).expect("24"))
        );
        // 60% → at custom critical → deny.
        let used = 16 * GB * 60 / 100;
        assert_eq!(
            derive_max_instances(16 * GB, used, thresholds),
            MaxChromeDecision::Deny
        );
    }

    /// TRIANGULATE: sweep RAM sizes; with zero usage pressure every output is
    /// byte-identical to today's governor formula.
    #[test]
    fn sweep_zero_usage_matches_legacy_formula() {
        for gib in [1_u64, 2, 4, 8, 12, 16, 24, 32, 64] {
            let total = gib * GB;
            let decision = derive_max_instances(total, 0, RamThresholds::default());
            let expected = legacy_compute_max_instances(total);
            assert_eq!(
                decision,
                MaxChromeDecision::Allow(
                    MaxChromeInstances::try_from(expected as usize).expect("non-zero")
                ),
                "{gib} GiB diverges from legacy governor formula"
            );
        }
    }

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
