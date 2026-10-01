# Feature: AI phase profile bench (issue #1618)

## Goal

Topology decisions for the AI path must rest on measured phase costs, not on the
`393 x 45 ms` mock. Deliver a bench target that measures the real phases of the
AI pipeline on a real ONNX model and reports them SEPARATELY:

    prune+chunk · model_load · tokenizer_load · tokenize · infer · embed_postprocess · persist

## Precondition settled: #1559 nightly bench scope does NOT cover the AI path

Evidence (this worktree, HEAD `7c383739`):

- All 9 `[[bench]]` targets live in `crates/webfang_core/Cargo.toml:179-221` and
  point at the workspace-root `benches/` tree. Their subjects: `cosine_similarity`,
  `export`, `html_conversion`, `link_extraction`, `readability`,
  `sitemap_parsing`, `url_parsing`, `waf_detection`, `tracing_overhead`. None
  touches `InferenceEngine`, `ort`, or `tokenizers`.
- `crates/webfang_ai` had NO `benches/` directory and NO `[[bench]]` entry.
- `crates/webfang_ai`'s only measurement artifact was
  `tests/mock_inference_benchmark.rs`, which is a **mock** test (45 ms
  `tokio::time::sleep`, no ORT session, no model file).
- `webfang_core` cannot host an AI bench: the dependency matrix is
  `ai -> core`, so `core` must not depend on `ai`.

=> No duplicate. A new bench target in `webfang_ai` is the correct,
non-duplicating placement. `benches.yml:90-91` runs a bare `cargo bench --locked`
(no `--all-features`), so a bench gated on the `ai` feature builds a **no-op
binary** in that lane and measures nothing. CI cost: one empty binary. CI never
depends on the numbers.

## Design decisions

| Decision | Choice | Why |
| --- | --- | --- |
| Bench target | `crates/webfang_ai/benches/ai_phase_profile.rs`, `[[bench]]` with `harness = false` | `criterion` is not a dev-dep of `webfang_ai` and adding a dependency is forbidden. A libtest harness does not work either: `cargo bench` runs libtest in `--bench` mode, which reports plain `#[test]` fns as `ignored` (verified). An own `main` is the only shape that both needs no dependency and actually runs. |
| Feature gate | `#[cfg(feature = "ai")]` on the measurement module; a separate `#[cfg(not(feature = "ai"))] fn main() {}` | Without the feature the whole `infrastructure_ai` tree is absent. The no-op `main` is required: with the whole file `#![cfg]`-ed out there would be no entry point and the nightly lane would fail to link. |
| Model source | Read-only scan of the local HF cache (`$HF_HUB_CACHE`, else `$HOME/.cache/huggingface/hub`), same discovery `tests/p0_001_measure.rs` uses | Offline, no downloads, real Granite-97M/311M weights. Graceful skip when absent, and the skip message names the directory searched. |
| Self-contained | The bench does NOT `#[path]`-include `tests/p0_001_common.rs` | A `#[path]` outside the package root breaks `cargo package` verification, and a bench must not reach into `tests/`. ~25 lines of discovery duplication, stated in the module docs. |
| Numbers are never asserted | Only structural assertions (384 dims, count equality, unit norm, one JSONL line per chunk) | The mission forbids CI depending on absolute numbers. |
| Production code | **Unchanged** | The bench calls the same public production functions the pipeline calls, so it measures production code without putting instrumentation on the hot path. Issue slice 1's production `#[instrument]` work is NOT in this scope. |
| Persistence phase | `StreamRepository` (JSONL) only | `webfang_core::infrastructure::persistence` is gated behind `webfang_core/persistence`, which `webfang_ai` does not enable. Enabling it would be a feature-flag change, forbidden. Documented as a gap. |
| `embed_postprocess` | Measured in isolation, and the table states it is already contained inside `infer` | `run_session_inference` fuses ORT's `session.run` with mean-pool + truncate + L2, and ORT exposes no timer that separates them. Reporting it as an additive row would double-count. |

## Tasks

- [x] T1. Settle the #1559 scope precondition.
- [x] T2. Add the `ai_phase_profile` bench target.
- [x] T3. Run it on both variants; record per-phase numbers and run-to-run variance.
- [x] T4. Verify: `cargo check`, `cargo clippy --all-targets --all-features ...`,
      `cargo fmt --all -- --check`, `cargo doc`, `cargo nextest run -p webfang_ai`.
- [x] T5. Report.

## Measured results (real model, this machine, 16 cores, `profile.bench`)

