# Error Classification Matrix

**Status:** Closed contract (2026-08-21). Every cell below is a final, human-approved decision.
**Contract ID:** `261bdb66-197e-420f-a73b-66c0e889102d`
**Source roadmap:** Stabilization Sprint 1-2 — Error classification (P0-err), Gate 1.

This matrix is the single source of truth for operational error semantics in WebFang.
Any new error variant MUST be classified here and in code in the same change; the
compiler enforces this (see [Exhaustiveness enforcement](#exhaustiveness-enforcement)).

## Taxonomy

`ErrorClass` lives in the domain layer (`domain/error/`) — classification is decided
where the error is born and derived upward through existing `From` conversions.

| Class | Meaning | Retry policy (wired in Sprint 7-8) |
|---|---|---|
| `TransientRetriable` | Expected transient failure; safe to retry immediately (bounded). | Yes, immediate |
| `TransientBackoff` | Transient but requires waiting (rate limits, timeouts, pools). | Yes, after backoff |
| `PermanentFatal` | Retrying cannot succeed (bad input, 4xx, WAF, TLS). | Never |
| `InternalFatal` | Invariant violation / bug indicator / data-integrity risk. | Never; abort batch |
| `DomainRecoverable` | Single-item domain failure; job continues without that item. | N/A (skip item) |

## Default exit codes by class (CLI boundary only)

Exit-code knowledge stays OUT of the domain. The mapping lives in `cli/error.rs`.

| Class | Default exit |
|---|---|
| `TransientRetriable` / `TransientBackoff` | 69 (EX_UNAVAILABLE) |
| `PermanentFatal` | varies by variant (see overrides) |
| `InternalFatal` | 3 (`ScraperFailure`) |
| `DomainRecoverable` | 0 if any item succeeded; 65 if ALL items failed |

## Matrix

### Family 1 — Network / HTTP / Timeout

| # | Error | Class | Exit | Retry |
|---|---|---|---|---|
| 1 | Connection reset / temporal DNS failure | TransientRetriable | 69 | Yes, immediate |
| 2 | HTTP 5xx | TransientRetriable | 69 | Yes, exponential backoff |
| 3 | RateLimited (429) | TransientBackoff | 69 | Yes, honors Retry-After |
| 4 | Timeout / GlobalTimeout / SlowlorisTimeout | TransientBackoff | 69 | Yes, after wait |
| 5 | HTTP 4xx (except 429) | PermanentFatal | 64/65 per variant | No |
| 6 | DNS failure / TLS failure | PermanentFatal | 69 / 76 | No |
| 7 | WafChallenge / WafBlocked | PermanentFatal | 77 | No |
| 8 | Generic indeterminate Network error | TransientRetriable | 69 | Yes, bounded |

Rationale for #8: indeterminate network errors are overwhelmingly transient in
practice. Classifying them `InternalFatal` would abort entire runs on routine
network hiccups — and would make crash-matrix resume testing (Sprint 3-5)
impossible.

### Family 2 — Limits / Budgets

| # | Error | Class | Exit | Retry |
|---|---|---|---|---|
| 9 | MaxDepthExceeded / MaxPagesExceeded / CrawlLimit | DomainRecoverable | 0/2 | No |
| 10 | UrlExcluded | DomainRecoverable | — | No |
| 11 | ResourceExhausted{SitemapUrls,SitemapDepth} / SitemapDepthExceeded | DomainRecoverable | 2 | No |
| 12 | ResourceExhausted{RamBudget} | TransientBackoff | 69 | Yes |
| 13 | SemaphoreInanition | InternalFatal | 3 | No |
| 14 | PayloadTooLarge | PermanentFatal | 65 | No |

Rationale: budget exhaustion by design (#9-11) is the crawler doing its job, not a
failure. RamBudget backpressure (#12) resolves itself when memory frees.
SemaphoreInanition (#13) indicates a backpressure configuration bug.

### Family 3 — Domain / Content

| # | Error | Class | Exit | Retry |
|---|---|---|---|---|
| 15 | InvalidUrl / UrlParse / Validation | PermanentFatal | 64 | No |
| 16 | Parse (HTML) / Readability / Conversion / Extraction | DomainRecoverable | 65* | No |
| 17 | ExtractionFailed | DomainRecoverable | **65 override** | No |
| 18 | InvalidContentType | DomainRecoverable | — | No |
| 19 | SpaDetected | PermanentFatal | 76 | No |
| 20 | SitemapEmpty / SitemapNotFound | DomainRecoverable | **2 override** | No |

\* Per-item failures surface as exit 65 only when ALL items failed.

### Family 4 — Persistence / Internal infrastructure / AI

| # | Error | Class | Exit | Retry |
|---|---|---|---|---|
| 21 | Io transient (`Interrupted`, `WouldBlock`, `TimedOut`) | TransientRetriable | 74* | Yes |
| 22 | Io permanent (`NotFound`, `PermissionDenied`, rest) | PermanentFatal | **74 override** | No |
| 23 | Persistence / Storage / Checkpoint / Serialization | InternalFatal | 3 | No |
| 24 | SessionPool | TransientBackoff | 69 | Yes |
| 25 | Middleware | TransientBackoff | 69 | Yes |
| 26 | Ingestion (Elastic) | TransientBackoff | 69 | Yes |
| 27 | Config / ConfigFile / H2Config | PermanentFatal | **78 override** | No |
| 28 | FeatureGated | PermanentFatal | 64 | No |
| 29 | AI: ModelLoad / Inference / InvalidThreshold | InternalFatal | 3 | No |
| 30 | AI: ChunkTooLarge / Tokenize / Download / CacheValidation / OfflineMode | DomainRecoverable | 78*/69* | No |
| 31 | CrawlSession: InvalidConfiguration / CheckpointUnwritable (P6-2) | PermanentFatal | **78 override** | No |
| 32 | CrawlSession: Internal (P6-2) | InternalFatal | 3 | No |
| 33 | CrawlError: InvalidSession (P6-2 slice 2) | PermanentFatal | **78 override** | No |

Rows 21-22 classify by `io::ErrorKind`, mirroring the existing
`DownloadError::classify()` behavior. Rows 29-30 document the existing
`SemanticError::classify()` decisions verbatim — no semantic change.

Row 23 is the heart of Gate 2: data-integrity errors are NEVER blindly retried.

### Special cell — `Cancelled`

`Cancelled` is a cooperative control signal (shutdown token), NOT an operational
failure. Treatment:

1. **Primary path:** intercepted at the CLI boundary BEFORE classification →
   graceful shutdown → `CliExit::Success` (exit 0).
2. **Defensive fallback:** if it ever escapes through an unexpected path,
   `classify() -> InternalFatal` so it can never be retried or silently swallowed
   while the runtime is tearing down.

## Typed overrides over class defaults

| Condition | Exit | Reason |
|---|---|---|
| All URLs blocked by robots.txt / WAF | 77 | Caller lacks permission, not a service fault |
| ExtractionFailed (typed `matches!`) | 65 | Content-quality failure, not network |
| Sitemap empty / empty discovery | 2 | Technical success, null result |
| `--dry-run` whose seed the SSRF entry guard refuses | 2 | Policy refusal, not an empty site (#1381) — the refusal is pre-socket, so discovery still returns `Ok` with zero URLs |
| Config errors | 78 | EX_CONFIG |
| Io permanent | 74 | EX_IOERR |
| InternalFatal | 3 | Job failure — NOT user usage error |

### Boundary status

As of PR #840, the Io → 74 override (rows 21/22) is materialized end-to-end:
`ScraperError::classify` splits `Io` by `io::ErrorKind` (mirroring
`CrawlError::classify`), and both all-failed precedence chains in
`cli/orchestrator.rs` (`report_phase` and `batch_exit_code`) route
permanent-kind I/O failures through `permanent_io_error_exit_for` to
`CliExit::IoError` (74), placed after the InternalFatal sweep and before the
ExtractionFailed check.

Remaining divergences between `ScraperError::classify` and this matrix
(ExtractionFailed, Conversion, Readability/Extraction, Middleware,
Ingestion) are tracked for reconciliation in issue #839.

As of #957, the Family 2 budget rows are materialized end-to-end too:
`ScraperError::CrawlLimit` classifies `DomainRecoverable` (rows 9 and 11) and
`ScraperError::ResourceExhausted` is a typed 1:1 pass-through of
`CrawlError::ResourceExhausted` that preserves the kind split (row 11 →
`DomainRecoverable`, row 12 → `TransientBackoff`). Previously both flattened to
variants classifying `PermanentFatal`/`InternalFatal`, inverting the class for
every consumer of `ErrorClass` at the `ScraperError` layer.

## Exhaustiveness enforcement

1. `CrawlError::classify()`: flat match, zero wildcard arms. Adding a variant
   without classification fails compilation (E0004).
2. `#[non_exhaustive]` removed from `CrawlError`: workspace-internal crate; the
   attribute only forced wildcards and hid misalignments.
3. `CrawlErrorCategory`: `_ =>` catch-all removed; every variant mapped explicitly.
4. DoD test: each `ErrorClass` maps to its documented default exit code.

    ## Explicit non-goals (this sprint)
    
    - **No retry-loop wiring.** `RetryPolicy` keeps its current behavior until the
      Sprint 7-8 budget model exists (thundering-herd protection). The matrix makes
      the data available; consumption is deferred by design.
    - No changes to `HttpError` / `DownloadError` classifications (already explicit
      and correct); they are documented here as upstream inputs.
    
    ## Boundary status of the class → exit helpers (F-10, #1294)
    
    `default_exit_code_for_class` and `cli_exit_for_class` are the canonical code form of
    the table above. Their production call-site count is **zero**: exit codes are still
    decided where each error is turned into a `CliExit`, across ~138 explicit
    constructions. That is a documented state, not a pending refactor — the one site that
    consumes `classify()` today (`cli/export_flow.rs`) uses it for control flow
    (abort vs. fall back vs. count), not to pick an exit code, so routing it through the
    helper would be wrong.
    
    Two invariants keep this honest without a mass migration:
    
    1. `matrix_doc_agrees_with_the_class_exit_helpers` (`src/cli/error.rs`) parses the
       "Default exit codes by class" rows and compares them against the helper, including
       the two classes that must stay unpinned. The document cannot drift from the code.
    2. The existing DoD tests pin the helper's own values.
    
    If the taxonomy is ever adopted, do it row by row with an exit-code test per site, not
    by rewriting the mapping call-sites in one pass.
    
    ## How a class reaches an MCP client instead of an exit code (P6-5, #1294)
    
    MCP has no exit channel, so "CLI says 69, MCP says…" is the wrong question: the two
    surfaces are projections of one classification, not two competing ones.
    
    | Situation | CLI | MCP |
    | :--- | :--- | :--- |
    | Unroutable request (bad params, unknown tool, forbidden target) | exit 64 usage / 69 unavailable, Spanish message on stderr | JSON-RPC error: `-32602` invalid params, `-32601` method not found |
    | The tool ran and failed (network, WAF, robots, budget, empty result) | per-class exit above | `result.isError: true` + Spanish text — deliberately **not** a protocol error, so the caller actually sees the message |
    | Request shape the transport rejects (bad `Accept`, bad `Content-Type`, non-2.0 body, no session) | n/a | HTTP 406/415/422 from rmcp, before JSON-RPC exists |
    | Server cannot serve at all (container construction) | exit 78 config | process refuses to start; typed stderr + exit code (#1123) |
    
    The last two rows are the boundary #1294 documents rather than closes: they are owned
    by the framework and by the supervisor, not by the error mapping. Rule of thumb, from
    rmcp's own contract: `Err(McpError)` means "your request was unroutable", `Ok(CallToolResult::error(..))`
    means "the job failed, here is why" — and an agent only reads the second one.
    
    See `docs/ssrf-layers.md` for the `-32602` SSRF row and `crates/webfang_mcp/tests/mcp_transport_contract_test.rs`
    for the framework rows.
