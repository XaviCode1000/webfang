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
- FIFO eviction of oldest INITIALIZED entries; in-flight (uninitialized) cells rotate to
  the back and are never evicted (no duplicate connections).
- Legacy ctor `Downloader::new` keeps `usize::MAX` (unbounded) → byte-identical to
  pre-cap releases for every other call path.
- Documented residual (design D2 PARTIAL): an evicted URL encountered again later may be
  re-downloaded once.

## AFTER numbers (same workload, bounded)

| Workload | Entries retained | RSS delta | Before (unbounded) |
|---|---:|---:|---:|
| 50 000 assets | 24 576 (plateau = cap) | 18.2 MiB | 28.8 MiB and growing linearly |

The plateau test asserts retention ≤ cap under a 50k-entry fill with cap=10k;
growth is now O(cap), not O(workload).

## Byte-identity verification (Group D)

`bounded_cache_within_cap_is_byte_identical_to_unbounded`: a 60-entry workload fully
inside a 100-entry cap produces byte-identical cached assets (url/local_path/
content_hash/size) between the bounded downloader and an unbounded baseline — the cap
never perturbs runs that complete within limits.
