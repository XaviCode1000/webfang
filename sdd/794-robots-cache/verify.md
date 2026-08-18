# Verify — #794 negative robots.txt caching

All gates run on 2026-08-18 in worktree `fix-batch-crawler` with
`CARGO_TARGET_DIR=$HOME/.cache/cargo-target/webfang`.

## Gate 1 — compile

```
cargo check -p webfang_core
cargo check -p webfang_core --all-targets
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
cargo fmt --all --check   (then rustfmt --edition 2021 on the new test file)
→ 0 diffs in this change's files
```

## Gate 4 — tests (`cargo nextest run` equivalents, verified via `cargo test`)

| Suite | Command | Result |
| :--- | :--- | :--- |
| Unit (robots_utils) | `cargo nextest run -p webfang_core --lib "infrastructure::crawler::robots_utils"` | **11 passed, 0 failed** |
| Integration (wire-level wiremock) | `cargo nextest run -p webfang_core --test robots_cache_integration` | **5 passed, 0 failed** |
| Behavioral robots (end-to-end CLI) | `cargo nextest run -p webfang_core --test behavioral "cli::robots_test"` | **4 passed, 0 failed** |
| scraper_service robots gate | `cargo nextest run -p webfang_core --test scraper_service_test "robots"` | **3 passed, 0 failed** |
| scrape_flow robots flags | `cargo nextest run -p webfang_core --lib robots_cache_allows_public_urls ignore_robots_flag_defaults_to_false` | **2 passed, 0 failed** |

Total: **25 passed, 0 failed** across robots-affected suites.

## Acceptance evidence (459 → 1)

- `negative_result_cached_after_first_missing_robots` — 50 `is_allowed` calls on a
  wiremock 404 site → **1** `/robots.txt` request on the wire (previously 50).
- `concurrent_first_fetches_are_bounded` — 8 concurrent first-checks against a
  delayed-404 robots.txt → **1** wire fetch (OnceCell single-flight; a naive
  insert-after-fetch design measured 5 fetches for 5 concurrent pages pre-fix).
- `missing_robots_txt_is_fetched_once_per_crawl` — real `webfang` binary crawling
  5 sitemap pages on a 404-robots site → **1** `/robots.txt`, 5/5 pages scraped,
  exit 0. Same scenario before the fix fetched robots.txt once per page check.
- `robots_txt_negative_cached` / `robots_txt_cache_hit` structured events added so
  the fix is observable in trace.jsonl (REQ-ROBOTS-NEG-CACHE-06).

## Spec traceability

| Requirement | Covered by |
| :--- | :--- |
| REQ-ROBOTS-NEG-CACHE-01 | `negative_result_cached_after_first_missing_robots`, `missing_robots_txt_is_fetched_once_per_crawl` |
| REQ-ROBOTS-NEG-CACHE-02 | `non_success_status_is_cached_as_allow_all` + cache-hit unit test |
| REQ-ROBOTS-NEG-CACHE-03 | `successful_rules_cached_and_enforced_once`, `test_robots_cache_hit` |
| REQ-ROBOTS-NEG-CACHE-04 | `cached_rules_are_not_downgraded_by_later_failures` |
| REQ-ROBOTS-NEG-CACHE-05 | `concurrent_first_fetches_are_bounded` (exactly 1 of 8) |
| REQ-ROBOTS-NEG-CACHE-06 | `robots_txt_negative_cached` / `robots_txt_cache_hit` events; trace layer runs at `webfang=trace` |
| REQ-ROBOTS-NEG-CACHE-07 | `ignore_robots_flag_allows_disallowed_fetch`, `test_ignore_robots_bypasses_the_gate` (crawl_task), scrape_flow flag test |
