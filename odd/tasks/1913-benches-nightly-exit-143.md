# Feature: #1913 — Benches nightly dies with exit 143 in the compile gate

**Issue:** #1913 — `chore(ci): benches nightly muere con exit 143 en el compile gate (5 de 6 noches)`
**Labels:** `type:chore`, `status:approved`
**Branch:** `chore/benches-nightly-exit-143`
**Worktree:** `~/Projects/Rust/webfang-worktrees/chore-benches-nightly-exit-143`
**Base:** `main` @ `3ac5599c` (no stacked PR: `check-topology.sh` admits only `main` as base for `chore/*`)

## Objective

The nightly `Benches` workflow completes its full criterion suite and stays green for 7
consecutive nights instead of dying at ~5 min with SIGTERM (exit 143) during the cold
compile.

## Problem

Five of the last six nights fail in `Compile gate (cargo bench --no-run)` with exit 143,
never reaching a single measurement. Both attempts of run `37612895027` (attempt 1 and the
rerun, attempt 2) died the same way; attempt 2 at `23:02:16` after 5 min 18 s of compiling.
The symptom tracker is #1911, which the CI Health Observer owns and reopens on each red.

## Why this root cause (verified, not inferred)

| evidence | what it rules in / out |
| --- | --- |
| `ci.yml` compiles the whole workspace with `--all-targets --all-features` and is green on the same runner class | **Workspace size is not the cause.** |
| `Cargo.toml` `[profile.bench]` = `inherits = "release"` (opt-level 3, `lto = "fat"`, `codegen-units = 1`) **plus `debug = true` and `strip = false`** | Bench is the **only** profile in the workspace stacking fat LTO on complete debuginfo. |
| `[profile.dev]` already uses `debug = "line-tables-only"`; `[profile.release]` already uses `strip = true` | The decision was made twice and never applied to bench. |
| `Record runner environment` (added by #1524) reported 103 G free, no OOM text | Not disk pressure. |
| The log carries `shutdown signal` / bare `143`, never an explicit OOM | **Cannot distinguish OOM from fleet preemption.** Both point at the same two levers. |

Secondary finding: the compile gate builds the whole workspace, but the declared bench
targets live only in `webfang_core` (9) and `webfang_ai` (1). `webfang_cli`, `webfang_mcp`
and `webfang_benchmark` are compiled and then never used by any bench.

## Scope

- T1 — `[profile.bench]`: `debug = true` → `debug = "line-tables-only"`, matching
  `[profile.dev]`.
- T2 — `benches.yml`: scope both `cargo bench` invocations to `-p webfang_core -p webfang_ai`.

### Deliberately out of scope

- `lto` and `codegen-units` in `[profile.bench]` — these **do** change generated code and
  would invalidate every baseline in `benches/BASELINES.md`. Not touched.
- `strip` in `[profile.bench]` — left `false` so a bench panic still symbolizes to
  file:line. The debuginfo reduction in T1 is the load-bearing change.
- Restoring `Swatinem/rust-cache` — removed on purpose by the #959 review finding
  (Actions cache pool sits at the ~10 GB eviction threshold, #1165/#1171). Adding a
  multi-GB nightly bench cache would evict PR CI caches.
- A larger runner — costs money and is not needed if T1+T2 close the gap.
- Dropping `ai_phase_profile` (`webfang_ai`): it is a no-op that measures nothing
  (#1618 PERF-EVID-2). Kept in scope so this change does **not** alter the declared bench
  set; if the nightly still dies after T1+T2, removing it is the next lever.

## Authorized edit surface

- `Cargo.toml`
- `.github/workflows/benches.yml`

New files: none outside `odd/tasks/` (this document, required by ODD).

## Acceptance criteria

- `cargo bench -p webfang_core -p webfang_ai --no-run --locked` succeeds in the worktree.
- The compile gate no longer builds `webfang_cli`, `webfang_mcp`, `webfang_benchmark`.
- Criterion numbers are unaffected: `debug`/`strip` do not alter codegen.
- 7 consecutive green nights closes #1913 and lets the observer close #1911.

## Route decision (per task, with trigger evidence)

| task | route | trigger evidence |
| --- | --- | --- |
| T1 + T2 | **inline** | 2 files, 4 changed lines, both already read in-session, zero design ambiguity left after the user chose option (a). Fails the "2+ non-trivial files" writer trigger: neither file is non-trivial. |
| Verification build | **delegated worker** | Per-action rule: the release-profile bench build is expensive and must not run in the parent context. |

## Verification evidence

Delegated read-only verifier (fresh worker), all commands green in the isolated worktree
target `~/.cache/cargo-target/chore-benches-nightly-exit-143`:

| command | exit | wall | result |
| --- | --- | --- | --- |
| `cargo metadata --format-version 1 --no-deps` | 0 | 0.02 s | cargo accepts `line-tables-only` for `[profile.bench]` |
| `cargo bench -p webfang_core -p webfang_ai --no-run --locked` | 0 | 376 s | **cold** build (empty target dir beforehand), 10 bench targets linked |
| `ls $CARGO_TARGET_DIR/release/deps \| grep -cE '^libwebfang_(cli\|mcp)-'` | — | — | **0** — cli/mcp excluded from the bench build |
| `cargo check --workspace --all-targets` | 0 | 130 s | cli/mcp/benchmark still build normally under the changed profile |

Independent confirmation that the debuginfo reduction is active, from DWARF inspection of
`waf_detection` rather than from the manifest: 56,505 `DW_TAG_subprogram` DIEs with **0**
`DW_TAG_variable` DIEs, 12.7 MiB of `.debug_*` in a 16.9 MiB binary. `debug = true` emits a
variable DIE per local; zero is the line-tables-only signature.

### Honest limits of this evidence

- The A/B size delta against `debug = true` was **not** measured (it needs a second cold
  compile under `RUSTFLAGS=-C debuginfo=2`). No "N× smaller" claim is made.
- **Compilation success is not proof the nightly stops dying.** The scoped cold build still
  takes 376 s on this 16-core workstation, and the runner is 4-core with the SIGTERM arriving
  at ~5 min. The fix removes real work and real memory pressure, but only 7 green nights
  close #1913.

## Progress

- [x] Root cause identified and verified against `main` @ `3ac5599c`.
- [x] T1 — `[profile.bench]` debuginfo reduction.
- [x] T2 — bench compile scope in `benches.yml`.
- [x] Verification build green.
- [x] Work-unit commit `34ec2bfa`.
- [x] Native review per RDD: **approved**, authority burned
      (`gentle-ai.review-acknowledged/v1`, lineage `review-3b93a94c748db171`).
- [ ] Push and PR (maintainer decision).

## Review outcome

Four lenses (risk, resilience, readability, reliability) returned **approved** with 0
blocking findings and 12 advisory ones. None opened a correction, so the candidate landed
as committed. Advisory findings, all non-blocking and all deferred as separate later work:

| id | lens | severity | substance |
| --- | --- | --- | --- |
| `R4-BENCH-SCOPE-SILENT-COVERAGE-LOSS` | resilience | WARNING | A `[[bench]]` added to a third crate later is silently skipped and the nightly still reports green; the package list is an allow-list, not a filter, and `cargo` exits 0. |
| `R3-scope-drift` | reliability | WARNING | Same root as above from the test lens: no guard ties the hardcoded list to the crates that declare bench targets. |
| `R2-scope-coupling-untagged` | readability | WARNING | No change context tells a maintainer to extend the package list when adding a bench target. |
| `R4-UNMEASURED-AGAINST-KILL-WINDOW` | resilience | WARNING | The reduced workload was never measured against the window that actually produces exit 143. |
| `R4-NO-DISCRIMINATING-SIGNAL-AFTER-FIX` | resilience | WARNING | If the next night is red again the log carries the same bare 143, with no new signal to tell "fixed but still too big" from "not fixed". |
| `R2-bench-count-constant` | readability | WARNING | The `9` / `1` counts in the comment are an unexplained constant restated in two files with no canonical source. |
| `R2-scope-duplication` | readability | WARNING | The package literal is duplicated across both `cargo bench` lines with no single source of truth. |
| `R2-profile-comment-duplication` | readability | WARNING | The eight-line `Cargo.toml` comment duplicates the incident narrative and restates inherited release values. |
| `R2-self-referential-note` | readability | SUGGESTION | The replacement comment narrates the deletion of its own predecessor, opaque to a reader who never saw it. |
| `R3-exclusion-evidence` | reliability | WARNING | The exclusion check greps only `cli`/`mcp` library names while the criterion also names `webfang_benchmark`, and an absence-only count cannot distinguish "not compiled" from "not present under that name". |
| `R3-profile-invariant` | reliability | SUGGESTION | The profile invariants are asserted in prose only; nothing would fail if a later edit restored full debuginfo. |
| `R1-doc-local-env-details` | risk | SUGGESTION | The task document commits local workstation layout details (home-directory worktree path, cache location). |

The two that deserve real follow-up are `R4-BENCH-SCOPE-SILENT-COVERAGE-LOSS` /
`R3-scope-drift` — a guard that fails when a crate outside the selected set declares a
bench target would convert silent coverage loss into a loud signal — and
`R4-NO-DISCRIMINATING-SIGNAL-AFTER-FIX`, which is the honest admission that this change
cannot prove its own effect before the nightly runs.

## Next step

Push and open the PR are the maintainer's call. Pre-push runs `scripts/ci_fast_gate.sh`;
the delivery gates stay unmanaged and never authorize delivery.


