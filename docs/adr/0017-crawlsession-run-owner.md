# ADR 0017: CrawlSession — a Single Owner for One Crawl Run

- **Status:** Accepted — signed by the orchestrator 2026-09-09 (P1 = A, P2 = seam now
  with MCP parity mandatory in slice 3, P3 = no persisted format change, P4 = all
  `pub(crate)` in slice 1). Design-only: this ADR authorizes no code and no delivery.
- **Date:** 2026-09-09 (numbered 0017 on landing, 2026-09-11 — ADR-0016 was taken in the
  interim by the interruption/resume lifecycle; see *Related* and *Landing notes*)
- **Deciders:** Project Architect, `webfang` maintainers
- **Related:** ADR-0010 / 0011 / 0012-B (intra-crate direction), ADR-0013 (`StateStorePort`),
  ADR-0014 (`domain::persistence`, no new backend), ADR-0015 (WAF stays in infra),
  **ADR-0016** (interruption/resume lifecycle — pins the *decision* authority for
  interruption; this ADR pins the *data* authority — read the two together, see
  *Landing notes* #2)
- **Issues:** #1270 (P6-1 → "P6-2 persistence"), #1282 (RC-1), #1279 (F-52-a), #1229,
  #1234 (F-39), #1214, #501 / #519 / #1119 (observability + async precedents),
  #1288 (slice index), #1343 (MCP run-parity, open)
- **Supersedes:** —

## Context

A crawl run has no owner. Per-run facts are assembled independently at seven sites
(**state at design time, 2026-09-09** — the first two rows were removed by slices 1-4, see
*Landing notes* #4):

| Site | Assembles |
|---|---|
| `Engine::new` | own `CorrelationId`, `RobotsPort` via container, `checkpoint_interval: 100`, `BudgetModel` |
| `Engine::build_task_ctx` | 19 fields + the four `Production*` port wrappers + `build_static_fetcher()` + `build_link_extractor()`, rebuilt per run |
| `crawl_site` / `crawl_site_capturing` / `crawl_site_with_options` | three public entries, each minting its own identity |
| `cli/url_discovery.rs::discover_urls_unified` | re-derives `EngineOptions` from `CrawlOptions` + `PersistenceMode`, then picks an entry |
| `application/batch/processor.rs::process_single_url` | `crawl_site[_capturing]`, no options |
| `webfang_mcp/…/scraping.rs::crawl_site` | `crawl_site(config)`, no options |
| `webfang_benchmark/src/runner.rs` | its own `EngineOptions` |

`EngineOptions`' 14 fields mix three concerns — run policy (checkpoint, robots,
autoscale), transport policy (JS strategy, TLS profile, retries/backoff, Obscura
binary) and injected seams (`downloader_factory`, `content_sink`). Because the
concerns are not separated, a knob can be dropped at one call site without a
compile error, and it has happened twice: #1229 (`ignore_robots`) and F-52-a / #1279
(`--js-strategy` silently degraded every CLI crawl to static rendering). The MCP
crawl tool still cannot express checkpointing, session pooling, JS rendering or
content capture at all.

Two run identities coexist in one invocation: `cli/orchestrator.rs` mints
`root_correlation` for the scrape plane while `Engine` mints its own for the crawl
plane (the batch branch documents this: "do not mint one here"). `RunId`
(`application/resume.rs`) is a third, durable, inside `RecordStore` v2. And the F-01
delete-vs-save checkpoint decision is invisible in traces.

RC-1 / P6-1 unified the *record* a run emits (#1270, #1282). The remaining
divergence is the *run* itself — issue #1270 names it "P6-2 persistence".

## Decision

1. **`CrawlSession` (application layer, `application/crawler/session.rs`) is the
   sole holder and validator of run-level facts**: identity, `Arc<CrawlerConfig>`,
   persistence position, transport policy, injected ports, cancellation. It is
   immutable after `build()`; `build()` validates completely before any worker
   starts and degrades honestly where degradation is safe (unwritable checkpoint
   directory → no-checkpoint + logged, never a silent fake).

   **"Cancellation" here means sole *holder*, not sole *decider*.** The session mints
   the one `CancellationToken` and owns it as run data; `Engine::from_session` adopts
   that same token into the engine (`engine.rs`, `engine.cancel_token =
   session.cancel_token()`) and `Engine::cancel_run` is the only place that fires it.
   This is *not* a second interruption authority: ADR-0016 assigns the decision to
   `Engine` ("nothing else may decide to stop the run") and stays authoritative on that
   point. Ownership of the fact and ownership of the decision are different concerns,
   and only the first one belongs to the session.
2. **`Engine` stays the executor.** `Engine::from_session(..)` replaces the knob
   bag; `Engine::run()` semantics are unchanged. `CrawlTaskCtx` is *derived* by the
   session (`task_ctx()`) and built once per run.
3. **`EngineOptions` splits into `PersistencePolicy` / `TransportPolicy` /
   `CrawlPorts`** (all three `pub(crate)`, per P4), so no caller breaks in the first
   slice. The transitional `From<EngineOptions>` impls this ADR originally sketched
   never landed and are not needed: the conversion happens explicitly at the entry
   functions (`TransportPolicy::from(&options)`, `CrawlPorts { ... }` field-by-field),
   which keeps the mapping visible at the call site instead of hidden in a `From`
   impl. `PersistenceMode` becomes the *only* persistence input to a
   run — `checkpoint_path` and `checkpoint_interval` stop being independently
   settable.
4. **Nothing persisted changes.** `CrawlCheckpoint` stays version 2 with CRC32 +
   atomic rename + `spawn_blocking` + `accept_version` discard + per-seed scoping +
   F-39 bounded frontier; `checkpoint_interval` keeps its 100 default;
   `ExportState {version:1}` and the `RecordStore` v2 contract are untouched. The
   session carries `CorrelationId` plus a log-only `run_label`; **no durable run
   identity is added** — persisting or joining `trace_id` across runs stays
   prohibited.
5. **Observability is part of the seam**: session span fields declared at span
   creation (#501), one run-identity event per run emitted by `begin()`, the
   existing periodic progress and 8-category summary retained, and
   `checkpoint_action = wrote|deleted|skipped` added to the summary.
6. **`CrawlSessionError` is application-layer**, mapped through the existing
   `ScraperError` → `ErrorClass` → `CliExit` chain, classified in
   `docs/error-classification-matrix.md` in the same change (rows 31-32).
   **No `AppError` level is introduced.** At design time `AGENTS.md:146` and
   `README.md:323` documented such a tier with 6 variants while
   `grep -rn AppError crates/` returned zero production hits — i.e. documentation
   drift, not a missing layer. **That drift has since been corrected in `main`**
   (both files now document the real `ScraperError` / `DomainError` / `InfraError`
   stratification, zero `AppError` mentions), so the tier stays rejected on merit
   and this row records a historical discrepancy rather than a live one.
7. **Naming is fixed: `CrawlSession`, never `Session`.** `SessionPort` / `SessionId`
   (`domain/session_port.rs`) already denote the HTTP cookie + session-health pool.
8. **Layering holds.** The session depends only inward on domain types and the
   existing composition root; concretes stay constructed in
   `application/container.rs` (ADR-0012-B). No new intra-crate allowlist entry
   (terminal 2), no new inter-crate edge.
9. **Sequenced slices** — as *signed* on 2026-09-09: (1) seam; (2) CLI discovery +
   batch consume it; (3) MCP run-parity — **signed as a mandatory follow-up, not
   optional**: it carries its own delta spec and snapshots, and until it lands the MCP
   `crawl_site` tool keeps no checkpoint, session pool, JS strategy or content capture;
   (4) deprecate `EngineOptions` and merge entry functions. Slice 1 is additive and
   revertible as one commit, and is `pub(crate)` throughout, so it ships as a `refactor`
   with no `type:breaking-change` label.

   **The executed slices renumbered items (2) and (3); only the ADR's (4) is still
   open, now tracked as #1343.** See *Landing notes* #5 — read this list as the plan of
   record, and that note as the state of the world.

## Consequences

- Adding a run knob becomes a one-field builder addition instead of an N-call-site
  convention, so the #1229 / #1279 bug class stops being reachable by omission.
- One run = one root identity by construction, so `--trace-file` reconstructs an
  entire invocation (discovery + crawl + scrape + export) without call-site
  discipline; `trace_correlation_test.rs` generalizes from scrape to crawl.
- Resume behavior becomes observable (`checkpoint_action`), which makes the F-01
  and #1214 regressions testable instead of inferable.
- `EngineOptions` becomes a compatibility view rather than the design's spine;
  slice 4 can retire it deliberately.
- Slice 1 is a `refactor` with no observable behavior change; slices 2–4 carry real
  risk and each gets its own spec delta, snapshots and rollback.
- **Declared destination (not this change):** making the session the public owner and
  demoting `Engine::run` (alternative B) remains the target end-state; it was deferred
  for blast-radius reasons, not rejected on merit.
- SDD artifacts cannot live at their conventional paths here: `.gitignore:39` ignores
  `openspec/` and `.gitignore:41` ignores `specs/` at any depth. `docs/architecture/
  p62-crawlsession-rc2/` plus this ADR is therefore the single canonical home
  (signed as P5), with `openspec/changes/crawl-session-abstraction/` kept only as
  ignored scratch.
- Pending decisions P1–P4 (ownership depth, MCP parity, durable run identity, public
  surface) gate the *scope* of slices 2–4, not the validity of slice 1.

## Alternatives rejected

| Option | Verdict |
| --- | --- |
| `CrawlSession` immediately replaces `Engine::run` as the public entry | **Not rejected on merit — deferred and declared** as the destination (signed P1). Rejected only as the *first* slice: it touches MCP, TUI, benchmark and the behavioral suite at once, and blows the 400-line review budget in one PR |
| `domain::CrawlSessionPort` (port in domain, impl in application) | A port with one implementation and no named second implementation — the same shape ADR-0014 rejected for a speculative SQLite backend |
| Fix the divergences call site by call site | Already attempted twice (#1229, #1279); each fix repaired one site and left the structure that produced the bug |
| Add `run_id` to `CrawlCheckpoint` for cross-run audit | Schema v3 → `accept_version` discards every existing checkpoint, expands a closed matrix contract, no consumer asked for it |
| Add the `AppError` level AGENTS.md documents | Invents a tier to match a stale doc instead of correcting the doc |
| Rename `SessionPort` to make room for `Session` | The HTTP-session name is load-bearing and sealed in `infrastructure::network::session_pool`; renaming it spends review budget on a non-problem |
| Merge engine checkpoint and export `RecordStore` into one envelope | Two planes with different guarantees (crash-resume vs committed-only skip); ADR-0014 already fixed the unified *domain* home, unifying the *formats* is a different ADR |

## Landing notes (verified against `main` on 2026-09-11)

This ADR was written on 2026-09-09 as a design of record; 76 commits landed since. These
notes were each checked against the code, not inferred from issue titles.

1. **Numbering.** Landed as **ADR-0017**. ADR-0016 was taken in the interim by
   `0016-interruption-resume-lifecycle.md` (Accepted 2026-09-10).

2. **Cancellation: two ADRs, one authority, no conflict.** Read together, they describe a
   split nobody would guess from either alone. This ADR owns the *fact* (session mints and
   holds the one `CancellationToken`); ADR-0016 owns the *decision* (`Engine` is the only
   thing that may stop a run). The reconciling mechanism is
   `engine.cancel_token = session.cancel_token()` in `Engine::from_session` — adoption, not
   duplication — with `Engine::cancel_run` the sole firer and the session copy left intact
   for close-path verdicts.

3. **Slice numbering diverged from the plan.** The child issues renumbered items (2) and
   (3) of decision 9, so an issue number no longer maps to the ADR's ordinal:

   | ADR decision 9 | child issue | state |
   |---|---|---|
   | (1) seam | #1287 | merged |
   | (2) CLI discovery + batch consume it | #1289 → **checkpoint IO migration** | closed |
   | (3) MCP run-parity (mandatory) | #1290 → **MCP *export* parity** (F-16) | closed |
   | (4) deprecate `EngineOptions` + merge entries | #1291 → **legacy removal** | closed |

   Every child issue is closed and #1288's checklist still shows slices 2-4 unchecked.
   The umbrella's checkbox state is not a reliable indicator of what landed.

4. **The context table is historical.** `Engine::new` and `Engine::build_task_ctx` no
   longer exist (`60766e31` retired the legacy build; `from_session` is the only
   constructor). `EngineOptions` is still public (14 fields, three concerns) and still
   hand-assembled by CLI discovery, batch, the MCP crawl tool and the benchmark runner —
   so the *problem* this ADR describes persists on the MCP plane even though the seam is in.

5. **The mandatory follow-up is still open and now tracked.** #1290 delivered export
   parity, which is a different axis: `git grep -c CrawlSession crates/webfang_mcp/` is
   **0**. The run facts the P2 signature promised to make reachable over MCP — checkpoint,
   session pool, `JsStrategy` / post-load wait, content capture — are still unreachable
   from the crawl tool, which is the exact failure mode behind #1229 and #1279.
   Follow-up: **#1343**.

6. **Deviation from decision 3.** The transitional `From<EngineOptions>` impls were never
   written; see the inline note on decision 3 for the substitute that did land.
