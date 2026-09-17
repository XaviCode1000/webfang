//! P0-001 paso-0 verification (issue #1456): fixed-latency mock benchmark.
//!
//! Three curves over synthetic pages (1/2/4/8 pages, each ~393 chunks of
//! identical synthetic content):
//!
//! - Curve A (external reference, NOT measured here): the issue's real
//!   single-session baseline, speedup 1→8 = 1.02× (serialized on the session
//!   `Mutex`). Quoted from the issue for attribution only.
//! - Curve B (measured): FULL per-page `clean()` path through
//!   `MockInferenceEngine` — mock `infer` (45ms sleep) + REAL
//!   chunk/tokenize/score CPU work.
//! - Curve C (measured): FULLY-STUBBED mock — the same N×M task shape (N pages
//!   × M chunk-tasks of 45ms sleep) with chunk/tokenize/score removed, so the
//!   B−C gap attributes the real-CPU overhead and C measures the pure
//!   sleep fan-out ceiling of this test executor.
//!
//! Verdict rule (from the issue):
//! - speedup 1→8 ≈ 8× (linear) → P2-001/002 are downstream of P0-001:
//!   archive them, do NOT touch `export_flow.rs`.
//! - speedup 1→8 ≈ 1× → fan-out/fan-in is an independent cause: report it,
//!   do NOT redesign `export_flow.rs` here (it gets its own MEASURE).
//!
//! Requires the `ai` feature (same gate as the other AI integration tests).

#![cfg(feature = "ai")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::join_all;
use webfang_ai::infrastructure_ai::{MockInferenceEngine, ModelConfig, SemanticCleanerImpl};
use webfang_ai::SemanticCleaner;

#[path = "p0_001_common.rs"]
#[allow(dead_code)]
mod p0_001_common;

/// Fixed per-chunk latency, calibrated against the issue baseline:
/// 393 serial chunks × 45ms ≈ 17.7s (the measured 1-page AI time).
const FIXED_LATENCY: Duration = Duration::from_millis(45);

/// Page-count sweep, mirroring the issue's 1/2/4/8 measurement.
const PAGE_COUNTS: [usize; 4] = [1, 2, 4, 8];

/// Repetitions per cell; the reported wall time is the median.
const REPS: usize = 3;

/// Cleaner wired to the mock engine: full `clean()` path, zero model bytes.
fn mock_cleaner() -> SemanticCleanerImpl<MockInferenceEngine> {
    let engine = Arc::new(MockInferenceEngine::new(FIXED_LATENCY));
    let tokenizer = Arc::new(p0_001_common::in_memory_tokenizer());
    SemanticCleanerImpl::from_parts(engine, tokenizer, ModelConfig::default())
}

