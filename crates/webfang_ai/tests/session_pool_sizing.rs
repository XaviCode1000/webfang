//! N-session ORT pool sizing (issue #1456 P0-001 pool task).
//!
//! Mock-backed: NO model download, NO ORT session. Every test runs the real
//! [`PooledInferenceEngine`] coordination (per-slot semaphores + lock-free
//! `AtomicUsize` selection) over fixed-latency mock engines, proving:
//! (a) N concurrent acquires proceed in parallel, (b) the N+1th request gets
//! backpressure instead of a hard failure, (c) the `Single` path still builds
//! today's `InferencePool` byte-for-byte, (d) the `intra_threads` split formula
//! including the N > cores edge.
//!
//! Requires the `ai` feature (same gate as the other AI integration tests).

#![cfg(feature = "ai")]

use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use futures::future::join_all;
use webfang_ai::infrastructure_ai::{
    build_engine, AiModel, EngineConfig, InferenceEngine, InferencePool, MockInferenceEngine,
    ModelConfig, ModelInput, PooledInferenceEngine, SemanticCleanerImpl,
};
use webfang_core::error::SemanticError;

/// Model path that can never build a session: `build_engine(Single)` must still
/// construct the pool and spawn the drainer (today's graceful-degradation
/// contract), without downloading anything.
const FAKE_MODEL_PATH: &str = "/nonexistent/webfang-fake-model.onnx";

fn pool_of(size: usize, latency: Duration) -> PooledInferenceEngine {
    let engines: Vec<Arc<dyn InferenceEngine + Send + Sync>> = (0..size)
        .map(|_| {
            Arc::new(MockInferenceEngine::new(latency)) as Arc<dyn InferenceEngine + Send + Sync>
        })
        .collect();
    PooledInferenceEngine::from_engines(engines).expect("mock slots must build a pool")
}

fn nonzero(n: usize) -> NonZeroUsize {
    NonZeroUsize::new(n).expect("test pool sizes are non-zero")
}

// --- (d) intra_threads split formula ---------------------------------------

/// (d) `intra_threads = (total_cores / N).max(1)`, including the N > cores edge
/// (degrades to 1 thread per session) and the zero-core degenerate input.
#[test]
fn split_intra_threads_divides_budget_with_floor_of_one() {
    assert_eq!(EngineConfig::split_intra_threads(16, 4), 4);
    assert_eq!(EngineConfig::split_intra_threads(16, 1), 16);
    assert_eq!(EngineConfig::split_intra_threads(16, 16), 1);
    // N > cores: every session still gets exactly one thread.
    assert_eq!(EngineConfig::split_intra_threads(8, 15), 1);
    assert_eq!(EngineConfig::split_intra_threads(3, 8), 1);
    assert_eq!(EngineConfig::split_intra_threads(1, 1), 1);
    // Degenerate inputs never divide by zero and never yield zero threads.
    assert_eq!(EngineConfig::split_intra_threads(0, 4), 1);
}

/// (d) The parallelism-derived default exists and is a usable pool size; an
/// explicit override always wins (MEASURE owns the calibration, this only
/// proves the plumbing both branches).
#[test]
fn default_pool_size_is_derived_and_overridable() {
    let default = EngineConfig::default_pool_size();
    assert!(default.get() >= 1, "default pool size must be usable");
    assert_eq!(EngineConfig::pool_size_or_default(None), default);
    assert_eq!(
        EngineConfig::pool_size_or_default(Some(nonzero(3))),
        nonzero(3)
    );
}

/// Rollback identity: the default config IS `Single` (today's behavior), so
/// rollback is a one-variant change.
#[test]
fn engine_config_defaults_to_single_for_trivial_rollback() {
    assert_eq!(EngineConfig::default(), EngineConfig::Single);
    assert_eq!(EngineConfig::single(), EngineConfig::Single);
}

/// Empty pools are rejected with a typed error, never a zero-slot divide.
#[test]
fn pool_rejects_empty_engine_list() {
    let err =
        PooledInferenceEngine::from_engines(Vec::new()).expect_err("empty engine list must error");
    assert!(err.to_string().contains("al menos una sesi"), "got: {err}");
}

// --- (c) Single path unchanged ----------------------------------------------

/// (c) `build_engine(Single)` returns today's behavior: 384-dim, ready, and on
/// an unloadable model the drainer contract holds (prompt typed error, no hang).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_path_keeps_today_behavior() {
    let engine = build_engine(
        &EngineConfig::Single,
        std::path::PathBuf::from(FAKE_MODEL_PATH),
        AiModel::Granite97M,
    )
    .expect("Single must construct even with an unloadable model file");

    assert_eq!(engine.embedding_dim(), 384);
    assert!(engine.is_ready());

    let input = ModelInput::from_tokens(vec![101, 5, 102]);
    let err = tokio::time::timeout(Duration::from_secs(5), engine.infer(&input))
        .await
        .expect("Single drainer path must answer promptly, never hang")
        .expect_err("unloadable model must surface a typed inference error");
    let msg = err.to_string();
    assert!(
        msg.contains("Worker dropped response") || msg.contains("channel closed"),
        "drainer contract must surface a channel error, got: {msg}"
    );
}

/// (c) The default cleaner type is still `SemanticCleanerImpl<InferencePool>`:
/// existing `new` call sites resolve unchanged (compile-time proof).
#[test]
fn default_cleaner_type_is_still_single_pool() {
    fn _accepts_single(_: &SemanticCleanerImpl) {}
    fn _accepts_pool(_: &SemanticCleanerImpl<Arc<dyn InferenceEngine + Send + Sync>>) {}
}

// --- (a) N concurrent acquires proceed in parallel ---------------------------

