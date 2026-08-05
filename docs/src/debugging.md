# Debugging & Trace Analysis

WebFang ships **built-in, always-available observability**. No external
collector, no feature flags, no infrastructure: run with `--trace-file` and
post-process the JSONL with `jq`.

> **Mandate:** every new feature or hot path must be observable. See the
> "Observability (MANDATORY)" section of `AGENTS.md`.

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
# Full trace + verbose logging
webfang --url https://example.com --trace-file debug.jsonl -vvv

# Batch / crawl
webfang --url https://example.com --max-pages 100 --trace-file crawl.jsonl -v
```

Each line of `debug.jsonl` is a JSON object:

```json
{
  "timestamp": "2026-01-29T10:00:00.123Z",
  "level": "INFO",
  "target": "webfang_core::application::crawler::engine",
  "span": "crawl_page",
  "span_id": "0000000000000042",
  "trace_id": "01949e0e8b8e70008000000000000001",
  "fields": {
    "url": "https://example.com/page1",
    "depth": 1,
    "correlation_id": "00-01949e0e8b8e70008000000000000001-0000000000000042-01"
  }
}
```

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

### Reconstruct one operation (crawl / scrape) by trace_id

```bash
TRACE=01949e0e8b8e70008000000000000001
jq -c "select(.trace_id == \"$TRACE\" or (.fields.trace_id? // \"\" | contains(\"$TRACE\")))" debug.jsonl
```

### All errors, with full context

```bash
jq -c 'select(.level == "ERROR") | {target, url: .fields.url, stage: .fields.stage, error: .fields.error, msg: .fields.message}' debug.jsonl
```

### Slowest spans (where the time goes)

```bash
jq -r 'select(.span_duration_ms != null) | [.span_duration_ms, .span] | @tsv' debug.jsonl | sort -rn | head -20
```

### Time distribution per pipeline stage

```bash
jq -r 'select(.span == "pipeline_stage") | .fields.stage' debug.jsonl | sort | uniq -c | sort -rn
```

### Crawl progress over time

```bash
jq -c 'select(.fields.message? == "crawl progress") | {pages: .fields.pages_crawled, pct: .fields.progress_pct, eta_s: .fields.eta_secs}' debug.jsonl
```

### Final crawl summary

```bash
jq -c 'select(.fields.message? == "crawl completed")' debug.jsonl
```

### Count operations by span type

```bash
jq -r '.span // "event"' debug.jsonl | sort | uniq -c | sort -rn
```

### URLs that failed

```bash
jq -r 'select(.level == "ERROR") | .fields.url // empty' debug.jsonl | sort -u
```

---

## Spans you will see

| Span | Emitted by | Key fields |
| :--- | :--- | :--- |
| `crawl_site` / `crawl_site_with_options` | `crawler::engine` | `correlation_id`, `trace_id`, `seed_url`, `max_depth`, `max_pages` |
| `crawl_page` | `crawler::engine::run_crawl_task` | `correlation_id`, `trace_id`, `url`, `depth` |
| `execute` | `pipeline::PipelineExecutor` | `url`, `stages` |
| `pipeline_stage` | `pipeline::PipelineExecutor` | `stage`, `url` |
| `export_batch` | `JsonlExporter` / `VectorExporter` / `FileExporter` | `exporter`, `documents` |
| `scrape_single_url` → `scrape_single` | `crawler::discovery::scrape_single_url_for_tui` | `url` (outer), `correlation_id`, `trace_id`, `url` (inner, #501) |
| `scrape_with_config` | `scraper_service` | `url`, `correlation_id`, `trace_id`, `has_downloads` |
| `scrape_multiple_with_limit` | `scraper_service` | `urls`, `concurrency` |

Identity follows the root-child contract: the OPERATION owns one root
`CorrelationId`, and every unit of work derives `.child()` from it — same
`trace_id`, fresh `span_id`. In a CLI run the orchestrator mints the root
and announces it with a `run identity` event (`correlation_id`, `trace_id`
in `.fields`); `scrape_multiple_with_limit` does the same with a
`scrape_multiple identity` event. So in a multi-page scrape:

- `span_fields.trace_id` is the **shared run-root UUID** across all page
  spans — the whole run is reconstructable by it.
- `span_fields.correlation_id` (full W3C traceparent) is **unique per
  page**; its trace part is the run-root UUID without dashes.

Identity is declared **at span creation time** because FileTraceLayer
snapshots span fields in `on_new_span` — fields recorded later never reach
the JSONL. `ScrapedContent` and the RAG exports carry the same identity, so
an exported document's `correlation_id` matches its page's
`span_fields.correlation_id`:

```bash
# Reconstruct an entire run by the shared run-root trace_id
ROOT=01949e0e-8b8e-7000-8000-000000000001
jq -c "select(.span_fields.trace_id == \"$ROOT\")" debug.jsonl

# The run-root identity (the `run identity` event carries it in .fields)
jq -c 'select(.message? == "run identity") | .fields' debug.jsonl

# Every page identity present in the trace
jq -r '.span_fields.correlation_id // empty' debug.jsonl | sort -u

# Reconstruct one page's scrape by its correlation_id
CID=00-01949e0e8b8e70008000000000000001-0000000000000042-01
jq -c "select(.span_fields.correlation_id? == \"$CID\")" debug.jsonl
```

Events (not spans): `run identity`, `scrape_multiple identity`,
`crawl progress`, `crawl completed`, and any `log_scrape_error(...)` error
carrying `error`, `url`, `stage`, `trace_id`.

---

## Concurrency debugging (Tokio Console)

For deadlocks, starved tasks, or async resource leaks, use the Tokio Console:

```bash
RUSTFLAGS="--cfg tokio_unstable" cargo run --features console -- --url https://example.com
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
