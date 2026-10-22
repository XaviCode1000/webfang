# Prune the dead store seam (#2243)

Prune the two dead methods of the public `StateStorePort` port and correct the
ADR that claims they are in live use. The historical ADR record stays intact;
the correction is a dated addendum.

## Specs

S1. "Slice autorizada por el owner, aún no implementada." — remove
    `load_for_export` from the public port trait `StateStorePort`
    (`crates/webfang_core/src/domain/persistence.rs`). It is a provided method
    with zero call sites anywhere in `crates/`, and it is the only direct
    constructor of `ExporterError::StateStore`.

S2. Remove `load_or_default` from the same public port trait. It is a REQUIRED
    method of the trait, so removing it changes the public contract: the commit
    carries a `BREAKING CHANGE:` footer. Its orphan delegation in the only
    implementor (`crates/webfang_core/src/infrastructure/export/state_store.rs`,
    the `impl StateStorePort for StateStore` block) goes with it. The inherent
    `pub fn load_or_default` on the concrete `StateStore` STAYS, with its seven
    unit tests: its behavior (stale-version discard + `.bak` preservation) is
    real and tested, and deleting the concrete's public method would be a
    second, unauthorized breaking change.

S3. "Addendum fechado permitido en `docs/adr/0013-state-store-port-seam.md`." —
    append a dated addendum to the ADR; do NOT rewrite the historical record.
    The addendum states the measured truth: the port's live surface is
    `get_state_path` alone (`record_store_bridge` in `cli/scrape_flow.rs`, plus
    the state-path read in the same file); `load`, `save`, `load_or_default` and
    `load_for_export` have zero production callers; resume filtering flows
    through the v2 `RecordStorePort` (`load_or_init`), and `load_or_default` is
    NOT wired into resume. It also corrects the trait's own doc comment, which
    claims the seam covers four methods "in live use".

S4. The trait's doc comment in `crates/webfang_core/src/domain/persistence.rs`
    must stop claiming that the seam covers "the four state-file methods in live
    use (`get_state_path`, `load`, `save`, `load_or_default`)".

S5. The trait's doc comment references `[`load_for_export`](Self::load_for_export)`;
    that intra-doc link dies with the method and must be removed, or the
    `cargo doc` gate (`RUSTDOCFLAGS=-D warnings`) fails on a broken link.

## Tasks

- [ ] T1 · S3, S4, S5 — this spec — commit `docs(odd): the spec of #2243`
- [ ] T2 · S1, S2, S3, S4, S5 — prune the port, drop the orphan impl, append the dated ADR addendum, fix the trait docs — commit `refactor(domain): prune the dead store seam from StateStorePort`, with a `BREAKING CHANGE:` footer
- [ ] T3 · S1, S2, S3, S4, S5 — verification + the evidence in this Log — commit `docs(odd): the evidence of #2243`

## Log

L1 (owner, 2026-10-22): "Slice autorizada por el owner, aún no implementada.
Addendum fechado permitido en `docs/adr/0013-state-store-port-seam.md`." The
owner's authorization, as recorded in the plan: "Slice nueva autorizada: el
owner habilitó el addendum fechado en docs/adr/0013". The historical record is
not rewritten.

L2 (measured, parent): the map in the plan was partly wrong. `load_or_default`
is not a dead inherent method: it is a REQUIRED method of the public port trait
(`crates/webfang_core/src/domain/persistence.rs:465`), with a delegating impl
in `crates/webfang_core/src/infrastructure/export/state_store.rs:232`. The only
implementor of the port is `StateStore` (`state_store.rs:223`), so removing a
required method breaks the public contract ⇒ `BREAKING CHANGE:`.

L3 (measured, parent): the port's live surface is ONE method. `get_state_path`
is called by `record_store_bridge` (`crates/webfang_core/src/cli/scrape_flow.rs:156`)
and by the state-path read in the same file (`:1403`). `load`, `save`,
`load_or_default` and `load_for_export` have zero production call sites
(exhaustive grep over `crates/` plus a read of the three consumers:
`cli/export_flow.rs` and `cli/orchestrator.rs` only pass the handle through).
⇒ the ADR's claim that four seam methods are "in live use" is false for three
of them, not one.

L4 (decision, parent): the port is pruned; the concrete keeps its inherent
`pub fn load_or_default` and its seven unit tests. Reason: the authorized scope
is the seam (the public port), the concrete's behavior (stale-version discard +
`.bak`) is real and tested, and removing the concrete's public method would be a
second, unauthorized breaking change. `load` and `save` stay in the port as
candidates for a follow-up slice, outside the authorized scope.

L5 (decision, parent): `load_or_default` is NOT wired into resume. The live
path is RecordStore v2 (`RecordStorePort::load_or_init`).

L6 (to measure, T2): `ExporterError::StateStore`
(`crates/webfang_core/src/domain/exporter.rs:78`) loses its only direct
constructor with `load_for_export`; confirm it does not become dead code (it
keeps the `#[from]` conversion) and that `cargo clippy --all-features -D warnings`
stays green.

L7 (constraint): no change to `Cargo.toml` or `Cargo.lock` ⇒ the seed
compatibility key of `main` is unaffected ⇒ this slice is safe to land before
the PRs open. The branch is `fix/prune-dead-store-seam` from `main`; the
worktree identity proxy must be checked before committing.
