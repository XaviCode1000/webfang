# #1778: the MCP auth leak test passes green when its capture captures nothing

## Goal
Make `mcp_server::auth::tests::a_rejection_log_carries_neither_the_expected_nor_the_presented_token`
deterministic in every suite configuration, including the libtest-based
`Coverage` lane where it intermittently observes a completely empty tracing
buffer — and make it impossible for the test to pass for the wrong reason
again.

## Root cause
Same defect class as #1638, in a file that fix did not reach.

`capture_logs_async` (`crates/webfang_mcp/src/mcp_server/auth.rs:251-298`)
installs its subscriber with `tracing::subscriber::set_default`, a
**thread-local** default, and holds that guard across `.await`. Two independent
ways for the capture to come back empty:

1. **Thread-local scope.** The auth check is an axum/tower middleware, so
   whether its `tracing` events land under the subscriber depends on which
   thread polls the future. The guard is held across an await point on a
   runtime that may move the future between threads.
2. **Callsite `Interest` poisoning.** `tracing` caches per-callsite `Interest`
   process-wide through a one-time compare-exchange. A sibling test that reaches
   the `rejected unauthenticated request` callsite with **no** subscriber
   registers `Interest::never()` for the whole process. Under libtest — one
   process, many threads — the capture test then writes zero bytes while
   asserting correctly. Under `cargo nextest` every test is its own process, so
   the callsite can never be poisoned by a sibling: that is why the flake only
   ever surfaces in the `Coverage` lane.

The doc comment already names the property that makes it thread-dependent:
`TRACE` is deliberate because the test "asserts the whole span, not just the
`warn!`".

Precedents: #417 → `1ef88a48` (PR #418), #664, #1638 (closed, see
`odd/tasks/mcp-tracing-callsite-flake.md`). This is the fourth occurrence and
the second in `webfang_mcp`.

## Why it is worse than noise
A security regression test that reports green whenever its capture silently
no-ops has stopped testing the thing it exists to test. The failure is rare
enough that a re-run turns it green and the signal is lost — so the next real
credential leak would also pass.

## Fix
Replicate the #1638 shape already proven in this crate:

- `ensure_global_subscriber()` guarded by `Once`, installing a global sink
  subscriber. Setting a *global* default rebuilds the cached interest of every
  already-registered callsite, so calling it repairs a poison that already
  happened.
- Restructure from `#[tokio::test]` + `set_default` (thread-local guard held
  across awaits) to `#[test]` + `with_default` (closure-scoped) with an explicit
  `current_thread` runtime built *inside* the closure.
- **Regression guard:** assert the buffer is non-empty **before** asserting that
  neither credential appears in it. A leak test must never be able to pass
  vacuously.

