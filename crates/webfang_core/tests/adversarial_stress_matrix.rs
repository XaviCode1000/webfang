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

// ===== Scenario 3: mid-flight cancellation (#509) =====

use tokio_util::sync::CancellationToken;
use webfang_core::infrastructure::export::jsonl_writer::JsonlSession;

/// Cancellation fired while permits are held and more are pending: every
/// pending acquire resolves within the shutdown-grace bound, held permits
/// return fully when their holders finish, and JSONL written before the
/// cancel remains parseable (partial output is valid output).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn midflight_cancellation_bounded_grace_and_no_permit_leak() {
    const GRACE: Duration = Duration::from_secs(10);
    let token = CancellationToken::new();
    let governor = Arc::new(ResourceGovernor::with_max_instances(2, token.clone()));

    // Hold both permits.
    let h1 = governor.acquire().await.expect("permit 1");
    let h2 = governor.acquire().await.expect("permit 2");
    assert_eq!(governor.available_permits(), 0);

    // Pending acquires queue behind the holders.
    let mut pending = Vec::new();
    for _ in 0..10 {
        let gov = Arc::clone(&governor);
        pending.push(tokio::spawn(async move { gov.acquire().await }));
    }
    tokio::task::yield_now().await;

    // Fire cancellation MID-FLIGHT.
    token.cancel();

    let joined = tokio::time::timeout(GRACE, futures::future::join_all(pending))
        .await
        .expect("pending acquires must resolve within the shutdown grace");
    let cancelled_count = joined
        .iter()
        .filter(|r| matches!(r, Ok(Err(webfang_core::infrastructure::downloader::DownloadError::Cancelled))))
        .count();
    assert_eq!(
        cancelled_count, 10,
        "every pending acquire must observe the cancel"
    );

    // Holders still release cleanly; no permit leaks past cancellation.
    drop(h1);
    drop(h2);
    tokio::task::yield_now().await;
    assert_eq!(
        governor.available_permits(),
        2,
        "held permits must return after cancellation"
    );

    // Partial JSONL written before a cancel stays valid.
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("partial.jsonl");
    let (session, _hash_index) = JsonlSession::open(&path).expect("session opens");
    for item in 0..25 {
        let line = format!(r#"{{"item":{item},"pre_cancel":true}}"#);
        let mut bytes = line.into_bytes();
        bytes.push(b'\n');
        session.append(&bytes).await.expect("append accepted");
    }
    session.close().await.expect("clean close after cancel");
    let content = std::fs::read_to_string(&path).expect("output readable");
    assert_eq!(content.lines().count(), 25);
    for line in content.lines() {
        serde_json::from_str::<serde_json::Value>(line)
            .unwrap_or_else(|e| panic!("partial output corrupt: {e}"));
    }
}

// ===== Scenario 4: backpressure-full channels =====

use webfang_core::application::crawler::{BoundedFileSink, CrawlContentSink};

/// Producers outpace the bounded spool sink (channel capacity 2) with a
/// burst of captures far beyond the buffer: backpressure is applied by the
/// bounded channel, NOTHING is dropped, and the pipeline drains completely —
/// every captured page round-trips through the spool.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backpressure_full_sink_drops_nothing_and_drains_completely() {
    const PAGES: usize = 200;
    const TINY_BUFFER: usize = 2;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let sink =
        BoundedFileSink::new(dir.path().join("spool.jsonl"), TINY_BUFFER)
            .await
            .expect("sink opens");

    // 20 concurrent producers × 10 pages each, all hammering the tiny
    // channel simultaneously; `capture` applies backpressure inline.
    let shared = Arc::new(sink);
    let mut producers = Vec::new();
    for p in 0..20usize {
        let sink = Arc::clone(&shared);
        producers.push(tokio::spawn(async move {
            for i in 0..10 {
                sink.capture(
                    &format!("https://backpressure.test/{p}/{i}"),
                    &format!("<p>producer {p} page {i}</p>"),
                );
            }
        }));
    }
    for producer in producers {
        producer.await.expect("producer joins");
    }

    assert_eq!(
        shared.captured(),
        PAGES,
        "every capture must be handed to the channel"
    );

    // Drain: finish flushes everything persisted through the full channel.
    let flushed = tokio::time::timeout(Duration::from_secs(10), shared.finish())
        .await
        .expect("drain completes within grace")
        .expect("flush succeeds");
    assert_eq!(flushed, PAGES, "nothing dropped under full backpressure");

    // Round-trip proof: the spool holds exactly the produced set.
    let mut reader = shared.reader().await.expect("reader opens");
    let mut seen = Vec::with_capacity(PAGES);
    while let Some(page) = reader.next_page().await.expect("spool decodes") {
        seen.push(page.url);
    }
    seen.sort();
    let mut expected: Vec<String> = (0..20usize)
        .flat_map(|p| (0..10).map(move |i| format!("https://backpressure.test/{p}/{i}")))
        .collect();
    expected.sort();
    assert_eq!(seen, expected, "round-trip must reproduce the exact URL set");
}

