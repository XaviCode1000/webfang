# #1638: tracing callsite Interest poisoning in the WAF capture test

## Goal
Make `verify_waf_integrity_blocked_keeps_pattern_in_tracing_not_in_response`
deterministic in every suite configuration, including the libtest-based
`Coverage` lane where it intermittently observes a completely empty tracing
buffer.

## Root cause
`tracing` caches per-callsite `Interest` process-wide through a one-time
`compare_exchange`. The test asserts on a `tracing::info!` event emitted at
`crates/webfang_mcp/src/mcp_server/handlers/security.rs:82` (the
`waf evidence matched pattern` callsite).

`verify_waf_integrity_handler_blocks_with_status` drives **the same blocked
path with the same HTML and installs no subscriber**. Under libtest — one
process, many threads — when that test's thread reaches the callsite first,
`Dispatch::none()` registers `Interest::never()`, permanently disabling that
callsite for every other thread. The capture test then writes zero bytes.

Under `cargo nextest` every test is its own process, so the callsite can never
be poisoned by a sibling test. That is why the flake only ever surfaces in the
libtest-based `Coverage` lane and never under nextest.

Third occurrence of this defect class; #417 → `1ef88a48` (PR #418) and #664
are the precedents.

## Fix
Reuse the established in-repo pattern from
`crates/webfang_core/src/application/pipeline/executor.rs`:

- Module-level `ensure_global_subscriber()` guarded by `Once`, installing a
  global sink subscriber. Setting a *global* default rebuilds the cached
  interest of every already-registered callsite, so calling it at the top of
  the capture test also repairs a poison that already happened.
- Restructure the test from `#[tokio::test]` + `set_default` (thread-local
  guard held across awaits) to `#[test]` + `with_default` (closure-scoped) with
  an explicit `current_thread` runtime, removing the async/thread-local
  interaction surface.
- Drop `.with_max_level(INFO)`: it is redundant against the fmt default and it
  mutates the process-wide `LevelFilter`, which is itself a poisoning vector.

## Scope
- `crates/webfang_mcp/src/mcp_server/handlers/security.rs` (test module only).

## Out of scope
- No production code. The `info!` event and the off-channel pattern guarantee
  (#1601) are unchanged — only their test observability is made deterministic.
- No change to `mcp_server/mod.rs`, `server.rs`, `handlers/mod.rs` or
  `test_probe.rs` (deliberately kept disjoint from the concurrent #1611 lane).
- The two other `capture_subscriber` copies in `webfang_core` are not touched;
  deduplicating them is a separate change.

## Tasks
1. [x] Bootstrap worktree, isolated `CARGO_TARGET_DIR`, CodeGraph + CodeDB indexes.
2. [x] Intelligence gate: locate the callsite and the #417 precedent before editing.
3. [x] Identify the in-binary poisoner (`verify_waf_integrity_handler_blocks_with_status`).
4. [x] Apply the guard and restructure the test.
5. [x] A/B measurement: flake rate with the guard disabled vs enabled.
6. [x] Full pre-commit gate: check, strict clippy, `fmt --check`, rustdoc.
7. [ ] Native review of the candidate, then work-unit commit.

## Acceptance
- The test passes deterministically under libtest, not just under nextest.
- The product guarantee the test protects is unchanged: the exact
  `matched_pattern` still reaches structured tracing and still never reaches
  the agent-facing MCP channel.
- Zero production-code change; the diff is confined to the test module.

## Evidence — A/B measurement (same binary, `--test-threads=8`)

| Variant | Result |
| :--- | :--- |
| Guard call removed (flake reproducible) | **3 / 60 runs FAILED** |
| Guard restored | **0 / 120 runs FAILED** |

A single green run proves nothing for an intermittent defect, so the fix is
backed by the A/B above rather than by one passing build.
