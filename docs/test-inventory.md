# Test Inventory — `#[ignore]` Catalog (Gate 0)

**Source of truth:** `rg -n "#\[ignore" crates/ --glob '!target'` — **31 rows** (26 test attributes + 5 doc/comment mentions).
Generated: `2026-08-21`. Linked to `COMPATIBILITY-MATRIX.md`.

**CI enforcement:** this baseline is a frozen budget — `scripts/check_ignored_guard.sh` runs in the CI `toolchain` job and fails on any drift between this inventory and the live count (stabilization-sitemap-regression). Update this file in the same PR when adding/removing an ignored test.

## Summary by group

| Group | Count | Reason pattern | Issue | Next action |
|-------|-------|----------------|-------|-------------|
| ONNX (AI) | 21 | `requires cached ONNX model` | #433 | Sprint 1 promote with cache |
| Network | 3 | `requires network` / DNS / client | #542 | Keep ignored; wiremock alternative in behavioral |
| Tracing | 1 | `tracing global subscriber` | #501 | Keep ignored; subscriber race |
| WAF | 1 | `waf` / bare `#[ignore]` | #337 | Keep ignored; manual gauntlet |
| Comments/docs | 5 | doc comment mentions `#[ignore]` | #386 | Not tests — count only |

Total: 21+3+1+1+5 = **31**.

## Sitemap correction

Stale roadmap claim "7 sitemap tests ignored" is **false**. Reality:
- **1 ignored** at `crates/webfang_core/src/infrastructure/crawler/sitemap_parser.rs:1218` (`requires network — hits real DNS for invalid-host-xyz-12345.com`, by design)
- **18 active** (`cargo nextest run -p webfang_core -- sitemap` passes)

Matrix: [`COMPATIBILITY-MATRIX.md`](../COMPATIBILITY-MATRIX.md).

## Full catalog (31 rows)

| # | Test / Location | File:Line | Reason | Issue | Next |
|---|-----------------|-----------|--------|-------|------|
| 1 | `doc comment` | `crates/webfang_ai/src/infrastructure_ai/granite_dom_inspector.rs:104` | `// Integration tests should use real models with #[ignore] annotation.` | #386 | docs only |
| 2 | `doc comment` | `crates/webfang_ai/tests/ai_integration.rs:331` | `/// ... These pipeline tests are #[ignore]'d (require the cached ONNX` | #386 | docs only |
| 3 | `test_ai_pipeline_1` | `crates/webfang_ai/tests/ai_integration.rs:346` | `requires cached ONNX model` | #433 | Sprint 1 cache |
| 4 | `test_ai_pipeline_2` | `crates/webfang_ai/tests/ai_integration.rs:365` | `requires cached ONNX model` | #433 | Sprint 1 cache |
| 5 | `test_ai_pipeline_3` | `crates/webfang_ai/tests/ai_integration.rs:442` | `requires cached ONNX model` | #433 | Sprint 1 cache |
| 6 | `test_ai_pipeline_4` | `crates/webfang_ai/tests/ai_integration.rs:495` | `requires cached ONNX model` | #433 | Sprint 1 cache |
| 7 | `test_ai_pipeline_5` | `crates/webfang_ai/tests/ai_integration.rs:512` | `requires cached ONNX model` | #433 | Sprint 1 cache |
| 8 | `http_client network` | `crates/webfang_core/src/application/http_client/client.rs:593` | `requires network - run with cargo test --ignored` | #542 | keep ignored |
| 9 | `sitemap DNS` | `crates/webfang_core/src/infrastructure/crawler/sitemap_parser.rs:1218` | `requires network — hits real DNS for invalid-host-xyz-12345.com` | #542 | keep ignored (by design) |
| 10 | `observability tracing` | `crates/webfang_core/src/infrastructure/observability/logging.rs:202` | `tracing global subscriber may already be set in test context` | #501 | keep ignored |
| 11 | `cli_binary network` | `crates/webfang_core/tests/cli_binary_test.rs:96` | `requires network access` | #542 | keep ignored |
| 12 | `doc comment` | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs:3` | `//! All tests are #[ignore = "requires cached ONNX model"]` | #386 | docs only |
| 13 | `ai_integration` | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs:37` | `requires cached ONNX model` | #433 | Sprint 1 |
| 14 | `ai_integration` | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs:76` | `requires cached ONNX model` | #433 | Sprint 1 |
| 15 | `ai_integration` | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs:131` | `requires cached ONNX model` | #433 | Sprint 1 |
| 16 | `ai_integration` | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs:177` | `requires cached ONNX model` | #433 | Sprint 1 |
| 17 | `ai_integration` | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs:221` | `requires cached ONNX model` | #433 | Sprint 1 |
| 18 | `ai_integration` | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs:255` | `requires cached ONNX model` | #433 | Sprint 1 |
| 19 | `ai_integration` | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs:289` | `requires cached ONNX model` | #433 | Sprint 1 |
| 20 | `ai_integration` | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs:342` | `requires cached ONNX model` | #433 | Sprint 1 |
| 21 | `ai_integration` | `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs:407` | `requires cached ONNX model` | #433 | Sprint 1 |
| 22 | `doc comment` | `crates/webfang_core/tests/behavioral/cli/error_path_test.rs:288` | `/// No #[ignore]: the gate fires before any ONNX model could load.` | #386 | docs only |
| 23 | `doc comment` | `crates/webfang_core/tests/behavioral/cli/error_path_test.rs:366` | `/// No #[ignore]: the gate fires before any ONNX model could load.` | #386 | docs only |
| 24 | `export_vector` | `crates/webfang_core/tests/behavioral/cli/export_test.rs:25` | `requires cached ONNX model` | #433 | Sprint 1 |
| 25 | `trace_correlation` | `crates/webfang_core/tests/behavioral/cli/trace_correlation_test.rs:217` | `requires cached ONNX model` | #433 | Sprint 1 |
| 26 | `waf_gauntlet` | `crates/webfang_core/tests/behavioral/cli/waf_gauntlet_test.rs:126` | `#[ignore]` (bare, WAF fixtures) | #337 | manual |
| 27 | `mcp behavioral` | `crates/webfang_mcp/tests/mcp_behavioral_test.rs:1375` | `requires cached ONNX model` | #433 | Sprint 1 |
| 28 | `mcp behavioral` | `crates/webfang_mcp/tests/mcp_behavioral_test.rs:1444` | `requires cached ONNX model` | #433 | Sprint 1 |
| 29 | `mcp behavioral` | `crates/webfang_mcp/tests/mcp_behavioral_test.rs:1475` | `requires cached ONNX model` | #433 | Sprint 1 |
| 30 | `mcp behavioral` | `crates/webfang_mcp/tests/mcp_behavioral_test.rs:1530` | `requires cached ONNX model` | #433 | Sprint 1 |
| 31 | `mcp behavioral` | `crates/webfang_mcp/tests/mcp_behavioral_test.rs:1559` | `requires cached ONNX model` | #433 | Sprint 1 |

## Generation

```bash
rg -n "#\[ignore" crates/ --glob '!target'
# Then categorize: ONNX 21, network 3, timing 3, env 3, tracing 1, waf 1, dry-run 1, comment 1+4
```

SDD: `sdd/stabilization-sprint0-baseline` | Matrix: `../COMPATIBILITY-MATRIX.md`

