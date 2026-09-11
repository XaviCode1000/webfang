# Design: CrawlSession — one owner for one crawl run (P6-2 / RC-2)

Design artifact only. Baseline `main` @ `95a368b7`. No production code in this
change. Normative level of each decision is marked: **[DECIDED]** by repo evidence,
**[Pn FIRMADO]** = signed by the orchestrator on the interrogation question Pn.

All four questions are now signed: **P1 = (a)** seam consumed by `Engine`,
(b) declared destination, (c) rejected; **P2 =** seam now, MCP parity in slice 3
with its own delta spec + snapshots, recorded as a mandatory follow-up; **P3 =**
no persisted format change, `run_label` logs/traces only; **P4 = (a)** everything
`pub(crate)` in slice 1, so the PR is a `refactor` with no `type:breaking-change`.
**P5** (canonical artifact home) is resolved in `README.md`.

## Technical Approach

Split *what is true for the run* from *how the run executes*. `CrawlSession` (new,
`application/crawler/session.rs`) becomes the sole validator and holder of run-level
facts; `Engine` keeps the JoinSet, scheduler, collector, counters, rate limiter and
autoscale loop and receives a session instead of a knob bag. `EngineOptions` stays
compilable as a transitional view. Callers migrate in later slices, so slice 1 is
additive and revertible.

## Architecture Decisions

