# Debugging & Trace Analysis

WebFang ships **built-in, always-available observability**. No external
collector, no feature flags, no infrastructure: run with `--trace-file` and
post-process the JSONL with `jq`.

> **Mandate:** every new feature or hot path must be observable. See the
> "Observability (MANDATORY)" section of `../../AGENTS.md`.

---

## The stack

| Layer | What it does | Always on? |
| :--- | :--- | :--- |
| **FileTraceLayer** | Writes every `tracing` span/event to a JSONL file (`--trace-file`) | ✅ Yes |
| **Correlation IDs** | Native `CorrelationId` (UUID v7 `trace_id` + `span_id`); one `trace_id` per operation, unique `span_id` per unit of work | ✅ Yes |
| **Structured logging** | `tracing-subscriber` to stderr (`-v`/`-vv`/`-vvv`, `--log-format json`) | ✅ Yes |
| **Tokio Console** | Async task/resource inspection for concurrency bugs | `--features console` |

There is **no OpenTelemetry** (removed in #356). If you need a metric, emit a
structured tracing event and query it from the JSONL.

---

## Generating a trace

```bash
{{#include ../samples/debugging/01-generate-trace.sh}}
```

Each line of `debug.jsonl` is a JSON object:

```json
{
  "timestamp": "2026-01-29T10:00:00.123Z",
  "level": "INFO",
  "target": "webfang_core::application::crawler::engine",
  "span": "crawl_page",
  "span_id": "0000000000000042",
  "parent_id": "0000000000000001",
  "trace_id": "0000000000000001",
  "fields": {
    "url": "https://example.com/page1",
    "depth": 1,
    "correlation_id": "00-01949e0e8b8e70008000000000000001-0000000000000042-01"
  }
}
```

Top-level `trace_id` is the root span `Id` (16-hex), one per run, EPHEMERAL
to the process/run (identity-within-run, not a durable global identity). Do
not persist or join on it across runs. Durable run correlation is the
`CorrelationId` UUID in `span_fields.trace_id` /
`span_fields.correlation_id` (W3C traceparent).

When a span closes, a second record type is emitted carrying a top-level
`span_duration_ms` (wall-clock milliseconds) — this is what the "Slowest spans"
query below reads:

```json
{
  "timestamp": "2026-01-29T10:00:00.456Z",
  "record": "span_close",
  "level": "INFO",
  "target": "webfang_core::application::crawler::engine",
  "span": "crawl_page",
  "span_id": "0000000000000042",
  "parent_id": "0000000000000001",
  "trace_id": "0000000000000001",
  "span_duration_ms": 333,
  "span_fields": {
    "url": "https://example.com/page1"
  }
}
```

---

## Query cookbook

A ready-made script lives at `scripts/analyze-trace.sh`. The most useful
queries:

### Reconstruct one run by top-level trace_id

```bash
{{#include ../samples/debugging/02-reconstruct-run.sh}}
```

`$ROOT` is the 16-hex root span `Id` (top-level `trace_id`, EPHEMERAL to the
run). It returns every page plus errors for that run. The same query is
`scripts/analyze-trace.sh debug.jsonl trace $ROOT`.

### All errors, with full context

```bash
{{#include ../samples/debugging/03-errors-with-context.sh}}
```

### Slowest spans (where the time goes)

```bash
{{#include ../samples/debugging/04-slowest-spans.sh}}
```

### Time distribution per pipeline stage

```bash
{{#include ../samples/debugging/05-time-per-stage.sh}}
```

### Crawl progress over time

```bash
{{#include ../samples/debugging/06-crawl-progress.sh}}
```

### Final crawl summary

```bash
{{#include ../samples/debugging/07-crawl-summary.sh}}
```

### Count operations by span type

```bash
{{#include ../samples/debugging/08-count-operations.sh}}
```

`span_close` records share the same `.span` name, so they must be excluded
when counting events (otherwise every span is double-counted).

### URLs that failed

```bash
{{#include ../samples/debugging/09-failed-urls.sh}}
```

### Was this run slow, or was it PACED?

Pacing and admission waits emit `rate limit wait` / `semaphore acquire`
events at DEBUG level. **`-vvv` is not required** for these: the
`--trace-file` layer always runs at TRACE, so they land in the JSONL
whenever a trace file was requested. They exist because a wait used to be
invisible — a throttled crawl looked exactly like a slow network.

```bash
{{#include ../samples/debugging/10-pacing-summary.sh}}
```

| Scope | Meaning |
| :--- | :--- |
| `crawl_discovery` | Per-page pacing inside the crawl engine |
| `cli_scrape` | Per-URL pacing in the CLI scrape phase |
| `batch_scrape` | Per-URL pacing in the batch scrape path |
| `http_client` | Per-session quota in the HTTP client |

A scope with `total_waited_ms` near zero means pacing was free and the
run's time went to the network. A large `cancelled` count means shutdown
was shedding queued work, not that the fetches were slow.

Which targets absorbed the wait (each event names the URL whose permit it
was waiting for, and the page's `correlation_id`):

```bash
{{#include ../samples/debugging/11-pacing-hot-targets.sh}}
```

Reconstruct one page's pacing from its own identity:

```bash
{{#include ../samples/debugging/12-page-pacing-by-correlation.sh}}
```

The same wait is also folded into the page's own span, so a page record
answers "how much of this page was pacing" without a join:

```bash
{{#include ../samples/debugging/13-page-pacing-from-span.sh}}
```

### Admission and dispatch queues

```bash
{{#include ../samples/debugging/14-admission-queues.sh}}
```

One row per `operation`, covering the downloader resource governor
(`browser.resource_governor.acquire`), the AI inference pool permit
(`ai.inference_pool.acquire`) and the AI pool dispatch queue
(`ai.inference_pool.dispatch`). `max_in_flight` is the pool occupancy the
waiter observed. If `max_waited_ms` is high and `max_in_flight` equals the
worker count, the pool was saturated — raise the worker budget or lower the
concurrency feeding it, rather than raising timeouts.

---

## Spans you will see

| Span | Emitted by | Key fields |
| :--- | :--- | :--- |
| `crawl_site` / `crawl_site_with_options` | `crawler::engine` | `correlation_id`, `trace_id`, `seed_url`, `max_depth`, `max_pages` |
| `crawl_page` | `crawler::engine::run_crawl_task` | `correlation_id`, `trace_id`, `url`, `depth` |
| `execute` | `pipeline::PipelineExecutor` | `url`, `stages` |
| `pipeline_stage` | `pipeline::PipelineExecutor` | `stage`, `url` |
| `export_batch` | `JsonlExporter` / `VectorExporter` / `FileExporter` | `exporter`, `documents` |
| `scrape_single_url` → `scrape_single` | `crawler::discovery::scrape_single_url` | `url` (outer), `correlation_id`, `trace_id`, `url` (inner, #501) |
| `scrape_with_config` | `scraper_service` | `url`, `correlation_id`, `trace_id`, `has_downloads` |
| `scrape_multiple_with_limit` | `scraper_service` | `urls`, `concurrency` |

Identity follows the root-child contract: the OPERATION owns one root
`CorrelationId`, and every unit of work derives `.child()` from it — same
`trace_id`, fresh `span_id`. In a CLI run the orchestrator mints the root
and announces it with a `run identity` event (`correlation_id`, `trace_id`
in `.fields`); `scrape_multiple_with_limit` does the same with a
`scrape_multiple identity` event. So in a multi-page scrape:

- Top-level `trace_id` is the **single logical run id**: the root span `Id`
  (16-hex), EPHEMERAL to the run. Reconstruct the whole run offline with
  `select(.trace_id == $ROOT)` — pages plus errors, no orphans.
- `span_fields.trace_id` is the **shared run-root UUID** (`CorrelationId`,
  durable across systems) across all page spans.
- `span_fields.correlation_id` (full W3C traceparent) is **unique per
  page**; its trace part is the run-root UUID without dashes.

Identity is declared **at span creation time**: FileTraceLayer snapshots
the span's declared fields in `on_new_span`, and `#[instrument]` can only
see function arguments. Fields that are only known at the END of an
operation (an outcome, an HTTP status) are therefore declared
`tracing::field::Empty` and filled in with `tracing::Span::record`, which
`on_record` merges into the same snapshot — they reach the JSONL both in
the `span_fields` of later events and in the `span_close` record. `ScrapedContent` and the RAG exports carry the same identity, so
an exported document's `correlation_id` matches its page's
`span_fields.correlation_id`:

```bash
{{#include ../samples/debugging/15-identity-queries.sh}}
```

Events (not spans): `run identity`, `scrape_multiple identity`,
`crawl progress`, `crawl completed`, and any `log_scrape_error(...)` error
carrying `error`, `url`, `stage`, `trace_id`.

---

## Concurrency debugging (Tokio Console)

For deadlocks, starved tasks, or async resource leaks, use the Tokio Console:

```bash
{{#include ../samples/debugging/16-tokio-console.sh}}
```

This opens an interactive TUI showing live tasks, their states, and poll times.

---

## Troubleshooting

See [troubleshooting.md](troubleshooting.md) for common problems (slow
crawls, silent page failures, WAF blocks, async deadlocks, poor content)
and how to diagnose each with the trace queries above.

---

## For contributors

When you add a hot path or operation, follow the observability mandate in
`AGENTS.md`:

- `#[instrument(skip(...), fields(url = %url, ...))]` on the function.
- Propagate the operation's `CorrelationId`; derive `.child()` per unit of work.
- Use `log_scrape_error(...)` on error paths (never a bare `warn!` for an
  operational error).
- Use `.instrument(span)` on async futures — never hold `span.enter()` across
  `.await`.
- Verify with: `webfang ... --trace-file debug.jsonl -vvv` and the queries above.

---

## Correlating a wait with the page that paid it

A pacing wait happens BEFORE the page span is created (the permit is taken
before any socket opens), so it is emitted as its own event rather than
inside the page span. It is still attributable, twice over:

```bash
{{#include ../samples/debugging/17-correlate-wait-with-page.sh}}
```

A page where `paced_ms` is a large share of `total_ms` was queued, not
slow. That distinction is the whole reason the wait emits at all.
