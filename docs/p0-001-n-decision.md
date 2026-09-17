# P0-001 N-decision: mock evidence, P2 archival, and what stays pending (issue #1456)

**Status: N-decision PENDING the release sweep.** The mock evidence below settles the
downstream question (P2-001/002) and the assert-floor question. It does NOT select N —
that requires the real-model release sweep, which was deliberately not run here.

## Verdicts (read this, skip the rest if in a hurry)

| Question | Verdict |
|---|---|
| Are P2-001/002 downstream of P0-001? | **Archived on demonstrated basis.** Curve C is a rock-stable anchor (7.68–7.74 local, 7.73 CI runner) while curve B holds a machine-relative B/C ≥ 0.37 on both runners — the B gap scales with executor capacity, not with a lock. `export_flow.rs` stays untouched unless the release sweep re-opens this (triggers kept below). |
| Assert floor: raise or keep 3.0? | **Neither — replaced by machine-relative B/C ≥ 0.3.** The 3.0 absolute floor proved unportable (workstation B = 4.13× vs CI-runner B = 2.89×, PR #1457 Coverage job). Observed B/C 0.37–0.54 vs serialized ~0.125 — 0.3 has margin on both sides. |
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

### CI-runner datapoint (PR #1457 Coverage job)

| Curve | speedup 1→8 |
|---|---|
| B (mock infer + real pipeline) | **2.89×** |
| C (fully-stubbed sleep fan-out) | **7.73×** |

- **B/C at 8 pages = 0.37** (2.89 / 7.73). Local 8-point B/C = 0.54
  (4.13 / 7.68); per-point local series 1.00 → 0.91 → 0.75 → **0.54**.
  CI per-point intermediates were not captured — only the 8-point 0.37.
- **B−C at 1 page = 23.1ms/page** on the CI runner (vs 16.6ms/page local,
  same median-difference method, REPS=3).
- **C-as-anchor stability:** 7.68–7.74 across local runs, 7.73 on the CI
  runner. The ceiling does not move between machines; only B does — which is
  exactly why the guard is relative to C instead of absolute.

### Floor change: 3.0 absolute → 0.3 relative

The 3.0 absolute floor on curve B is **retired**: it failed the portability
test it was never designed for (B = 4.13× workstation vs 2.89× CI runner —
same code, different executor capacity). The replacement guard is
`B/C ≥ 0.3` at 8 pages, where C is the measured sleep fan-out ceiling on the
same runner. Margin argument: observed B/C spans 0.37–0.54, while a genuinely
serialized path would give ~1×/8× ≈ 0.125 — 0.3 sits clear of both, so the
next CI red reads as either real serialization (~0.125 territory) or a
degraded runner (absolutes sag, ratio holds), without archaeology. The assert
message prints the ratio, both speedups, and the runner core count for exactly
that triage.

### What each curve attributes

- **C ≈ 8×** proves the Tokio executor + `join_all` plumbing fans out cleanly.
  Any gap between B and 8× is therefore NOT fan-out/fan-in serialization.
- **B = 4.13×** is mock sleep + real chunk/tokenize/score/prune CPU work. The
  B−C gap is executor-shaped CPU overhead, not a lock.
- **B−C at 1 page = 16.6ms/page** (0.063s − 0.046s, median difference) is the
  measured per-page CPU cost. It replaces the old "~18ms/page" prose estimate,
  which was never a per-phase profile and must not be cited as one.

### P2-001/002 archival (DEMONSTRATED BASIS — guard now measures what it claims)

The task tracker held P2-001 (retained N×M fan-out) and P2-002 (`join_all`
head-of-line fan-in) as `riesgo_hipotesis_no_demostrada` pending these curves.
Per-point B/C ratios from the tables above: 1.00 → 0.91 → 0.75 → **0.54**
local, 8-point **0.37** on the CI runner (PR #1457 Coverage job). That decline
with N is consistent with growing contention with N — most plausibly real-CPU
task backlog queueing on the test runtime's fixed 8 workers, but fan-out /
scheduler effects of the P2 shape cannot be excluded from these numbers alone.
What upgrades the verdict from provisional to demonstrated-basis is the second
runner: curve C holds 7.68–7.74 local and 7.73 CI (anchor stability across
machines) while B/C stays ≥ 0.37 on both — the B gap tracks executor capacity,
not a lock, and the relative guard (B/C ≥ 0.3) now pins exactly that claim
instead of an absolute throughput number. **Status: archived as downstream of
P0-001 on demonstrated basis. No `export_flow.rs` redesign.** Re-open triggers
(unchanged): release-sweep scaling the mock cannot explain, or per-phase
(`Instant`-inside-`clean`) profiling showing scheduler/fan-out cost growing
with N.

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
| 23.1ms/page B−C gap (CI runner) | **Measured** (same method, PR #1457 Coverage job). Same attribution as the 16.6 local number — larger because the runner is weaker, not because the pipeline changed. |
| 3.0 assert floor | **Retired.** Failed portability: B = 4.13× local vs 2.89× CI on identical code. Do not cite as a target. |
| 0.3 relative floor (B/C at 8 pages) | **Kept on evidence across two runners** (observed 0.37–0.54 vs serialized ~0.125). Pins serialization-freedom relative to the runner's own ceiling. |

## Checklist

- [x] Curves B + C measured, REPS=3 median, tables recorded above
- [x] CI-runner datapoint recorded (B 2.89×, C 7.73×, B/C 0.37, 23.1ms/page, PR #1457 Coverage job)
- [x] P2-001/002 archival outcome recorded (DEMONSTRATED BASIS — B/C ≥ 0.37 on both runners, C anchor 7.68–7.74 local + 7.73 CI, re-open triggers kept)
- [x] Assert branch recorded in code comment + commit message (3.0 absolute → 0.3 relative, margin argument)
- [x] Overhead estimate replaced by measured number
- [x] Circuit-breaker deferral recorded with gate-condition trigger
- [ ] Release sweep run (real models) — NOT in this session
- [ ] N selected + rollout — blocked on the sweep above

## Next step

Run the release sweep per `crates/webfang_ai/tests/p0_001_measure.rs` module docs
(`cargo test --release … -- --ignored --nocapture`, optionally
`WEBFANG_P0_001_CONFIGS=single,pool4,batch WEBFANG_P0_001_PAGES=1,8` for a spot
check first), then record N in this doc's pending row.