/// Median of a non-empty wall-time sample.
fn median_duration(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// Curve B sweep: N identical pages through `join_all(clean)` — the same
/// fan-out shape as `export_flow::clean_all_pages` — with [`REPS`] repetitions
/// per cell, reporting the median. Mock `infer` + real pipeline CPU work.
async fn sweep_curve_b(
    cleaner: &SemanticCleanerImpl<MockInferenceEngine>,
    chunks_per_page: usize,
) -> Vec<Duration> {
    let mut medians = Vec::with_capacity(PAGE_COUNTS.len());
    for &pages in &PAGE_COUNTS {
        let html = p0_001_common::synthetic_page();
        let mut samples = Vec::with_capacity(REPS);
        for _ in 0..REPS {
            let urls: Vec<String> = (0..pages)
                .map(|i| format!("https://example.com/paso-0-p{i}"))
                .collect();
            let started = Instant::now();
            let results = join_all(urls.iter().map(|url| cleaner.clean(url.as_str(), &html))).await;
            samples.push(started.elapsed());
            for (i, result) in results.iter().enumerate() {
                let chunks = result
                    .as_ref()
                    .unwrap_or_else(|e| panic!("mock clean of page {i} (N={pages}) failed: {e}"));
                assert_eq!(
                    chunks.len(),
                    chunks_per_page,
                    "identical pages must yield identical chunk counts"
                );
            }
        }
        medians.push(median_duration(samples));
    }
    medians
}

/// One fully-stubbed page: M concurrent sleeps, zero CPU work. Keeps the N×M
/// task shape of curve B (N pages × M chunk-tasks) so curve C isolates the
/// pure sleep fan-out from real pipeline CPU work.
async fn stub_page(chunk_count: usize) {
    join_all((0..chunk_count).map(|_| tokio::time::sleep(FIXED_LATENCY))).await;
}

/// Curve C sweep: same page counts and [`REPS`] medians as curve B, but every
/// page is [`stub_page`] — no chunk/tokenize/score, only the sleep fan-out.
async fn sweep_curve_c(chunk_count: usize) -> Vec<Duration> {
    let mut medians = Vec::with_capacity(PAGE_COUNTS.len());
    for &pages in &PAGE_COUNTS {
        let mut samples = Vec::with_capacity(REPS);
        for _ in 0..REPS {
            let started = Instant::now();
            join_all((0..pages).map(|_| stub_page(chunk_count))).await;
            samples.push(started.elapsed());
        }
        medians.push(median_duration(samples));
    }
    medians
}

/// Report one curve: wall time per page-count + speedup vs 1 page, where
/// speedup(N) = (T1 × N) / TN (the issue's convention: serial ⇒ 1×,
/// perfectly parallel ⇒ N×). Returns speedup 1→8.
fn report_curve(name: &str, wall_times: &[Duration]) -> f64 {
    let t1 = wall_times[0].as_secs_f64();
    eprintln!("{name} (fixed_latency={FIXED_LATENCY:?}, reps={REPS} median):");
    eprintln!("| pages | wall time | speedup vs 1 |");
    eprintln!("|---|---|---|");
    let mut speedup_8 = 0.0;
    for (&pages, elapsed) in PAGE_COUNTS.iter().zip(wall_times.iter()) {
        let secs = elapsed.as_secs_f64();
        let speedup = (t1 * pages as f64) / secs;
        if pages == 8 {
            speedup_8 = speedup;
        }
        eprintln!("| {pages} | {secs:.3}s | {speedup:.2}x |");
    }
    speedup_8
}

/// Paso-0 benchmark: sweep 1/2/4/8 pages through curves B and C and record
/// the timing tables + speedups. Multi-thread runtime is REQUIRED: per-page
/// chunk/tokenize CPU work must parallelize across pages exactly like the
/// production Tokio runtime, otherwise serial CPU overhead alone would fake a
/// ~1× speedup on a current-thread executor. `worker_threads = 8` is FIXED:
/// the 8-worker suspicion is read from the curve shape (B vs C gap), and
/// varying the executor would add a fourth variable to the three curves.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn mock_fixed_latency_scales_linearly() {
    let cleaner = mock_cleaner();
    assert!(cleaner.is_ready(), "mock cleaner must report ready");

    // Self-calibration: chunk count of one synthetic page (identical for all
    // pages since the content is byte-identical). Curve C reuses this count
    // so its N×M task shape matches curve B exactly.
    let probe_page = p0_001_common::synthetic_page();
    let probe = cleaner
        .clean("https://example.com/paso-0", &probe_page)
        .await
        .expect("mock clean of the probe page must succeed");
    let chunks_per_page = probe.len();
    assert!(
        !probe.is_empty(),
        "synthetic page must yield chunks (got 0 — chunker/pruner ate the fixture)"
    );
    for chunk in &probe {
        let embedding = chunk
            .embeddings
            .as_ref()
            .expect("mock path must preserve embeddings");
        assert_eq!(embedding.len(), 384, "mock embedding must be 384-dim");
    }
    eprintln!("paso-0 calibration: {chunks_per_page} chunks per synthetic page");

    let wall_b = sweep_curve_b(&cleaner, chunks_per_page).await;
    let wall_c = sweep_curve_c(chunks_per_page).await;

    let speedup_b_8 = report_curve("paso-0 curve B (mock infer + real pipeline)", &wall_b);
    let speedup_c_8 = report_curve("paso-0 curve C (fully stubbed sleep fan-out)", &wall_c);
    eprintln!(
        "paso-0 verdict input: curve B speedup 1→8 = {speedup_b_8:.2}x, \
         curve C speedup 1→8 = {speedup_c_8:.2}x \
         (≈8x ⇒ P2-001/002 downstream; ≈1x ⇒ independent cause; \
         curve A reference from the issue: real single-session 1.02x)"
    );

    // B−C attribution: both curves run the same N×M sleeps on the same
    // executor, so the 1-page wall gap is the real per-page CPU overhead
    // (chunk/tokenize/score/prune) — MEASURED here as a median difference,
    // never cited as a per-phase profile.
    let overhead_per_page_ms = (wall_b[0].as_secs_f64() - wall_c[0].as_secs_f64()) * 1000.0;
    eprintln!(
        "paso-0 overhead attribution: curve B 1-page median {:.3}s − curve C 1-page median {:.3}s \
         = {overhead_per_page_ms:.1}ms/page of real CPU work (median difference, {REPS} reps)",
        wall_b[0].as_secs_f64(),
        wall_c[0].as_secs_f64(),
    );

    // Guard against accidental serialization of the mock path (e.g. a mutex
    // sneaking back in): the parallel ceiling is curve C (pure fan-out), the
    // serial floor is 1×. DECISION (assert branch: machine-relative B/C ≥ 0.3,
    // REPLACING the retired 3.0 absolute floor): the absolute floor proved
    // unportable — 16-core workstation B = 4.13× vs CI-runner B = 2.89× (PR
    // #1457 Coverage job), while curve C is rock-stable (7.68–7.74 local,
    // 7.73 CI). Per-point B/C decays with N on both runners (local
    // 1.00/0.91/0.75/0.54, CI ending 0.37 at 8 pages), so an absolute floor
    // pins an executor-shaped number instead of serialization-freedom and
    // turns runner noise into red CI. The ratio normalizes that out: curve C
    // is the anchor for what this executor can fan out, and B/C measures how
    // much of that ceiling the real pipeline keeps. Observed ratio 0.37–0.54;
    // a serialized path would give ~1×/8× ≈ 0.125 — so 0.3 has margin on both
    // sides (real serialization lands in ~0.125 territory, a degraded runner
    // stays above 0.3 while its absolute B sags).
    // gap B−C (~16.6ms) no descompuesto por fase; ver curva 1/2/4/8 y ratio
    // B/C decreciente en docs/p0-001-n-decision.md.
    let ratio = speedup_b_8 / speedup_c_8;
    let cores = std::thread::available_parallelism()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    eprintln!("paso-0 ratio verdict: B/C = {ratio:.2} (B = {speedup_b_8:.2}x, C = {speedup_c_8:.2}x, cores = {cores})");
    assert!(
        ratio >= 0.3,
        "mock fan-out must parallelize (B/C ratio = {ratio:.2}, curve B speedup 1→8 = {speedup_b_8:.2}x, \
         curve C speedup 1→8 = {speedup_c_8:.2}x, cores = {cores}; \
         ratio ≈ 0.125 territory means the mock path itself serializes, \
         ratio in [0.3, 0.37) with sagging absolutes means a degraded runner — \
         see docs/p0-001-n-decision.md)"
    );
}
