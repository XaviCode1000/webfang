# Design — #794 negative robots.txt caching

## State model

```rust
/// Cached robots.txt decision per domain (#794). Each domain maps to a
/// OnceCell-guarded decision: first check initializes it with exactly one
/// fetch; concurrent first-checks share the initialization.
pub type RobotsCache = DashMap<String, Arc<OnceCell<Arc<RobotsCacheEntry>>>>;

/// The cached robots.txt outcome for one domain.
#[derive(Debug, Clone)]
pub enum RobotsCacheEntry {
    Rules(Arc<RobotsRules>), // successfully fetched + parsed robots.txt
    AllowAll,                // negative cache: robots.txt unavailable → fail-open, remembered
}
```

`new_robots_cache()` signature is unchanged (`DashMap::new()`). `tokio::sync::OnceCell`
is used because it supports async initialization and guarantees exactly-once init.

## Control flow — `RobotsFetcher::resolve_entry` (renamed from `fetch_rules`)

1. `cache.entry(domain).or_insert_with(empty OnceCell)` — clone the cell `Arc`, then **drop
   the shard guard** (no lock held across `.await`).
2. Cell already initialized → `tracing::trace!(domain, entry)`, `"robots_txt_cache_hit"`;
   return cloned `Arc<RobotsCacheEntry>`.
3. Cell empty → `get_or_init(fetch_or_allow_all)`: exactly one task runs the fetch;
   concurrent callers await the same initialization (single-flight, zero stampede).
   `fetch_or_allow_all` emits `tracing::debug!("Fetching robots.txt from …")` (the line
   the issue greps — now once per domain).
4. Success → parse → `Rules(arc)` (same as before).
5. Failure → structured `RobotsFetchFailure` reason (`network_error` /
   `http_status:<code>` / `body_read_error`); emit
   `tracing::debug!(domain, reason, "robots_txt_negative_cached")`; store `AllowAll`.

`fetch_robots_content` refactored to `Result<String, RobotsFetchFailure>` so the failure
reason is structured data instead of dropped. The existing per-failure `warn!`/`debug!`
console events are kept (one per domain now, not per URL).

## `is_allowed` / `get_crawl_delay`

- `is_allowed`: match entry → `Rules(r)` → `DefaultMatcher::one_agent_allowed_by_robots`;
  `AllowAll` → `true`. Same fail-open semantics, now persistent.
- `get_crawl_delay`: `cache.get(domain)` → only `Rules(r)` yields `r.crawl_delay_secs`;
  `AllowAll` → `None` (no robots.txt ⇒ no Crawl-delay directive possible). No production
  callers exist, so this is a strict clarification.

## Concurrency — exactly-once via `tokio::sync::OnceCell`

The map value is an `OnceCell`: the shard guard from `DashMap::entry` is dropped before
any `.await`, and `OnceCell::get_or_init` guarantees exactly one initialization of the
decision per domain — concurrent first-checks await the same init future. Verified by
test: 8 concurrent first-checks against a delayed 404 produce exactly **1** wire fetch.
A naive "fetch then insert" stampeded 5 fetches for 5 concurrent pages (reproducing the
issue at small scale), so the single-flight guard is required for the 459→1 contract
under default (`auto`) crawl concurrency — not optional complexity.

## Lifetime / scoping

`RobotsCache` is owned by `RobotsFetcher`, built fresh per crawl/batch/container
(engine.rs:136, scrape_flow.rs:210, mcp state.rs:418). A cached negative entry dies with
the crawl; a transient outage on a later crawl re-probes. No TTL needed.

## Observability contract (REQ-..-06)

| Event | Level | Fields |
| :--- | :--- | :--- |
| `Fetching robots.txt from {url}` | debug | (message; unchanged) |
| `robots_txt_negative_cached` | debug | `domain`, `reason` ∈ {`network_error`, `http_status:<n>`, `body_read_error`} |
| `robots_txt_cache_hit` | trace | `domain`, `entry` ∈ {`rules`, `allow_all`} |

`--trace-file` captures webfang at TRACE level (cli/config.rs:97), so both land in
trace.jsonl for the issue's `grep -c "Fetching robots.txt"` acceptance (459 → 1).

## Files

| File | Change |
| :--- | :--- |
| `crates/webfang_core/src/infrastructure/crawler/robots_utils.rs` | enum, resolve_entry, is_allowed, get_crawl_delay, events, tests |
| `crates/webfang_core/tests/behavioral/cli/robots_test.rs` | new 404-robots end-to-end test asserting exactly 1 robots fetch |

No changes to `RobotFetcher` constructors, trait ports (`ProductionRobotsChecker` calls
`is_allowed` only), MCP/AI/TUI crates, or `mod.rs` re-exports signature surface
(`RobotsFetcher`, `RobotsRules` still exported; `RobotsCacheEntry` is `pub` but needs no
new re-export for workspace consumers).
