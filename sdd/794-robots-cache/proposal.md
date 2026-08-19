# Proposal — #794 cache robots.txt 404/fail-open per domain

## Chosen approach

Replace the cache value with a two-state enum guarded by a per-domain single-flight
`OnceCell`:

```rust
pub enum RobotsCacheEntry {
    Rules(Arc<RobotsRules>), // successfully fetched + parsed robots.txt
    AllowAll,                // negative cache: robots.txt unavailable → fail-open, remembered
}
pub type RobotsCache = DashMap<String, Arc<OnceCell<Arc<RobotsCacheEntry>>>>;
```

`fetch_rules` becomes `resolve_entry`: the per-domain `OnceCell` runs exactly one fetch
(success → `Rules`, any failure → `AllowAll`); concurrent first-checks share that one
initialization. Subsequent `is_allowed` calls hit the cached entry and never re-fetch.
Fail-open behavior is unchanged; "fail-open" stops meaning "fail-repeat".

## Scope (in)

- `crates/webfang_core/src/infrastructure/crawler/robots_utils.rs` — cache value enum,
  `resolve_entry`, `is_allowed`, `get_crawl_delay`, negative-caching observability events.
- Existing in-module tests adapted to the new cache value type; new unit tests:
  404 → cached AllowAll (single fetch), 200-with-Disallow cached + enforced,
  success-then-404 keeps cached rules, bounded concurrent first-fetch stampede.
- Behavioral wire-level test (`tests/behavioral/cli/robots_test.rs`): 404 robots.txt +
  5-page sitemap scrape asserts exactly ONE `/robots.txt` request and all pages allowed.

## Scope (out)

- `--ignore-robots` plumbing (callers gate before `is_allowed`; unaffected).
- Sitemap auto-discovery's own robots.txt parse fetch (`sitemap_discovery.rs`, separate
  client and code path).
- Singleflight dedup of concurrent first-fetches (bounded by concurrency; documented).
- TTL / expiry (cache is per-crawl; dies with the `RobotsFetcher` instance).
- Crawl-delay honoring (no production callers; negative entries carry no delay).

## Risk

- Public type alias `RobotsCache` changes value type — no workspace consumer outside
  `robots_utils.rs` (verified via callers search); MCP/AI/TUI crates only construct
  `RobotsFetcher` and call `is_allowed`/`get_crawl_delay`.
- Concurrent first-fetches may insert equivalent entries more than once (last-write-wins);
  bounded, self-healing, documented in design.md.
