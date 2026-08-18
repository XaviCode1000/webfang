# Verify — #785 hybrid shell escalation

All gates run on 2026-08-18 in worktree `fix-batch-crawler` with
`CARGO_TARGET_DIR=$HOME/.cache/cargo-target/webfang`.

## Gate 1 — compile

```
cargo check -p webfang_core
→ Finished `dev` profile, 0 warnings, 0 errors
```

## Gate 2 — strict clippy (exact CI gate)

```
cargo clippy -p webfang_core --all-targets --all-features -- \
  -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines
→ Finished, 0 warnings
```

## Gate 3 — format

```
cargo fmt --all -- --check
→ clean (0 diffs)
```

## Gate 4 — tests (cargo nextest)

| Suite | Command | Result |
| :--- | :--- | :--- |
| Mandated `spa` gate | `cargo nextest run -p webfang_core spa` | **114 passed, 0 failed** |
| spa_detector unit | `cargo nextest run -p webfang_core --lib spa_detector` | **27 passed, 0 failed** |
| hybrid_router unit | `cargo nextest run -p webfang_core --lib hybrid_router` | **14 passed, 0 failed** |
| spa_detector doctest | `cargo test -p webfang_core --doc spa_detector` | **1 passed, 0 failed** |
| engine js-strategy | `cargo nextest run -p webfang_core --test engine_js_strategy_timeout_test` | **1 passed, 0 failed** |

## Requirement evidence

| REQ | Test(s) | Outcome |
| :--- | :--- | :--- |
| REQ-HYBRID-SHELL-01 | `test_shell_with_boilerplate_escalates` (quotes.toscrape.com/js shape: nav + Login + pager + footer, ~80 total chars, 0 content) → `BoilerplateOnlyText{total ≥ 50, content < 50}` | PASS |
| REQ-HYBRID-SHELL-02 | `test_boilerplate_region_text_excluded_without_links` (header/footer, no anchors, 80/0), `test_anchor_text_excluded_outside_regions` (link listing 60/0), plus `test_invisible_tags_do_not_count_as_text` (script/style/noscript/template still void) | PASS |
| REQ-HYBRID-SHELL-03 | `test_article_page_with_chrome_stays_static`, `test_content_text_exact_threshold_stays_static` (content exactly 50), plus pre-existing `test_static_content_normal_page`, `test_ssr_with_content_is_static`, `test_static_content_exact_threshold` | PASS |
| REQ-HYBRID-SHELL-04 | `test_boilerplate_shell_with_mount_point_reports_marker` (marker wins), `test_fat_shell_without_markers_escalates` + `test_insufficient_text_empty` (empty shell `InsufficientText(0)` intact), `test_fat_shell_escalates_to_layer2` (router escalates, no router change needed) | PASS |

## Threshold values chosen

- `MIN_VISIBLE_CHARS = 50` — unchanged; now applied to content (boilerplate-free)
  text instead of total visible text.
- `BOILERPLATE_TAGS = ["nav", "header", "footer", "aside"]`.
- `ANCHOR_TAG = "a"` — link text excluded everywhere (not only inside regions).
- New verdict `SpaReason::BoilerplateOnlyText { total_chars, content_chars }` fired
  when total ≥ 50 but content < 50; marker enrichment still takes precedence.

## Risk note — static pages

Bare anchor-listing pages (only link titles, zero paragraph/heading/list body text)
now escalate instead of staying static: escalation costs one extra Obscura/Chromium
pass but still yields content, so the bias is fail-safe (false escalation ≪ lost
page). Any page with ≥50 non-anchor region chars (prose, headings, list text) keeps
`StaticContent` — all pre-existing static-page tests remain green.
