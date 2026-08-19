# Tasks — #794 negative robots.txt caching

- [x] T1 `robots_utils.rs`: introduce `RobotsCacheEntry { Rules(Arc<RobotsRules>), AllowAll }`; repoint `RobotsCache` at `DashMap<String, Arc<OnceCell<Arc<RobotsCacheEntry>>>>`
- [x] T2 `robots_utils.rs`: `fetch_robots_content` → `Result<String, RobotsFetchFailure>` with structured reason
- [x] T3 `robots_utils.rs`: `fetch_rules` → `resolve_entry` + `fetch_or_allow_all`; insert `AllowAll` on failure; emit `robots_txt_negative_cached` (domain, reason) + `robots_txt_cache_hit` (domain, entry) events; per-domain `OnceCell` single-flight (exactly 1 fetch under concurrency)
- [x] T4 `robots_utils.rs`: `is_allowed` matches both entry variants; `get_crawl_delay` returns None for `AllowAll` / uninitialized cell
- [x] T5 Tests: in-module unit tests adapted + `test_get_crawl_delay_is_none_for_negative_entry`, `test_cached_allow_all_entry_allows_without_fetch`; new wiremock `tests/robots_cache_integration.rs` (5 tests: 404-once, 503-once, 200-Disallow cached+enforced once, no-downgrade-after-site-fails, concurrent single-flight = exactly 1)
- [x] T6 `tests/behavioral/cli/robots_test.rs`: `missing_robots_txt_is_fetched_once_per_crawl` — 404 robots.txt + 5-page sitemap crawl → exactly 1 `/robots.txt` request, 5/5 pages fetched, run succeeds
- [x] T7 Gates: `cargo check` (clean), strict clippy match CI (0 warnings), `cargo fmt --all --check` (0 diffs), `cargo nextest run` on all robots-affected suites — see verify.md
- [x] T8 verify.md written with gate evidence
