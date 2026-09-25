# C-01a: MCP SSRF disable switch exact-value policy

## Goal
Make `WEBFANG_MCP_DISABLE_SSRF` disable only the MCP entry guard for the exact value `"1"`, while preserving the separate presence-based `WEBFANG_DISABLE_SSRF` contract in LLM extraction.

## Scope
- Add a pure domain parser for exact-`"1"` disable values.
- Wire the MCP entry validator to the parser and warn once for invalid present values.
- Add pure unit coverage and MCP entry-point matrix coverage using the existing EnvGuard/test fixture.
- Record the operational behavior change and migration in the repository's change-proposal record if one exists; otherwise report the missing artifact rather than inventing a new location.

## Out of scope
- Do not change `WEBFANG_DISABLE_SSRF` or `llm_extraction::ssrf_gate`.
- Do not add compatibility aliases for invalid MCP disable values.
- Do not change the core layered SSRF knobs or their documented semantics.

## Tasks
1. [x] Inspect the real MCP matrix fixture and choose the existing EnvGuard pattern.
2. [x] Implement the domain parser, MCP wiring, observability, tests, and migration documentation.
3. [x] Run focused tests, strict formatting/check gates, and inspect the diff.
4. [ ] Record final verification evidence and close the work unit; blocked because `review-risk` cannot route a provider/model from this host.

## Acceptance
- `"1"` disables only the MCP entry guard.
- absent, `"0"`, `"false"`, `"yes"`, `"TRUE"`, empty, and whitespace variants leave the entry guard active.
- invalid present values warn at most once per process.
- `WEBFANG_DISABLE_SSRF` behavior is unchanged.
- `crates/webfang_test_utils/src/lib.rs` documents the separate MCP hatch contract.
- tests and documentation are included in the same work unit.

## Verification evidence
- Worktree: `/home/xavi/Projects/Rust/webfang-worktrees/fix-c01a-mcp-ssrf`
- Branch: `fix/c01a-mcp-ssrf`
- Starting HEAD: `d8c2216c5f4551504356454068d502ee50094dde`
- `cargo test -p webfang_core disable_value_tests -- --nocapture` — 1 passed.
- `cargo nextest run -p webfang_mcp --features mcp --test mcp_ssrf_knob_matrix_test` — 4 passed.
- `cargo check` — passed.
- Strict clippy gate — passed.
- `cargo fmt --all -- --check` — passed.
- `env 'RUSTDOCFLAGS=-D warnings' cargo doc --workspace --all-features --no-deps` — passed.
- `git diff --check` — passed.
- `llm_extraction.rs` has no diff; C-01b remains out of scope.
- Native review collection was attempted on the frozen candidate but produced no reviewer result. Host relay returned HTTP 403 `FreeTierError: OpenCode's free tier can only be used from within OpenCode`; 0 reviewers were prepared or submitted. The candidate remains unmutated, and no review capture or delivery was performed.
- Delivery completed after the review blocker was documented: commit `a9eff4cd` was pushed to `origin/fix/c01a-mcp-ssrf`.
- Native review remains unavailable (`review-risk` host relay returned HTTP 403 `FreeTierError`); delivery therefore followed ordinary repository policy.
