# Explore — #794 robots.txt 404/fail-open not cached

## Findings (CodeDB, absolute worktree path per §2.3)

### Current cache semantics (`crates/webfang_core/src/infrastructure/crawler/robots_utils.rs`)

- `RobotsCache = DashMap<String, Arc<RobotsRules>>` (L42); `new_robots_cache()` (L46).
  No TTL; lives exactly as long as its `RobotsFetcher` instance.
- `fetch_rules` (L195-212): on cache miss → `tracing::debug!("Fetching robots.txt from …")`
  → `self.fetch_robots_content(domain, &robots_url).await?` — the `?` propagates the
  failed fetch's `None` **before** `cache.insert` (L210). Failures are never cached.
- `fetch_robots_content` (L215-234) returns `Option<String>`: network error → `warn!` +
  `None`; non-2xx → `debug!` + `None`; body-read failure → `warn!` + `None`.
- `is_allowed` (L273-281): `fetch_rules == None` → `return true` (fail-open).
- `get_crawl_delay` (L295-297): reads the cache map directly; **zero production callers**
  in the workspace (only its two unit tests).

### Who owns a `RobotsFetcher` instance (cache lifetime)

| Site | File:line | Lifetime |
| :--- | :--- | :--- |
| Crawl engine | `application/crawler/engine.rs:136` | one `Arc<RobotsFetcher>` per `Engine::new` (per crawl) |
| CLI batch scrape | `cli/scrape_flow.rs:210` | one per `scrape_urls` batch; gate per URL at L346-352 |
| MCP | `webfang_mcp/src/mcp_server/state.rs:418` | one per container config; gate in `scraper_service.rs:202-220` |
| Crawl task gate | `application/crawler/crawl_task.rs:352` | via `ports.rs:136-167` `ProductionRobotsChecker` |

**Conclusion: the cache is per-instance = per crawl/session, never static/global.**
A cached negative decision dies with the crawl — exactly the scoping the issue asks for;
no risk of a persistent negative cache outliving a transient network error.

### `--ignore-robots` paths

- CLI scrape: gate at `scrape_flow.rs:346` before `is_allowed`.
- Engine: `crawl_task.rs:352` short-circuits on the flag.
- Service: `enforce_robots_policy` (`scraper_service.rs:202`) early-returns.
- `RobotsFetcher` never sees the flag — callers enforce it. No change needed there.

### Unrelated robots.txt fetch (out of scope)

`sitemap_discovery.rs:402` fetches robots.txt with the *discovery* client only to parse
the `Sitemap:` directive during auto-discovery; skipped when `--sitemap-url` is explicit
(`resolve_sitemap_url` L145-157). Not `RobotsFetcher`, not the reported hotspot.

### Observability wiring

`--trace-file` runs `FileTraceLayer` behind `EnvFilter::new("webfang=trace,…")`
(`cli/config.rs:97`), so both `debug!` and `trace!` events land in trace.jsonl regardless
of console verbosity. That is how the issue measured the bug (459 × `Fetching robots.txt`,
which is a `debug!` event).

## Root cause

`fetch_rules` only reaches `cache.insert` on success. On a 404 site, every `is_allowed`
call per crawled URL repeats: cache miss → fetch → 404 → fail-open → discard. With 94+
links checked per seed and per-page re-checks, one 5-page crawl produced 459 robots
fetches and 27.1s wall time vs 5.4s with `--ignore-robots`.

## Options considered

| # | Option | Verdict |
| :--- | :--- | :--- |
| A | Sentinel `RobotsRules { content: "" }` in the existing map | Rejected — conflates "200 with empty robots.txt" with "robots.txt unavailable"; ambiguous state. |
| B | Cache-value enum `RobotsCacheEntry { Rules(Arc<RobotsRules>), AllowAll }` | **Chosen** — explicit "known no rules" vs "never fetched", self-documenting, matches issue direction. |
| C | Singleflight / in-flight guard to dedupe concurrent first-fetches | **Needed** — a naive "fetch then insert" still stampeded: 5 concurrent pages produced 5 wire fetches in the behavioral test, so exactly-once (via `tokio::sync::OnceCell` per domain) is required for the 459→1 contract under default concurrency. |
| D | TTL on negative entries | Rejected — cache already lives only one crawl; TTL adds complexity for zero benefit at this lifetime. |
