# ADR 0016: Interruption/Resume Lifecycle — Single Owner, Pinned Precedence, and the F-R3-7 Accepted Residual

- **Status:** Accepted
- **Date:** 2026-09-10
- **Deciders:** Project Architect (owner decisions relayed via orchestrator), AGENTE-1 (SDD mission)
- **Related:** issue #1292 (F-07 / F-39 / P8-4 / P8-5 / P8-6 / F-R3-7), issues #1230, #1234, PR #1247, ADR-0013, ADR-0014, issue #1289 (checkpoint IO — out of scope here), issue #509 (cancellation token), AUDIT-01/02

## Context

AUDIT-01/02 surfaced a cluster of lifecycle defects that share one root: interruption
and resume were handled in scattered places without a single owner and without pinned
evidence. Two of the six findings were already fixed in code by PR #1247 (which
consolidated #1230 and #1234):

- **F-07** — concurrent `--resume` read-modify-write race. Fixed by the
  transactional record store: `RecordStorePort::update()` holds an exclusive
  `flock(2)` spanning load → mutate → save (`domain/persistence.rs`), and it is the
  ONLY port method allowed to change persisted state.
- **F-39** — SIGINT persisted the whole discovered BFS frontier. Fixed by checkpoint
  schema v2: the frontier is capped at the run's own `max_pages`, and `visited`
  semantics were re-anchored to durable processing state.

Still open at mission start: **P8-4** (resume after completion re-scraped),
**P8-5** (no testable SIGINT/SIGTERM handling), **P8-6** (`failed`/`retryable` not
persisted), and **F-R3-7** (hybrid L2/L3 perform their own networking; dial-level
guards are architecturally inapplicable, leaving a LOW DNS-rebinding TOCTOU residual
between the L1 fetch and an L2/L3 re-fetch).

The issue mandates ONE unified design before any per-finding PR.

## Decision

### 1. Ownership (single owner per concern)

| Concern | Owner | Rationale |
| :--- | :--- | :--- |
| **Interruption** (SIGINT/SIGTERM/cancel) | `Engine` — its `ShutdownSignal` (atomic flag) + `CancellationToken` (#509). Nothing else may decide to stop the run. | One flag, one token, one handler task; workers already treat cancellation as a control signal, not a failure. |
| **Durable page lifecycle truth** | `RecordStore` — the 8-state `PageStatus` (`DISCOVERED → … → COMMITTED`) persisted per domain. | The record survives crashes and signals; it is the only cross-run truth. Skip-on-resume happens ONLY from `COMMITTED`-proven records through the typed gate (`application/resume.rs::filter_committed`). |
| **Scheduling state** (visited/frontier/banned domains) | `CrawlCheckpoint` — engine-internal, versioned, bounded. | It is an execution artefact, not a lifecycle map (already documented in `domain/page_state/mod.rs`). Its IO mechanics belong to #1289. |

### 2. Interruption precedence (pinned order — deviations are bugs)

```text
SIGINT/SIGTERM (or cancel_handle())
  → shutdown flag = true AND cancel token fires
  → drain: no NEW fetches; waits blocked on rate-limit/governor abort as
    control signals (never classified as retries or errors)
  → CommitSession persists in-flight pages at their NON-advanced status
    (classified last_error + attempts on failure; partial progress on success)
  → checkpoint written with the bounded frontier (≤ the run's own max_pages)
  → process exits (SIGINT surfaces the conventional signal exit path)
```

Stage order is mandatory: a cancelled wait counts as skipped, never as a blocked
fetch; retry classification never runs after cancellation; the body of a response
that will not be accepted is never read.

### 3. State machine per resume path

| Resume path | Records consulted | Skipped | Re-driven | Checkpoint | Exit |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **Fresh** (no `--resume`) | none consulted | nothing | everything | none read; per-seed file created on first write | 0 / 68 |
| **Partial** (crash/SIGINT mid-run, `--resume`) | all, via typed gate | only `COMMITTED`-proven | everything else (incl. records with classified `last_error` at non-advanced status — P8-6 semantics) | v2 read; stale/invalid versions discarded, frontier treated as bounded hint | 0 |
| **After completion** (`--resume`, all committed) | all | all (`COMMITTED`-proven) | none — **idempotent, zero page fetches** (P8-4) | absent (deleted at completion); absence is not an error | 0 |
| **After crash** (SIGKILL mid-pipeline, `--resume`) | all, via typed gate | only `COMMITTED`-proven | everything else; crash matrix rows prove no record loss at any pinned point | same as partial | 0 |
| **After signal** (SIGINT/SIGTERM, `--resume`) | same as partial | only `COMMITTED`-proven | remaining + never-committed | bounded-frontier checkpoint IS written before exit (P8-5/F-39) | signal exit path |

### 4. Failure persistence (P8-6 — decision: re-drive, no new status)

A failed page persists a record at its NON-advanced status with a classified
`LastError` and `attempts += 1` (`fail_item`, SC6). Resume re-drives failed pages —
a permanent `Failed` status was REJECTED because it would hide transient failures
behind a skip and touches the reconcile/invariant tables for a 9th state. The
evidence test pins: classified error survives crash + resume; success clears
`last_error` and advances to `COMMITTED`.

### 5. Finding → evidence map (one named test per finding)

| Finding | Named evidence | Nature |
| :--- | :--- | :--- |
| F-07 | `record_store_transaction_test.rs` — `f07_*`-prefixed deterministic tests; the multi-process behavioral test stays `#[ignore]`d as a DOCUMENTED stress check (real contention is not deterministic; un-ignoring would violate the absolute-determinism rule) | deterministic, in-process |
| F-39 | `f39_sigint_checkpoint_frontier_is_bounded` | E2E, real SIGINT, wiremock |
| P8-4 | `p84_resume_after_completion_is_idempotent` | E2E, zero page fetches on re-run |
| P8-5 | `p85_sigint_shutdown_is_resumable` + `p85_sigterm_shutdown_is_resumable` | E2E, deterministic trigger: signal delivered after the k-th request observed by wiremock (crash-matrix pattern) |
| P8-6 | `p86_failed_record_is_redriven_and_error_cleared_on_success` | deterministic, in-process |
| F-R3-7 | this ADR §6 + module docs in `domain/ssrf_guard.rs` and `infrastructure/downloader/hybrid_router.rs` | documented acceptance |

### 6. F-R3-7 — accepted residual (owner decision)

**Decision: accept and document.** The hybrid escalation layers (L2 Obscura
subprocess, L3 headless Chromium/CDP) perform their own networking; the wreq
dial-level resolver guard is architecturally inapplicable inside those processes.
An entry-time re-check does NOT close the residual: the DNS-rebinding TOCTOU window
re-appears between the re-check and the subprocess's own resolution/dial. The
residual is LOW (requires an attacker-controlled hostname that rebinds between the
L1 fetch and the L2/L3 escalation of the SAME page), so accepting it with explicit
documentation beats a control that only appears to close it.

## Consequences

- The lifecycle has exactly one place to reason about for each concern (owner
  table above); future fetch paths inherit the precedence chain by construction.
- P8-4/P8-5/F-39 behavior is pinned by E2E tests that cannot pass vacuously
  (real process, real signal, request-counted wiremock).
- The 8-state `PageStatus` enum is frozen by policy: adding a state requires a new
  ADR (P8-6 decision).
- F-R3-7 remains an open-by-design residual; any change to L2/L3 networking must
  re-evaluate this acceptance.
- Checkpoint IO mechanics stay with #1289; this ADR constrains only ownership,
  precedence, and evidence.
