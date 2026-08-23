//! Adversarial stress matrix — Sprint 7-8 P1-conc Phase 3 (change
//! `stabilization-concurrency-budget`, spec Group B).
//!
//! Six scenarios exercising the enforcement mechanisms rewired onto the
//! budget model (PR #896 / squash c0d7a39e), at and beyond their ceilings:
//!
//! 1. **Max-budget saturation** — every tier held at its ceiling under
//!    saturating load; no scope exceeds its tier bound.
//! 2. **Domain contention** — many concurrent workers against few domains;
//!    per-domain slot limits hold, domains stay independent.
//! 3. **Mid-flight cancellation (#509)** — cancel tokens fired while permits
//!    are held/pending; bounded shutdown grace, no permit leaks, partial
//!    JSONL output remains valid.
//! 4. **Backpressure-full channels** — producers outpace the bounded spool
//!    sink; nothing dropped, everything drains.
//! 5. **Mixed Operation/Asset isolation** — simultaneous tier work; no slot
//!    stealing across tiers.
//! 6. **JSONL fan-in corruption-proof (SC4)** — 100×10 writers through ONE
//!    session; exactly 1000 lines, sha256-verified.
//!
//! Determinism notes: cooldown timing uses the injectable [`MockClock`];
//! cancellation grace uses generous real-time bounds (10 s) so CI jitter
//! cannot flip results. No network: all workloads are in-process.
//!
//! Scope note: `ResultsCollector::send` is `pub(crate)` and therefore not
//! reachable from an integration test; its single-writer channel behavior is
//! covered here through the [`JsonlSession`] fan-in scenario (same
//! mpsc-single-writer pattern) and by the engine-level suites.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::Semaphore;

use webfang_core::domain::budget::{
    detector::FixedDetector, tiers, BudgetModel, BudgetOverrides, CrawlConcurrency,
};
use webfang_core::infrastructure::downloader::resource_governor::ResourceGovernor;

/// Build a model with explicit, ceiling-safe overrides so every scenario sees
/// deterministic numbers regardless of the host machine.
fn preset_model(crawl: usize, batch: usize, asset: usize) -> BudgetModel {
    let detector = FixedDetector::with_detection(
        std::num::NonZeroUsize::new(8).expect("preset cores"),
        Some(16 * 1024 * 1024 * 1024),
    );
    let overrides = BudgetOverrides {
        crawl: Some(CrawlConcurrency::new(crawl).expect("crawl > 0")),
        batch: Some(tiers::BatchConcurrency::new(batch).expect("batch > 0")),
        asset: Some(tiers::DownloadConcurrency::new(asset).expect("asset > 0")),
        ..BudgetOverrides::default()
    };
    BudgetModel::build(overrides, &detector)
}

/// Scaffold smoke: the preset model resolves with the explicit overrides and
/// every tier accessor returns the injected value (guards against silent
/// auto-derivation sneaking back into the scenarios below).
#[test]
fn scaffold_preset_model_honors_overrides() {
    let model = preset_model(5, 4, 3);
    assert_eq!(model.crawl().get(), 5);
    assert_eq!(model.batch().get(), 4);
    assert_eq!(model.asset().get(), 3);
}