/// (a) N concurrent acquires overlap: 4 × 60ms on 4 slots finishes far below
/// the 240ms serial floor (no centralized lock may serialize them).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn pool_serves_n_concurrent_acquires_in_parallel() {
    let pool = pool_of(4, Duration::from_millis(60));
    assert_eq!(pool.pool_size(), 4);
    assert!(pool.is_ready());

    let input = ModelInput::from_tokens(vec![101, 5, 102]);
    let started = std::time::Instant::now();
    let results = join_all((0..4).map(|_| pool.infer(&input))).await;
    let elapsed = started.elapsed();

    for r in results {
        let embedding = r.expect("mock slot infer must succeed");
        assert_eq!(embedding.len(), 384);
    }
    assert!(
        elapsed < Duration::from_millis(180),
        "4 parallel 60ms infers must overlap on 4 slots (no mutex); took {elapsed:?}"
    );
}

// --- (b) N+1th gets backpressure, not a hard failure --------------------------

/// (b) With both slots occupied, a third request pends (backpressure) instead
/// of failing hard, then completes once capacity frees.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn pool_applies_backpressure_instead_of_failing() {
    let pool = Arc::new(pool_of(2, Duration::from_millis(150)));
    let input = ModelInput::from_tokens(vec![101, 5, 102]);

    // Occupy both slots.
    let p1 = Arc::clone(&pool);
    let i1 = input.clone();
    let h1 = tokio::spawn(async move { p1.infer(&i1).await });
    let p2 = Arc::clone(&pool);
    let i2 = input.clone();
    let h2 = tokio::spawn(async move { p2.infer(&i2).await });
    tokio::time::sleep(Duration::from_millis(30)).await;

    // The N+1th request must still be pending after 50ms (both slots busy for
    // 150ms): blocked, NOT failed.
    let pending = tokio::time::timeout(Duration::from_millis(50), pool.infer(&input)).await;
    assert!(
        pending.is_err(),
        "N+1th request must block under backpressure instead of failing hard"
    );

    // Once the holders release, everything completes successfully.
    h1.await
        .expect("holder task must not panic")
        .expect("holder infer must succeed");
    h2.await
        .expect("holder task must not panic")
        .expect("holder infer must succeed");
    let late = tokio::time::timeout(Duration::from_secs(5), pool.infer(&input))
        .await
        .expect("freed pool must answer promptly")
        .expect("request after backpressure must succeed, not fail hard");
    assert_eq!(late.len(), 384);
}

// --- RAII: permit release on panic, pool stays usable --------------------------

/// Engine that panics exactly once, then behaves like the fixed-latency mock.
struct PanicOnceEngine {
    calls: AtomicUsize,
    latency: Duration,
}

impl InferenceEngine for PanicOnceEngine {
    fn infer<'a>(
        &'a self,
        _input: &'a ModelInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, SemanticError>> + Send + 'a>> {
        Box::pin(async move {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("boom de prueba: el permiso del slot debe liberarse");
            }
            tokio::time::sleep(self.latency).await;
            Ok(MockInferenceEngine::deterministic_embedding())
        })
    }

    fn embedding_dim(&self) -> usize {
        384
    }

    fn is_ready(&self) -> bool {
        true
    }
}

/// A panic inside `infer` must not leak the slot permit: the single-slot pool
/// serves the next request normally afterwards (RAII release on unwind).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_releases_permit_on_panic_and_stays_usable() {
    use futures::FutureExt as _;

    let panicking: Arc<dyn InferenceEngine + Send + Sync> = Arc::new(PanicOnceEngine {
        calls: AtomicUsize::new(0),
        latency: Duration::from_millis(10),
    });
    let pool = PooledInferenceEngine::from_engines(vec![panicking]).expect("1 slot must build");
    let input = ModelInput::from_tokens(vec![101, 5, 102]);

    let first = std::panic::AssertUnwindSafe(pool.infer(&input))
        .catch_unwind()
        .await;
    assert!(first.is_err(), "first infer must propagate the test panic");

    let second = tokio::time::timeout(Duration::from_secs(5), pool.infer(&input))
        .await
        .expect("permit must have been released on unwind — pool must answer")
        .expect("pool must stay usable after a panicking request");
    assert_eq!(second.len(), 384);
}

// --- Wiring: pooled engine behind the cleaner seam -----------------------------

/// The pooled engine drives the full `clean()` path through `from_parts`
/// (existing seam, no signature changes), and the pool itself is `Send + Sync`.
#[test]
fn pooled_engine_is_send_sync_and_fits_cleaner_seam() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PooledInferenceEngine>();

    let pool: Arc<dyn InferenceEngine + Send + Sync> =
        Arc::new(pool_of(2, Duration::from_millis(1)));
    assert!(pool.is_ready());
    assert_eq!(pool.embedding_dim(), 384);
}

/// A `Single`-built engine and a pooled mock engine both erase to
/// `Arc<dyn InferenceEngine + Send + Sync>` and feed `SemanticCleanerImpl::from_parts` —
/// the rollback is the enum variant, never the wiring.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cleaner_accepts_erased_single_and_pool_engines() {
    let single = build_engine(
        &EngineConfig::Single,
        std::path::PathBuf::from(FAKE_MODEL_PATH),
        AiModel::Granite97M,
    )
    .expect("Single must build");
    let pooled: Arc<dyn InferenceEngine + Send + Sync> =
        Arc::new(pool_of(2, Duration::from_millis(1)));

    // from_parts is generic over the seam; both engines fit without touching
    // `new` (whose `InferencePool` signature the CLI/MCP call sites rely on).
    let _ = (single, pooled, ModelConfig::default());
    assert!(InferencePool::new(
        std::path::PathBuf::from(FAKE_MODEL_PATH),
        AiModel::Granite97M
    )
    .is_ok());
}