Machine: 16-core workstation, `cargo bench` (`[profile.bench]`, inherits release).
Corpus: 400 paragraphs -> 400 chunks (the issue's ~393-chunk shape).
Pool: `EngineConfig::default_pool_size()` = N=4, `intra_threads=4`. Reps: 3.
Command: `cargo bench -p webfang_ai --features ai --bench ai_phase_profile`

### Granite-97M (default tier), medians of 3 reps, warm run

| phase | scope | median_ms | per_unit | share of page |
| --- | --- | ---: | ---: | ---: |
| `infer` | per page (join_all over 400 chunks) | 2787.7 | 6969 us/chunk | **99.30 %** |
| `tokenize` | per page | 21.3 | 53.1 us/chunk | 0.76 % |
| `persist` | per page (JSONL) | 11.3 | 28.3 us/chunk | 0.41 % |
| `prune+chunk` | per page | 3.4 | 8.5 us/chunk | 0.12 % |
| `embed_postprocess` | per page (isolated) | 2.2 | 5.5 us/chunk | 0.08 % (inside `infer`) |
| `model_load` | per session set (N=4) | 2947.6 | 2947.6 ms one-time | — |
| `tokenizer_load` | one-time | 355.8 | 355.8 ms one-time | — |

`model_load` marginal: N=1 1088.5 ms · N=2 1484.1 (+395.6) · N=3 2245.0 (+760.9)
· N=4 2947.6 (+702.7).

### Granite-311M (fallback tier), medians of 3 reps, warm run

| phase | median_ms | per_unit |
| --- | ---: | ---: |
| `infer` | 11694.3 | 29235.7 us/chunk |
| `tokenize` | 18.5 | 46.3 us/chunk |
| `persist` | 10.3 | 25.8 us/chunk |
| `prune+chunk` | 3.6 | 9.0 us/chunk |
| `embed_postprocess` | 4.4 | 10.9 us/chunk |
| `model_load` (N=4) | 9943.5 | one-time |
| `tokenizer_load` | 862.4 | one-time |

### What this says about the mock (PERF-MOCK-1)

The mock hardcoded 45 ms per inference and anchored `393 x 45 ms ~= 17.7 s` as
"the measured 1-page AI time". **Real Granite-97M at N=4 is ~7.0 ms of wall per
chunk, i.e. ~2.79 s for a 400-chunk page** — the mock overstates per-inference
cost by **~6.4x** and the page total by **~6.3x**. Every other phase together is
under 1.4 % of the page. Two consequences:

1. The mock was directionally right (inference dominates absolutely) and
   quantitatively wrong by ~6x. Any speedup ratio derived from it is still a
   ratio of sleep to sleep, so it says nothing about the real per-chunk cost.
2. `embed_postprocess` is ~0.08 % of inference: the in-process tail is NOT a
   lever. `PERF-SER-1` (the legacy `Arc<Mutex<Session>>`) is therefore purely a
   parallelism question, and `P1.2`'s "reuse per-slot scratch" is below the
   measurement floor on this machine.

### Determinism / variance (the question the mission asks directly)

Two warm 97M runs, same binary, back to back:

| phase | run 2 median | run 3 median | delta |
| --- | ---: | ---: | ---: |
| `infer` | 2787.7 | 2942.3 | 5.5 % |
| `model_load` (N=4) | 2947.6 | 2986.3 | 1.3 % |
| `tokenizer_load` | 355.8 | 343.7 | 3.4 % |
| `tokenize` | 21.26 | 21.41 | 0.7 % |
| `persist` | 11.31 | 10.95 | 3.2 % |
| `prune+chunk` | 3.40 | 3.60 | 6.0 % |
| `embed_postprocess` | 2.203 | 2.204 | 0.05 % |

**Warm-run medians agree within ~6 %,** which is enough to compare two runs and
to rank phases whose share differs by orders of magnitude. Two caveats, both
reproduced:

- **The first run after a cold build is a large outlier.** Run 1 (immediately
  after the bench-profile link) measured `infer` at 10111 ms median vs 2788 ms
  warm — 3.6x. A cold 390 MB weight file plus a cold page cache. **A profile
  must be taken warm; discard the first run.**
- **311M has a bimodal rep pattern at N=4.** Both 311M runs showed one rep at
  roughly twice the others (`infer` 97M-stable, but 311M: min 10751 / median
  18873 / max 30610 in run 1, and min 10914 / median 11694 / max 23131 in run 2).
  3 reps is not enough to characterize 311M; any N decision on that tier needs
  more reps and a declared idle machine.

## Evidence

- `ca5f7bc9` — `test(ai): add real-model AI phase profile bench`
- (this file, committed separately) — `docs(ai): record #1618 phase profile results`

## Left for later slices (NOT done here)

- Production-side per-phase `#[instrument]` on the cleaner hot path (issue
  slice 1 proper). The harness measures the same public functions but does not
  instrument them.
- The N decision itself (P0.3) and `docs/p0-001-n-decision.md` (slice 3). This
  profile is the input to that decision, not the decision.
- `PERF-RSS-1` (RSS per session). The per-N `model_load` cost is now measured;
  the per-N RSS is not.
- SQLite persistence timing (blocked on the `webfang_core/persistence` feature).
