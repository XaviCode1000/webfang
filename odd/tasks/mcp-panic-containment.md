# Feature: MCP panic containment (issue #1611, slice F2)

## Goal

A panic in an MCP tool handler must become a tool error, not a dead transport: the
session worker survives and the client keeps using the same session.

## Why F2 first

The audit sequence is F2 → F6/F7 → F5 → G-18 because mounting panic containment
changes the failure mode of every downstream cap: once a panicking handler returns
an error instead of killing the worker, the caps become the load-bearing defense.

## Key finding (verified against rmcp 1.8.0 source)

- `StreamableHttpService::spawn_session_worker` (`tower.rs:665`) runs the handler
  inside `tokio::spawn`, so a tool-handler panic does **not** unwind the HTTP
  request future.
- `grep -rn "catch_unwind" rmcp-1.8.0/src` → no hits. The session worker is not
  panic-resilient, so a panic drops `svc.waiting()`, closes the session, and the
  client sees a dead transport.

⇒ Two layers are needed, not one:

1. `CatchPanicLayer` **outermost** on the router — contains panics raised on the
   HTTP request path and maps them to a JSON-RPC `-32603` error body.
2. `AssertUnwindSafe(fut).catch_unwind()` around tool dispatch in
   `McpHandler::call_tool` — the load-bearing part for the session worker, and
   transport-agnostic (covers stdio too). Maps the panic to a `CallToolResult`
   with `is_error = true`.

## Tasks

- [x] T1 — Composition seam: `build_mcp_router_with_service` +
      `McpHandler::with_tool_router` so a test can mount a panicking tool on the
      real middleware stack. `build_mcp_router(state, options)` keeps its
      signature.
- [x] T2 — `tower-http` `catch-panic` feature + `CatchPanicLayer` outermost with
      a Spanish-mapped JSON-RPC `-32603` response.
- [x] T3 — `catch_unwind` guard in `McpHandler::call_tool` → tool error, with a
      structured `tracing::error!` (tool name + panic payload).
- [x] T4 — Transport-level test: handshake → panicking `tools/call` returns
      `isError` over HTTP 200 → a following `tools/call` on the **same session
      id** still succeeds.
- [x] T5 — Verification chain + work-unit commits.

## Diagnostic-path hygiene (#1626, slice F2-DIAG)

The bounded review of F2 left five advisory findings, all in the containment's own
diagnostic path. None blocks the fix; each degrades what the operator can see.
Delivered as PR #1645, which carries `Closes part of #1626` — the umbrella stays
open while PC-4 has an owner.

- [x] **PC-1** — `render_panic_payload` was documented as "non-sensitive" while it
      copies the payload verbatim. Doc now states the trade and puts the rule at the
      call site: never write a secret into a panic message.
- [x] **PC-2** — the bound was applied *after* a full `chars().count()` walk, then a
      second full walk to build the result. Now `char_indices().nth(MAX_CHARS)`:
      bounded by the constant, output byte-identical, char-boundary safe.
- [x] **PC-3** — the stdio transport never installed the panic hook, so a
      contained panic there yielded no structured record with the panic LOCATION
      (the HTTP path gets one from `start_mcp_server`). Fixed in `0dec4566`: the
      hook is installed on the stdio boot (stdout-safe by construction — it and
      the default hook both write to the stderr the transport already reserves for
      logs), plus an env-gated `test_panic_probe` so a contained panic is
      reachable from the wire and the E2E half of the contract is testable.
- [ ] **PC-4** (deferred, not started) — the HTTP mapping answers `{"id": null}` for
      every contained panic, so a client cannot correlate the `-32603` with the
      request that caused it. #1626 itself sequences this with #1613 (EC-01 owns the
      error-channel decision table) and says "do not solve it twice". Tracked as a
      child issue of #1626 pointing at #1613.
- [x] **PC-5** — no code by design. Containment restores the transport, not the side
      effects of a half-executed tool; rollback is per-tool idempotency, owned by the
      export/crawl paths. Accepted and documented at the `call_tool` call site.

### PC-3 design decision (maintainer-chosen, 2026-09-28)

The E2E bar #1626 sets is "a stdio round trip asserting a record exists after a
contained panic". No tool panics on demand, so the test needs one. Three options were
put to the maintainer; the choice was **an env-gated probe tool**, over a Cargo
feature, over shipping the hook with no E2E test:

- Env-gated wins because the test runs in the DEFAULT `cargo nextest run` lane, which
  is where CI actually verifies it. A `#[cfg(feature = ...)]` probe would put the test
  behind the same cfg, so the default lane would silently SKIP it — and a skipped
  check is not evidence.
- The accepted cost: a release binary can be asked, via env var, to register a tool
  that panics. It is contained by the F2 layer (the session survives, the caller gets
  the normal `isError`), and it only exists when the operator sets the variable.
- The hook itself is stdout-safe: it writes through `tracing` and the default hook,
  both of which go to stderr. The stdio transport reserves stderr for logs and the
  bin already installs a subscriber, so installing the hook cannot corrupt the
  JSON-RPC stream on stdout.

Result: `0dec4566`. The probe is gated on the PRESENCE of
`WEBFANG_MCP_TEST_PANIC_TOOL` (any non-empty value), registered after the schema
bridge, `pub(crate)`, and it carries the provenance notice like every other
advertised tool. Registry size is 36 with the switch off and 37 with it on, which
the pre-existing `tools.len() == 36` assertion pins. E2E test:
`stdio_contained_panic_is_recorded_with_location_and_keeps_the_session` drives the
real binary and asserts four things — the switch reached the child, the call is
contained (`isError`, plain explanation, payload withheld), stderr carries the
hook's `panic.location`, and a following call on the SAME session still succeeds.
Two format traps worth keeping in mind if that assertion is ever touched:
`tracing_subscriber::fmt()` emits ANSI **even with stderr piped to a file** (so
`"panic.location="` is not a contiguous substring — assert the field NAME), and one
record spans two physical lines because `PanicHookInfo`'s `Display` embeds a
newline. Verified by removing the hook install: the test then fails on exactly
`server panicked`.

## Out of scope

F6/F7 (session/memory caps), F5 (rate limiter inside auth), G-18/G-21.

## Evidence log

- `9b3d5c0e` — fix(mcp): contain a panicking tool handler instead of killing the
  session
- `d84c4b22` — docs(odd): task record
- native review lineage `review-c10cc26094843cc9` (4 lenses): **approved**,
  acknowledged. Five advisory findings, none blocking; the hygiene ones are
  folded into the follow-up commit, and the retry/duplicate-side-effect limit is
  recorded in the `call_tool` doc.
- `98c134c2` — fix(mcp): bound the panic-payload walk and stop calling it
  non-sensitive (PC-1 + PC-2). Rebased onto `d38970b5`; patch-id unchanged across
  the rebase. Review lineage `review-26f4929e9fed6ff3` approved + acknowledged.
  `assess --base-ref origin/main` → medium, `under_budget`, `review_due: false`.
- PR #1645 — `Closes part of #1626` + `type:bug`. The commit body says
  `Addresses #1626` on purpose: a closing keyword in a squash-merged message
  auto-closes the umbrella with PC-3/PC-4 still open (the #994/#1010 trap).
- `0dec4566` — fix(mcp): give the stdio transport the same panic record as HTTP
  (PC-3). Crate suite 394 pass (302 lib, unchanged). check, clippy (CI flags),
  fmt --check, rustdoc -D warnings clean. Negative control run: removing the hook
  install fails the E2E test on `server panicked`.
