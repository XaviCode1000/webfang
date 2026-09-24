# P0-001 N-decision: release sweep, mock evidence, P2 archival (issue #1456)

**Status: N DECIDED (2026-09-24, release sweep with real models) — N=4 as the recalibrated
`default_pool_size()` knee; rollout flips the default engine to `Pool`.** The deciding tables
and the verdict chain are in "Release sweep" below. The mock evidence further down settles the
downstream question (P2-001/002) and the assert-floor question.

## Verdicts (read this, skip the rest if in a hurry)

| Question | Verdict |
|---|---|
| Are P2-001/002 downstream of P0-001? | **Archived on demonstrated basis.** Curve C is a rock-stable anchor (7.68–7.74 local, 7.73 CI runner) while curve B holds a machine-relative B/C ≥ 0.37 on both runners — the B gap scales with executor capacity, not with a lock. `export_flow.rs` stays untouched unless the release sweep re-opens this (triggers kept below). |
| Assert floor: raise or keep 3.0? | **Neither — replaced by machine-relative B/C ≥ 0.3.** The 3.0 absolute floor proved unportable (workstation B = 4.13× vs CI-runner B = 2.89×, PR #1457 Coverage job). Observed B/C 0.37–0.54 vs serialized ~0.125 — 0.3 has margin on both sides. |
| Micro-batching vs Pool{N}? | **Pool{N} wins on both axes.** 8-page wall: pool4 22.5s vs batch 50.8s (batch 2.3× slower), AND batch carries the table's HIGHEST peak RSS (2,535 MiB vs pool4's 1,822 MiB steady) — the "batch is memory-free" hypothesis is measured dead. |
| N (pool size)? | **N=4** (measured knee, 16-core/97m): pool2→4 buys 20.5% wall for +764 MiB; 4→8 buys 4.7% for +1,535 MiB (inside run-to-run variance). Ships as `default_pool_size()` recalibrated to clamp [2,4]; `pool:8` stays available via `WEBFANG_AI_ENGINE`. |
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

## Release sweep (2026-09-24 — the N-deciding evidence)

Run on the 16-core workstation, Granite-97M from the read-only local HF cache, RELEASE
profile, fresh child process per cell (VmHWM isolation), REPS=3 median, 16 tokio workers
per child (production shape: `num_cpus`). Corpus: synthetic ~153KB page, **385 chunks/page**
(the issue baseline measured 393 — same fixture shape). Commands and env knobs:
`crates/webfang_ai/tests/p0_001_measure.rs` module docs.

### Wall time (medians)

| config | 1p | 2p | 4p | 8p | s/page (at 8p) | vs single (at 8p) |
|---|---|---|---|---|---|---|
| single (`intra_threads(1)`) | 15.205s | — | — | 122.231s | 15.28 | 1.00× |
| batch (1 session, `intra=16`) | 6.320s | — | — | 50.830s | 6.35 | 2.41× |
| pool2 (2 sessions, `intra=8`) | 3.457s | — | — | 28.287s | 3.54 | 4.32× |
| pool4 (4 sessions, `intra=4`) | 2.860s | 5.847s | 11.780s | 22.463s | 2.81 | **5.44×** |
| pool8 (8 sessions, `intra=2`) | 2.695s | 5.322s | 10.964s | 21.401s | 2.68 | **5.71×** |

Reading of the shape:

- **Cross-page speedup ≈ 1.0× for EVERY config** (pool4: 2.860×8/22.463 = 1.02×;
  pool8: 1.01×; single: 0.995×). This is NOT a plumbing failure — curve C (7.68× sleep
  fan-out) already proved the N×M fan-out serialization-free. It is **CPU saturation**:
  pool2/4/8 and batch each occupy the whole 16-core machine on ONE page (N sessions ×
  cores/N intra-threads = 16), so a second page has no idle CPU to overlap onto.
  `single` is the mirror case: `intra_threads(1)` leaves 15 cores idle behind the Mutex.
- The real lever is **per-page time** — batching the page's 385 chunks across parallel
  sessions: 15.28 s/page (single) → 2.81 (pool4). A trickle workload (pages arriving
  over network time) gets the same ~5.4× per-page latency win; the fixed 8-page corpus
  gets the 5.44× wall win. Both are real; only the harness shape made "speedup(8)"
  look like the metric.

### Peak RSS (VmHWM, fresh child per config, 97m)

| config | post-load | peak | notes |
|---|---|---|---|
| single | 741 MiB | 741 MiB | baseline |
| batch | 681 MiB | **2,535 MiB** | +1,854 MiB transient (activations of one 385-chunk batch) |
| pool2 | 1,058 MiB | 1,058 MiB | +317 MiB vs single |
| pool4 | 1,822 MiB | 1,822 MiB | +1,081 MiB vs single |
| pool8 | 3,357 MiB | 3,357 MiB | +2,616 MiB vs single |