## Scope
- `crates/webfang_mcp/src/mcp_server/auth.rs` (test module only).
- `crates/webfang_mcp/src/mcp_server/handlers/test_support.rs` — host the
  shared `ensure_global_subscriber()` beside the already-shared
  `SharedBufWriter`, instead of adding a fifth verbatim copy of a function the
  duplication ratchet (`scripts/check_duplication.sh`, jscpd `--min-tokens 50`)
  is actively descending on (#516, #1757).

## Out of scope
- No production code. The refusal log, its fields and the constant-time bearer
  comparison are untouched — only their test observability changes.
- The four existing `ensure_global_subscriber()` copies in `security.rs`,
  `content.rs`, `state.rs` and `metrics.rs` are **not** re-pointed at the new
  shared helper: they work, and consolidating them is a separate chore that
  would make this diff four files wide for zero behaviour change.
- No change to the 429/threat-model surface.

## Tasks
1. [x] Triage the open issue set; confirm #1766 / #1697 are already fixed and
   #1778 is the live flake of this class.
2. [x] Intelligence gate: read `odd/tasks/mcp-tracing-callsite-flake.md` and the
   four in-repo copies of the guard.
3. [x] Audit the acceptance criterion about `set_default`: `auth.rs:288` is the
   **only** `set_default` capture in `webfang_mcp`; every other capture already
   uses `with_default` or `set_global_default`.
4. [x] Bootstrap worktree `fix-mcp-auth-capture`, isolated `CARGO_TARGET_DIR`
   (`seed: cold`, no compatible seed), CodeGraph + CodeDB indexes verified
   (`root` = worktree, `head` = `169668d19`).
5. [x] Apply the guard, the restructure and the non-empty-first assertion.
6. [x] A/B measurement: flake rate with the guard disabled vs enabled, same
   binary, `--test-threads=8` under libtest.
7. [x] Full pre-commit gate: check, strict clippy, `fmt --check`, rustdoc,
   affected `nextest`.
8. [x] Work-unit commit.
9. [x] Native review of the candidate — **approved**, authority burned.

## Native review — APPROVED

First pass failed and this section recorded it as blocked; a retry succeeded, so
the correction is recorded here rather than by rewriting the pushed commit.

**Why the first pass failed.** Two STARTs against the *uncommitted workspace*
projection each returned a consent binding already expired on arrival
(`consent-binding-expired`, 10-minute TTL: `bb0c5279-…`, `8b95fe55-…`), both
with `lineage_created: false`. System clock synchronized, `gentle-ai 4.0.0`. No
authority burned, nothing frozen.

**What changed on the retry.** The candidate was the **committed range** rather
than the dirty workspace: after the work-unit commit the workspace projection had
no diff left to review. `inspect` then projected `base-diff` with
`--base-ref=169668d19… --committed-only=true`, and START was granted.

| | |
| :--- | :--- |
| lineage | `review-c0faa96b21f36704` |
| target | `sha256:a7027cf3d4df1e28ffb983cdb83f3c7d018ddbcf1b5ca4178ae67ab9f0e7fa49` |
| risk tier | **high** (`hot_path` / `auth`) |
| lenses | risk, resilience, readability, reliability — 4/4 prepared, 4/4 admitted |
| verdict | **approved**, no correction transition offered |
| authority | **burned** (`gentle-ai.review-acknowledged/v1`) |

**Advisory findings — all non-blocking, none reopened the review.** Separate
later work, never a reason to re-review this candidate:

| id | lens | location | severity |
| :--- | :--- | :--- | :--- |
| R2-001 | readability | `auth.rs:251-258` | WARNING |
| R2-002 | readability | `handlers/test_support.rs:11-17` | WARNING |
| R2-003 | readability | this doc `:90-102` | SUGGESTION |
| R3-NONEMPTY-CAPTURE | reliability | `auth.rs:356-360` | WARNING |
| R4-nonelog-spans | resilience | `auth.rs:355-359` | WARNING |

The lifecycle stops here: `delivery: ordinary-repository-policy`. An approved
review is evidence, never delivery authority — merge remains a human decision.

## Acceptance
- [x] The capture mechanism is not thread-local-scoped: `with_default`
  (closure-scoped) with a `current_thread` runtime built and `block_on`-ed
  inside the closure.
- [x] The test asserts the buffer is non-empty **before** asserting the absence
  of either credential.
- [x] The capture is also protected against callsite poisoning by a shared
  `ensure_global_subscriber()` (`set_global_default` rebuilds cached interest).
- [x] `cargo nextest run -p webfang_mcp` green (502/502); full libtest lib
  suite green (366/366 at `--test-threads=1`, 0/30 failed at
  `--test-threads=8`).
- [x] No other test in `webfang_mcp` relies on `set_default` being thread-local:
  `auth.rs:288` was the **only** `set_default` capture in the crate; every
  other capture already used `with_default` or `set_global_default`.
- [ ] `Coverage` passes on three consecutive runs of the branch — CI-side, not
  runnable locally.

## Evidence

### A/B flake measurement (same binary, libtest, `--test-threads=8`)

The filter is the whole `mcp_server::auth` module, because the poisoners ARE
its sibling tests — running the capture test alone could never poison it.

| Variant | Command | Result |
| :--- | :--- | :--- |
| A — guard call removed | `for i in $(seq 1 30); do "$BIN" mcp_server::auth --test-threads=8; done` | **25 / 30 runs FAILED** |
| B — guard restored | same binary, `seq 1 60` | **0 / 60 runs FAILED** |

Orchestrator-measured, independently of the writer's own run (which measured
114/120 vs 0/240 with the same shape). The flake is far more than
"intermittent": with 8 libtest threads the un-guarded test is poisoned almost
every time, which is exactly what the root cause predicts — `rejects_wrong_token`
and `refuses_every_request_when_no_token_is_configured_and_none_was_asked_for`
hit the same two `warn!` callsites with no subscriber.

Every Variant A failure is the **new** non-empty assertion firing:
`the refusal produced no log output at all — a leak test that captures nothing
passes every absence assertion vacuously (#1778)`. That is the issue's point,
demonstrated: before this change those runs were green.

### Gates (all from the worktree, after `eval "$(direnv export bash)"`)

| Gate | Result |
| :--- | :--- |
| `cargo check -p webfang_mcp --all-targets --all-features` | clean |
| `cargo clippy -p webfang_mcp --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines` | clean, 0 warnings |
| `cargo fmt --all -- --check` | exit 0 (verification form) |
| `env "RUSTDOCFLAGS=-D warnings" cargo doc -p webfang_mcp --all-features --no-deps` | exit 0 |
| `cargo test -p webfang_mcp --lib mcp_server::auth -- --test-threads=8` | 9 passed; 0 failed |
| `cargo test -p webfang_mcp --lib -- --test-threads=1` | 366 passed; 0 failed |
| `cargo nextest run -p webfang_mcp` | 502/502 PASS |
| `scripts/check_duplication.sh` | 7537 duplicated lines vs baseline 7683 — below, ratchet satisfied |

### Original failure and control
- Failure: `Coverage` job `110938184811` on run `37035367813` —
  `363 passed; 1 failed`, this test only.
- Control: `Coverage` green on the last two `main` runs (`36782973563`,
  `36756585276`); #1775 does not touch `auth.rs`.