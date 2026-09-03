# ADR 0014: Persistence Unification in `domain::persistence` and the Hybrid D3 Commit-Point Strategy

- **Status:** Accepted
- **Date:** 2026-09-04
- **Deciders:** Project Architect, webfang maintainers
- **Related:** ADR-0012-B §3.H, ADR-0013, issue #994 (sub-slice 3), PR #1101 (#1097), PR #1136 (#1045)

## Context

Issue #994 sub-slice 3 was specified before ADR-0012-B §3.H and PR #1101 landed.
Its premise — that the persistence port, its DTOs, and a SQLite backend do not
yet exist — is now obsolete. Verified state at main @ `19597396`:

- `RecordStorePort` (`domain/exporter.rs:413`) and the DTOs `RawRecord`,
  `LastError`, `DomainRecords` (`domain/exporter.rs:264+`) already live in
  `domain` (ADR-0012-B §3.H), implemented by the JSON-per-domain concrete at
  `infrastructure/export/record_store.rs:529`.
- `StateStorePort` (`domain/exporter.rs:446`) already exists (ADR-0013),
  constructed only in `application::container::build_state_store`.
- The allowlist entry for `infrastructure::export` is already gone (ADR-0012-B
  drain): the live allowlist is 2 entries (DI root + observability), stricter
  than the issue's spec baseline of 33/5.
- `StoreLock` is a **flock(2)-based cross-process lock** (`lock_exclusive` on
  `<state>.lock`), not an in-process mutex — loom cannot model it.
- The D3 commit-point (atomic save via tmp write + `rename(2)`, `.v1.bak.`
  backup, `CURRENT_VERSION=1`) has 23 deterministic tests including
  fault-injected rename (`RenameFailFs`, D6), v1 migration, quarantine, and
  tmp garbage collection — but zero concurrency tests and no state-transition
  model.

The spec's `infrastructure::persistence::sqlite` backend was aspirational
naming, never a requirement grounded in the roadmap; a real SQLite backend
implies state migration (v1/v2 JSON → SQLite) and is out of scope for the
inversion goal.

## Decision

1. **Unified persistence home.** Relocate `StateStorePort`, `RecordStorePort`,
   and the record DTOs from `domain/exporter.rs` to
   `domain/persistence.rs` (the module that already owns the
   `PersistenceMode`/`ResumeConfig` resolver). `domain/exporter.rs` keeps
   `pub use` re-exports for at least one release cycle so merged public paths
   (ADR-0013 consumers) keep compiling.
2. **No new backend.** The JSON-per-domain concrete stays as the sole
   implementation behind both ports. "SQLite" from the issue is recorded as a
   dead aspiration, not deferred work.
3. **Hybrid D3 verification strategy** (matches what each layer can falsify):
   - **In-memory protocol:** extract the record state-transition rules
     (`Discovered → Exported → Committed`, attempt/invariants) as a pure
     state machine and verify exhaustive interleavings with **loom**.
   - **Filesystem semantics:** deterministic fault-injected tests via the
     existing `StoreFs` seam for everything the kernel owns (rename atomicity,
     crash between the EXPORTED and COMMITTED saves, backup integrity).
   - Loom does not target `StoreLock` (flock is invisible to loom) and does
     not attempt to model `rename(2)`; both are kernel-owned and falsifiable
     only by test.
4. **Dead code removal.** `StateStore::mark_processed` / `is_processed`
   (zero production callers since ADR-0013) are deleted rather than ported.

## Consequences

- `domain::persistence` becomes the single domain-side home for persistence:
  resolver, ports, and DTOs. `domain/exporter.rs` refocuses on the exporter
  trait and its configuration.
- The relocation touches the public paths merged in ADR-0013; re-exports keep
  compatibility and the churn is paid once.
- Concurrency coverage goes from none to exhaustive on the transition
  protocol, and crash coverage extends to the full EXPORTED→COMMITTED
  sequence. The kernel-level rename atomicity remains unformalizable by
  construction (documented non-goal, consistent with the existing "no fsync"
  stance).
- The strict intra-crate gate keeps 2 allowlist entries; no new entries are
  needed by this slice.

## Alternatives rejected

- **Port + real SQLite backend** (issue text, literal): a second backend
  implies state-file migration, an ADR of its own, and no consumer demand —
  violates the smallest-viable-slice rule.
- **Leave ports in `domain::exporter`**: two persistence homes in `domain`;
  rejected in favor of the unified home chosen for this slice.
- **Loom over the whole commit sequence modeled in memory**: partial false
  confidence — the fs rename path would remain unproven while the model
  suggests otherwise; the hybrid assigns each claim to the tool that can
  actually falsify it.