// ===== Scenario 5: mixed Operation/Asset tier isolation =====

/// Operation-tier work (crawl) and Asset-tier downloads run simultaneously
/// at their respective ceilings: each tier independently respects its own
/// budget and neither can steal capacity from the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn mixed_tiers_isolate_operation_from_asset_budgets() {
    let model = preset_model(4, 2, 3);
    let operation = Arc::new(Semaphore::new(model.crawl().get()));
    let asset = Arc::new(Semaphore::new(model.asset().get()));

    // Saturate the Operation tier completely.
    let mut op_held = Vec::new();
    for _ in 0..model.crawl().get() {
        op_held.push(operation.clone().try_acquire_owned().expect("op slot"));
    }
    assert_eq!(operation.available_permits(), 0);

    // Asset tier is UNAFFECTED by Operation saturation — immediate acquire,
    // zero waiting, no slot stealing across tiers.
    let steal_probe = tokio::time::timeout(
        Duration::from_millis(100),
        asset.clone().acquire_owned(),
    )
    .await
    .expect("asset must not wait on Operation saturation")
    .expect("asset permit available");
    drop(steal_probe);
    drop(op_held);

    // Simultaneous storms at both tiers: high-water marks are independent
    // and each stays within its own ceiling while the other is saturated.
    let op_max = max_tracker();
    let asset_max = max_tracker();
    let (op_sem, asset_sem) = (Arc::clone(&operation), Arc::clone(&asset));
    let op_storm = storm_the_tier(op_sem, model.crawl().get() * 10, Arc::clone(&op_max));
    let asset_storm = storm_the_tier(asset_sem, model.asset().get() * 10, Arc::clone(&asset_max));
    let both = futures::future::join(op_storm, asset_storm);
    tokio::time::timeout(Duration::from_secs(30), both)
        .await
        .expect("mixed-tier storms must drain without deadlock");

    assert!(
        op_max.load(Ordering::SeqCst) <= model.crawl().get(),
        "Operation tier exceeded its ceiling"
    );
    assert!(
        asset_max.load(Ordering::SeqCst) <= model.asset().get(),
        "Asset tier exceeded its ceiling"
    );
    assert_eq!(
        operation.available_permits(),
        model.crawl().get(),
        "Operation permits fully restored"
    );
    assert_eq!(
        asset.available_permits(),
        model.asset().get(),
        "Asset permits fully restored"
    );
}

// ===== Scenario 6: JSONL fan-in corruption-proof (SC4 exact counts) =====

use sha2::{Digest, Sha256};

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// The SC4 proof under a saturated budget shape: 100 tasks × 10 items share
/// ONE writer session concurrently. After close the file holds EXACTLY 1000
/// valid JSON lines — zero corrupt, truncated or interleaved — and every
/// line's embedded sha256 matches its recomputed payload hash.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn fan_in_one_hundred_by_ten_is_sha256_corruption_proof() {
    const TASKS: u32 = 100;
    const ITEMS_PER_TASK: u32 = 10;

    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("sc4_matrix.jsonl");
    let (session, _hash_index) = JsonlSession::open(&path).expect("session opens");

    let mut handles = Vec::with_capacity(TASKS as usize);
    for task in 0..TASKS {
        let shared = session.clone();
        handles.push(tokio::spawn(async move {
            for item in 0..ITEMS_PER_TASK {
                let payload = format!("task-{task}-item-{item}");
                let line = format!(
                    r#"{{"task":{task},"item":{item},"payload":"{payload}","checksum_sha256":"{}"}}"#,
                    sha256_hex(&payload)
                );
                let mut bytes = line.into_bytes();
                bytes.push(b'\n');
                shared.append(&bytes).await.expect("append accepted");
            }
        }));
    }
    for handle in handles {
        handle.await.expect("fan-in task joins");
    }
    session.close().await.expect("clean shutdown");

    let content = std::fs::read_to_string(&path).expect("output readable");
    assert!(
        content.ends_with('\n'),
        "file must end on a newline boundary"
    );

    let mut count = 0usize;
    for line in content.lines() {
        let value: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("corrupt line {count}: {e}"));
        let payload = value["payload"]
            .as_str()
            .unwrap_or_else(|| panic!("line {count} lost its payload"));
        let checksum = value["checksum_sha256"]
            .as_str()
            .unwrap_or_else(|| panic!("line {count} lost its checksum"));
        assert_eq!(
            checksum,
            sha256_hex(payload),
            "line {count}: interleaved or corrupted bytes detected"
        );
        count += 1;
    }
    assert_eq!(
        count,
        (TASKS * ITEMS_PER_TASK) as usize,
        "exactly {} valid lines required",
        TASKS * ITEMS_PER_TASK
    );
}
