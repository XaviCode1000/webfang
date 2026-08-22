# Persistence & Resume — durability scope and contracts

This document is the authoritative description of WebFang's crash-recovery
model: what is guaranteed, what is explicitly NOT guaranteed, and the exact
policies implemented by the state machine, record store, JSONL writer, and
resume gate (SDD change `stabilization-state-machine-resume`).

---

## Durability scope — read this first

> **NON-GOAL: fsync / power-loss durability.**
>
> Every atomic write in the persistence layer is **temp-file + `rename(2)`**
> with **deliberately NO `fsync`**. `rename(2)` is atomic against *process
> death* (SIGKILL, panic, abort): after a crash the target path holds either
> the complete old content or the complete new content — never a partial mix.
> It is NOT atomic against power loss or kernel panic: a rename observed as
> successful may still be lost when the machine loses power before the page
> cache is flushed.
>
> If you need power-loss durability, that is a new change proposal — do not
> "just add an fsync" inside these modules.

What this buys you, concretely:

| Failure | Guarantee |
|---|---|
| SIGKILL at any point | No torn files under final names; no lost committed lines; no duplicated exports |
| Half-written JSONL tail | Truncated back to the last valid newline on next open (`warn!` carries byte counts) |
| Crash while holding the store lock | Stale lock detected/released; next run recovers |
| Stray `.tmp` artifacts | Garbage-collected on next run of the same output; never read as data |
| Power loss | **Nothing promised.** Re-run without expectations; the store may roll back to the previous complete state |

---

## The 8-state lifecycle and COMMITTED-only skip

Every scraped URL lives in a typed 8-state machine
(`crates/webfang_core/src/domain/page_state/`):

```text
DISCOVERED → QUEUED → FETCHING → FETCHED → EXTRACTED → PROCESSED
                                                          ↓
                                            EXPORTED → COMMITTED
```

- The typestate wrapper `Stateful<R, S>` makes illegal transitions a
  **compile error**; the only backward transition is
  `reopen_for_reexport` (EXPORTED → PROCESSED).
- Each record's persisted `status` field tracks the typestate position
  exactly; load-time reconciliation (`Stateful::reconcile`) validates the D2
  invariant table and quarantines impossible states instead of panicking.

### Skip-on-resume contract

**`--resume` skips ONLY records proven `COMMITTED`.** The proof is the typed
reconciliation boundary (`Stateful::<RawRecord, Committed>::reconcile`),
which enforces:

- persisted `status == COMMITTED`,
- `attempts >= 1`, no `last_error`,
- `output_location` + `content_hash` present (**exempt**: records carrying
  `run_id == MIGRATED_V1_RUN_ID`, see migration below).

Anything else — QUEUED, FETCHED, an EXPORTED whose hash is not provably on
disk — is **re-driven**, never skipped.

Exactly-once export is enforced by two flush-proof mechanisms:

1. **Record claim**: if the record's own `content_hash` appears in the
   output file's hash index, the line is already durable → promote to
   `COMMITTED` without re-appending.
2. **Record-less flush proof**: a previous run may have died AFTER flushing
   the item's line but BEFORE persisting any record (the critical window).
   The fresh item's content hash is matched against the same index;
   membership promotes a synthesized fresh lifecycle to `COMMITTED`.

A record whose OWN claim is unproven (hash not in index) always re-exports —
the record's claim takes precedence over the fresh-content match.

---

## v1 → v2 state migration

Legacy stores were `{ domain, processed_urls[], last_export, total_exported }`
(optionally WITHOUT a `version` field — absence means v1 by definition). On
load, `RecordStore` migrates them backup-first:

1. Parse `processed_urls`; anything unparseable is a hard `Corrupt` error —
   there is NO silent discard (fail-closed policy).
2. **Backup FIRST**: `<state-file>.v1.bak.<millis>` is written before
   anything touches the live file. Backup failure aborts migration loudly
   (the caller decides; the default store policy starts FRESH with a loud
   warning naming the path).
