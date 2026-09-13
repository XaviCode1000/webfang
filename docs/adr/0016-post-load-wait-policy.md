# ADR 0016: Post-Load Wait Policy — Network-Idle Default with Fixed-Wait Override (F-52-b)

- **Status:** Accepted — implemented and shipped. The design questions below were answered
  by issue #1277 (closed 2026-09-11): `PostLoadWait::Idle` is the default, the operator
  surface is `--js-wait` / `WEBFANG_JS_WAIT`, and the settle cost is observable via the
  `post_load_wait_ms` tracing field. Kept as the record of *why* idle-with-ceiling was chosen
  over a fixed wait; the F-52-b contract tests pin the behavior.
- **Date:** 2026-09-09
- **Deciders:** Project Architect (sole orchestrator, delegated design authority)
- **Related:** issue #1277, evidence `evidence/mode-d/JS-RENDERING.md` (F-52-b),
  ADR-0012 (downloader factory seam), #509 (cancel-aware waits), F-52-a propagation lesson

## Context

`ChromiumoxideDownloader::fetch` navigates and captures `page.content()` immediately.
The evidence sweep proves post-load hydration is lost 0/3 for mutations at ≥5 ms and up
to 1200 ms, so `JsStrategy::Full` fails the one job its docs advertise (SPA round-trip).
The fix shape is an API decision, not a patch: fixed wait / configurable fixed wait /
wait-for-selector / network-idle were the candidates.

## Decision

The post-load settlement is a **configuration type in `domain`** — `PostLoadWait`
(`Idle` | `Fixed(ms)` | `None`), default **`Idle`** — carried in `DownloaderSpec` and
applied **only inside the chromium fetch**, after navigation and before `page.content()`.
Network-idle is detected via CDP network events with a 500 ms quiet window; the total
wait is bounded by `timeout_secs`. The wait is **best-effort**: ceiling expiry or
CDP-subscription failure logs (WARN) and proceeds — it can never fail the fetch.
`--js-wait` (env `WEBFANG_JS_WAIT`) is the operator surface; `none` restores historical
behavior; wait-for-selector is explicitly deferred.

## Consequences

- `JsStrategy::Full` (and Hybrid L3) captures post-load hydration without per-site tuning;
  the documented behavior and the actual behavior converge.
- Every chromium fetch pays up to the idle window/ceiling in latency; the cost is visible
  via `post_load_wait_ms` tracing fields, and `none` is the documented opt-out.
- Never-idle sites (SSE, websockets, long-polling) degrade to the ceiling bound with a
  WARN, never a hang or data-loss silence.
- Guard-chain ordering is untouched: the wait sits after navigation, before body read;
  no new dial, no rate-budget consumption, no retry interaction.
- Future `wait-for-selector` reuses the same settle stage — the seam is the extension
  point, not the CLI flag count.

## Alternatives Rejected

- **Always-on fixed wait**: arbitrary constant; evidence falsified single-value windows
  (misses ≥5 ms already); taxes every fetch.
- **Fixed wait as the only mechanism**: operator must guess per-site; no automatic fix.
- **Wait-for-selector as default**: requires per-site knowledge, contradicts the
  automatic-escalation design of the hybrid stack, adds a second selection API.
- **Router- or engine-level sleep**: fires on the wrong layers, pollutes guard-chain
  ordering, and multiplies propagation sites (the exact F-52-a failure class).
