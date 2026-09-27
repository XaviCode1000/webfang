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
