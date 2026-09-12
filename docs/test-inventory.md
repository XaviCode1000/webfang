# Test Inventory — `#[ignore]` Catalog (Gate 0)

**Source of truth:** `rg -n "#\[ignore" crates/ --glob '!target'` — **35 rows** (29 test attributes + 6 doc/comment mentions).
Generated: `2026-08-21`, updated `2026-09-07`, re-baselined `2026-09-12` (#1328: composition corrected from the stale 27+5 claim to the on-disk 26+6; rows re-keyed from drifting `file:line` to stable `file` + test name). Linked to `COMPATIBILITY-MATRIX.md`.

**CI enforcement:** this baseline is a frozen budget — `scripts/check_ignored_guard.sh` runs in the CI `repo-guards` job and fails on any drift between this inventory and the live scan, **per category**: each group's declared count, the file+test-name pair set, and the per-file doc/comment counts are all checked, so a composition swap with an equal total fails even when the sum matches (the totals-only blind spot that #1328 killed). Update this file in the same PR when adding/removing an ignored test or a doc/comment mention.

## Summary by group

| Group | Count | Reason pattern | Issue | Next action |
|-------|-------|----------------|-------|-------------|
| ONNX | 23 | `requires cached ONNX model` / `requires the granite-<tier> model in the native HF cache` | #433, #1315 | Sprint 1 promote with cache |
| Network | 4 | `requires network` / DNS / client | #542, #1316 | Keep ignored; wiremock alternative in behavioral |
| Tracing | 1 | `tracing global subscriber` | #501 | Keep ignored; subscriber race |
| Reproduction | 1 | race window too narrow to force from a fixture | #1230 | Keep ignored; the deterministic pin is the seam test |
| Comments/docs | 6 | doc comment mentions `#[ignore]` | #386 | Not tests — counted as their own checked category |

Total: 23+4+1+1+6 = **35**.

> The former **WAF** group (1 row, `waf_gauntlet` at `waf_gauntlet_test.rs:126`, #337) is gone:
> `waf_gauntlet_observability_trace` was un-ignored — the mock is counter-based and deterministic,
> so the historical wiremock-FIFO flakiness that motivated the ignore no longer applies. Its
> mention of `#[ignore]` survives only as the doc comment at the top of the test, which is why
> Comments/docs grew 5 → 6 while the attribute budget shrank 27 → 26. This exact swap passed the
> old totals-only guard green — the regression #1328 pins against.

## Sitemap correction

Stale roadmap claim "7 sitemap tests ignored" is **false**. Reality:
- **1 ignored**: `test_parse_from_url_depth_one_attempts_fetch` in `crates/webfang_core/src/infrastructure/crawler/sitemap_parser.rs` (`requires network — hits real DNS for invalid-host-xyz-12345.com`, by design)
- **18 active** (`cargo nextest run -p webfang_core -- sitemap` passes)

Matrix: [`COMPATIBILITY-MATRIX.md`](../COMPATIBILITY-MATRIX.md).

## Full catalog (35 rows)

Rows are keyed by **file + identifier**, never by line number — inserting code above an ignored
test must not invalidate its row. `Identifier` is the test function name for attributes and `doc`
for doc/comment mentions (compared per file by count). `Line` is not recorded on purpose.

| # | Group | File | Identifier | Reason | Issue | Next |
|---|-------|------|------------|--------|-------|------|
| 1 | ONNX | `crates/webfang_ai/tests/ai_integration.rs` | `test_semantic_cleaner_full_pipeline` | `requires cached ONNX model` | #433 | Sprint 1 cache |
| 2 | ONNX | `crates/webfang_ai/tests/ai_integration.rs` | `test_semantic_cleaner_long_content` | `requires cached ONNX model` | #433 | Sprint 1 cache |
| 3 | ONNX | `crates/webfang_ai/tests/ai_integration.rs` | `test_error_chunk_too_large` | `requires cached ONNX model` | #433 | Sprint 1 cache |
| 4 | ONNX | `crates/webfang_ai/tests/ai_integration.rs` | `test_pipeline_empty_input` | `requires cached ONNX model` | #433 | Sprint 1 cache |
| 5 | ONNX | `crates/webfang_ai/tests/ai_integration.rs` | `test_pipeline_html_only` | `requires cached ONNX model` | #433 | Sprint 1 cache |
| 6 | ONNX | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs` | `scrape_clean_ai_pipeline` | `requires cached ONNX model` | #433 | Sprint 1 |
| 7 | ONNX | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs` | `clean_ai_output_vectors` | `requires cached ONNX model` | #433 | Sprint 1 |
| 8 | ONNX | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs` | `batch_mode_clean_ai` | `requires cached ONNX model` | #433 | Sprint 1 |
| 9 | ONNX | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs` | `clean_ai_error_no_model` | `requires cached ONNX model` | #433 | Sprint 1 |
| 10 | ONNX | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs` | `clean_ai_threshold_reject` | `requires cached ONNX model` | #433 | Sprint 1 |
| 11 | ONNX | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs` | `clean_ai_model_reject` | `requires cached ONNX model` | #433 | Sprint 1 |
| 12 | ONNX | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs` | `clean_ai_trace_file` | `requires cached ONNX model` | #433 | Sprint 1 |
| 13 | ONNX | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs` | `clean_ai_feature_gated` | `requires cached ONNX model` | #433 | Sprint 1 |
| 14 | ONNX | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs` | `clean_ai_exports_chunks_regression_569` | `requires cached ONNX model` | #433 | Sprint 1 |
| 15 | ONNX | `crates/webfang_core/tests/behavioral/cli/export_test.rs` | `vector_export_total_documents_matches_documents` | `requires cached ONNX model` | #433 | Sprint 1 |
| 16 | ONNX | `crates/webfang_core/tests/behavioral/cli/trace_correlation_test.rs` | `scrape_trace_and_vector_export_share_per_page_correlation` | `requires cached ONNX model` | #433 | Sprint 1 |
| 17 | ONNX | `crates/webfang_mcp/tests/mcp_behavioral_test.rs` | `semantic_cleaner_with_cleaner_success` | `requires cached ONNX model` | #433 | Sprint 1 |
| 18 | ONNX | `crates/webfang_mcp/tests/mcp_behavioral_test.rs` | `search_obsidian_honest_error` | `requires cached ONNX model` | #433 | Sprint 1 |
| 19 | ONNX | `crates/webfang_mcp/tests/mcp_behavioral_test.rs` | `mcp_ai_observability` | `requires cached ONNX model` | #433 | Sprint 1 |
| 20 | ONNX | `crates/webfang_mcp/tests/mcp_behavioral_test.rs` | `semantic_cleaner_invalid_url_reject` | `requires cached ONNX model` | #433 | Sprint 1 |
| 21 | ONNX | `crates/webfang_mcp/tests/mcp_behavioral_test.rs` | `mcp_ai_tools_registered_with_ai_off` | `requires cached ONNX model` | #433 | Sprint 1 |
| 22 | ONNX | `crates/webfang_core/tests/behavioral/cli/model_asset_test.rs` | `tier_97m_peak_rss_under_budget` | `requires the granite-97m model in the native HF cache` | #1315 | Keep ignored; run by hand for per-tier RSS evidence (plan row 7.12) |
| 23 | ONNX | `crates/webfang_core/tests/behavioral/cli/model_asset_test.rs` | `tier_311m_peak_rss_under_budget` | `requires the granite-311m ONNX model (~1.2 GB) in the native HF cache` | #1315 | Keep ignored; this is the tier the mmap fix targets — run by hand |
| 24 | Network | `crates/webfang_core/tests/cli_binary_test.rs` | `test_dry_run_with_url` | `requires network access` | #542 | keep ignored |
| 25 | Network | `crates/webfang_core/src/application/http_client/client.rs` | `test_http_client_get_example_com` | `requires network - run with cargo test --ignored` | #542 | keep ignored |
| 26 | Network | `crates/webfang_core/src/infrastructure/crawler/sitemap_parser.rs` | `test_parse_from_url_depth_one_attempts_fetch` | `requires network — hits real DNS for invalid-host-xyz-12345.com` | #542 | keep ignored (by design) |
| 27 | Network | `crates/webfang_core/tests/behavioral/cli/model_asset_test.rs` | `cold_pull_emits_structured_events_without_tty` | `requires network: performs a real cold pull (~390 MB) into an isolated HF_HOME` | #1316 | Keep ignored; run by hand to prove non-TTY resolve events |
| 28 | Tracing | `crates/webfang_core/src/infrastructure/observability/logging.rs` | `test_init_json_logging_with_temp_dir` | `tracing global subscriber may already be set in test context` | #501 | keep ignored |
| 29 | Reproduction | `crates/webfang_core/tests/behavioral/cli/transactional_store_test.rs` | `concurrent_resume_processes_lose_no_records` | asserts the no-loss invariant under real multi-process contention, but measured 8/8 runs against the unfixed code with zero records lost — the clobber window is microseconds wide and a fixture cannot force it. NOT a falsifier of #1230; the deterministic falsifier is tests/record_store_transaction_test.rs | #1230 | Keep ignored; run by hand for stress evidence |
| 30 | Comments/docs | `crates/webfang_ai/src/infrastructure_ai/granite_dom_inspector.rs` | `doc` | `// Integration tests should use real models with #[ignore] annotation.` | #386 | docs only |
| 31 | Comments/docs | `crates/webfang_ai/tests/ai_integration.rs` | `doc` | `/// ... These pipeline tests are #[ignore]'d (require the cached ONNX` | #386 | docs only |
| 32 | Comments/docs | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs` | `doc` | `//! All tests are #[ignore = "requires cached ONNX model"]` | #386 | docs only |
| 33 | Comments/docs | `crates/webfang_core/tests/behavioral/cli/error_path_test.rs` | `doc` | `/// No #[ignore]: the gate fires before any ONNX model could load.` | #386 | docs only |
| 34 | Comments/docs | `crates/webfang_core/tests/behavioral/cli/error_path_test.rs` | `doc` | `/// No #[ignore]: the gate fires before any ONNX model could load.` | #386 | docs only |
| 35 | Comments/docs | `crates/webfang_core/tests/behavioral/cli/waf_gauntlet_test.rs` | `doc` | `/// ... the historical wiremock-FIFO flakiness that motivated #[ignore] no longer applies.` | #386 | docs only |

> **Guard contract (since #1328, 2026-09-12):** `check_ignored_guard.sh` compares this catalog to
> the live scan per category — group counts, the file+test-name pair set, and per-file doc/comment
> counts — not just the total. The old totals-only check tolerated a wrong composition (the WAF
> un-ignore plus a new doc mention kept 32/32 green while both parts were wrong) and keyed rows by
> `file:line`, which drifted silently: 14 of 32 rows pointed at blank or unrelated lines and the
> diagnostic printed 16 untracked + 15 stale on a PR that added zero ignored tests. Both failure
> modes are pinned by `scripts/test_ignored_guard.sh`.

## Generation

```bash
rg -n "#\[ignore" crates/ --glob '!target'
# Attributes: lines whose content starts with #[ignore — pair each with the
# following test fn name. Prose: every other mention — group per file.
# Then categorize: ONNX 21, network 3, tracing 1, reproduction 1, doc/comment 6.
```

SDD: `sdd/stabilization-sprint0-baseline` | Matrix: `../COMPATIBILITY-MATRIX.md`
