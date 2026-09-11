# Proposal: CrawlSession — one owner for one crawl run (P6-2 / RC-2)

## Intent

A crawl run has no owner. Per-run state — identity, persistence mode, transport
policy, injected ports, cancellation — is assembled independently by `Engine::new`,
`Engine::build_task_ctx`, three public entry functions (`crawl_site`,
`crawl_site_capturing`, `crawl_site_with_options`), the CLI discovery path, the
batch processor, the MCP `crawl_site` tool and the benchmark runner. Every knob
must therefore be re-remembered at every call site, and forgetting one is silent:
#1229 dropped `ignore_robots`, F-52-a / #1279 dropped `--js-strategy`, and the MCP
crawl tool still cannot express checkpointing, session pooling or JS rendering at
all. RC-1 (P6-1) unified the *record* a run emits; RC-2 unifies the *run* that
emits it.

## Scope

### In Scope

- `CrawlSession`: a validated, immutable-after-build description of one crawl run.
- `CrawlSessionBuilder` + a `PersistencePolicy` / `TransportPolicy` / `CrawlPorts`
  split of today's 14-field `EngineOptions`.
- `Engine::from_session(CrawlSession)` and `CrawlSession::begin()/finish()`;
  `CrawlTaskCtx` derived from the session instead of re-built by the executor.
- One run-root identity per session, emitted once, shared by discovery, crawl,
  scrape and export within a process.
- `#[instrument]` fields for the session span, plus a `checkpoint_action` field on
  the run summary (make the F-01 delete-vs-save decision visible in traces).
- Delta spec for the `crawl-session` capability; ADR-0017.

### Out of Scope

- Any change to persisted formats: `CrawlCheckpoint` v2, `ExportState {version:1}`,
  `RecordStore` v2, `COMPATIBILITY-MATRIX.md` rows.
- Changing `checkpoint_interval` (stays 100) or the resume *gate* itself.
- MCP run-parity behavior change (deferred to slice 3; pending decision P2).
- A `domain::CrawlSessionPort`, an `AppError` level, SQLite, fsync, OpenTelemetry.
- Public API removal: `crawl_site*` and `EngineOptions` stay as shims.

## Capabilities

### New Capabilities

- `crawl-session`: ownership, validation, lifecycle, identity, persistence
  positioning, cancellation and observability of a single crawl run.

### Modified Capabilities

None. `openspec/specs/` has no `crawler`/`persistence` capability yet, so there is
no existing requirement block to modify; `PersistenceMode` and the `StateStore`
version contract are consumed unchanged.

## Approach

Introduce the seam first, move the callers second. `CrawlSession` becomes the only
type allowed to decide *what is true for the whole run*; `Engine` keeps deciding
*how the run executes*. `EngineOptions` survives as a transitional view
(`From<EngineOptions> for TransportPolicy`) so no caller breaks in slice 1, and the
MCP/batch/CLI divergences become one-field builder additions rather than new entry
functions.

## Affected Areas

| Area | Impact | Description |
|---|---|---|
| `application/crawler/session.rs` | New | `CrawlSession`, builder, policies, `CrawlPorts` |
| `application/crawler/engine.rs` | Modified | `from_session`, `run` unchanged semantics; `build_task_ctx` derives |
| `application/crawler/crawl_task_ctx.rs` | Modified | constructed from the session |
| `application/crawler/mod.rs`, `application/mod.rs` | Modified | re-exports |
| `application/crawler/checkpoint.rs` | Untouched | format, CRC32, version gate identical |
| `cli/url_discovery.rs`, `application/batch/processor.rs`, MCP, benchmark | Slice 2+ | consume the session |

## Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Mechanical move changes behavior | Med | existing determinism / checkpoint / parity tests are the tripwire; no logic edits in slice 1 |
| Checkpoint lost on early drop | Med | `#[must_use]` session; `finish(self)` consumes; `Drop` performs no IO |
| Guard held across `.await` | Low | `#![deny(clippy::await_holding_lock)]` in the new module (engine.rs:19 precedent) |
| Name confusion with `SessionPort` | Med | never abbreviate to `Session`; ADR-0017 states the collision |
| Review-size blowout | Med | chained slices, ≤400 authored lines each |

## Rollback Plan

Slice 1 is additive: `crawl_site*` keep their exact current behavior because they
internally build a `CrawlSession` and delegate. Reverting the single
`git revert` of the seam commit restores the previous call graph; no persisted
artifact, feature flag, or data migration is involved, and no on-disk format is
read differently.

## Dependencies

- ADR-0012-B (composition root owns concretes), ADR-0013/0014 (persistence ports),
  error-classification matrix (closed contract — any new variant is classified in
  the same change).

## Success Criteria

- [ ] Every knob reachable by `crawl_site_with_options` is reachable by
      `CrawlSession::builder()` with no field added to two types.
- [ ] One crawl emits exactly one run-root `trace_id` shared by discovery, crawl
      and export spans in `--trace-file`.
- [ ] `cargo nextest run` green with zero snapshot churn.
- [ ] `checkpoint_action` observable on the summary event.
- [ ] No change to `CrawlCheckpoint` bytes for the same crawl.