3. Every legacy URL becomes a `COMMITTED` record stamped with
   `run_id = MIGRATED_V1_RUN_ID` (`"migrated-v1"`), `attempts = 1`.
   Rationale: those URLs WERE fully exported under the legacy model; marking
   them anything else would re-scrape whole sites on upgrade.
4. **Caveat (by design)**: migrated records carry `content_hash = None` and
   `output_location = None`. They predate hash tracking, so the D2
   requirement for `COMMITTED` is waived for them only
   (`is_migrated_v1()`). Consequence: a migrated URL can be skipped on
   resume but cannot participate in flush-proof dedup — if its content is
   scraped again in a non-resume run it will simply produce a fresh honest
   record.
5. The v2 envelope replaces the live file via temp+rename; the original v1
   bytes stay untouched until that rename succeeds.

---

## Commit-point ordering (D3) and cancellation

Per exported item the sequence is strictly:

```text
serialize → append line → FLUSH (ack = durability barrier)
          → save record @ EXPORTED (checkpoint)
          → ★ commit point → save record @ COMMITTED
```

- The flush ack comes from the single-writer `JsonlSession`: ONE writer
  thread per output file per run owns the handle; producers clone a session
  handle; `flush()` resolves only after the OS-level flush returns.
- Records persist through the same temp+rename primitive.
- A crash anywhere leaves either "line + COMMITTED", "line + EXPORTED
  checkpoint" (recovered by flush-proof promotion), or "no line, no record"
  (plainly re-driven). Exactly once, every time.

**Cooperative cancellation (#653/#653-style ShutdownGuard)**: SIGINT/SIGTERM
stop NEW work; the in-flight item completes its FULL D3 sequence atomically;
the loop then drains and performs one final persist ("drain-before-final-
persist"). Cancelled runs exit **0** (cancellation beats error-class
routing), and every persisted status is honest — no invented states. A
following `--resume` re-drives everything not `COMMITTED`, exactly once.

---

## Crash matrix (SIGKILL injection harness)

`WEBFANG_CRASH_AT=<point>[:<n>]` arms the test-only crash harness
(`cli/crash_points.rs`); unarmed cost is one atomic read. The armed site
kills the process with SIGKILL at a deterministic CODE position (never a
wall-clock race), giving true crash semantics: no Drop impls, no cleanup.

| # | Point | Window exercised |
|---|---|---|
| 1 | `pre_first_persist` | discovery done, nothing saved |
| 2 | `mid_fetch` | response received, unprocessed |
| 3 | `post_fetch_pre_extract` | fetched+cleaned, extraction pending |
| 4 | `mid_jsonl_line` | half a line flushed (torn tail) |
| 5 | `post_flush_pre_commit` | flush ack received, EXPORTED not saved |
| 6 | `while_holding_lock` | killed inside store save txn |
| 7 | `tmp_written_pre_rename` | tmp complete, rename pending |
| 8 | `mid_state_file_write` | tmp PARTIAL (truncated artifact) |
| 9 | `during_cancel_drain` | cancelled mid-drain, pre-final-persist |

The matrix lives in `crates/webfang_core/tests/crash_matrix_test.rs`: each
row kills a real CLI child at the pinned point, reruns with `--resume`, and
asserts four global invariants — (a) zero URLs lost, (b) every
`checksum_sha256` exactly once in the parsed JSONL, (c) every persisted
record passes the D2 table, (d) resume exits successfully.

Run it with:

```bash
cargo nextest run --test crash_matrix_test
```

---

## Where things live

| Component | Path |
|---|---|
| Typed lifecycle (`PageStatus`, `Stateful`) | `domain/page_state/{status,typed}.rs` |
| RecordStore v2 + migration + invariants | `infrastructure/export/record_store.rs` |
| Single-writer JSONL session | `infrastructure/export/jsonl_writer.rs` |
| Commit protocol / resume gate wiring | `application/export_factory.rs`, `application/resume.rs` |
| Cancellation guard | `cli/shutdown.rs` |
| Crash-injection harness | `cli/crash_points.rs` |
| Matrix tests | `tests/crash_matrix_test.rs`, `tests/resume_gate_test.rs` |
