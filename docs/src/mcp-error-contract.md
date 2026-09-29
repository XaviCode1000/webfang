# MCP error contract — which channel carries which failure

**Status:** published decision table for issue #1613 slice 1 (EC-01). Derived from the
code as it stands on `main`; it describes what the server **does**, not what it should do.
Slices 2–5 change some of these rows; this page is what they must conform to.

**Evidence class: static only — `runtime_validation: NOT_RUN`.** Every row below was read
out of the source, not observed over a transport. Two rows (A1, A2) depend on rmcp's
internal error mapping and are therefore marked *unverified*; see
[Unsettled rows](#unsettled-rows).

## Quick path for an agent consumer

```text
HTTP status != 200                      → Channel 0. Transport refused the request. Fix the request shape.
JSON-RPC "error" member present         → Channel A. The call never ran. Fix the arguments.
result.isError == true                  → Channel B. The call ran and failed. The text is the reason.
result without isError                  → Channel C. The call ran. Read the body: it may be a
                                          diagnostic, a partial result, or a success.
```

"Retriable" in this table means: **repeating the identical call can succeed without the
caller changing anything.** Anything else is a caller change, not a retry.

## The three channels (plus the one before them)

| Channel | Wire shape | HTTP | Carries | Code |
| :--- | :--- | :--- | :--- | :--- |
| **0** — transport | bare status, no JSON-RPC body | 401/406/408/413/415/422/429/500 | a request that never reached a tool | none |
| **A** — protocol error | `{"error": {"code": …, "message": …}}` | 200 | the call was unroutable or its arguments were refused | `-32601` / `-32602` / `-32603` |
| **B** — tool error | `result.isError = true` | 200 | the tool ran and the job failed | none |
| **C** — success-shaped diagnostic | `result`, no `isError` | 200 | the tool ran; the body describes a non-success outcome | none |

Channel B carries **no code**: the only machine-readable signal is the boolean, and the
reason is free text. Channel C has the same gap, which is the whole reason it is listed
separately — see row C1.

---

## Channel A — JSON-RPC protocol errors

Emitted when a tool function returns `Err(McpError)`; `McpHandler::call_tool`
(`mcp_server/mod.rs:359-382`) forwards that `Err` unchanged to rmcp.

### A1–A3 · argument deserialization (before the handler body)

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :--- | :--- | :--- | :-: |
| A1 | URL argument is not http(s), is oversize, or carries embedded credentials | **unverified** — see [Unsettled rows](#unsettled-rows) | No | Reshape the URL. Never retry unchanged. | `mcp_server/params.rs:82-92` (`McpUrl::try_from`) |
| A2 | Unknown JSON key in the argument object (`deny_unknown_fields`) | **unverified** — same mapping as A1 | No | Drop the unknown key. | `mcp_server/params.rs:6-11` (per-struct attribute); mapping asserted at `tests/params_rejection_test.rs:305-315` |
| A3 | In-handler `params.validate()?` semantic rejection | `-32602` | No | Fix the named field; the envelope carries it. | `mcp_server/params.rs:445-471` and every `validate()`; shape pinned at `tests/params_rejection_test.rs:249` |

A1 and A3 produce the **same Rust error type** (`McpError::invalid_params`) but travel
**different routes**: A1 is raised inside rmcp's deserialization, A3 is returned from the
tool function. Whether they reach the client as the same JSON-RPC code is exactly the
open question in [Unsettled rows](#unsettled-rows).

A3 covers every branch below. They share one constructor, so they share one channel and
one code — the split is by *condition*, not by envelope.

| # | A3 sub-condition | Field tag | Retriable | Agent should | Source |
| :-: | :--- | :--- | :--- | :--- | :-: |
| A3.1 | URL empty, > 8192 B, unparseable, or non-http(s) scheme | `url` | No | Resend a valid absolute http(s) URL. | `mcp_server/validation.rs:56-73` |
| A3.2 | Path empty, > 1024 B, absolute, drive-lettered, `..`-bearing | varies | No | Send a relative, traversal-free path. | `mcp_server/validation.rs:84-128` |
| A3.3 | HTML / markdown / content blob > 1 MiB | varies | No | Chunk the input. | `mcp_server/validation.rs:35`, `374-405` |
| A3.4 | `max_pages` outside 1..=100 000, `max_depth` outside 0..=10 | `max_pages` / `max_depth` | No | Clamp to the advertised cap. | `mcp_server/params.rs:144-167` |
| A3.5 | `urls` empty, over cap, or `concurrency` out of range | `urls` / `concurrency` | No | Fix the batch. | `mcp_server/params.rs:446-471` |
| A3.6 | `js_strategy` not a known strategy | `js_strategy` | No | Use `static` / `hybrid` / `full`. | `mcp_server/params.rs:509-518` |
| A3.7 | `checkpoint_dir` present but blank | `checkpoint_dir` | No | Omit it or name a real directory. | `mcp_server/params.rs:519-526` |
| A3.8 | Enum-ish string not in the allowed set | varies | No | Use a listed value; the message enumerates them. | `mcp_server/validation.rs:545-553` |
| A3.9 | Filename not a single safe component (traversal, separator, reserved name) | `filename` | No | Send one flat component. | `mcp_server/validation.rs:436-483` |

### A4 · path confinement (root-of-trust)

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :--- | :-: |
| A4.1 | Absolute `output_dir` / `checkpoint_dir` with no export roots configured | `-32602` | No | Use a relative path, or tell the operator to set `--export-roots`. | `mcp_server/path_gate.rs:308-320` via `mcp_server/state.rs:331, 351` |
| A4.2 | Absolute path outside every configured export root | `-32602` | No | Re-target under a configured root. | `mcp_server/path_gate.rs:322-330` |
| A4.3 | Rooted-but-not-absolute form (`C:foo`, `\foo`) | `-32602` | No | Use a genuinely absolute or genuinely relative path. | `mcp_server/path_gate.rs:282-290` |
| A4.4 | Empty / oversize / `..`-bearing path | `-32602` | No | Fix the path. | `mcp_server/path_gate.rs:271-280`, `292-300` |

The gate is fail-closed: with zero roots configured, every absolute path is refused
(`mcp_server/path_gate.rs:308`). Relative paths are allowed and resolve against the
server's CWD (`mcp_server/path_gate.rs:302-306`).

### A5–A8 · SSRF entry pre-check

All five branches come from one function, `validate_url_no_ssrf`
(`mcp_server/ssrf.rs:73-143`), and **all four failure branches emit `-32602`**
(`mcp_server/ssrf.rs:94, 103, 119, 124, 133`).

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :--- | :-: |
| A5 | URL has no host | `-32602` | No | Unreachable in practice — `McpUrl` always carries a host. | `mcp_server/ssrf.rs:93-94` |
| A6 | Literal IP is in a forbidden range (loopback / private / link-local / CGNAT / ULA) | `-32602` | No | Stop. Retargeting is the only fix. | `mcp_server/ssrf.rs:100-111` |
| A7 | DNS resolver returned an error | `-32602` | **Undecidable** — same envelope as A6 | Nothing safe. See the EC-03 note below. | `mcp_server/ssrf.rs:117-121` |
| A8 | DNS returned an empty answer set | `-32602` | **Undecidable** — same envelope as A6 | Nothing safe. See the EC-03 note below. | `mcp_server/ssrf.rs:123-128` |
| A9 | A resolved address is in a forbidden range | `-32602` | No | Stop. This is a policy refusal, not a transient. | `mcp_server/ssrf.rs:130-139` |

> **EC-03 (out of scope here).** A7 and A8 are *infrastructure* failures sharing an
> envelope with A6 and A9, which are *policy* refusals. A transient DNS outage is
> indistinguishable from a caller targeting internal infrastructure. Slice 2 owns
> separating them; until then an agent must read the message text to tell them apart.

This pre-check is not the enforcement point. Every scrape client is independently guarded
at connect time by `webfang_core::domain::ssrf_guard` (literal-IP redirect policy +
validating resolver), so A6/A9 are fast-fail UX and defense in depth. See
[`ssrf-layers.md`](../ssrf-layers.md) for the full layer matrix.

### A10–A16 · handler-local protocol errors

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :--- | :-: |
| A10 | `content_format` / `pipeline_format` not a known export format | `-32602` | No | Use `jsonl` / `vector` / `auto`. | `mcp_server/handlers/export.rs:191-198`, `450-457` |
| A11 | `filename` rejected by the sanitizer | `-32602` | No | Send one flat component. | `mcp_server/handlers/export.rs:206-211`, `230-240`, `311-317`, `348-355`, `393-400`, `452-459` |
| A12 | Synthetic export URL for the filename fails the hardened URL gate | `-32602` | No | Reshape the filename. | `mcp_server/handlers/export.rs:228-240` |
| A13 | `js_strategy` failed to parse **in the handler** (post-validate path) | `-32602` | No | Use a known strategy. | `mcp_server/handlers/scraping.rs:452-459` |
| A14 | Explicit `sitemap_url` fails the hardened URL re-wrap | `-32602` | No | Reshape the sitemap URL. | `mcp_server/handlers/scraping.rs:592-601` |
| A15 | Per-category semaphore closed or poisoned | `-32603` | No | Report it; the server is unhealthy. | `mcp_server/macros.rs:16` |
| A16 | Record conversion failed: `DocumentChunk::validate()` or metadata serialization | `-32603` | No | Report it; the payload was not produced. | `mcp_server/handlers/scraping.rs:103, 106, 209, 212, 1119, 1125` |
| A17 | SSRF pre-check failure on the axtree tool (re-raised unchanged) | `-32602` | No | As A6–A9. | `mcp_server/handlers/axtree.rs:68-77` |

A15 and A16 are the only `-32603` values this crate constructs for a tool call. They are
server faults, not caller mistakes, and must not be retried.

### A18–A19 · produced by rmcp, not by this crate

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :--- | :-: |
| A18 | Unknown JSON-RPC method (e.g. a tool name that is not registered) | `-32601` | No | Call a name from `tools/list`. | rmcp; not constructed here — `mcp_server/server.rs:174-177`, pinned by `tests/mcp_transport_contract_test.rs:138` |
| A19 | A panic escapes the HTTP request path | `-32603` body, HTTP 500 | No | Report it. The tool-level panic path (B1) is the one that keeps the session alive. | `mcp_server/server.rs:244-285` (`jsonrpc_panic_response`) |

---

## Channel B — `isError: true` tool results

Every row here is built by `provenance::neutralized_error`
(`mcp_server/provenance.rs:281-294`) or by the `honest_error` wrappers
(`mcp_server/handlers/ai.rs:38-42`, `mcp_server/handlers/axtree.rs:25-28`). Text is
neutralized (ANSI + C0/DEL stripped, fence sentinels escaped) and capped, but **no
provenance envelope and no code is added** — the reason is the text and nothing else.

### B1 · contained panic

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :--- | :-: |
| B1 | A tool body panicked; the panic was caught at the dispatch boundary | none | **Yes, with a caveat** | Retry, but know that containment restores the *transport*, not the side effects — a tool that panicked after writing an export or charging a rate-limit token will duplicate that work. | `mcp_server/mod.rs:359-380` (containment + rationale), client text at `mcp_server/mod.rs:268-270` |

### B2 · site policy denial (robots.txt / WAF)

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B2.1 | robots.txt rules forbid the URL | none | No | Stop, or retry with the tool's own opt-out (`ignore_robots`) if the caller has permission. | `mcp_server/handlers/scraping.rs:1152-1171` (`robots_denied_response`), used by `mcp_server/handlers/scraping.rs:750-761` (`discover_urls`), `mcp_server/handlers/scraping.rs:957-968` (`detect_spa`), and the inline gate at `mcp_server/handlers/scraping.rs:73-81`; `mcp_server/handlers/ai.rs:122-125` |
| B2.2 | The policy guard refused (guard condition, not a rules denial) | none | No | Treat as a policy refusal. | `mcp_server/state.rs:375-390` |
| B2.3 | Live scrape inside the export pipeline denied | none | No | As B2.1; the message is prefixed with the target URL. | `mcp_server/handlers/export.rs:412-419` |

### B3 · scrape / crawl operational failure

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B3.1 | `scrape_url` — fetch, Readability extraction, or content sink failed | none | Depends on the class in the text | Read the text; a transient network fault and a hard WAF block look the same here. | `mcp_server/handlers/scraping.rs:118-127` |
| B3.2 | `scrape_with_options` — same | none | Same | Same. | `mcp_server/handlers/scraping.rs:224-233` |
| B3.3 | `scrape_batch` — whole-batch failure | none | Yes if transient | Retry the batch; per-URL failures inside a *successful* batch are rows, not errors (C4). | `mcp_server/handlers/scraping.rs:379-392` |
| B3.4 | `crawl_site` — engine run failed | none | Yes if transient | Retry the crawl. | `mcp_server/handlers/scraping.rs:512-518` |
| B3.5 | `crawl_with_sitemap` — discovery failed | none | Yes if transient | Retry. | `mcp_server/handlers/scraping.rs:706-718` |
| B3.6 | `crawl_with_sitemap` — sitemap session run failed | none | Yes if transient | Retry. | `mcp_server/handlers/scraping.rs:684-701` |
| B3.7 | `discover_urls` / `discover_sitemap` — link extraction or sitemap read failed | none | Yes if transient | Retry. | `mcp_server/handlers/scraping.rs:826-836`, `mcp_server/handlers/scraping.rs:914-924` |
| B3.8 | `download_assets` — asset fetch or write failed | none | Yes if transient | Retry; already-downloaded assets are content-addressed. | `mcp_server/handlers/assets.rs:90` |
| B3.9 | `extract_links` — base-URL resolution failed | none | No | Fix the input HTML or base URL. | `mcp_server/handlers/content.rs:93` |
| B3.10 | `url_to_file_path` — `OutputPath::from_url` failed | none | No | Fix the URL. | `mcp_server/handlers/url_utils.rs:193` |

The per-class retry policy these texts inherit is
[`error-classification-matrix.md`](../error-classification-matrix.md). The MCP channel
carries no exit code, so the class is not machine-readable here — the agent must parse
the text.

### B4 · direct-fetch HTTP failures

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B4.1 | `discover_urls` — non-2xx status (a real failure, deliberately not an empty link set) | none | 4xx no / 5xx yes | Read the status in the text. | `mcp_server/handlers/scraping.rs:772-781` |
| B4.2 | `discover_urls` — transport failure | none | Yes | Retry. | `mcp_server/handlers/scraping.rs:844-847` |
| B4.3 | `detect_spa` — transport failure | none | Yes | Retry. | `mcp_server/handlers/scraping.rs:1015-1018` |
| B4.4 | `semantic_cleaner` — page fetch failed | none | Yes if transient | Retry. | `mcp_server/handlers/ai.rs:140-143` |
| B4.5 | `get_accessibility_snapshot` — snapshot fetch failed (both formats) | none | Yes if transient | Retry. | `mcp_server/handlers/axtree.rs:133-145`, `163-176` |

### B5 · export failures

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B5.1 | No session results to export (no crawl ran, or its extraction produced nothing) | none | No | Run a crawl first. This is a precondition, not a fault. | `mcp_server/handlers/export.rs:113-119` |
| B5.2 | Blocking-pool join for the session snapshot failed | none | Yes | Retry. | `mcp_server/handlers/export.rs:134-140` |
| B5.3 | `export_file` with empty / whitespace-only content | none | No | Send real content. | `mcp_server/handlers/export.rs:184-188` |
| B5.4 | `export_file` — document chunk validation failed | none | No | Fix the content. | `mcp_server/handlers/export.rs:256-262` |
| B5.5 | Exporter construction failed | none | Depends | Read the text. | `mcp_server/handlers/export.rs:265-271` |
| B5.6 | Export write failed (all four export tools) | none | Depends | Read the text. | `mcp_server/handlers/export.rs:55-59`, `281-285` |
| B5.7 | `process_export_pipeline` — live scrape failed | none | Depends | Read the text; it names the URL. | `mcp_server/handlers/export.rs:428-433` |

### B6 · feature gating and missing ports

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B6.1 | AI compiled but not enabled, or the model is still warming up | none | No (or retry after warmup) | Restart with `--enable-ai`, or wait for the model to load and retry. | `mcp_server/handlers/ai.rs:48-58` (`ai_unavailable`), used at `mcp_server/handlers/ai.rs:129`, `mcp_server/handlers/ai.rs:197-200`, `mcp_server/handlers/ai.rs:207` |
| B6.2 | AI not compiled into the binary | none | No | Rebuild with `--features ai`. | same helper, `mcp_server/handlers/ai.rs:54-57` |
| B6.3 | Vault note repository not configured | none | No | Operator must configure persistence. | `mcp_server/handlers/ai.rs:201-204` |
| B6.4 | `chromium` not compiled (axtree tool) | none | No | Rebuild with `--features chromium`. | `mcp_server/handlers/axtree.rs:186-197` |
| B6.5 | Semantic cleaner / inference / embedding failed | none | No | Report it; this is a server-side AI fault. | `mcp_server/handlers/ai.rs:144-147`, `270` |
| B6.6 | Response serialization failed (AI + axtree) | none | No | Report it; the payload could not be built. | `mcp_server/handlers/ai.rs:163-168`, `265-269`; `mcp_server/handlers/axtree.rs:122-131` |

### B7 · local integration failures

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| B7.1 | `build_obsidian_uri` / `open_in_obsidian` — vault name or file path rejected | none | No | Fix the vault name or note path. | `mcp_server/handlers/obsidian.rs:72-76`, `99-103` |
| B7.2 | `open_in_obsidian` — URI dispatch failed | none | Yes | Retry; Obsidian may not be running. | `mcp_server/handlers/obsidian.rs:116-119` |
| B7.3 | `get_scrape_metrics` — no scrape has been recorded in this process | none | No | Run a scrape first. Precondition, not a fault. | `mcp_server/handlers/security.rs:203-208` |
| B7.4 | `get_scrape_metrics` — snapshot serialization failed | none | No | Report it. | `mcp_server/handlers/security.rs:209-213` |
| B7.5 | `scrape_batch` — every input URL was rejected before the run | none | No | Fix the URLs. | `mcp_server/handlers/scraping.rs:260-262` |

---

## Channel C — success-shaped diagnostics

A `result` with **no** `isError` and no `error` member. The call succeeded; the body
describes something that is not a plain success. This is the channel an agent cannot
distinguish from a real success by shape alone — that is why it is enumerated rather than
folded into B.

| # | Condition | Code | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: | :-: |
| C1 | `validate_url` reports an unusable URL as `{"valid": false, "reason": …}` | none | No | Parse the JSON and branch on `valid` — never on `isError`. This is the only tool whose failure is a *success* by design (issue #590, bug #7). | `mcp_server/handlers/url_utils.rs:58-67`; success twin at `mcp_server/handlers/url_utils.rs:44-57` |
| C2 | `open_in_obsidian` — the URI handler reported `HandlerFailed` (Obsidian may be absent) | none | Yes | Read the ⚠️ prefix. This is a partial success and is not encoded structurally. | `mcp_server/handlers/obsidian.rs:113-115` |
| C3 | A serialization failure fell back to the literal text `"failed to serialize"` inside an otherwise successful result | none | No | Treat a body equal to that string as a server fault. The tool did not fail; the report was lost. | `mcp_server/handlers/assets.rs:80-88`, `mcp_server/handlers/content.rs:83-91`, `mcp_server/handlers/scraping.rs:814-822`, `mcp_server/handlers/scraping.rs:903-911` |
| C4 | A run completed with per-item failures (`failed_url` records; `errors` / `error_breakdown` fields) | none | Per item | Read the per-item records; the run itself is a success. | `mcp_server/handlers/scraping.rs:1128-1140` (failure records), returned through `mcp_server/handlers/scraping.rs:370-375`; crawl summary at `mcp_server/handlers/scraping.rs:517-529` |
| C5 | `detect_obsidian_vault` found no vault (`"no vault detected"`) | none | No | A negative *result*, not a failure. | `mcp_server/handlers/obsidian.rs:53-55` |
| C6 | `detect_spa` found sufficient content (`"not an SPA - sufficient content found"`) | none | No | A negative *result*, not a failure. | `mcp_server/handlers/scraping.rs:1004-1006` |

---

## Channel 0 — transport refusals (no JSON-RPC exists yet)

These never reach a tool. The client sees an HTTP status and, in most cases, no
JSON-RPC body at all.

| # | Condition | Status | Retriable | Agent should | Source |
| :-: | :--- | :-: | :-: | :-: | :-: |
| T1 | Missing / malformed / wrong bearer token | `401` | No | Fix the `Authorization` header. | `mcp_server/auth.rs:38-60` |
| T2 | Rate-limit quota exceeded | `429` | Yes, after backoff | Slow down. | `mcp_server/server.rs:298-312` |
| T3 | Session-admission cap exceeded | `429` | Yes, after the window | Slow down. Bounded by `--max-sessions` / `--session-cap-window-secs`. | `mcp_server/server.rs:628-680` |
| T4 | Request took longer than `--request-timeout-secs` | `408` | Yes | Retry. | `mcp_server/server.rs:222-225` |
| T5 | Body over `--body-limit-bytes` | `413` | No | Shrink the payload. | `mcp_server/server.rs:226` |
| T6 | `Accept` missing both `application/json` and `text/event-stream` | `406` | No | Fix the request headers. | rmcp; `mcp_server/server.rs:168-170` |
| T7 | `Content-Type` not `application/json`, or the body is not JSON-RPC 2.0 | `415` | No | Fix the request. | rmcp; `mcp_server/server.rs:171-172` |
| T8 | Non-`initialize` request with no `mcp-session-id` | `422` | No | Complete the handshake first. | rmcp; `mcp_server/server.rs:173` |
| T9 | Panic on the HTTP request path | `500` + JSON-RPC `-32603` body | No | Report it. | `mcp_server/server.rs:263-285` |

Layer order matters for reading this: the session cap is the **innermost** gate, so a
request rejected `401` or shed `429` upstream never consumes a session slot
(`mcp_server/server.rs:205-215`).

---

## Unsettled rows

**EC-02 — the rmcp mapping is NOT verified.** The workspace declares `rmcp = "1.7"`
(root `Cargo.toml:119`) while the lock resolves `1.8.0` (`Cargo.lock`), and the source
comments disagree about the same code path:

- `mcp_server/params.rs:43-47` states that an `McpUrl` deserialization failure becomes
  `isError: true` (rmcp's `into_tool_argument_error`), **not** a JSON-RPC `-32602`.
- `tests/params_rejection_test.rs:85-100` documents the same, and the
  `typo_field` case at `tests/params_rejection_test.rs:304-315` asserts `is_tool_error`.
- But `tests/params_rejection_test.rs:6-9, 116` states that handler-level
  `params.validate()?` rejections map to `-32602`, and line 249 asserts exactly that.

Those are two *different* routes that happen to share a Rust error type, so they are
consistent — but nothing in this crate proves it. **Rows A1 and A2 are marked
*unverified* for that reason.** Settling it needs one transport-level test through a real
rmcp router that records both `code` and `isError`; that is slice 3, and it is
explicitly not a code fix here.

**Rows the code slices will change, deliberately:**

| Finding | Row affected | Slice |
| :--- | :--- | :-: |
| EC-03 — SSRF policy and DNS failures share `invalid_params` | A7, A8 | 2 |
| EC-04 — the third channel is undocumented | this page (C1) | 5 |
| EC-06 — a serialization failure returns success with a placeholder | C3 | 4 |
| EC-07 — `DispatchStatus::HandlerFailed` is returned as tool success | C2 | 4 |
| EC-08 — validation text is English while operational errors are Spanish; no machine-readable reason field | A3.x, B*.x | 5 |

**Deliberately not touched by any slice:** EC-05, the panic-capable serialization
`expect` sites (`mcp_server/handlers/content.rs:215`,
`mcp_server/handlers/scraping.rs:528`, `mcp_server/handlers/scraping.rs:681`,
`mcp_server/handlers/scraping.rs:1000`, `mcp_server/handlers/url_utils.rs:56`,
`mcp_server/handlers/url_utils.rs:63`, `mcp_server/handlers/url_utils.rs:190`).
Replacing them is not automatically an improvement — the
correct remedy is a proven-total serialization boundary or an explicit `internal_error`
mapping, and they are classified as *not observed panics*. Blind replacement would be
churn.

**Overlapping, not duplicated here:** EC-08's *message-language* half overlaps #1604,
which normalizes operational error event text to structured fields. #1604 does not own
the message-language or machine-readable-reason halves; those remain a contract decision
for slice 5.

---

## Appendix — construction-site inventory

Counted in `crates/webfang_mcp/src/mcp_server/` at commit `763714b6`, production code
only (test modules excluded).

| Channel | Construction points | Sites | Accounted for |
| :--- | :--- | :-: | :-: |
| A | `McpError::invalid_params` (inline) | 16 — `mcp_server/handlers/export.rs:194, 206, 230, 239, 312, 349, 394, 453`; `mcp_server/params.rs:449, 512, 520`; `mcp_server/ssrf.rs:94, 103, 119, 124, 133` | A5–A9, A10, A11, A13, A14 |
| A | `invalid_params` factory + its rejection branches | 1 factory (`mcp_server/validation.rs:43`) + 43 branches | A3.1–A3.9 |
| A | path-confinement branches | 6 — `mcp_server/path_gate.rs:273, 276, 284, 294, 311, 328` | A4.1–A4.4 |
| A | `McpError::internal_error` | 7 — `mcp_server/handlers/scraping.rs:103, 106, 209, 212, 1119, 1125`; `mcp_server/macros.rs:16` | A15, A16 |
| A | re-raised `Err(e)` | 1 — `mcp_server/handlers/axtree.rs:74` | A17 |
| B | `provenance::neutralized_error` | 45 — 31 direct calls in `handlers/` + 2 wrapper definitions (`mcp_server/handlers/ai.rs:40`, `mcp_server/handlers/axtree.rs:26`) + 14 calls through those wrappers | B1–B7 |
| B | direct `CallToolResult::error` | 1 — `mcp_server/mod.rs:377` | B1 |
| C | `provenance::local_text` / `untrusted_text` on a non-success outcome | 10 construction sites across 6 conditions | C1–C6 |
| 0 | HTTP status producers | 9 | T1–T9 |

Totals: **86** error-producing construction sites found, **86** accounted for, **0**
unclassified.