| # | Decision | Alternatives rejected | Rationale | Status |
| --- | --- | --- | --- | --- |
| D1 | `CrawlSession` = validated run description consumed by `Engine`; `Engine::run` stays the executor | (B) session replaces `Engine::run` as public entry; (C) `domain::CrawlSessionPort` | B is the destination but touches MCP/TUI/benchmark/behavioral tests at once; C inverts a port with one implementation and zero second-impl demand — the same shape ADR-0014 rejected for SQLite. Note `Engine::new` is already `pub(crate)` (engine.rs:164): outside the crate a crawl is reachable *only* through the three free functions, so adding one validated sibling is the cheap seam and removing two of them is the later consolidation | [P1 **FIRMADO** = A] B is the declared destination, C rejected |
| D2 | Name is `CrawlSession`, never `Session` | `Session`, `RunSession`, `CrawlRun` | `SessionPort`/`SessionId` (`domain/session_port.rs`) already mean HTTP cookie + session-health pool; a bare `Session` would be a semantic collision inside the same module tree | [DECIDED] |
| D3 | Three sub-policies split out of `EngineOptions`: `PersistencePolicy`, `TransportPolicy`, `CrawlPorts` | keep one 14-field struct | the 14 fields mix run policy / transport policy / injected seams; that mix is why `--js-strategy` (#1279) and `ignore_robots` (#1229) were each droppable at one call site | [DECIDED] |
| D4 | `PersistenceMode` (domain, ADR-0014) is the only persistence input to a session | keep `checkpoint_path` + `checkpoint_interval` settable independently | two independent setters for one decision is the F-01/#1214 bug class; the resolver is already pure and total | [DECIDED] |
| D5 | Session holds `CorrelationId` + a read-only `run_label` for logs; nothing new is persisted | add `run_id` to `CrawlCheckpoint` (schema v3) | AGENTS.md forbids persisting/ joining `trace_id` across runs; v3 breaks `accept_version`, discards every existing checkpoint, and expands a matrix contract for no named consumer | [P3 **FIRMADO** = no schema change; `run_label` logs/traces only] |
| D6 | `CrawlTaskCtx` is *derived* by the session (`task_ctx()`), constructed once per run | keep building it inside `Engine::build_task_ctx` | `build_task_ctx` today rebuilds the four `Production*` wrappers and re-calls `build_static_fetcher()` / `build_link_extractor()` per run; the wrapper set is a run fact, not an execution fact | [DECIDED] |
| D7 | `finish(self)` (consuming) owns the delete-vs-save checkpoint decision; `Drop` does no IO | keep it in `Engine::run` + `Engine::shutdown`; or do it in `Drop` | `Drop` IO is unfalsifiable and untraceable; consuming `finish` makes "run ended without a verdict" a compile-level smell (`#[must_use]` on the session) | [DECIDED] |
| D8 | Concrete construction stays in `application/container.rs`; session takes `Arc<dyn …>` only | build concretes in the session | ADR-0012-B: trait-in-domain, concrete-in-infra, DI-via-container; allowlist is at its terminal 2 entries — a new outward import would re-open a closed gate | [DECIDED] |
| D9 | MCP keeps today's exact behavior in slice 1; run-parity is **slice 3, mandatory** | land MCP run-parity now | parity is an observable change to the 36-tool surface and needs its own delta spec + snapshots. It must not silently drop out of the plan: until it lands, the MCP `crawl_site` tool keeps no checkpoint / session pool / JS / capture (see exploration.md) | [P2 **FIRMADO** = seam now, parity in slice 3 with own delta spec + snapshots] |
| D10 | `pub(crate)` in slice 1 | export `CrawlSession` publicly now | a public type is a contract with doctests and semver weight; widening is a deliberate slice-4 act. Consequence of the signature: slice 1 carries no public API change, so it is labeled `refactor`, **not** `type:breaking-change` | [P4 **FIRMADO** = A] |

## Interfaces / Contracts (signature sketch — not compilable, shape only)

```rust
// application/crawler/session.rs   [pub(crate) per D10]
#![deny(clippy::await_holding_lock)]           // engine.rs:19 precedent

#[must_use = "a crawl session that is never run leaks its ports"]
pub(crate) struct CrawlSession { /* immutable after build() */ }

pub(crate) struct CrawlIdentity { root: CorrelationId, run_label: String }

pub(crate) struct PersistencePolicy { mode: PersistenceMode, loaded: Option<CrawlCheckpoint> }
pub(crate) struct TransportPolicy {
    js_strategy: JsStrategy, tls_emulation: Profile, ignore_waf: bool,
    max_retries: u32, backoff_base_ms: u64, backoff_max_ms: u64,
    obscura_binary: String, session_pool_enabled: bool, autoscale_enabled: bool,
}
pub(crate) struct CrawlPorts {
    robots: Arc<dyn RobotsPort>,
    session_pool: Option<Arc<dyn SessionPort>>,
    downloader_factory: Option<Arc<dyn DownloaderFactory>>,
    content_sink: Option<Arc<dyn CrawlContentSink>>,
    pipeline: Option<Arc<PipelineExecutor>>,
    output_stages: Vec<Arc<Box<dyn OutputStage>>>,
}

impl CrawlSession {
    pub(crate) fn builder() -> CrawlSessionBuilder;
    pub(crate) fn identity(&self) -> &CrawlIdentity;
    pub(crate) fn config(&self) -> &Arc<CrawlerConfig>;
    pub(crate) fn begin(&mut self) -> BeginOutcome;      // loads checkpoint once; emits run-identity event
    pub(crate) fn task_ctx(&self) -> Arc<CrawlTaskCtx>;  // derives, never invents
    pub(crate) fn cancel(&self);                          // single cancellation authority
    pub(crate) async fn finish(self, run: RunOutcome) -> Result<CrawlResult, CrawlSessionError>;
}

impl CrawlSessionBuilder {
    pub(crate) fn config(self, CrawlerConfig) -> Self;
    pub(crate) fn persistence(self, PersistenceMode) -> Self;   // D4: sole persistence input
    pub(crate) fn transport(self, TransportPolicy) -> Self;
    pub(crate) fn ports(self, CrawlPorts) -> Self;
    pub(crate) fn identity(self, CrawlIdentity) -> Self;
    pub(crate) fn build(self) -> Result<CrawlSession, CrawlSessionError>;
}

impl Engine {
    pub(crate) fn from_session(session: CrawlSession) -> Result<Self, CrawlError>;
    pub(crate) async fn run(&mut self) -> Result<CrawlResult, CrawlError>;   // unchanged
}

impl From<EngineOptions> for TransportPolicy {}          // transitional shim
impl From<EngineOptions> for CrawlPorts {}               // transitional shim
```

## Async Rules (Tokio)

- No `Mutex`/`RwLock` guard across `.await`; enforced per-module by
  `#![deny(clippy::await_holding_lock)]`. Session state is read-only after
  `build()`. Still-mutable parts keep today's disciplines: atomics
  (`pages_crawled`, `error_count`, `error_breakdown`), `tokio::sync::RwLock` for the
  cookie bridge (#1119), `std::sync::RwLock` for `banned_domains` only in
  non-await scopes.
- Checkpoint IO stays `spawn_blocking` + `in_current_span()`
  (`Engine::persist_checkpoint`); directory creation happens in `begin()`, before
  any worker is spawned.
- No unbounded channel is introduced. Results keep flowing through the existing
  bounded `ResultsCollector` mpsc. A future `CrawlEvent` progress channel (slice 3+)
  MUST be `tokio::sync::mpsc::channel(n)` with `try_send` plus a drop counter.
- Spans attach with `.instrument(span)` / `in_current_span()`; no `span.enter()`
  crosses an `.await` (#519).
- Port traits keep manual `BoxFuture` desugaring (frozen decision #1). rust-analyzer
  is advisory here (#1034): `cargo check --all-targets --all-features` decides
  E0308.
- CPU-heavy work stays behind `CpuBridge.dispatch` (`spawn_blocking`); the session
  adds no compute path.

## Data Flow

```text
caller (CLI discovery | batch | MCP | benchmark | test)
   │  CrawlerConfig + PersistenceMode + TransportPolicy + CrawlPorts + identity
   ▼
CrawlSessionBuilder ──build()──> CrawlSession ──begin()──> loads CrawlCheckpoint (per-seed, v2)
   │                                                            │ restore visited/queued/banned
   ▼                                                            ▼
Engine::from_session ──run()──> crawl_loop ──> Arc<CrawlTaskCtx> (derived, once)
   │                                 │              │ per-page: CorrelationId.child()
   │                                 ▼              ▼
   │                          JoinSet workers ── fetch → pipeline → collector(mpsc) → queue
   ▼
RunOutcome ──finish()──> completed fully? delete checkpoint : save checkpoint
                          └─ emit "crawl completed" summary { totals, breakdown,
                               trace_id, run_label, checkpoint_action }
```

## Observability (mandatory)

- Session span: `#[instrument(name = "crawl_session", skip(…))]` with
  `correlation_id`, `trace_id`, `seed_url`, `max_depth`, `max_pages`,
  `checkpoint_enabled`, `session_pool`, `js_strategy`, `capture_enabled`,
  `resume_mode` — all declared **at span creation** (#501: `FileTraceLayer`
  snapshots fields in `on_new_span`; later-added fields never reach the JSONL).
- One `info!(correlation_id, trace_id, run_label, "run identity")` per run, emitted
  by `begin()` so run identity stops being call-site policy.
- Progress: keep `crawl progress` (pct, pages_per_sec, eta, trace_id), owned by the
  session clock so one invocation tells one progress story.
- Summary: keep all 8 `CrawlErrorCategory` counters, add `run_label` and
  `checkpoint_action = wrote|deleted|skipped` (today the F-01 decision is invisible
  in traces).
- Failures use `log_scrape_error(…, "session", Some(correlation), …)`; never a bare
  `warn!`/`eprintln!`.
- User-facing text Spanish; trace fields and log messages English. `run_label`,
  `correlation_id`, `trace_id` stay `#[serde(skip)]` on exported records and are
  redacted by `redact_nondeterministic()` in snapshots.

## Error Stratification

`CrawlSessionError` is an **application-layer** error (domain stays pure; the
failure is orchestration, not domain validation). Mapped to the existing chain —
`From<CrawlSessionError> for ScraperError`, then
`domain::error::ErrorClass::classify()`, then `cli/error.rs` class→`CliExit`. No new
level: AGENTS.md:146 and README.md:323 document `AppError (6 variants)` and
`grep -rn AppError crates/` returns zero production hits, so introducing it here
would invent a tier to match a stale doc.

| Variant | Class | Exit | Retry |
| --- | --- | --- | --- |
| `InvalidConfiguration(String)` | PermanentFatal | 78 | never |
| `CheckpointUnwritable { path, reason }` | PermanentFatal | 78 | never (degrades to no-checkpoint when honest) |
| `StaleResumeDiscarded` (not an error; logged) | — | — | — |
| `Cancelled` | special cell | 0 | — |
| `Internal(String)` | InternalFatal | 3 | never |

Every variant MUST gain a row in `docs/error-classification-matrix.md` in the same
change (closed contract, `261bdb66-…`).

## File Changes

| File | Action | Description |
| --- | --- | --- |
| `application/crawler/session.rs` | Create | session, builder, policies, `CrawlPorts`, `CrawlSessionError` |
| `application/crawler/mod.rs` | Modify | `mod session;` + `pub(crate)` re-exports |
| `application/crawler/engine.rs` | Modify | `from_session`; `build_task_ctx` delegates to `session.task_ctx()`; `Engine::new` internals unchanged |
| `application/crawler/crawl_task_ctx.rs` | Modify | add constructor from session; field set unchanged |
| `domain/error/crawl_error.rs` + `cli/error.rs` + matrix doc | Modify | new variant wiring |
| `application/container.rs` | Modify (minor) | expose port bundle builder if a helper is cleaner than per-caller assembly |
| `docs/architecture/p62-crawlsession-rc2/*`, `docs/adr/0017-*.md` | Create | this artifact set (`openspec/` is scratch — see README) |

## Migration / Rollout

No data migration, no feature flag, no format change. Slice 1 lands behind the
existing shims: `crawl_site*` build a session internally and delegate, so the
observable call graph is unchanged and one `git revert` restores it. Slices 2–4 are
each independently revertible PRs; `EngineOptions` deprecation is last and needs its
own `type:breaking-change` decision.

## Testing Strategy

| Layer | What to test | Approach |
| --- | --- | --- |
| Unit | builder validation matrix; policy splits; `From<EngineOptions>` round-trip; identity single-mint; `checkpoint_action` decision table | `#[tokio::test]` in `session.rs`, fake ports (`ports.rs` inline-double precedent) |
| Integration | entry-point equivalence: session-built run vs `crawl_site*` on same wiremock fixture | `tests/integration_engine_tests.rs`, `tests/behavioral/cli/checkpoint_determinism_test.rs` |
| Behavioral | run identity shared across phases; resume-state residue; Spanish error text; exit codes | `BehavioralTest` + `--trace-file` + insta snapshots with `redact_nondeterministic()` |
| Regression tripwires (must stay green unchanged) | checkpoint bytes, determinism, discovery parity/capture, JS timeout, WAF gauntlet | `tests/behavioral/cli/checkpoint_determinism_test.rs`, `tests/discovery_determinism_1237.rs`, `tests/discovery_parity_1232.rs`, `tests/discovery_capture_1229.rs`, `tests/engine_js_strategy_timeout_test.rs`, `tests/behavioral/cli/waf_gauntlet_test.rs`, `tests/behavioral/cli/trace_correlation_test.rs` |

Determinism contract for new tests: injected clock (domain `clock.rs`) instead of
`Instant::now()` in progress math; wiremock-only HTTP; TempDir-only filesystem; no
wall-clock or `correlation_id` in snapshots; `webfang_path()` (never
`assert_cmd::cargo_bin`); explicit `[[test]]` wiring in
`crates/webfang_core/Cargo.toml`.

## Threat Matrix

**N/A** — this design changes no routing, shell command, subprocess, VCS/PR
automation, executable-file classification or process integration. Row-by-row:
documentation-like paths N/A (no file classification); git repository selection,
commit state, push state, PR commands N/A (no VCS/PR surface; the design is
repo-internal Rust).

Adjacent boundary, recorded to avoid a false N/A: `TransportPolicy::obscura_binary`
is *carried* by the session and is consumed by a subprocess-owning downloader
("a path is invoked as given; a bare name is resolved from `PATH`", #787). This
change moves the field's owner, never its resolution or execution site, and MUST NOT
harden or reinterpret it. Any hardening is a separate change with its own matrix.

## Decisions signed by the orchestrator

- [x] **P1 = (a)** `CrawlSession` is the seam consumed by `Engine`; `Engine::run`
      stays the entry point. **(b)** — session as the public owner — is the declared
      destination in ADR-0017. **(c)** domain port rejected.
- [x] **P2 =** seam now, MCP run-parity in slice 3, with its own delta spec and
      snapshots, tracked as a mandatory follow-up (it does not fall out of the plan).
- [x] **P3 =** no persisted format changes. `CrawlCheckpoint` stays v2,
      `ExportState {version:1}` and `RecordStore` v2 stay as they are; `run_label`
      exists in logs and traces only.
- [x] **P4 = (a)** everything `pub(crate)` in slice 1 → PR labeled `refactor`, no
      `type:breaking-change`.
- [x] **P5 =** `docs/architecture/p62-crawlsession-rc2/` + `docs/adr/0017-*.md` is the
      single canonical home; `openspec/` stays ignored scratch.

The four signatures unblock slices 2–4 scope-wise; they authorize no delivery. Nothing
on this branch has been pushed and no PR has been opened.

## Sizing (review workload guard)

Slice 1 forecast: ~450–550 authored diff lines (new module ~200, engine rewiring
~120, error wiring ~40, tests ~120). `Decision needed before apply: Yes`.
`Chained PRs recommended: Yes`. `400-line budget risk: Medium-High` — split seam /
callers / MCP parity / `EngineOptions` deprecation as four deliverable work units.
