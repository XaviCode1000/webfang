# ADR 0013: StateStorePort — Domain Seam Over the Legacy State Store

- **Status:** Accepted
- **Date:** 2026-09-03
- **Deciders:** Project Architect, webfang maintainers
- **Related:** ADR-0012-B §3.H, issues #1097 (this slice), #1100 (consumer), #1061 (non-goal)

## Context

The legacy JSON-per-domain `infrastructure::export::state_store::StateStore`
concrete was named directly by `cli` (`scrape_flow`, `export_flow`,
`orchestrator`) and by `application::container` (dead `state_store` field —
verified zero live readers, left untouched). Resume filtering itself already
flows through the v2 `RecordStorePort` seam; the legacy handle survived only
as the bridge source (directory + domain derivation). `ExporterError::StateStore(#[from]
ScraperError)` was pre-wired with zero callers, and `StateStore::mark_processed` /
`is_processed` have zero production callers.

## Decision

- Add `domain::exporter::StateStorePort: Send + Sync` (object-safe: no
  generics, no `Self` returns) with the four seam-only methods in live use:
  `get_state_path`, `load`, `save`, `load_or_default`. `mark_processed` /
  `is_processed` are excluded from the port.
- Reuse `ScraperError` as the port error type (no new variants); the provided
  `load_for_export` maps failures into `ExporterError::StateStore` via `?` —
  the variant's first honest use.
- Construct only in `application::container::build_state_store` (composition
  root, mirroring `build_binary_writer`); `cli` consumes `Arc<dyn
  StateStorePort>` / `&dyn StateStorePort` and bridges to `RecordStore` via
  the single shared `scrape_flow::record_store_bridge` helper.
- Rank `cli/` as the outermost composition edge (`LAYER_RANK[cli]=-1`) in
  `scripts/check_intra_crate_direction.sh`: a `cli` source can never flag, so
  construction stays at the edge with no new allowlist entry.

## Consequences

- `application` / `domain` name no legacy state-store concrete on this path;
  the intra-crate gate stays green in strict mode with the 2 permanent
  ADR-0011 entries (no addition).
- No behavior change: creation stays lazy/infallible (pinned by the moved
  `test_build_state_store_succeeds_even_when_state_dir_is_a_file`); resume
  filtering still goes through `RecordStorePort`.
- `RecordStorePort` and the v2 persistence contracts are untouched.
- Input to #1100 (remaining `cli` concrete namings drain through the same
  edge); #1061 (lateral intra-layer references) is an explicit non-goal.

## Alternatives Rejected

| Option | Verdict |
|--------|---------|
| Port `mark_processed` / `is_processed` too | Dead surface — zero production callers; would widen the seam for nothing |
| New dedicated error type for the port | Rejected per budget — `ScraperError` reuse + the pre-wired `ExporterError::StateStore` mapping is sufficient |
| Allowlist entry for the `cli` namings | Unnecessary — the `cli=-1` rank expresses "outermost edge" structurally instead of exempting sites one by one |

## Addendum (2026-10-22) — the live surface of the port, measured

The historical record above is kept as written. This addendum corrects two
claims it makes, with the measurements taken when the dead seam was pruned
(#2243).

**Correction 1 — the Decision's "four seam-only methods in live use" was
false.** The Decision lists `get_state_path`, `load`, `save` and
`load_or_default` as the methods "in live use". The measured truth:

- `get_state_path` is the **only** method of the port with a production
  caller. `record_store_bridge` (`cli/scrape_flow.rs`) reads the state path
  through `&dyn StateStorePort` to derive the directory and the domain of the
  v2 record store. The only other reads of the state path through the port
  are inside unit tests (`cli/scrape_flow.rs`, `application/container.rs`),
  not production callers.
- `load` had zero production callers. `save` was never a method of this port
  at all — it was dropped together with the other dead state-store methods in
  #1140 — so the Decision's list named a method the seam does not declare.
- `load_or_default` and `load_for_export` also had zero production callers.

The trait's own doc comment repeated the same false claim ("Seam covers the
four state-file methods in live use"); it now states the measured surface.

**Correction 2 — the port was pruned.** Two methods were removed from
`StateStorePort`:

- `load_or_default` — a **required** method, so removing it changes the
  public contract of the port (the change ships with a `BREAKING CHANGE:`
  footer). Its delegating impl in the only implementor
  (`infrastructure/export/state_store.rs`, `impl StateStorePort for
  StateStore`) went with it.
- `load_for_export` — a **provided** method (the only direct constructor of
  `ExporterError::StateStore`).

Evidence: an exhaustive grep over `crates/` found zero production call sites
for either method, and the three consumers of the port handle
(`cli/export_flow.rs`, `cli/orchestrator.rs`, `cli/scrape_flow.rs`) were read
in full — they only pass the handle through or read the state path.
`StateStore` is the only implementor of the port.

**What did not change.** The inherent `pub fn load_or_default` on the
concrete `StateStore` survives, with its behavior (stale-version discard plus
`.bak` preservation) and its seven unit tests: that behavior is real and
tested, and removing the concrete's public method would be a second,
unauthorized breaking change. `ExporterError::StateStore` keeps its `#[from]`
conversion, so the variant is not dead code.

**Resume.** The live resume path is the v2 seam
(`RecordStorePort::load_or_init`); `load_or_default` was never wired into
resume.

**Follow-up (out of this slice).** `load` stays declared on the port without
production callers and remains a candidate for a follow-up slice. The `save`
of the historical list belongs to the v2 seam (`RecordStorePort::save` /
`RecordStore::save`), not to this port nor to the `StateStore` concrete.
