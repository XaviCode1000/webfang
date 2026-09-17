# P0-001 N-decision: mock evidence, P2 archival, and what stays pending (issue #1456)

**Status: N-decision PENDING the release sweep.** The mock evidence below settles the
downstream question (P2-001/002) and the assert-floor question. It does NOT select N —
that requires the real-model release sweep, which was deliberately not run here.

## Verdicts (read this, skip the rest if in a hurry)

| Question | Verdict |
|---|---|
| Are P2-001/002 downstream of P0-001? | **Yes — ARCHIVED as downstream.** Curve C hits 7.68×, so the executor fans out cleanly; curve B's gap is CPU-shaped, not a second serialization. `export_flow.rs` stays untouched. |
| Assert floor: raise or keep 3.0? | **KEEP 3.0.** Curve B (4.13×) passes with margin; the floor pins serialization-freedom, not throughput. Raising toward curve C would turn runner noise into red CI. |
| Micro-batching vs Pool{N}? | **OPEN — needs the release sweep.** The harness-only `batch` cell exists and compiles; no numbers yet. |
| N (pool size)? | **PENDING the release sweep (not run).** Criterion stands: minimum N with speedup(8) ≥ 6.0× inside the ops RSS budget. |
| Circuit breaker? | **Explicitly DEFERRED, not forgotten.** Re-open trigger below. |

## Mock evidence (measured 2026-09-17, 16-core workstation, REPS=3 median)

Command: `cargo test -p webfang_ai --features ai --test mock_inference_benchmark -- --nocapture`

### Curve B — mock infer + real pipeline (`clean()` via `MockInferenceEngine`, 45ms/chunk, 400 chunks/page)

| pages | wall time | speedup vs 1 |
|---|---|---|
| 1 | 0.063s | 1.00× |
| 2 | 0.070s | 1.81× |
| 4 | 0.086s | 2.94× |
| 8 | 0.122s | **4.13×** |

### Curve C — fully-stubbed sleep fan-out (same N×M task shape, zero CPU work)

| pages | wall time | speedup vs 1 |
|---|---|---|
| 1 | 0.046s | 1.00× |
| 2 | 0.047s | 1.99× |
| 4 | 0.047s | 3.94× |
| 8 | 0.048s | **7.68×** |

### Curve A — external reference (NOT measured here)

The issue's real single-session baseline: speedup 1→8 = **1.02×**. Quoted for
attribution only; no real model runs in this harness.

### What each curve attributes

- **C ≈ 8×** proves the Tokio executor + `join_all` plumbing fans out cleanly.
  Any gap between B and 8× is therefore NOT fan-out/fan-in serialization.
- **B = 4.13×** is mock sleep + real chunk/tokenize/score/prune CPU work. The
  B−C gap is executor-shaped CPU overhead, not a lock.
- **B−C at 1 page = 16.6ms/page** (0.063s − 0.046s, median difference) is the
  measured per-page CPU cost. It replaces the old "~18ms/page" prose estimate,
  which was never a per-phase profile and must not be cited as one.

### P2-001/002 archival (was provisional, now recorded)

The task tracker held P2-001 (retained N×M fan-out) and P2-002 (`join_all`
head-of-line fan-in) as `riesgo_hipotesis_no_demostrada` pending these curves.
Curve C at 7.68× demonstrates the hypothesis: with sleeps only, the exact same
fan-out/fan-in shape scales linearly, so neither P2 item is an independent
cause on this path. **Archived as downstream of P0-001. No `export_flow.rs`
redesign.** If the future release sweep shows real-model scaling that the mock
cannot explain, re-open with those numbers — not with suspicion.

## N-decision: explicitly pending

The release sweep (`p0_001_measure_sweep`, `#[ignore]`-gated BENCH, real models
in release, fresh process per cell) was NOT run in this session. The `batch`
cell is wired into it (`WEBFANG_P0_001_CONFIGS` default now includes `batch`;
single session, `intra_threads = 16`, per-page `run_batched_inference`, pages
sequential). N is selected from that table when it exists — minimum N with
speedup(8) ≥ 6.0× inside the ops RSS budget — never from the mock curves above.

## Circuit breaker: deferred by explicit decision

There is no circuit breaker, no `PoolExhausted` error, and no production
timeout on the pool path — backpressure by design (`OwnedSemaphorePermit`
RAII queues the N+1th request instead of failing it). This is a deliberate
deferral, not an oversight, and it is NOT a loose backlog ticket. Re-open
trigger (a gate condition): **when `EngineConfig::Pool` is activated via
`WEBFANG_AI_ENGINE` under sustained production traffic** and saturation,
slow-slot, or head-of-line symptoms appear in the trace, the breaker design
re-opens with those numbers attached. Until that gate fires, no breaker work.

## Overhead-number reconciliation

| Number | Status |
|---|---|
| ~18ms/page CPU overhead (old comment) | **Retired.** Was an unverified estimate phrased as attribution, never a measurement. Do not cite. |
| 16.6ms/page B−C gap | **Measured** (median difference, REPS=3, same sleeps + executor). The honest replacement. Still not a per-phase profile — per-phase (`Instant` inside `clean`) was never instrumented. |
| 3.0 assert floor | **Kept on evidence** (B = 4.13× passes with 1.1× margin; C = 7.68× is the ceiling, not the target). Pins serialization-freedom. |

## Checklist

- [x] Curves B + C measured, REPS=3 median, tables recorded above
- [x] P2-001/002 archival outcome recorded (downstream, not independent cause)
- [x] Assert branch recorded in code comment + commit message (keep 3.0)
- [x] Overhead estimate replaced by measured number
- [x] Circuit-breaker deferral recorded with gate-condition trigger
- [ ] Release sweep run (real models) — NOT in this session
- [ ] N selected + rollout — blocked on the sweep above

## Next step

Run the release sweep per `crates/webfang_ai/tests/p0_001_measure.rs` module docs
(`cargo test --release … -- --ignored --nocapture`, optionally
`WEBFANG_P0_001_CONFIGS=single,pool4,batch WEBFANG_P0_001_PAGES=1,8` for a spot
check first), then record N in this doc's pending row.
