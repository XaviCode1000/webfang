# ADR 0012: Roadmap to Empty the Intra-Crate Allowlist — Issue #994 Sub-Slices

- **Status:** Accepted
- **Date:** 2026-08-29
- **Deciders:** Project Architect, webfang maintainers
- **Related:** ADR-0010, ADR-0010-A, ADR-0011, issue #994
- **Closes:** nothing (planning artifact — execution happens in sub-slices 1, 3, 4)

## Context

PR #993 (squash `d442119f`) and PR #997 (squash `6894f18a`) shipped a strict-capable
intra-crate layering gate and absorbed 19 pre-existing inward-only violations into
`scripts/check_intra_crate_direction_allowlist.txt` (5 broad `infrastructure::*` +
12 per-file `application/*` + 1 `adapters/downloader/mod.rs` + 1 `crate::ScraperConfig`
alias). Every entry cites the specific #994 sub-slice that will remove it.

The gate is in `warn` mode today. The roadmap below turns the allowlist from
permanent debt into a tracking artifact that shrinks sub-slice by sub-slice until
it is empty, then `INTRA_CRATE_MODE` flips to `strict` permanently.

## Current state (measured 2026-08-29 against `main`)

`INTRA_CRATE_MODE=warn`: 0 violations, 133 absorbed sites, allowlist 19/22.

Affected files (LOC and inline-path-site count measured by `grep -E` over
`use crate::(infrastructure|ScraperConfig|AutotuningConfig|...)` plus the inline
`crate::<layer>::` regex from `check_intra_crate_direction.sh`):

| File | LOC | use | inline | Allowlist entry |
|------|----:|----:|-------:|-----------------|
| `adapters/downloader/mod.rs` | 1895 | 2 | 7 | line 19 |
| `application/crawler/engine.rs` | 1652 | 4 | 8 | line 8 |
| `application/crawler/discovery.rs` | 965 | 7 | 8 | line 7 |
| `application/elastic_ingestion.rs` | 869 | 3 | 16 | line 10 |
| `application/scraper_service.rs` | 739 | 2 | 9 | line 15 |
| `application/llm_extraction.rs` | 650 | 1 | 3 | line 14 |
| `application/extraction.rs` | 643 | 2 | 5 | line 11 |
| `application/vault_search.rs` | 740 | 0 | 2 | line 17 |
| `domain/waf.rs` | 318 | 0 | 7 | line 18 |
| `application/som_capture.rs` | 118 | 0 | 3 | line 16 |
| `application/http_client/factory.rs` | 161 | 0 | 2 | line 13 |
| `application/asset_download.rs` | 142 | 0 | 6 | line 6 |
| `application/crawler_service.rs` | 29 | 3 | 3 | line 9 |
| **Total** | **8921** | **24** | **79** | 13 files |

53 `use` lines (24 in the 13 files below + 29 absorbed by the 5 broad
`infrastructure::*` entries elsewhere in `application/*.rs`) + 79 inline
qualified-path sites in the 13 files = **132 explicit porting sites** spread
across the 13 files in the table. The earlier "133 absorbed" number from the
gate output is the match count, which counts multi-match lines more than once
and includes broad-entry coverage that the per-file entries already inherit.

## Decision

