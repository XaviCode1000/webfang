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
