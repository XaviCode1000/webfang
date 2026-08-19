# Spec — #794 negative robots.txt caching

## REQ-ROBOTS-NEG-CACHE-01 — Negative result cached once per domain

**Given** a `RobotsFetcher` against a site whose `/robots.txt` returns 404 (or any non-2xx)
**When** `is_allowed(url, domain)` is called many times for URLs of that domain
**Then** the robots.txt is fetched from the server **exactly once** (wire-level proof),
every call returns `true` (fail-open preserved), and the second+ calls hit the negative
cache entry instead of re-fetching.

## REQ-ROBOTS-NEG-CACHE-02 — Network/transport failures also cached as AllowAll

**Given** a domain whose robots.txt fetch fails at transport level (connection refused)
**When** `is_allowed` is called sequentially for several URLs of that domain
**Then** the fetch is attempted once, the failure is cached as `AllowAll`, later calls
return `true` without further attempts.

## REQ-ROBOTS-NEG-CACHE-03 — Successful robots.txt behavior unchanged

**Given** a site serving `User-agent: * / Disallow: /private` with 200
**When** `is_allowed` is checked for public and private URLs, repeatedly
**Then** public URLs are allowed, private URLs denied, robots.txt fetched once (cached
`Rules`), and `get_crawl_delay` returns the parsed Crawl-delay exactly as before.

## REQ-ROBOTS-NEG-CACHE-04 — Cached success is never downgraded

**Given** a fetcher whose cache already holds `Rules` for a domain (a prior successful
fetch)
**When** subsequent checks for that domain occur even if the site would now 404/error
**Then** the cached `Rules` keep being enforced; the entry is not replaced by `AllowAll`.

## REQ-ROBOTS-NEG-CACHE-05 — Concurrent first-fetches are exactly-once

**Given** N tasks calling `is_allowed` for the same domain while the cache is empty
**When** all calls overlap in time
**Then** the robots.txt is fetched **exactly once** (per-domain `OnceCell` single-flight
guards the fetch), no re-fetch loop is possible, and every call returns a consistent
fail-open result.

## REQ-ROBOTS-NEG-CACHE-06 — Negative caching is observable

**Given** `--trace-file` enabled in a CLI run against a 404-robots site
**When** multiple pages of that domain are scraped
**Then** trace.jsonl contains (a) one structured event recording the negative result was
cached (`robots_txt_negative_cached`, fields: `domain`, `reason`) and (b) one structured
event per subsequent cached reuse (`robots_txt_cache_hit`, field: `entry`), and
`Fetching robots.txt` appears exactly once for that domain.

## REQ-ROBOTS-NEG-CACHE-07 — `--ignore-robots` unaffected

**Given** a run with `--ignore-robots`
**When** pages are scraped
**Then** robots.txt is never fetched (callers gate before `is_allowed`); no behavior or
event change from the negative cache.
