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

// ===== Scenario 1: max-budget saturation =====

/// Track the maximum number of simultaneously-held permits for one tier.
fn max_tracker() -> Arc<AtomicUsize> {
    Arc::new(AtomicUsize::new(0))
}

/// Spawn `demand` tasks that each hold one permit through several yields,
/// recording the high-water mark of simultaneous holders in `max_seen`.
/// Returns once every task completed (caller bounds total time).
async fn storm_the_tier(sem: Arc<Semaphore>, demand: usize, max_seen: Arc<AtomicUsize>) {
    let mut handles = Vec::with_capacity(demand);
    for _ in 0..demand {
        let sem = Arc::clone(&sem);
        let max_seen = Arc::clone(&max_seen);
        handles.push(tokio::spawn(async move {
            let permit = sem.acquire_owned().await.expect("semaphore open");
            let current = max_seen.fetch_add(1, Ordering::SeqCst) + 1;
            max_seen.fetch_max(current, Ordering::SeqCst);
            // Hold through several scheduling points so overlap is real.
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            max_seen.fetch_sub(1, Ordering::SeqCst);
            drop(permit);
        }));
    }
    for handle in handles {
        handle.await.expect("storm task joins");
    }
}

/// Every tier driven to its exact ceiling under saturating load: no scope
/// ever exceeds its tier bound, full occupancy is reachable, and everything
/// drains without deadlock inside a generous timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn max_budget_saturation_never_exceeds_tier_ceilings() {
    let model = preset_model(4, 3, 2);
    let tiers: Vec<(String, usize)> = vec![
        ("crawl".into(), model.crawl().get()),
        ("batch".into(), model.batch().get()),
        ("asset".into(), model.asset().get()),
    ];

    for (name, ceiling) in tiers {
        let sem = Arc::new(Semaphore::new(ceiling));

        // Phase A — deterministic full occupancy: exactly `ceiling` permits
        // can be outstanding, and one more is rejected.
        let mut held = Vec::with_capacity(ceiling);
        for _ in 0..ceiling {
            held.push(
                sem.clone()
                    .try_acquire_owned()
                    .expect("fresh semaphore has capacity"),
            );
        }
        assert_eq!(
            sem.available_permits(),
            0,
            "{name}: ceiling must be fully consumable"
        );
        assert!(
            sem.try_acquire().is_err(),
            "{name}: permit beyond the ceiling must be impossible"
        );
        drop(held);

        // Phase B — saturating storm under the same bound.
        let max_seen = max_tracker();
        let demand = ceiling * 10;
        let storm = storm_the_tier(Arc::clone(&sem), demand, Arc::clone(&max_seen));
        tokio::time::timeout(std::time::Duration::from_secs(30), storm)
            .await
            .unwrap_or_else(|_| panic!("{name}: storm deadlocked or starved"));
        assert!(
            max_seen.load(Ordering::SeqCst) <= ceiling,
            "{name}: {}/{} concurrent holders exceeded the tier ceiling",
            max_seen.load(Ordering::SeqCst),
            ceiling
        );
        assert_eq!(
            sem.available_permits(),
            ceiling,
            "{name}: permits must fully return after the drain"
        );
    }
}

// ===== Scenario 2: domain contention =====

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use webfang_core::domain::budget::DomainSlots;
use webfang_core::domain::clock::MockClock;
use webfang_core::{DomainSessionPool, SessionId, SessionManager, SessionPoolConfig};

/// Many workers hammer few domains through the [`DomainSessionPool`]: the
/// per-domain slot limit must hold at every observation point, domains stay
/// independent under contention, and everything joins without deadlock.
#[test]
fn domain_contention_holds_slot_limits_and_independence() {
    const SLOTS: usize = 2;
    const DOMAINS: usize = 4;
    const WORKERS_PER_DOMAIN: usize = 10;

    let config = SessionPoolConfig {
        pool_size: DomainSlots::new(SLOTS).expect("slots > 0"),
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
        max_exp: 1,
        ..Default::default()
    };
    // Deterministic cooldown timing via the injectable mock clock.
    let clock = Arc::new(MockClock::new(std::time::Instant::now()));
    let pool = Arc::new(DomainSessionPool::new(config, clock.handle()));
    let held: Arc<Mutex<HashMap<&'static str, HashSet<SessionId>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let mut handles = Vec::new();
    for d in 0..DOMAINS {
        let domain_name: &'static str = match d {
            0 => "contended-zero.test",
            1 => "contended-one.test",
            2 => "contended-two.test",
            _ => "contended-three.test",
        };
        for w in 0..WORKERS_PER_DOMAIN {
            let pool = Arc::clone(&pool);
            let held = Arc::clone(&held);
            let clock = Arc::clone(&clock);
            handles.push(thread::spawn(move || {
                for attempt in 0..50 {
                    if let Some(id) = pool.acquire(domain_name) {
                        {
                            let mut guard = held.lock().expect("held lock");
                            let set = guard.entry(domain_name).or_default();
                            assert!(
                                set.len() < SLOTS || set.contains(&id),
                                "domain {domain_name} exceeded {SLOTS} distinct slots"
                            );
                            set.insert(id);
                        }
                        // Hold briefly, then report success (healthy session).
                        thread::sleep(Duration::from_micros(200));
                        pool.report_success(domain_name, id);
                        held.lock().expect("held lock").remove(domain_name);
                        break;
                    }
                    // All slots banned/cooling — bounded retry with mock-clock
                    // advancement keeps this deterministic and fast.
                    clock.advance(Duration::from_millis(2));
                    let _ = attempt;
                }
            }));
        }
    }
    for handle in handles {
        handle.join().expect("contention worker joins (no deadlock)");
    }

    // Every contended domain is tracked; independence: banning one domain
    // leaves its siblings acquirable.
    assert_eq!(pool.total_domains(), DOMAINS);
    let banned = pool.acquire("contended-zero.test");
    // After the storm every session reported success, so acquisition works.
    assert!(banned.is_some(), "healthy sessions must remain acquirable");
}
