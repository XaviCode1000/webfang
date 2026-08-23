//! Domain budget model — Global→Domain→Operation→Asset concurrency tiers.
//!
//! ONE place in WebFang decides, clamps, or derives a concurrency number.
//! Enforcement adapters (semaphores, `buffer_unordered`, JoinSet gating,
//! governor permits) keep their mechanisms but derive every numeric bound
//! from this model. Dependencies point inward only: this module depends on
//! nothing outside `domain`.

pub(crate) mod clamp;
/// Pure derivation fns: hardware snapshots → tier newtypes (no IO, no clock).
pub mod derivation;
/// Canonical hardware-detection seam (`HardwareDetector`, Q2 UNIFY NOW).
pub mod detector;
/// Hardware-detector seam + pure derivation fns live in sibling modules;
/// tier newtypes and the tier aggregate are re-exported here.
pub mod tiers;

use std::num::NonZeroUsize;

use self::clamp::MAX_CONCURRENCY_CEILING;
use self::derivation::{
    derive_burst, derive_crawl, derive_max_instances, MaxChromeDecision, RamThresholds,
};
use self::detector::HardwareDetector;
use self::tiers::{
    BatchConcurrency, BudgetTiers, DownloadConcurrency, ElasticPermits, GlobalConcurrency,
    InferenceWorkers, MaxChromeInstances, OperationTier,
};
/// Re-exported for adapter layers (preflight budget staging, CLI parsers,
/// hot-path adapters deriving their bounds from the model).
pub use self::tiers::{BurstPermits, CrawlConcurrency, DomainSlots};

/// Default per-domain session-pool slots; TODAY'S `SessionPoolConfig
/// ::pool_size` value (task 2.2c wires this tier into the pool config).
pub(crate) const DOMAIN_SLOTS_DEFAULT: usize = 8;

/// Default asset-download concurrency; TODAY'S fixed default of 3.
pub(crate) const DOWNLOAD_CONCURRENCY_DEFAULT: usize = 3;

/// Default batch-processing concurrency; TODAY'S `--batch-concurrency`
/// CLI default.
pub(crate) const BATCH_CONCURRENCY_DEFAULT: usize = 5;

/// Default inference-worker count; TODAY'S adaptive-engine
/// `max_concurrent_inference` default.
pub(crate) const INFERENCE_WORKERS_DEFAULT: usize = 4;

/// Operator-level budget overrides. Every field is `None` unless the
/// operator explicitly set the knob; `Default` therefore reproduces
/// today's behavior exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BudgetOverrides {
    /// Explicit rate-limiter burst (`WEBFANG_RATE_LIMIT_BURST`, Phase 2).
    pub rate_burst: Option<BurstPermits>,
    /// Explicit crawl/scrape concurrency (`--concurrency` / TOML / TUI when
    /// not "auto"). `None` = auto-derive from the detector seam.
    pub crawl: Option<CrawlConcurrency>,
    /// Explicit batch concurrency (`--batch-concurrency`). `None` = tier
    /// default.
    pub batch: Option<BatchConcurrency>,
    /// Explicit asset-download concurrency (`--download-concurrency`).
    /// `None` = tier default.
    pub asset: Option<DownloadConcurrency>,
}

/// Immutable snapshot of every concurrency budget, built ONCE at engine /
/// orchestrator entry from [`BudgetOverrides`] plus a [`HardwareDetector`].
///
/// The model has no mutation surface after construction: adapters receive
/// derived tiers by value and no mid-run recomputation exists, so no locks
/// are needed and the type is trivially `Send + Sync`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetModel {
    tiers: BudgetTiers,
}

