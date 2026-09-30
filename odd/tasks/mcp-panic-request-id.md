# Feature: recover the JSON-RPC request id for a contained panic (issue #1646)

## Goal

A panic contained on the HTTP request path must answer JSON-RPC `-32603` carrying the
`id` of the request that triggered it, instead of a hardcoded `null`. Acceptance bar set
by #1626 PC-4: a **transport-level** assertion through the real stack, not a unit test
of a helper.

## Precondition verified

#1613 landed in `~/Projects/Rust/webfang-worktrees/fix-mcp-error-decision-table`
(`fix/mcp-error-decision-table`, 5 commits, tip `9dd88b39`). Its contract is real:
`docs/src/mcp-error-contract.md` row **A19** ("A panic escapes the HTTP request path →
`-32603` body, HTTP 500") and transport row **T9** both exist, and the reason-slug
taxonomy is published. This work changes only the `id` member of that answer — it does
not re-decide the channel (that is #1613's, and it is done).

## Design (settled before writing)

1. New module `crates/webfang_mcp/src/mcp_server/panic_containment.rs`:
   - `RecoveredRequestId` — the id plus how it was recovered.
   - `scan_envelope_id(&[u8])` — a bounded, non-allocating scanner for the top-level
     `id` member of a JSON-RPC envelope (no `serde_json::from_slice` on the whole body,
     so a truncated prefix can never be mistaken for a parse).
   - `capture_request_id(Request) -> (Request, RecoveredRequestId)` — reads only a
     **prefix** of the body, then re-chains the unread remainder, so streaming, the
     `413` body-limit row and every other transport row are untouched.
   - `jsonrpc_panic_containment` — `axum::middleware::from_fn`: capture id → insert as a
     request extension → `AssertUnwindSafe(next.run(..)).catch_unwind()` → on panic,
     answer `-32603` carrying the id.
2. `server.rs`: mount that layer **innermost** (applied first, right after
   `nest_service`) so it sits inside auth / rate limit / timeout / body limit —
   an unauthenticated flood can never make the server buffer a body. `CatchPanicLayer`
   stays as the outermost backstop for panics raised in those gates, where no body has
   been read: those answer `id: null` by the documented rule, not silently.
3. `jsonrpc_panic_response` keeps its signature (it is the `CatchPanicLayer` handler)
   and delegates body construction to a shared builder that takes the id.

## Documented id rules (never a silent null)

| Body | `id` in the `-32603` |
| :--- | :--- |
| single JSON-RPC request with an `id` | echoed verbatim |
| single request, `id` absent (notification) | `null` — the spec's own "unknown id" |
| single request, `id: null` | `null` |
| JSON-RPC batch (array) | `null` — one answer cannot name N requests; this is the case the spec assigns null to |
| non-JSON / not an object / body read error | `null` |
| envelope not closed within the 64 KiB scan window | `null` |
| panic in auth / rate limit / timeout / body limit | `null` — the body was never read by design |

## Tasks

- [x] T1 Verify #1613's contract (A19 / T9 rows, reason taxonomy) — done, reported.
- [x] T2 Intelligence gate on `jsonrpc_panic_response` (2 call sites, both in
      `server.rs`; the integration suite drives the stack) — done.
- [x] T3 Implement `panic_containment.rs` + mount it + refactor the response builder.
- [x] T4 Transport-level tests: id echoed, batch, non-JSON, notification.
- [x] T5 Update the existing suite/unit tests that pinned `id: null` for the HTTP path.
- [x] T6 Document the id rule on the contract page (row A19 / T9).
- [x] T7 Gates: check, strict clippy, fmt --check, rustdoc, `nextest -p webfang_mcp`.

## Verification

Recorded in the final report; commits in the history of `fix/mcp-panic-request-id`.