Per extra session ≈ +370–430 MiB (weights committed per session; pool cells add no
inference-time delta — their per-session batches are 385/N chunks, unlike batch's single
385-chunk call). Reconciliation with #1315 (2.05 GiB single / 3.2 GiB doubled in the full
CLI process): that baseline includes crawl/tokenizer/export working set absent in this
bare child, so its per-session delta (~1.15 GiB) is an upper bound for the AI stack
alone (~0.4 GiB here). Memory stays the hard constraint either way.

### Correctness gate

`p0_001_engine_parity` (release, same corpus): pool2 / pool4 / pool8 vs single —
385 chunks each, embeddings **bit-identical** (max-abs-diff 0.00e0, all three pools).

### Criterion audit: 6.0× was unreachable — what replaces it

The issue's fixed criterion (min N with speedup(8) ≥ 6.0×) assumed cross-page idle
capacity that does not exist once one page saturates the CPU. Measured ceiling: 5.71×
(pool8). The decision therefore falls to the metric this saga's methodology already
named: **absolute 8-page wall inside the RSS budget, tie-breaking toward less machinery**
(CORRECTNESS > ROBUSTNESS > PREDICTABILITY > PERFORMANCE):

1. `batch` loses on BOTH axes (50.8s — 2.3× slower than pool4 — and the highest peak
   RSS in the table). Eliminated.
2. pool2 → pool4: −20.5% wall (28.3 → 22.5s) for +764 MiB. Buy.
3. pool4 → pool8: −4.7% wall (22.5 → 21.4s) for +1,535 MiB and 2× the session
   machinery — a margin inside the observed run-to-run spread (direct-run replication:
   pool4 1p 2.90s here vs 2.86s in-sweep; pool8 2.91 vs 2.695). Don't buy;
   `WEBFANG_AI_ENGINE=pool:8` remains the explicit override for those who want it.

**Verdict: N = 4** on the 16-core/97m class — shipped as the recalibrated
`default_pool_size()` (cores/2 clamped to [2,4]) so smaller machines scale down
structurally instead of inheriting a workstation constant.

### 311m robustness spot-check

Anchors at 1 page (single 57.688s baseline): batch 22.528s (**2.56×**), pool4 10.625s
(**5.43×**), pool8 10.289s (**5.61×**) — the 97m ranking is model-invariant. At 8 pages:
pool4 87.497s (8.00× linear), pool8 83.575s (4.5% under pool4 — the same inside-variance
knee margin as 97m), batch 188.594s (**2.16× slower than pool4**, and the highest RSS
again: +1,622 MiB peak delta). Parity bit-identical (max-abs-diff 0.00e0) for
pool2/pool4/pool8 vs single. 381 chunks/page (311m tokenizer).

Methodology note: batch×8p and pool8×8p were first measured with concurrent cargo builds
running on the same machine (+31–33% inflated medians); both cells were re-measured on an
idle machine and only those clean numbers are reported above. Lesson recorded: sweep cells
and builds must never share the machine — the harness measures wall time, and compile jobs
are contention.

## Rollout (this branch)

- `EngineConfig::default()` flips `Single` → `Pool { size: default_pool_size() }`;
  `resolve_spec` (hence `WEBFANG_AI_ENGINE`) treats unset/blank as the new default.
  `WEBFANG_AI_ENGINE=single` is the documented rollback hatch; `pool:<N>` stays the
  explicit override and set-but-invalid values still fail loud (#874).
- `default_pool_size()` clamp [2,8] → [2,4], with this sweep as the calibration record
  its doc comment always demanded (16-core → 4; smaller machines scale down structurally).
- **Known, accepted degradation (CLI):** the vault-search embedding port and the Tier 2
  semantic inspector stay typed to the concrete single-session `InferencePool`, so a
  Pool-mode CLI run degrades them honestly (loud startup warning) instead of loading a
  second model. `WEBFANG_AI_ENGINE=single` restores them. Removing the degradation is the
  scoped follow-up: generalize `EmbeddingAdapter` / `GraniteDomInspector` /
  `shared_inference` to erased `Arc<dyn InferenceEngine>` ports.
- **MCP daemon NOT flipped in this PR:** `spawn_ai_wiring` still builds
  `SemanticCleanerImpl::new` (Single) and never resolves `WEBFANG_AI_ENGINE`; its
  vault-search embedding port is the MCP core feature, so flipping it belongs to the same
  seam-generalization follow-up. Flipping only the CLI keeps the two entry points
  inconsistent on purpose: the CLI's cleaning path is the measured hotspot (its phase-AI
  is 100% of the run), while MCP vault workloads are embedding-sized, not clean-sized.
- Circuit-breaker deferral unchanged, but its gate condition ("Pool activated under
  sustained production traffic") goes LIVE for default CLI `--clean-ai` runs with this
  rollout — the re-open trigger below is armed by default now, not only for opt-in users.

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
- [x] Release sweep run (real models, 2026-09-24): 97m full wall matrix + peak RSS + parity; 311m spot-check
- [x] N selected (N=4 — knee + RSS tie-break, tables above); rollout on this branch

## Next step

The sweep ran (tables above). Remaining on this branch: the rollout commit (default →
`Pool`, clamp [2,4]) with green gates, then the PR closing #1456.