impl BudgetModel {
    /// Build the model from operator overrides plus detected hardware.
    ///
    /// With `BudgetOverrides::default()` this reproduces TODAY'S resolved
    /// numbers field-for-field (crawl auto table, downloads 3, session
    /// slots 8, burst = today's effective default, governor RAM formula);
    /// explicit overrides win per-knob without moving any other tier.
    #[must_use]
    pub fn build(overrides: BudgetOverrides, detector: &dyn HardwareDetector) -> BudgetModel {
        let detected = detector.detect();

        let crawl = derive_crawl(overrides.crawl, detected);
        let burst = derive_burst(overrides.rate_burst, detected);

        // Fixed defaults reproduce TODAY'S constants (see const docs for
        // each legacy source). Zero is unrepresentable in every tier type,
        // so construction cannot silently degrade to a zero budget.
        let global = GlobalConcurrency::from(
            NonZeroUsize::new(MAX_CONCURRENCY_CEILING)
                .unwrap_or_else(|| unreachable!("ceiling constant is non-zero")),
        );
        let domain = DomainSlots::new(DOMAIN_SLOTS_DEFAULT)
            .unwrap_or_else(|_| unreachable!("domain slot default is non-zero"));
        // Explicit operator overrides win per-knob; None falls back to
        // the tier default (today's constant).
        let batch = overrides.batch.unwrap_or_else(|| {
            BatchConcurrency::new(BATCH_CONCURRENCY_DEFAULT)
                .unwrap_or_else(|_| unreachable!("batch default is non-zero"))
        });
        let inference = InferenceWorkers::new(INFERENCE_WORKERS_DEFAULT)
            .unwrap_or_else(|_| unreachable!("inference default is non-zero"));
        let asset = overrides.asset.unwrap_or_else(|| {
            DownloadConcurrency::new(DOWNLOAD_CONCURRENCY_DEFAULT)
                .unwrap_or_else(|_| unreachable!("download default is non-zero"))
        });

        // Elastic mirrors the legacy `num_cpus::get().max(4)` bound, fed
        // through the canonical seam instead of a second core counter.
        let elastic_raw = detected.parallelism.get().max(4);
        let elastic = ElasticPermits::from(
            NonZeroUsize::new(elastic_raw).unwrap_or_else(|| unreachable!("max(x, 4) is non-zero")),
        );

        // Governor tier: legacy RAM formula at zero usage pressure;
        // `None` when RAM is undetectable (adapter keeps its fallback).
        let max_chrome_instances = detected.total_ram_bytes.and_then(|total| {
            match derive_max_instances(total, 0, RamThresholds::default()) {
                MaxChromeDecision::Allow(instances) => Some(instances),
                // Unreachable at zero pressure with sane thresholds; the
                // adapter falls back if thresholds are ever misconfigured.
                MaxChromeDecision::Deny => None,
            }
        });

        BudgetModel {
            tiers: BudgetTiers {
                global,
                domain,
                operation: OperationTier {
                    crawl,
                    batch,
                    inference,
                    elastic,
                },
                asset,
                burst,
                max_chrome_instances,
            },
        }
    }

    /// Whole-process ceiling (Global tier).
    #[must_use]
    pub const fn global(&self) -> GlobalConcurrency {
        self.tiers.global
    }

    /// Per-domain session-pool slot count (Domain tier).
    #[must_use]
    pub const fn domain(&self) -> DomainSlots {
        self.tiers.domain
    }

    /// Crawl/scrape-path concurrency (Operation tier).
    #[must_use]
    pub const fn crawl(&self) -> CrawlConcurrency {
        self.tiers.operation.crawl
    }

    /// Batch-processing concurrency (Operation tier).
    #[must_use]
    pub const fn batch(&self) -> BatchConcurrency {
        self.tiers.operation.batch
    }

    /// Inference worker count (Operation tier).
    #[must_use]
    pub const fn inference(&self) -> InferenceWorkers {
        self.tiers.operation.inference
    }

    /// Elastic-ingestion permit count (Operation tier).
    #[must_use]
    pub const fn elastic(&self) -> ElasticPermits {
        self.tiers.operation.elastic
    }

    /// Asset-download concurrency (Asset tier).
    #[must_use]
    pub const fn asset(&self) -> DownloadConcurrency {
        self.tiers.asset
    }

    /// Rate-limiter burst permits (independent knob, Q1 DECOUPLE).
    #[must_use]
    pub const fn burst(&self) -> BurstPermits {
        self.tiers.burst
    }

    /// Governor RAM-derived Chrome-instance cap; `None` when system RAM is
    /// undetectable and the governor must fall back to its legacy path.
    #[must_use]
    pub const fn max_chrome_instances(&self) -> Option<MaxChromeInstances> {
        self.tiers.max_chrome_instances
    }

    /// Test-only convenience preset: a model derived from a fixed 6-core,
    /// RAM-less [`detector::FixedDetector`] snapshot through the canonical
    /// [`BudgetModel::build`] path (auto table for 6 cores ⇒ crawl = 5).
    ///
    /// Unit tests use this instead of inline magic numbers so assertions stay
    /// tied to real derivation rules; it is compiled only under `cfg(test)`
    /// and never ships in the production surface.
    #[cfg(test)]
    #[must_use]
    pub fn for_test_preset() -> BudgetModel {
        let detector = self::detector::FixedDetector::with_detection(
            std::num::NonZeroUsize::new(6)
                .unwrap_or_else(|| unreachable!("preset core count is non-zero")),
            None,
        );
        BudgetModel::build(BudgetOverrides::default(), &detector)
    }
}

