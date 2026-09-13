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

## A local or internal target discovers nothing

**Symptom.** You point WebFang at a `localhost`, `127.0.0.1`, `10.x`, `192.168.x` or
`169.254.169.254` target and get nothing back. How loud that is depends on how the target
is *spelled*:

```bash
$ webfang http://127.0.0.1:8080/ --dry-run     # an IP literal
Warning: SSRF detectado: la IP 127.0.0.1 del host '127.0.0.1' está prohibida (acceso a
red interna/cloud metadata bloqueado). La vista previa no descubrió URLs porque el guard
cortó la semilla antes de abrir conexión. Para un destino interno de confianza: …
$ echo $?
2
```

```bash
$ webfang http://localhost:8080/ --dry-run     # a name that resolves into the same range
Dry-run: 0 URL(s) would be scraped:
$ echo $?
0
```

The literal case names its own cause (since #1391): exit 2, the code a null discovery result
already uses, with no `Dry-run:` line to misread. The hostname case still reads as an empty
site, because the layer that refuses it — the connect-time resolver — answers with what
looks like a DNS failure, and the preview cannot tell that from a broken name. Either way,
the same target in a real crawl fails loudly.

**What happened.** The SSRF guard refused the target **before any socket opened**: zero
packets leave the process, so nothing is retried and nothing can time out. Discovery itself
still answers `Ok` with an empty URL list — inside a crawl, one refused URL is a legitimate
skip, not a failed run — and the engine's own page-error count (`crawl completed … errors: 1`)
is not part of that list. What changed in #1391 is that the preview asks the guard its own
question before calling the result a success. The guard policy is the long-standing one: the
scrape path has always paid it, the MCP crawl tools since #1355, and plain DOM discovery
since #1369 retired the last entry point that fetched without the full guard chain.

**What you see, by surface.** The same refusal, spelled two ways, produces five
different-looking outcomes:

| You ran | What you see | Exit |
| :--- | :--- | :--- |
| `webfang <literal-target> --dry-run` | `Warning: SSRF detectado: la IP … está prohibida …`, naming both hatches — and no `Dry-run:` line (#1391) | 2 |
| `webfang <hostname-target> --dry-run` | `Dry-run: 0 URL(s) would be scraped:` | 0 |
| `webfang <target>` (crawl) | `Failed to scrape <target>: URL inválida: SSRF detectado: la IP 127.0.0.1 del host '127.0.0.1' está prohibida (acceso a red interna/cloud metadata bloqueado)` | 69 |
| `webfang <target> --single-page` | the same `SSRF detectado` line — the scrape path carries the same guard | 69 |
| `discover_urls_unified` / `discover_urls_recursive` (library) | `Ok` with an empty `Vec` | — |

Two exit codes for one refusal, and both are right. The crawl is loud because discovery
returns nothing, `plan_urls` re-injects the seed, and the scrape path refuses it a second
time — a scrape that produced no page is a network-class failure, so it leaves at 69. The
preview is the other case: nothing failed to *run*, there was simply no URL to schedule,
which is the null result the sitemap arms already report as 2 — now with its cause named, so
the code stops reading as "the site is empty". `--single-page` never runs discovery, so it
only ever shows the loud half. Do not read either code as "retry me and it might work": both
are one policy refusal. Whether a path is seed-guarded at all depends on which of the four
SSRF layers it wires; the layer map lives in `docs/ssrf-layers.md`.

**How to tell a refusal from a genuinely empty site.** The refusal is logged, never
hidden — and for an IP-literal seed the exit code now says it too (2, with the cause
named). A hostname that resolves into a forbidden range is the case that still looks
silent, so the `WARN` line is the discriminator: at default verbosity it is already on
stderr.

```text
WARN webfang_core::domain::ssrf_guard: SSRF literal-IP target rejected at entry (no socket opened), host: 127.0.0.1, ip: 127.0.0.1
```

To query it instead of squinting at stderr, add `--trace-file` (with `-v` the engine also
emits its run summary, where the refused seed shows up as `errors: 1` on `total_pages: 0`):

```bash
webfang http://127.0.0.1:8080/ --dry-run -v --trace-file debug.jsonl
jq -c 'select((.message // "") | test("SSRF|crawl completed"))' debug.jsonl
scripts/analyze-trace.sh debug.jsonl errors
```

One trap inside the trap: the hostname row above is not a bug waiting to be fixed, and
its `download error: DNS error: name resolution failed` is not a typo. The validating
resolver refused to hand back the address, and at this boundary that is indistinguishable
from a name that genuinely does not exist — which is why the preview keeps exiting 0 for
`localhost` and 2 for `127.0.0.1`. Literals and hostnames are checked by different layers;
see `docs/ssrf-layers.md`.

**Opting out, for a target you trust.** There is no per-target allowlist flag; the
disarmers are environment variables, and only the exact value `1` disarms anything:

```bash
# Literal address (127.0.0.1, 10.0.0.5, …): lifts the literal entry guard.
WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1 webfang http://127.0.0.1:8080/ --dry-run

# Hostname that resolves into a private range: lifts the connect-time resolver.
WEBFANG_DISABLE_SSRF_RESOLVER=1 webfang http://localhost:8080/ --dry-run

# A local target that answers to both spellings, or that you simply want to work:
WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1 WEBFANG_DISABLE_SSRF_RESOLVER=1 \
  webfang http://127.0.0.1:8080/ --dry-run
```

Rules of thumb:

- **Set them per command, never in a profile.** These variables disarm a control that
  exists because a crawler follows links written by strangers: an allow-everything
  environment is how a scraping run becomes an internal-network scan. A `WEBFANG_*` line
  in your shell rc applies to every target you will ever crawl, including the ones you
  did not mean to fetch.
- **The resolver value is read once, at client construction.** Export it *before* starting
  a long-lived process (the MCP server, a batch runner); setting it mid-run changes
  nothing for the clients already built.
- **MCP needs its own variable too.** `WEBFANG_MCP_DISABLE_SSRF=1` lifts MCP's entry
  pre-check (layer 1) only; the core layers above still apply. A local target driven
  through MCP therefore needs the MCP switch *and* the core pair.
- Redirects have a fourth, separate disarmer (`WEBFANG_DISABLE_SSRF_REDIRECT_GUARD=1`).
  Leave it armed: a public site that redirects into your internal range is exactly the
  case the guard was written for.

The full layer map — which variable lifts which layer, and what stays protected after you
set it — is in `docs/ssrf-layers.md`.

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
