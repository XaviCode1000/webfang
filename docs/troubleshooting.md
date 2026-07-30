# Troubleshooting

Common problems and how to diagnose them with WebFang's built-in tracing.

> Generate a trace first: `webfang --url <URL> --trace-file debug.jsonl -vvv`,
> then query it with `scripts/analyze-trace.sh` or `jq`. See
> [debugging.md](debugging.md) for the full query cookbook.

---

## The crawl is slow

**Diagnose:**

```bash
scripts/analyze-trace.sh debug.jsonl slow 20      # slowest spans
scripts/analyze-trace.sh debug.jsonl stages       # time per pipeline stage
```

**Common causes:**

- A single stage dominates (e.g. `clean` with the AI feature) — check the
  `stages` distribution.
- Network latency / rate limiting — look for large gaps between `crawl_page`
  spans; consider `--delay` and concurrency tuning.
- Export bottleneck — check `export_batch` span durations.

---

## Pages are failing silently

Every operational error is logged as a structured `ERROR` event with `url`,
`stage`, and (when available) `trace_id`.

```bash
scripts/analyze-trace.sh debug.jsonl errors       # all errors with context
scripts/analyze-trace.sh debug.jsonl urls-failed  # unique failed URLs
```

**Common causes by `stage`:**

| `stage` | Meaning | Fix |
| :--- | :--- | :--- |
| `fetch` | HTTP/network failure or WAF challenge | Check connectivity; the site may be blocking — see WAF section below |
| `extract` | Content extraction produced too little text | The page may be JS-rendered or non-article; try a CSS `--selector` or JS rendering |

---

## WAF / bot detection blocks

```bash
scripts/analyze-trace.sh debug.jsonl waf          # WAF challenges + banned domains
```

If you see `WAF challenge detected` errors:

- The site is presenting a CAPTCHA / challenge page. WebFang bans the domain
  for the rest of the crawl to avoid hammering it.
- Try a different TLS fingerprint profile (`--tls-emulation`) or JS rendering.
- Slow down (`--delay`, lower concurrency) to avoid rate-limit triggers.

---

## I can't tell which logs belong to one page / one crawl

- One **crawl** shares a single `trace_id`. Filter by it:
  ```bash
  scripts/analyze-trace.sh debug.jsonl trace <trace_id>
  ```
- Each **page** is a `crawl_page` span with its own `span_id` under that
  `trace_id`.

---

## Non-deterministic snapshot failures in tests

`correlation_id` / `trace_id` are internal and `#[serde(skip)]` on scraped
output, so they never appear in scraped JSON/JSONL snapshots. If a *new*
field you added is non-deterministic (timestamps, ports, temp paths, random
IDs), redact it via `redact_nondeterministic()` in `tests/common/cli_harness.rs`.

---

## Async deadlocks / starved tasks

For concurrency bugs (a crawl hangs, tasks never complete), use the Tokio
Console:

```bash
RUSTFLAGS="--cfg tokio_unstable" cargo run --features console -- --url <URL>
```

This shows live task states and poll times, making stuck tasks visible.

---

## Empty or poor content

- `content extraction failed` (`stage: extract`) — the fallback extractor got
  less than the minimum content. The page is likely JS-rendered, an
  interactive app, or not an article.
- Try `--selector '.main-content'` (or the right CSS selector for the site),
  or enable JS rendering for SPA content.
