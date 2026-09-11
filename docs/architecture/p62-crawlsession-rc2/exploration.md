# Exploration: P6-2 / RC-2 — `CrawlSession` run abstraction

Baseline: `main` @ `95a368b7`. Design artifact — no production code touched.

## Current state

A crawl run has **no owner**. Per-run state is assembled in five places, each of
which re-derives a subset of the knobs:

| Site | What it assembles | Path |
|---|---|---|
| `Engine::new` | mints its own `CorrelationId`, builds `RobotsPort` via container, hardcodes `checkpoint_interval: 100`, builds `BudgetModel` | `application/crawler/engine.rs:164` |
| `Engine::build_task_ctx` | rebuilds 19 fields + the four `Production*` port wrappers + `build_static_fetcher()` + `build_link_extractor()` on every run | `engine.rs:799` |
| entry fns | `crawl_site` / `crawl_site_capturing` / `crawl_site_with_options` — three public functions, each minting a fresh `CorrelationId` | `engine.rs:1201,1217,1310` |
| CLI discovery | re-derives `EngineOptions` from `CrawlOptions` + `PersistenceMode`, then *chooses* an entry fn | `cli/url_discovery.rs:110` |
| batch | `process_single_url` → `crawl_site[_capturing]`, no options at all | `application/batch/processor.rs:299` |
| MCP | `crawl_site` tool → `crawl_site(config)`, no options | `webfang_mcp/src/mcp_server/handlers/scraping.rs:393` |
| benchmark | `crawl_site_with_options` with its own `EngineOptions` | `webfang_benchmark/src/runner.rs:166` |

`EngineOptions` (14 fields) mixes three different concerns: run policy
(checkpoint path/interval, robots, autoscale), transport policy (`js_strategy`,
`tls_emulation`, retries/backoff, `obscura_binary`), and injected seams
(`downloader_factory`, `content_sink`).

`CrawlTaskCtx` is the *de facto* session object — but it is `pub(crate)`, has no
validation, no lifecycle, no persistence authority, and is constructed by the
executor rather than by the caller.

## Observable consequences (not aesthetic)

1. **Per-entry-point capability loss.** MCP `crawl_site` cannot express
   checkpointing, session pooling, JS rendering or content capture — those exist
   only behind `crawl_site_with_options`. Every knob must be remembered at N call
   sites. This bug class is already ticketed twice: #1229 (`ignore_robots`
   dropped) and F-52-a / #1279 (`--js-strategy` dropped on the CLI discovery
   path). Both were fixed *at one call site each*.
2. **Two run identities inside one invocation.** `cli/orchestrator.rs:135` mints
   `root_correlation` for the scrape plane; `Engine` mints its own for the crawl
   plane — and the batch branch comments say so explicitly ("the crawl Engine…
   mints its own run-root identity per crawl — do not mint one here"). Trace
   reconstruction across discovery → scrape → export is therefore not guaranteed
   by construction. `application/resume.rs::RunId` is a *third* identity, durable,
   in `RecordStore` v2.
3. **Checkpoint/resume decisions are silent.** The F-01 delete-vs-save branch
   (`engine.rs:729`) is not emitted as a trace field, so an operator cannot tell
   from `--trace-file` whether a run left resumable state behind.
4. **P6-1 already closed the output shape.** Issue #1270: "Follow-ups
   (batch/discover tools, **P6-2 persistence**, G2/G4)". RC-2 is the run-semantics
   counterpart of RC-1's record-shape unification.

## Approaches

| # | Approach | Pros | Cons | Effort |
|---|---|---|---|---|
| A | `CrawlSession` as validated run **parameter object**, `pub(crate)`, consumed by `Engine`; `crawl_site*` stay authoritative | minimal blast radius; no test churn; makes parity *possible* without forcing it; reversible | leaves three entry fns alive; duplication becomes *reachable* but is not *removed* | Low |
| B | `CrawlSession` as the **public owner**; `Engine::run` demoted, `CrawlSession::run()` is the entry | removes the duplication instead of containing it; single identity by construction | touches behavioral tests, benchmark, MCP, TUI; ~4× A; needs deprecation cycle | High |
| C | `CrawlSessionPort` in `domain` + impl in `application` | textbook inversion | `domain` must name run types that do not exist yet (`EngineOptions`, `PersistenceMode` already covers part of it); a port with exactly one implementation and zero second-impl demand — same shape ADR-0014 rejected for SQLite | Med |

## Recommendation

**A now, B as the stated destination, C rejected.** A is the smallest slice that
makes the divergence structurally impossible to *worsen*: once a validated
`CrawlSession` exists, adding a knob means adding one builder field, not N call
sites. B is then a widening + deprecation, not a redesign. C is deferred until a
second implementation is demanded.

Slice plan: (1) seam + `Engine::from_session`; (2) CLI discovery + batch consume
it; (3) MCP run-parity (needs the orchestrator's P2 decision); (4) deprecate
`EngineOptions` + merge entry functions.

## Risks

- Mechanical move that silently changes behavior — mitigated by the checkpoint /
  determinism / parity tests that already pin this area (see design.md §
  Verification).
- Name collision with the existing `SessionPort` / `SessionId` (HTTP cookie +
  session-health pool, `domain/session_port.rs`). `CrawlSession` must never be
  shortened to `Session`.
- Doc drift: `AGENTS.md:146` and `README.md:323` document an `AppError` with 6
  variants; `grep -rn AppError crates/` returns **zero** production hits. Do not
  invent the level to satisfy the doc.

## Ready for proposal

Yes — and it went ahead: the four decisions came back signed by the orchestrator
(ownership = seam consumed by `Engine`; MCP parity = mandatory slice 3; no persisted
format change; everything `pub(crate)` in slice 1). See ADR-0017 and `design.md`
§Decisions signed by the orchestrator.