Split the work into the four sub-slices already cited in the allowlist
comments (plus a final optional cleanup). Each sub-slice lands as its own PR,
removes the cited allowlist entries, and is independently reviewable within the
400-line budget (with `size:exception` if it can't).

### Sub-slice 1 — `domain::config` (the `crate::ScraperConfig` family)

**Cited by:** allowlist line 12 (`crate::ScraperConfig`) and partial dependencies
in lines 6, 7, 9, 10, 11, 15.

**Scope:** move the `ScraperConfig`, `AutotuningConfig`, `SitemapConfig`,
`ElasticConfig`, `ElasticOverrides` value types from `infrastructure::config`
to `domain::config`. Keep `infrastructure::config` as a thin re-export shim for
binary compat (matches the WafInspector pattern from PR #993).

**Why first:** the alias `crate::ScraperConfig` is a substring match against 5
separate allowlist entries; cleaning it removes the alias entry (line 12) and
shortens the cited reason on the 6 per-file entries. The other 5 broad entries
(`infrastructure::crawler`, `infrastructure::downloader`, `infrastructure::export`,
`infrastructure::observability`, `application/container.rs`) keep absorbing the
remaining sites until sub-slices 3 and 4 land.

**Estimated size:** ~150L (5 VOs + shim + tests). Within 400-line budget.

### Sub-slice 2 — strict mode flip when broad entries are isolated

After sub-slice 1, the `crate::ScraperConfig` alias entry drops, leaving 18
entries: 5 broad + 12 per-file + 1 `adapters/downloader`. This is the smallest
the allowlist gets until sub-slice 3 lands.

No code change. Update `check_intra_crate_direction.sh` default mode to
`strict`, run `INTRA_CRATE_MODE=strict` in CI. This validates the gate is
truly hermetic before we attempt the larger ports.

### Sub-slice 3 — `domain::*` ports for crawler/downloader/scraper/axtree/obsidian/llm/elastic

**Cited by:** allowlist lines 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 19.

**Scope:** port the 11 per-file entries plus `adapters/downloader` to domain
trait/value objects. Concrete modules to extract:

- `infrastructure::crawler::{UrlQueue, RobotsFetcher, SitemapParser, extract_links, UrlSource}` → `domain::crawler` port
- `infrastructure::downloader::{Wreq, Obscura, Chromium, ResourceGovernor, CookieBridge}` → `domain::downloader` port
- `infrastructure::scraper::*` and `infrastructure::converter::*` → `domain::scraper_port` and `domain::converter_port`
- `infrastructure::ssrf::{redirect_policy, ValidatingResolver, is_forbidden_literal_host}` → `domain::ssrf` port
- `infrastructure::axtree::{fetch_raw_axtree, parse_axtree}` → `domain::axtree` port
- `infrastructure::obsidian::read_vault_notes` → `domain::obsidian` port
- `infrastructure::bridge::CpuBridge` (8 sites in `elastic_ingestion.rs`) → `domain::bridge` port
- `infrastructure::network::session_pool::DomainSessionPool` (3 sites in `engine.rs`) → already partially ported; finish wiring through `Container`

After this sub-slice, the 5 broad `infrastructure::*` entries and the 11
per-file `application/*` entries all drop. Remaining: only
`application/container.rs` (DI root — composition root must know concretes) and
`infrastructure::observability` (transversal — `tracing` is the stack).

**Estimated size:** ~1100L, **exceeds 400-line budget**. This is the slice
that needs `size:exception` or to be split into stacked PRs (chained via
`feature-branch-chain` so review diffs stay focused). Decide strategy at task
planning time.

### Sub-slice 4 — `domain::waf` full port (remove the `domain/waf.rs` shim)

**Cited by:** allowlist line 18 (`infrastructure::http::waf_engine`).

**Scope:** `domain/waf.rs` currently lives in `domain/` but acts as a port
delegating to `infrastructure::http::waf_engine` (7 inline sites). Sub-slice 4
moves the WAF AC automaton logic fully into `domain::waf` so the delegation
disappears. The `WafInspector` concrete added in PR #993 (fix WafInspector +
HeaderMap) becomes the canonical implementation; `infrastructure::http::waf_engine`
becomes a thin re-export shim or is deleted.

**Estimated size:** ~250L. Within 400-line budget.

### Sub-slice 5 — final cleanup

After sub-slices 3 and 4, only `application/container.rs` and
`infrastructure::observability` remain (2 entries, well under cap 22).
`INTRA_CRATE_MODE=strict` is already on. Decide whether the 2 remaining entries
deserve their own sub-slice (probably yes — `container.rs` is composition root
defensible; `infrastructure::observability` is transversal and could be ported
to a `domain::observability` port if we ever want a pure logging abstraction).

## Consequences

- **Allowlist shrinks 19 → 2 over 5 sub-slices** (plus alias drop in sub-slice 1).
- **No public API break** at any sub-slice (shims preserve re-exports).
- **`domain::waf` misfile resolved** (sub-slice 4 deletes the last `domain/*`
  file that delegates to `infrastructure::*`).
- **Total estimated churn:** ~1500L (sub-slice 1: 150 + sub-slice 3: 1100 +
  sub-slice 4: 250). Sub-slice 3 is the only one that may need `size:exception`.
- **Hard gate `INTRA_CRATE_MODE=strict`** becomes the default in sub-slice 2 and
  stays on permanently thereafter.

## Alternatives Rejected

| Option | Verdict |
|--------|---------|
| Port all 19 entries in one PR | ~1500L, exceeds budget, single point of review failure |
| Keep the allowlist permanent | ADR-0010 explicitly rejects this — debt compounds |
| Delete the gate and rely on PR review | Loses the CI enforcement, no auto-regression on merge |
| Apply `INTRA_CRATE_MODE=strict` now without sub-slices | Fails CI immediately — 133 absorbed sites become 133 errors |

## References

- `scripts/check_intra_crate_direction.sh` (gate, ADR-0010 + ADR-0010-A hardened)
- `scripts/check_intra_crate_direction_allowlist.txt` (19 entries, each cites its sub-slice)
- `docs/adr/0010-intra-crate-direction-allowlist.md`
- `docs/adr/0011-tighten-intra-crate-allowlist.md`
- PRs #993 (squash `d442119f`) and #997 (squash `6894f18a`)
- Issue #994 (tracking)
