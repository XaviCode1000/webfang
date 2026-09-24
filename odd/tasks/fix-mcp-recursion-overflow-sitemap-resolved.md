# Fix MCP sitemap recursion overflow

## Objective

Eliminate the Rust `recursion_depth_exceeding_limit` / `CoerceUnsized` warning caused by the shared sitemap-discovery future crossing the `rmcp::#[tool]` async boundary, while preserving the public API and existing tracing spans.

## Evidence

- Dual-removal A/B left the warning present and moved it to `discover_sitemap`.
- Boxing the shared `crawl_with_sitemap_resolved` future removed both diagnostics.
- A production-shaped experiment preserved the public `async fn` and its original `#[instrument]` attribute, delegating to a private boxed `Send` helper; nightly `cargo check -p webfang_mcp` passed with no recursion diagnostics.

## Scope

- `crates/webfang_core/src/application/crawler/sitemap_discovery.rs`
- Add the private boxed future helper and explanatory compiler-workaround comment.
- Preserve the public `crawl_with_sitemap_resolved` signature, span name, and fields.

## Constraints

- No `recursion_limit` suppression.
- No `unsafe impl Send`.
- No changes to `Engine::run`, session orchestration, MCP handlers, or unrelated warnings.
- No commit or push without explicit user request.

## Tasks

- [x] T1 Implement the private boxed `Send` future boundary behind the public async wrapper.
- [ ] T2 Run focused compiler checks and the Miri webfang_mcp library suite with the prescribed flags. Compiler/workspace/format gates pass; full Miri and the targeted sitemap filter are blocked by the pre-existing BoringSSL `TLS_method` FFI limitation. The targeted filter passed 1 test before aborting on 1 FFI test; 2 tests were not run.
- [x] T3 Read back the diff and report verification evidence; leave delivery commit/PR to explicit user direction.

## Acceptance criteria

- `crawl_with_sitemap_resolved` remains `pub async fn` with its original `#[instrument]` metadata.
- The compiler emits no `recursion_depth_exceeding_limit`, `CoerceUnsized`, or `overflow evaluating` diagnostic for `webfang_mcp`.
- `cargo check -p webfang_mcp` passes on `nightly-2026-08-27`.
- `cargo miri test -p webfang_mcp --lib` passes with the prescribed MIRIFLAGS, or any environmental blocker is reported precisely.
- The diff is limited to the sitemap discovery boundary and its explanatory comment.

## Hallazgos colaterales

- `crawl_with_sitemap_rejects_internal_sitemap_url` lacks an Miri `#[ignore]` and aborts while constructing a real `wreq` client through BoringSSL's unsupported `TLS_method` FFI. This is the same pre-existing omission class as `preflight.rs` and is not introduced by this fix. The native filtered test passes; a follow-up Miri-coverage unit should either add a documented ignore or provide a non-FFI mock path.

## Progress

- 2026-09-16: Disposable A/B evidence confirmed the shared boundary and the tracing-preserving helper shape.
- 2026-09-16: T1 implemented; focused nightly `cargo check -p webfang_mcp` passed with no recursion/CoerceUnsized diagnostics.
- 2026-09-16: Native `cargo test -p webfang_mcp --lib -- crawl_with_sitemap discover_sitemap` passed all 5 selected tests with no failures or panics. Miri did not report a regression in the subset that reached execution; the full suite remained blocked by the pre-existing BoringSSL `TLS_method` FFI limitation in an unrelated AI-handler test, so the remaining tests were not evaluated. Targeted `crawl_with_sitemap` Miri selected 4 tests, passed 1, then aborted on the same FFI in the SSRF test; 2 were not run. This is not a claim that Miri validated the full tree. Final diff review passed; LSP diagnostics were clean. T2 remains pending solely because Miri cannot execute the FFI-dependent tests.
- 2026-09-16: Work-unit commit created with message `fix(mcp): bound sitemap future recursion depth`; post-commit native verification passed 5/5 and `git show --check` passed. The final commit identity is recorded by `git rev-parse HEAD` on the feature branch.
- 2026-09-16: Created linked issue #1567 with `type:bug`; `status:approved` is intentionally pending maintainer approval before PR creation.

## Route and verification

- Route: delegated bounded writer for the one-file implementation, then fresh read-only verifier.
- TDD: not activated in the resolved session configuration; ordinary compiler and Miri checks are required.
- Rollback boundary: remove the private helper and restore the original single-line body in `sitemap_discovery.rs`.
