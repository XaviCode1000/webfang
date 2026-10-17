# Bounded memory — measured decisions (Q3 MEASURE FIRST)

Change: `stabilization-concurrency-budget` · branch `perf/bounded-memory`
Reproduce: `WEBFANG_MEMORY_REPORT_PATH=/tmp/r.md cargo test -p webfang_core --lib memory_probe_ bounded`

## BEFORE numbers (pre-cap, this branch's harness, Linux x86_64)

| Structure | Entries | RSS delta | Per-entry cost |
|---|---:|---:|---:|
| robots cache (`RobotsCache`, AllowAll lower bound) | 5 000 hosts | 3.1–9.6 MiB run-dependent | ~1.7 KB/host |
| downloaded_urls dedup cache | 50 000 assets | **28.8 MiB** | ~600 B/asset |
| visited_urls checkpoint mirror | 200 000 URLs | 21.7–27.1 MiB | ~140 B/URL |
| crawl_result_repository index | 20 000 results | 6.8 MiB | ~360 B/result |

(RSS deltas vary between runs with allocator behavior; the per-entry costs are the
stable signal. Entry-count probes assert exact counts; no absolute-byte assertions
anywhere, per design D2.)

## Materiality decisions (50 MB rule, data-decided)

| Structure | Crosses 50 MB at | Realistic? | Decision |
|---|---|---|---|
| robots cache | ~30 k hosts in ONE session | No (single-session host counts are orders of magnitude lower) | **DOCUMENT** — no cap |
| visited_urls mirror | ~350 k unique pages | Borderline but cap would break byte-identical checkpoints (Group D) | **DOCUMENT** — no cap |
| **downloaded_urls dedup cache** | **~87 k assets (≈60 MB)** | Yes for long archival runs | **CAP** (design D2 "PARTIAL" confirmed by data) |
| repository index | ∝ persisted results by construction | n/a — index mirrors durable state; rotation out of scope | **DOCUMENT** — no cap |

## Cap implementation (only qualifying structure)

- `Downloader::with_asset_cache_capacity(config, capacity)`; capacity derived from the
  budget model's Asset tier at the production wiring site (orchestrator): tier ×
  `ASSET_CACHE_ENTRIES_PER_PERMIT` (8 192). Default tier 3 → 24 576 entries ≈ ≤15 MiB ceiling.
- FIFO eviction with three classes: initialized cells are normal victims; actively
  downloading cells (RAII-guarded `in_flight` registry, cleared on completion, failure
  OR task cancellation #509) rotate to the back and are never evicted (no duplicate
  connections); uninitialized cells WITHOUT an active download are permanent-failure
  zombies and ARE evicted — otherwise error-heavy long runs would grow unbounded
  through the retry-on-failure design of `run_download`.
- Legacy ctor `Downloader::new` keeps `usize::MAX` (unbounded) AND skips the insertion
  ledger entirely (no per-URL strings, no mutex traffic) → memory-behavior identical to
  pre-cap releases on every other call path. Both constructors share one wreq client
  builder, so the SSRF policy (#703) cannot diverge.
- Ledger membership is a parallel `HashSet`: O(1) per insert under the ledger mutex,
  not O(queue) string scans.
- Documented residual (design D2 PARTIAL): an evicted URL encountered again later may be
  re-downloaded once.

## AFTER numbers (same workload, bounded)

| Workload | Entries retained | RSS delta | Before (unbounded) |
|---|---:|---:|---:|
| 50 000 assets | 24 576 (plateau = cap) | 18.2 MiB | 28.8 MiB and growing linearly |

The plateau test asserts retention ≤ cap under a 50k-entry fill with cap=10k;
growth is now O(cap), not O(workload).

Why measured 18.2 MiB vs the ~15 MiB entry ceiling: the delta is the ledger
(FIFO strings + membership set, ~140 B × 24 576 ≈ 3.3 MiB) plus allocator slack;
both are also bounded by the cap.

RSS absolute values assume a 4 KiB page (`unsafe_code` is workspace-denied, so
the real kernel page size cannot be queried); entry counts are exact and every
BEFORE/AFTER comparison uses the same constant, so relative deltas hold on any
kernel. On 64 KiB-page aarch64 configs absolute MiB reads up to ~16x high.

## Byte-identity verification (Group D)

`bounded_cache_within_cap_is_byte_identical_to_unbounded`: a 60-entry workload fully
inside a 100-entry cap produces byte-identical cached assets (url/local_path/
content_hash/mime_type/size) between the bounded downloader and an unbounded baseline
— the cap never perturbs runs that complete within limits.

Review-driven eviction-class tests: `abandoned_failure_zombies_evicted_inflight_preserved`
(zombies evicted FIFO-first, successes retained, in-flight never touched) and
`eviction_terminates_when_excess_is_all_inflight` (rotation bound terminates; completed
cells become evictable again), plus `legacy_unbounded_skips_insertion_ledger`.

## Long-lived MCP server follow-ups (#1130, #1120)

- **`DomainSessionPool` domain map (#1130)** — CAP. The per-domain `DashMap`
  never removed entries (`evict_stale` only reset states), so the long-lived
  MCP server grew linearly with domain cardinality. Now each entry carries a
  `last_seen` stamp: `acquire()` enforces the shared `MAX_TRACKED_DOMAINS`
  cap (500 — the same constant `ScrapeMetrics` uses, moved to
  `domain::budget` as the single source) by evicting the least-recently-seen
  domain, and `evict_stale()` removes domains idle past the TTL outright.
  Soak test: 2×cap unique domains → tracked domains plateau at the cap.
- **MCP HTTP server downloader (#1120)** — WIRING, not a new policy. The
  binary built the shared `Downloader` through the legacy `usize::MAX` path,
  so eviction never ran in the one process that outlives every crawl. It now
  goes through `mcp_server::build_shared_downloader()`: the same
  `asset_cache_capacity(budget.asset().get())` derivation the CLI
  orchestrator uses. One policy, two composition roots.