#[cfg(test)]
mod tests {
    use super::clamp::clamp_budget;
    use super::*;
    use crate::domain::budget::derivation::CHROME_INSTANCE_COST_BYTES;
    use crate::domain::budget::detector::{FixedDetector, SystemDetector};

    const GB: u64 = 1_000_000_000;

    fn cores(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).expect("test core counts are non-zero")
    }

    fn detector(cores_n: usize, ram: Option<u64>) -> FixedDetector {
        FixedDetector::with_detection(cores(cores_n), ram)
    }

    /// Verbatim replica of TODAY'S `ResourceGovernor::compute_max_instances`
    /// math so drift in either direction fails loudly.
    fn legacy_compute_max_instances(total_ram_bytes: u64) -> u64 {
        let budget = (total_ram_bytes as f64 * 0.6) as u64;
        (budget / CHROME_INSTANCE_COST_BYTES).max(1)
    }

    /// Reference impl of the legacy auto table (same as derivation tests).
    fn legacy_auto_crawl(cores: usize) -> usize {
        let optimal = match cores {
            1 | 2 => 1,
            3 | 4 => 3,
            5..=7 => 5,
            n => (n - 1).min(8),
        };
        clamp_budget(optimal, NonZeroUsize::MIN, MAX_CONCURRENCY_CEILING).get()
    }

    #[test]
    fn default_overrides_reproduce_today_numbers_field_for_field() {
        let model = BudgetModel::build(BudgetOverrides::default(), &detector(4, Some(16 * GB)));
        // Crawl: auto table for 4 cores.
        assert_eq!(model.crawl().get(), 3);
        // Burst: today's effective value == same auto table (Q1).
        assert_eq!(model.burst().get() as usize, 3);
        // Asset downloads: fixed default 3.
        assert_eq!(model.asset().get(), 3);
        // Domain session slots: pool_size 8.
        assert_eq!(model.domain().get(), 8);
        // Batch: CLI --batch-concurrency default 5.
        assert_eq!(model.batch().get(), 5);
        // Inference workers: adaptive-engine default 4.
        assert_eq!(model.inference().get(), 4);
        // Elastic: num_cpus().max(4) through the seam.
        assert_eq!(model.elastic().get(), 4);
        // Global: whole-process ceiling == canonical clamp ceiling.
        assert_eq!(model.global().get(), MAX_CONCURRENCY_CEILING);
        // Governor formula output at zero RAM pressure.
        assert_eq!(
            model.max_chrome_instances(),
            Some(
                MaxChromeInstances::try_from(legacy_compute_max_instances(16 * GB) as usize)
                    .expect("non-zero")
            )
        );
    }

    #[test]
    fn crawl_burst_and_elastic_follow_detector_across_core_counts() {
        for cores_n in [1_usize, 2, 3, 6, 9, 32] {
            let model =
                BudgetModel::build(BudgetOverrides::default(), &detector(cores_n, Some(8 * GB)));
            let expected = legacy_auto_crawl(cores_n);
            assert_eq!(model.crawl().get(), expected, "crawl for {cores_n} cores");
            assert_eq!(
                model.burst().get() as usize,
                expected,
                "burst for {cores_n} cores"
            );
            assert_eq!(
                model.elastic().get(),
                cores_n.max(4),
                "elastic for {cores_n} cores"
            );
            // The other tiers are hardware-independent today.
            assert_eq!(model.asset().get(), 3);
            assert_eq!(model.domain().get(), 8);
            assert_eq!(model.batch().get(), 5);
            assert_eq!(model.inference().get(), 4);
        }
    }

    #[test]
    fn explicit_rate_burst_override_wins_without_moving_other_tiers() {
        let overrides = BudgetOverrides {
            rate_burst: Some(BurstPermits::new(7).expect("7 is non-zero")),
            crawl: None,
            batch: None,
            asset: None,
        };
        let model = BudgetModel::build(overrides, &detector(6, Some(16 * GB)));
        assert_eq!(model.burst().get(), 7);
        // Every other tier stays at its default-derived value.
        assert_eq!(model.crawl().get(), 5);
        assert_eq!(model.asset().get(), 3);
        assert_eq!(model.domain().get(), 8);
    }

    #[test]
    fn governor_tier_is_none_when_ram_undetectable() {
        let model = BudgetModel::build(BudgetOverrides::default(), &detector(4, None));
        assert_eq!(model.max_chrome_instances(), None);
    }

    #[test]
    fn explicit_crawl_override_wins_over_detector_table() {
        // Auto table for 2 cores would yield 1; the operator's explicit
        // value must win verbatim (spec: existing surfaces keep semantics).
        let overrides = BudgetOverrides {
            rate_burst: None,
            crawl: crate::domain::budget::tiers::CrawlConcurrency::new(12).ok(),
            batch: None,
            asset: None,
        };
        let model = BudgetModel::build(overrides, &detector(2, None));
        assert_eq!(model.crawl().get(), 12);
    }

    #[test]
    fn crawl_none_falls_back_to_auto_table() {
        let overrides = BudgetOverrides {
            rate_burst: None,
            crawl: None,
            batch: None,
            asset: None,
        };
        let model = BudgetModel::build(overrides, &detector(6, None));
        assert_eq!(model.crawl().get(), 5);
    }

    #[test]
    fn explicit_batch_and_asset_overrides_win() {
        let overrides = BudgetOverrides {
            rate_burst: None,
            crawl: None,
            batch: crate::domain::budget::tiers::BatchConcurrency::new(9).ok(),
            asset: crate::domain::budget::tiers::DownloadConcurrency::new(6).ok(),
        };
        let model = BudgetModel::build(overrides, &detector(6, None));
        assert_eq!(model.batch().get(), 9);
        assert_eq!(model.asset().get(), 6);
    }

    #[test]
    fn batch_and_asset_none_keep_tier_defaults() {
        let model = BudgetModel::build(BudgetOverrides::default(), &detector(6, None));
        assert_eq!(model.batch().get(), 5);
        assert_eq!(model.asset().get(), 3);
    }

    /// TRIANGULATE: governor tier equals the legacy formula across a RAM
    /// sweep at zero usage pressure.
    #[test]
    fn governor_tier_matches_legacy_formula_across_ram_sweep() {
        for gib in [1_u64, 2, 8, 32] {
            let model =
                BudgetModel::build(BudgetOverrides::default(), &detector(8, Some(gib * GB)));
            assert_eq!(
                model.max_chrome_instances(),
                Some(
                    MaxChromeInstances::try_from(legacy_compute_max_instances(gib * GB) as usize)
                        .expect("non-zero")
                ),
                "{gib} GiB diverges from legacy governor formula"
            );
        }
    }

    /// LIVE-MACHINE equivalence (spec RED requirement): with default overrides
    /// and the real [`SystemDetector`], EVERY tier equals TODAY'S resolved
    /// numbers field-for-field on this machine (SystemDetector reports RAM here,
    /// so the governor tier must be `Some`). Requires the real (sysinfo-backed)
    /// detector, which is cfg-gated out under Miri — excluded there accordingly.
    #[test]
    #[cfg(not(miri))]
    fn system_detector_default_build_matches_todays_resolved_numbers_live() {
        let detector = SystemDetector::new();
        let model = BudgetModel::build(BudgetOverrides::default(), &detector);
        let detected = detector.detect();

        // Crawl: today's `resolve()` output on this machine.
        assert_eq!(
            model.crawl().get(),
            crate::domain::config::ConcurrencyConfig::default().resolve(),
            "live crawl must equal today's resolve()"
        );
        // Burst: today's effective default == the same auto table value (Q1).
        assert_eq!(model.burst().get() as usize, model.crawl().get());
        // Fixed tiers reproduce today's constants verbatim.
        assert_eq!(model.asset().get(), 3);
        assert_eq!(model.domain().get(), 8);
        assert_eq!(model.batch().get(), 5);
        assert_eq!(model.inference().get(), 4);
        // Governor formula output from DETECTED RAM at zero pressure.
        let ram = detected
            .total_ram_bytes
            .expect("SystemDetector reports RAM on real machines");
        assert_eq!(
            model.max_chrome_instances(),
            Some(
                MaxChromeInstances::try_from(legacy_compute_max_instances(ram) as usize)
                    .expect("non-zero")
            )
        );
    }

    /// Immutability: the model is `Copy`; consuming a copy leaves the
    /// original fully usable and identical (no interior mutability).
    #[test]
    fn model_is_copy_and_immutable_after_construction() {
        let model = BudgetModel::build(BudgetOverrides::default(), &detector(4, Some(GB)));
        let copy = model;
        assert_eq!(copy, model);
        assert_eq!(copy.crawl(), model.crawl());
    }
}
