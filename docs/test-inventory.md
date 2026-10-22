# Test Inventory — `#[ignore]` Catalog (Gate 0)

**Source of truth:** `rg -n "#\[ignore" crates/ --glob '!target'` — **12 rows** (8 test attributes + 4 doc/comment mentions).
Generated: `2026-08-21`, updated `2026-09-07`, re-baselined `2026-09-12` (#1328: composition corrected from the stale 27+5 claim to the on-disk 26+6; rows re-keyed from drifting `file:line` to stable `file` + test name). Catalogued #1368 measurement rows `2026-09-13` (#1384 decision: retain + catalogue — the corpus lives outside the repo, so the ignore is legitimate). Catalogued the #1561 timing benchmark `2026-10-03` (quarantined, assertion kept verbatim; needs an idle box with ≥8 cores). Catalogued the #1576 two-spec 97m budgets `2026-10-04` (the new `pool:4` sibling). Linked to `COMPATIBILITY-MATRIX.md`.

**ADR-0004 paso 6 (`2026-10-19`):** the 14 rows under `crates/webfang_ai/**` (9 attributes + 6 doc mentions)
were removed together with the crate — the crate's test targets no longer exist, so the live scan they
fed is gone with them. Rows renumbered 45 → 30; the Quantification and Timing benchmark groups dropped
with their rows (#1368, #1561 lived inside the crate). The 19 remaining ONNX rows live in surviving
core/mcp behavioral tests and keep their ignore budget until the post-paso-6 suite is re-baselined.

**Unit A3 (`2026-10-19`, same ADR):** the local-ONNX test surface is gone from the surviving crates
too, so the budget was re-baselined once more. `crates/webfang_core/tests/behavioral/cli/ai_integration_test.rs`
was deleted (9 attributes + 1 doc mention), the five MCP AI tests are no longer `#[ignore]`d (they run
against a fixture embedding port, not a cached model), and the one MCP doc mention that remains is
catalogued as a Comments/docs row. `trace_correlation_test.rs` KEEPS its ignore, but the reason is
now its real precondition: a configured remote embedding provider. Rows 30 → 16 (12 attributes +
4 doc mentions).

**Unit A4 (`2026-10-19`, same ADR):** the last local-ONNX rows left the tree with
`crates/webfang_core/tests/behavioral/cli/model_asset_test.rs` — the Granite RSS budgets (#1315,
#1576) and the cold-pull event check (#1316) measured artifacts that no longer exist, so their
ignores went with the file. Rows 16 → 12 (8 attributes + 4 doc mentions). The ONNX group keeps the
two rows that do NOT need a local model (a cached ONNX export fixture and a configured remote
embedding provider); the Network group loses the cold pull, which was the only network row outside
the "real DNS / real HTTP" class.

**CI enforcement:** this baseline is a frozen budget — `scripts/check_ignored_guard.sh` runs in the CI `repo-guards` job and fails on any drift between this inventory and the live scan, **per category**: each group's declared count, the file+test-name pair set, and the per-file doc/comment counts are all checked, so a composition swap with an equal total fails even when the sum matches (the totals-only blind spot that #1328 killed). Update this file in the same PR when adding/removing an ignored test or a doc/comment mention.

## Summary by group

| Group | Count | Reason pattern | Issue | Next action |
|-------|-------|----------------|-------|-------------|
| ONNX | 2 | `requires cached ONNX model` / `requires a configured remote embedding provider` | #433 | Re-baseline the post-paso-6 surface |
| Network | 3 | `requires network` / DNS / client | #542 | Keep ignored; wiremock alternative in behavioral |
| Tracing | 1 | `tracing global subscriber` | #501 | Keep ignored; subscriber race |
| Reproduction | 2 | race window too narrow to force from a fixture; kept-RED TDD evidence | #1230, #1429 | Keep ignored; the deterministic pins are the seam test / GREEN bound pin |
| Comments/docs | 4 | doc comment mentions `#[ignore]` | #386 | Not tests — counted as their own checked category |

Total: 2+3+1+2+4 = **12**. (Post-ADR-0004-paso-6 budget, re-baselined twice: by unit A3 the
Quantification and Timing benchmark groups went with the deleted crate — #1368 and #1561's measured
rows lived in `crates/webfang_ai/**` — and the surviving core/mcp ONNX surface was re-counted with
the deleted `ai_integration_test.rs` and the un-ignored MCP AI tests removed from it; by unit A4 the
Granite RSS budgets and the cold-pull check went with the deleted `model_asset_test.rs`.)

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

## Enforced compatibility gates (#1617)

The catalog above answers "what is ignored"; this section answers "what FAILS when the migration contract
is broken", because a documented contract nothing enforces is the defect. None of these tests are
ignored — they run in the ordinary suite and are listed here so the contract's enforcement is
discoverable from the same document as its `#[ignore]` budget.

| Contract | Gate | File |
| :--- | :--- | :--- |
| SQLite schema version | `setup_schema_stamps_the_schema_version_marker`, `setup_schema_refuses_a_foreign_schema_version_without_writing`, `setup_schema_adopts_an_unversioned_database` | `infrastructure/persistence/sqlite.rs` |
| Backup symmetry before every destructive rewrite | `preserve_abandoned_bytes_writes_rejected_bytes_once`, `backup_sibling_extends_the_existing_extension`, `unsupported_version_preserves_a_bak_before_the_downgrade_rewrite`, `corrupt_envelope_preserves_a_bak_before_the_rewrite`, `crc32_mismatch_preserves_bak_before_the_rewrite`, `torn_tail_recovery_backs_up_the_pre_truncation_bytes` | `application/resume.rs`, `infrastructure/export/{record_store,jsonl_writer}.rs`, `application/crawler/checkpoint.rs` |
| JSONL checksum field name pinned | `serialized_uses_the_pinned_checksum_field_name`, `renamed_checksum_field_empties_the_index_instead_of_silently_filling_it` | `infrastructure/export/jsonl_exporter.rs`, `jsonl_writer.rs` |

Adding or removing any of them does NOT change this file's counts — the `#[ignore]` budget and this
table are independent by design, and `scripts/check_ignored_guard.sh` enforces only the former.

## Full catalog (12 rows)

Rows are keyed by **file + identifier**, never by line number — inserting code above an ignored
test must not invalidate its row. `Identifier` is the test function name for attributes and `doc`
for doc/comment mentions (compared per file by count). `Line` is not recorded on purpose.

| # | Group | File | Identifier | Reason | Issue | Next |
|---|-------|------|------------|--------|-------|------|
| 1 | ONNX | `crates/webfang_core/tests/behavioral/cli/export_test.rs` | `vector_export_total_documents_matches_documents` | `requires cached ONNX model` | #433 | Sprint 1 |
| 2 | ONNX | `crates/webfang_core/tests/behavioral/cli/trace_correlation_test.rs` | `scrape_trace_and_vector_export_share_per_page_correlation` | `requires a configured remote embedding provider` | #433 | Sprint 1 |
| 3 | Network | `crates/webfang_core/tests/cli_binary_test.rs` | `test_dry_run_with_url` | `requires network access` | #542 | keep ignored |
| 4 | Network | `crates/webfang_core/src/application/http_client/client.rs` | `test_http_client_get_example_com` | `requires network - run with cargo test --ignored` | #542 | keep ignored |
| 5 | Network | `crates/webfang_core/src/infrastructure/crawler/sitemap_parser.rs` | `test_parse_from_url_depth_one_attempts_fetch` | `requires network — hits real DNS for invalid-host-xyz-12345.com` | #542 | keep ignored (by design) |
| 6 | Tracing | `crates/webfang_core/src/infrastructure/observability/logging.rs` | `test_init_json_logging_with_temp_dir` | `tracing global subscriber may already be set in test context` | #501 | keep ignored |
| 7 | Reproduction | `crates/webfang_core/tests/behavioral/cli/transactional_store_test.rs` | `concurrent_resume_processes_lose_no_records` | asserts the no-loss invariant under real multi-process contention, but measured 8/8 runs against the unfixed code with zero records lost — the clobber window is microseconds wide and a fixture cannot force it. NOT a falsifier of #1230; the deterministic falsifier is tests/record_store_transaction_test.rs | #1230 | Keep ignored; run by hand for stress evidence |
| 8 | Reproduction | `crates/webfang_core/src/application/crawler/sitemap_session.rs` | `sitemap_entry_max_pages_truncates_many_url_fixture` | kept RED by design: asserts the naive unbounded reading (all 9 pages with max_pages=3), fails against the truncating bound; GREEN pin `sitemap_entry_max_pages_bound_pins_inflight_drain` carries the contract | #1429 | Keep ignored; RED-evidence for the overshoot bound, never run in CI |
| 9 | Comments/docs | `crates/webfang_mcp/tests/mcp_behavioral_test.rs` | `doc` | `/// (A second test asserted the same thing under the name `mcp_ai_tools_registered_with_ai_off`; it was `#[ignore]`d behind a "requires cached ONNX model" reason it never needed. One test, one assertion.)` | #386 | docs only |
| 10 | Comments/docs | `crates/webfang_core/tests/behavioral/cli/error_path_test.rs` | `doc` | `/// No `#[ignore]`: the gate fires before any cleaner could be built.` | #386 | docs only |
| 11 | Comments/docs | `crates/webfang_core/tests/behavioral/cli/error_path_test.rs` | `doc` | `/// No `#[ignore]`: the gate fires before any cleaner could be built.` | #386 | docs only |
| 12 | Comments/docs | `crates/webfang_core/tests/behavioral/cli/waf_gauntlet_test.rs` | `doc` | `/// ... the historical wiremock-FIFO flakiness that motivated `#[ignore]` no longer applies.` | #386 | docs only |
| 13 | Reproduction | `crates/webfang_core/src/infrastructure/export/tail_window.rs` | `tail_read_memory_probe` | measures the RSS band of the tail recovery: it re-runs the test binary as a child process over a 69 MB export fixture, so it is slow and needs a real filesystem | #2237 | Keep ignored; run by hand for the memory-band evidence |

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
# Then categorize into the groups in the table above (counts live in the Summary table).
```

SDD: `sdd/stabilization-sprint0-baseline` | Matrix: `../COMPATIBILITY-MATRIX.md`
