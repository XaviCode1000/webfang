# ADR 0012-B: Intra-Crate Allowlist 10→2 — Corrected Plan 2026-09-01

- **Status:** Proposed
> **Note:** Accepted upon merge of #1067. Until then, Proposed.
- **Date:** 2026-09-01
- **Deciders:** Project Architect, webfang maintainers
- **Related:** ADR-0010, ADR-0010-A, ADR-0011, ADR-0012 (superseded for numbers), #1068/#1069 (scanner semantics this plan depends on)
- **Closes:** nothing (planning artifact — execution happens in sub-slices 3.D–3.K and 4)
- **Supersedes:** ADR-0012 for all counts, mode, and breakdown (0012 remains historical)

> **Normative status.** This document is the single normative plan for the
> intra-crate allowlist. ADR-0012 (2026-08-29) and its two errata are frozen as
> historical record. For counts, gate mode, slice decomposition, and
> removal-conditions, this ADR (0012-B) governs. Where the two disagree, 0012-B
> wins.

## 1. Context

### 1.1 What ADR-0012 said vs. measured reality (re-measured 2026-09-01 on `8dc58c6e`)

ADR-0012 (and its 2026-08-29 / 2026-08-30 errata) described the state as
`19` allowlist entries, `133` absorbed sites (strict), `warn` gate, cap `22`.

**Reality measured on `main` at `8dc58c6e` (2026-09-01, after the #1069 scanner fix) — gate flipped at `a6b931ab` where allowlist was 18 entries:**

| Dimension | ADR-0012 (2026-08-29) | Measured 2026-09-01 | Delta |
|---|---:|---:|---|
| Allowlist entries | 19 | **10** | −9 |
| Absorbed `allowlisted` sites (`INTRA_CRATE_MODE=strict`) | 133 | **71** | −62 |
| Gate default (`scripts/check_intra_crate_direction.sh:43`) | `warn` | **`strict`** | flipped at `a6b931ab` |
| Hard cap (`ALLOWLIST_CAP`) | 22 | **22** (unchanged) | — |
| Soft warn threshold (`ALLOWLIST_WARN_AT`) | 20 | **20** (unchanged) | — |
| `domain/` entries (`ls crates/webfang_core/src/domain/`) | ~30 | **57** | +27 ports/modules |
| `crate::infrastructure` in `domain/` (prod) | — | **0** (`grep -rn crate::infrastructure domain/` → 0 productive hits) | clean |

Cited measurement commands (reproducible on `main`):

```bash
# 1. Strict gate — authoritatively counts allowlisted sites
INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh
# → allowlisted 71 (max 22, file: scripts/check_intra_crate_direction_allowlist.txt, entries: 10)
# → OK: intra-crate Clean Architecture layering is inward-only (ADR-0010, strict mode)
    
# 2. Same count with allowlist physically removed (proves scanner sees 71 violations)
#    (temporarily move allowlist aside; gate then reports 71 violations as ::error::)
grep -rn 'crate::infrastructure' crates/webfang_core/src/domain/ --include='*.rs' | grep -v '//' | grep -v 'test'
# → 0 productive hits (only doc-comments referencing the gate)

ls crates/webfang_core/src/domain/ | wc -l
# → 57 entries (files + dirs)
```

> **71-site provenance — and why this document used to say `84`.** `71` is the
> scanner's `allowlisted` count in `strict` mode with the 10-entry allowlist
> present, measured on `8dc58c6e`. Removing the allowlist makes the same 71
> sites surface as `::error::` violations — the count is not an estimate.
> `INTRA_CRATE_MODE=warn` on the same commit also reports `71`, but as
> `::warning::` with exit `0`; the gate has been `strict` as default since
> `a6b931ab`, so `warn` is no longer the CI mode.
>
> **The `84` figure was a unit error, not a drift.** Before #1069 the gate
> counted *regex hits*, not code locations: `INLINE_LAYER_REGEX` matches
> `crate::<layer>::` in any position, so every `use crate::infrastructure::X`
> was emitted twice — once by the `use` pass and once by the inline pass. The
> three numbers are all real and all measured on the same 10 entries:
>
> | Scanner | Reported | What the number actually meant |
> |---|---:|---|
> | pre-#1069 | 84 | regex hits (double-counted `use` lines) |
> | #1069 first commit | 52 | distinct code locations, brace imports still folded |
> | #1069 final (`8dc58c6e`) | **71** | distinct code locations, brace imports expanded per symbol |
>
> The +19 from 52 to 71 are the symbols that brace truncation had folded into
> bare module paths. Nothing was lost — the only entries that disappear between
> the two are the 8 truncated bare forms, each replaced by its expansions.
>
> **Why this matters for the plan, not just for accuracy.** Every acceptance
> test in §5 is written as "the `allowlisted` count drops by N". Under the old
> scanner that test was *unsatisfiable*: deleting a `use` line moved the counter
> by 2, deleting an inline path by 1. #1069 made the counter mean what the plan
> assumes it means. Any site count written before #1069 — including every
> `Sites` value in §5 that came from a landed slice — is in the old unit and is
> labelled as such there.

### 1.2 What already landed (and why the counts moved)

| Slice | PR(s) | What it did | Allowlist effect |
|---|---|---|---|
| **Sub-slice 1** — `domain::config` (`ScraperConfig` family) | #998 | Moved `ScraperConfig`/`AutotuningConfig`/etc. VOs to `domain::config`; infra shim | 19 → 18 |
| **3.A / 3.A.2 / 3.B-0 / 3.B-1a / 3.B-1b / 3.B-1c / 3.C** | #1002, #1005 (`e9d9f2da`), #1023 (`e428dcdf`), #1042, #1059 | 3.B decomposed into 4 PRs (see §2.3); 3.C created `domain::ssrf_guard` (`is_forbidden_ip`, `redirect_policy`, `SsrfGuard`/`DefaultSsrfGuard` + `OnceLock` registry) and `domain::ram_probe_port` (`RamProbePort`, `SystemRamProbe` shim) | 18 → 16 → 10 |
| **3.D (partial)** | #1055 (partial), #1064/#1065/#1066 (sequential, MERGED) | `domain::scraper_port` / `domain::html_cleaner` / `domain::content_processor` extraction | 10 → (shrinking; see §2.3) |
| **Gate flip** | `a6b931ab` | `INTRA_CRATE_MODE` default `warn` → `strict`; CI `toolchain` job runs `INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh` | Strict is permanent |
| **Scanner semantics fix** | #1068 → #1069 (`8dc58c6e`) | Count is now distinct code locations, not regex hits; brace imports expand per symbol; allowlist module paths match at segment boundaries only; `INTRA_CRATE_ROOT`/`INTRA_CRATE_ALLOWLIST` fixture overrides added | Entries unchanged at 10; the reported unit changed (84 hits → 52 → **71** sites). Narrowing became implementable, and the cap-22 budget became binding — see §1.2.2 |

Current allowlist (`scripts/check_intra_crate_direction_allowlist.txt`, 10 entries, 71 sites, measured on `8dc58c6e`):

```
application/container.rs                          # DI root — permanent
infrastructure::observability                     # transversal — permanent
infrastructure::crawler                           # broad — to narrow
infrastructure::export                            # broad — to narrow
application/asset_download.rs                     # crate::adapters::downloader::Downloader::new
application/crawler/discovery.rs                  # broad crawler/observability residuals
application/crawler/engine.rs                     # DomainSessionPool + SystemRamProbe default
application/crawler_service.rs                    # broad crawler residual
application/elastic_ingestion.rs                  # CpuBridge
application/vault_search.rs                       # read_vault_notes
```

#### 1.2.1 Per-entry absorption (measured, new scanner unit)

The §5 acceptance tests need to know how much the counter moves when a given
entry goes away. That is not derivable from the entry text — it has to be
measured. Method: drop exactly one entry via `INTRA_CRATE_ALLOWLIST` (no repo
file touched) and read the resulting `allowlisted` count; the delta is what that
entry absorbs exclusively.

| # | Entry | `allowlisted` without it | Exclusive sites |
|---:|---|---:|---:|
| 1 | `application/container.rs` | 60 | 11 |
| 2 | `infrastructure::observability` | 66 | 5 |
| 3 | `infrastructure::crawler` | 60 | 11 |
| 4 | `infrastructure::export` | 57 | **14** |
| 5 | `application/asset_download.rs` | 70 | 1 |
| 6 | `application/crawler/discovery.rs` | 71 | **0** |
| 7 | `application/crawler/engine.rs` | 67 | 4 |
| 8 | `application/crawler_service.rs` | 71 | **0** |
| 9 | `application/elastic_ingestion.rs` | 63 | 8 |
| 10 | `application/vault_search.rs` | 70 | 1 |

Exclusive sites sum to **55**, not 71. The remaining **16** are double-covered:
a per-file entry whose sites also match one of the three broad module entries.
That is not a bug — it is the reason entries 6 and 8 exist at all — but it does
mean the column is not additive and no slice should claim a drop equal to its
entry's exclusive count without re-measuring the whole gate.

> **Two entries are already dead weight.** Entries 6 (`crawler/discovery.rs`) and
> 8 (`crawler_service.rs`) absorb **zero** sites exclusively: every site in those
> two files is already covered by the `infrastructure::crawler` broad entry. Their
> own comments say so ("remaining inline sites absorbed by the
> infrastructure::crawler / infrastructure::observability broad entries").
>
> Verified: dropping **both** leaves `allowlisted 71` with 8 entries and the gate
> still exits `0`. So **10 → 8 is free today, with no code change at all.**
>
> That is a finding, not yet a decision. Deleting them removes a reviewed
> file-level exemption, so a *future* site in those files pointing at a non-crawler
> infra module would start failing the gate — which is arguably the correct
> behaviour, but it is a policy change and belongs in its own PR with its own
> rationale, not folded into a porting slice.

#### 1.2.2 The cap-22 constraint makes narrowing strictly one-module-at-a-time

#1069 made narrowing actually implementable (brace imports now expand per symbol,
so a per-symbol entry can match what the scanner records). It also exposed a hard
budget limit. Measured on `8dc58c6e` by replacing one broad entry with its
per-symbol list:

| Narrow | Symbols needed | Resulting entries | Gate |
|---|---:|---:|---|
| `infrastructure::crawler` | 9 | 18 | green, `allowlisted 71` |
| `infrastructure::observability` | 2 | 11 | green, `allowlisted 71` |
| `infrastructure::export` | 8 | 17 | green, `allowlisted 71` |
| **all three at once** | 19 | **26** | **FAIL** — `::error::allowlist has 26 entries, max is 22` |

Each module fits alone; the set does not fit together. **Consequence for the
plan: no slice may narrow more than one broad module, and a narrowing PR must
land and delete its own symbols before the next module is narrowed.** The
narrow → port → delete sequence cannot be parallelised across the three broad
entries, which is what makes §5 a strictly sequential chain rather than a set of
independent slices.

The per-symbol lists the gate currently records for the three broad entries
(measured by removing the entry and reading the `::error::` paths):

- `infrastructure::crawler` (11 exclusive): `UrlQueue` ×2, `robots_utils::RobotsFetcher` ×2, `RobotsFetcher`, `SitemapParser`, `SitemapUrl`, `SitemapError`, `SitemapConfig`, `fetch_url`, `extract_links`
- `infrastructure::observability` (5 exclusive): `log_scrape_error` ×4, `log_classified_error` ×1
- `infrastructure::export` (14 exclusive): `state_store::StateStore` ×3, `RecordStore` ×3, `RawRecord` ×2, `DomainRecords` ×2, `StateStore`, `LastError`, `vector_exporter::VectorExporter`, `jsonl_exporter`

### 1.3 Preconditions — UNBLOCKED (updated 2026-09-01)

> **Note:** Counts re-verified on `8dc58c6e` (post-#1069 scanner): 10 entries / 71 sites / 57 domain, strict, CAP 22. The earlier note on this section cited 10/84/57 at `a32b2607` — the `84` was the pre-#1069 regex-hit unit, see §1.1.

Two preconditions cited as critical in #994 were **UNBLOCKED (EnvGuard #1066 MERGED at a32b2607 2026-09-01T00:55:45Z, Miri #1065 MERGED at 179be72a 00:49:00Z)** (plus #1064 choke-point #1060 MERGED):

- **#1066 — `EnvGuard` unification (issue #1063)** (`crates/webfang_test_utils/src/lib.rs:EnvGuard`): centralizes `WEBFANG_DISABLE_SSRF_*` and other env-var test guards. Blocks `3.E`/`3.E.2` (`domain::bridge`/`CpuBridge`) which needs deterministic env isolation for `CpuExecutorPort` wiring.
- **#1065 — Miri pin (issue #1058)** (`nightly-2026-08-27` in `.github/workflows/ci.yml`, `MIRIFLAGS=-Zmiri-tree-borrows`): pins the nightly that produces green `miri-infra-*`. Without it, `CpuBridge`/`ResourceGovernor` thread-pool tests are flaky under Tree Borrows.

Both were gating `3.E` and required green before the bridge slice landed — now **MERGED and green**, so 3.E/3.E.2 are **UNBLOCKED**. This ADR freezes the dependency as explicit (now satisfied).

### 1.4 Domain purity is aspirational — document the known leaks

`crates/webfang_core/src/domain/mod.rs` header reads *"puro sin frameworks"*.
That is **aspirational today**. The domain layer intentionally leaks three
third-party types, each with an ADR-acknowledged precedent and a narrow scope:

| Module | Leaked type(s) | Precedent / rationale |
|---|---|---|
| `domain::downloader_factory` | `wreq::cookie::Jar` (`DownloaderSpec::initial_cookie_jar`), `tokio_util::sync::CancellationToken` (`DownloaderFactory::build` param) | First non-`wreq_util` external types in domain; gate only inspects `crate::<layer>::` paths, so it cannot see this. Future newtype (`CookieJar`, `CancelToken`) would close it. |
| `domain::ssrf_guard` | `wreq::redirect::Policy` (`redirect_policy()` return), `wreq::ClientBuilder` (`SsrfGuard::secure_client` param) | Follows `downloader_factory` precedent; avoids a domain-owned client-builder newtype that would add indirection with no business value. |
| `domain::cpu_executor` | `tokio::sync::oneshot::Receiver` (`CpuExecutorPort::dispatch` return) | Join-handle would force `Future` in trait; `oneshot` is the minimal async seam for Tokio→Rayon crossing. |

All three modules carry `# Third-party types in domain — accepted deliberately`
doc-comments citing the gate's blind spot. Do not read a green strict gate as
"domain is framework-free". Issue #1045 tracks the long-term newtype
replacement; it is **out of scope** for the 10→2 allowlist.

---

## 2. Decision

### 2.1 Principle — trait in domain, concrete in infra, DI via Container

Correcting the original ADR-0012 error (moving I/O concretes into `domain`):

1. **Define the trait / VO in `domain::*`** with the surface `application::*`
   actually needs. Zero infra imports.
2. **Keep the concrete in `infrastructure::*`**, implementing the domain trait.
3. **Migrate call sites in `application/*` and `adapters/*`** to `Arc<dyn Trait>` where stored.
4. **Container wires the concrete** (`application/container.rs` is the only file allowed to name concretes — permanent allowlist entry).

The shim pattern (`pub use domain::*` in `infrastructure::*`) preserves backwards
compat for one minor version, then the infra path is deleted. Every port keeps
`INTRA_CRATE_MODE=strict` green in its own PR — no follow-up allowlist nuke.

### 2.2 Target — 10 → 2, not 10 → 0

| Entry | Verdict | Reason |
|---|---|---|
| `application/container.rs` | **Permanent** | Composition root must know concretes (`CpuBridge`, `RayonCpuPool`, `DomainSessionPool`, `ResourceDownloader`). ADR-0010 §2 and ADR-0009 explicitly name this exception. |
| `infrastructure::observability` | **Permanent** | Transversal `tracing` concern (`log_scrape_error`, `memory_probe::rss_bytes`). A `domain::observability` port adds indirection with no business value. |
| All other 8 entries | **Remove** | Each has a concrete port target below. |

Broad entries `infrastructure::crawler` and `infrastructure::export` are
**narrowed before deletion** — replaced by per-symbol entries (e.g.
`infrastructure::crawler::resource_downloader`) so shadowing never hides a new
violation. A broad `infrastructure::crawler` substring-matches any future
`infrastructure::crawler::*` path silently; narrow entries fail closed.

**Cap trajectory:** `22` (today) → `22` (while 8 entries remain, headroom is
needed for non-intra-crate PRs) → **drop to `10` after 10→2 lands**, then
`5` after the two permanent entries are the only ones left (soft warn at `cap-2`).

### 2.3 Re-breakdown — corrected, symbols not lines, measured

> **Sizing convention (all LOC estimates below):** `git diff -C --shortstat`
> (content-copy detection) for any move+shim PR (1.9× smaller than `-M` — see
> ADR-0012 erratum 2026-08-30: `cookie_bridge.rs` measured `676` with `-M` vs.
> `354` with `-C`). Pure migrations use `git diff --shortstat`. A PR is
> **reviewable** iff `insertions + deletions ≤ 400` by the correct flag.

Gate condition for every row: `INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh` exits `0` and `allowlisted` count drops by the row's
`Sites` (or stays flat when the row only narrows a broad entry).
    
> **Read the `Sites` column with the unit header from §1.1.** Rows marked
> **DONE** were measured by their own PRs under the pre-#1069 scanner, which
> counted regex hits — those numbers are in the old unit and are kept as
> historical record, not as acceptance targets. Rows still **TODO** carry the
> value measured on `8dc58c6e` with the corrected scanner (§1.2.1), which is the
> unit the acceptance test now uses. Do not sum across the two groups.

| Slice | Symbol / Port (cite, not line) | Sites | Est. LOC (`-C`) | New port? | Allowlist entries removed / narrowed | Depends | Status |
|---|---|---:|---:|---|---|---|---|
| **3.B-0** | `domain::downloader_port` repoint (4 crawler imports → port) | 4 | ~80 | NO | — (repoint) | — | **DONE** (#1005) |
| **3.B-1a** | `CookieBridge` → `domain::cookie_bridge` (`git mv` + `pub use` shim) | 3 | ~180 (`-C`: 354 counted as 180 logical) | **YES** | — | 3.B-0 | **DONE** (#1005) |
| **3.B-1b** | `DownloaderFactory` (`DownloaderSpec` + `DownloaderFactory::build`) + `fetch_router::DefaultDownloaderFactory` seam `EngineOptions::downloader_factory` | 23 | ~320 (`-M` would report ~460) | **YES** | `infrastructure::downloader` (broad, first in chain) | 3.B-1a | **DONE** (#1023) |
| **3.B-1c** | `RamProbePort` (`RamProbePercent`, `ram_usage_percent()`) + `Engine::with_ram_probe()` injection; default `SystemRamProbe` stays in infra | 1 prod + 1 `use SystemRamProbe` default | ~120 | **YES** | — (autoscale loop now reads `RamProbePort`; `engine.rs: RamProbePort` symbol, not `engine.rs:377`) | 3.B-1b | **DONE** (#1042) |
| **3.C** | `domain::ssrf_guard` (`is_forbidden_ip`, `is_forbidden_literal_host`, `redirect_policy()`, `SsrfGuard`/`DefaultSsrfGuard` + `OnceLock` registry) | 17 | ~250 (estimate — no -C provenance yet) | **YES** | 3 per-file entries removed: `application/http_client/factory.rs`, `application/llm_extraction.rs`, `adapters/downloader/mod.rs` (verified: `git diff 117863c9^..117863c9 -- allowlist.txt`) | — | **DONE** (#1059) |
| **3.D** | `domain::scraper_port` / `domain::html_cleaner` / `domain::content_processor` (pure) — `ScraperPort`, `AuthorExtractor`, `DomPruner`, `clean_html`, `ContentProcessor` | ~10 residual (`pipeline/stages/clean.rs:9 use crate::domain::content_processor::ContentProcessor`, `infrastructure::bridge::CpuBridge` — grep verified) | ~180 (estimate — no -C provenance yet) | NO (reuse) | — (partial; `scraper`/`converter` broad residuals) | — | **DONE partial** (#1055) |
| **3.E** | `domain::cpu_executor::CpuExecutorPort` trait + `ProcessedChunk` DTO + `infrastructure::bridge` shim (`CpuBridge` implements `CpuExecutorPort`) | 8 | ~180 (estimate — no -C provenance yet) | **YES** (trait) | — | **— (was #1063, now unblocked)** | **UNBLOCKED (EnvGuard #1066 MERGED at a32b2607 2026-09-01T00:55:45Z, Miri #1065 MERGED at 179be72a 00:49:00Z)** |
| **3.E.2** | `application/elastic_ingestion.rs: ElasticIngestion { bridge: Arc<dyn CpuExecutorPort> }` field rewrite + `Container` wiring + call sites | 0 new (rewrite) | ~200 (estimate — no -C provenance yet) | NO (uses 3.E) | `application/elastic_ingestion.rs: CpuBridge` (symbol, not line) | 3.E | **UNBLOCKED (EnvGuard #1066 MERGED at a32b2607 2026-09-01T00:55:45Z, Miri #1065 MERGED at 179be72a 00:49:00Z)** |
| **3.F** | `domain::session_port` (`SessionPort`, `SessionId`, `SessionPoolConfig`, `DomainSessionPool` trait) | 3 of the 4 exclusive sites of `application/crawler/engine.rs` — **verified** `engine.rs:98` + `:322` `DomainSessionPool`, `:314` `SessionPoolConfig`; the 4th (`:57` `SystemRamProbe` default) is **not** in scope | ~120 (estimate — no -C provenance yet) | NO | `application/crawler/engine.rs: DomainSessionPool` (narrow, not broad) | — | TODO |
| **3.G** | `domain::config::AutotuningConfig::from_elastic` / `resolve` impls moved from `infrastructure::autotuning` shim into `domain::config` | **0 — see §5.1** | ~120 (estimate — no -C provenance yet) | NO (move impls) | **none — no `infrastructure::autotuning` entry exists** | Sub-slice 1 (#998) | TODO |
| **3.H** | `domain::exporter` / `domain::export` port (`ExportState`, `Exporter`, `DomainRecords`, `RawRecord` — partial, state-store stays infra) | 14 exclusive to `infrastructure::export` (**not** 5 — the row undercounted; the module is the single largest absorber) | ~150 (estimate — no -C provenance yet) | **YES (partial)** | `infrastructure::export` (narrowed first: 8 per-symbol entries, then removed) — **cap-bound, see §1.2.2** | — | TODO |
| **3.I** | `domain::note_repository::VaultNoteReader` + `domain::content_processor` already covers; `infrastructure::obsidian::read_vault_notes` → port | 1 (`vault_search.rs: read_vault_notes`) — **verified** | ~120 (estimate — no -C provenance yet) | **YES** (`VaultNoteReader`) | `application/vault_search.rs: read_vault_notes` (symbol) | — | TODO |
| **3.J** | `domain::http_port` / `domain::user_agent` misc (`HttpClientPort`, `UserAgentProvider`) | **0 — see §5.1** | ~100 (estimate — no -C provenance yet) | NO | **none — no `infrastructure::http` entry exists** | — | TODO |
| **3.K** | `domain::persistence` (`PersistenceMode`, `ResumeConfig`) | **0 — see §5.1** | ~120 (estimate — no -C provenance yet) | NO | **none — no `infrastructure::persistence` entry exists** | — | TODO |
| **4** | `domain::waf` full port — move `infrastructure::http::waf_engine` AC automaton into `domain::waf` (`WafInspectorPort`, `WafVerdict`, `EvidenceSource`); infra becomes `pub use` shim | **0 — see §5.1** | ~250 (estimate — no -C provenance yet) | — (intra-domain logic move) | **none — no `infrastructure::http::waf_engine` entry exists** | — | TODO |
| **Perm** | `application/container.rs` + `infrastructure::observability` remain | 0 | 0 | — | **Never removed** | — | Permanent |

    **How the 8 removable entries map to slices** (Sites column = exclusive absorption measured on `8dc58c6e`, §1.2.1):
    
    | Allowlist entry | Removal slice | Removal-condition (symbol, not line) | Sites |
    |---|---|---|---:|
    | `infrastructure::crawler` (broad) | **Narrow then 3.F/3.D** | `engine.rs: DomainSessionPool` + `discovery.rs: crawl_task_ctx::CrawlTaskCtx` → `domain::session_port` / `domain::crawler_port` | **11** (9 symbols, see §1.2.2) |
    | `infrastructure::export` (broad) | **3.H** | `export: ExportState` / `DomainRecords` → `domain::exporter` (narrowed first) | **14** (was written as 5) |
    | `application/asset_download.rs` | **cheap wins — DONE** | `asset_download::Downloader::new` → `AssetDownloaderFactory::build` via `domain::asset_downloader_factory::default_factory()`. The originally recorded condition (`DownloaderFactory::build`) was **wrong**: that port builds the page-fetch `domain::downloader_port::Downloader` (`fetch`/`supports_interactions`/`memory_cost`) and needs a run-scoped `CookieBridge` + `CancellationToken`, while this site needs `AssetDownloaderPort::download_batch` built from a bare `ScraperConfig`. Two different downloaders, so the asset side got its own port. | 1 |
    | `application/crawler/discovery.rs` | **free — no slice needed** | absorbs 0 exclusively; every site already covered by `infrastructure::crawler` | **0** |
    | `application/crawler/engine.rs` | **3.F** (session sites) + **cheap wins — DONE** (probe default) | `engine.rs: DomainSessionPool` + `engine.rs: SessionPoolConfig` → `domain::session_port` (3.F, #1075); `engine.rs: SystemRamProbe` default → `domain::ram_probe_port::system_default()`, with the type moved to `domain` and its sysinfo `impl RamProbePort` left in `infrastructure::downloader::system_ram_probe` (the `DefaultSsrfGuard` split). No `Container` hoist was needed. | 4 (3 + 1 default) |
    | `application/crawler_service.rs` | **free — no slice needed** | absorbs 0 exclusively; every site already covered by `infrastructure::crawler` | **0** |
    | `application/elastic_ingestion.rs` | **3.E + 3.E.2** | `elastic_ingestion.rs: CpuBridge` → `domain::cpu_executor::CpuExecutorPort` | 8 |
    | `application/vault_search.rs` | **3.I** | `vault_search.rs: read_vault_notes` → `domain::note_repository::VaultNoteReader` | 1 |
    
    ### 5.1 Four rows are not on the 10→2 path at all (3.G, 3.J, 3.K, 4)
    
    These rows were written against allowlist entries that **no longer exist**. The
    current allowlist has 10 entries (§1.2); none of them names
    `infrastructure::autotuning`, `infrastructure::http`,
    `infrastructure::persistence`, or `infrastructure::http::waf_engine`.
    
    Measured, not inferred: with only `application/container.rs` allowlisted, the
    gate reports sites in exactly seven infrastructure modules — `crawler` (23),
    `export` (14), `bridge` (8), `observability` (6), `network` (3), `obsidian` (1),
    `downloader` (1). The four modules above contribute **zero**.
    
    They are not dead code — they are referenced. But every reference from the
    scanned tree is either:
    
    - inside `application/container.rs`, which is the **permanent** DI-root entry and
      is never removed (so the site is absorbed there regardless of these slices);
    - inside `#[cfg(test)] mod tests` — verified for
      `crawler/discovery.rs:421`, `crawler/sitemap_discovery.rs:647`,
      `http_client/client.rs:801`, all three `infrastructure::http::waf_engine`;
    - or in the `webfang_cli` crate (`cli/scrape_flow.rs:460`,
      `cli/args/export.rs:116`), which this gate does not scan — `ROOT` is
      `crates/webfang_core/src`.
    
    **Consequence:** these four rows claim 8 + 3 + 2 + 7 = **20 sites** and name
    entry deletions that cannot happen. Their real gate delta is `0`, and there is
    no entry to remove. They are legitimate *purity* refactors — moving logic into
    `domain` so the DI root stops knowing concretes — but they must not be counted
    as steps on the 10→2 path, and no acceptance test of the form "the count drops
    by N" can pass for them.
    
    The honest restatement of the remaining path is therefore **10 → 2 via six
    entries, not eight**: two are free (§1.2.1 dead weight), four need real porting
    work (`crawler`, `export`, `engine.rs`, `elastic_ingestion.rs`,
    `asset_download.rs`, `vault_search.rs` — six entries across five slices), and
    two are permanent. 3.G / 3.J / 3.K / 4 move to a separate purity backlog with no
    allowlist acceptance criterion attached.

**Ordering constraints (why not all in parallel):**

- 3.E → 3.E.2 is sequential (trait must exist before field rewrite).
- 3.F, 3.G, 3.H, 3.I, 3.J/K can land in any order **except** they must serialize
  through `domain/mod.rs` (each adds a `pub mod`). PRs #1064/#1065/#1066
  demonstrated the pattern: **sequential merge, one at a time, strict green per
  PR (MERGED at a32b2607/179be72a)** — batch-merge via `fix/batch-3-*` is forbidden because `domain/mod.rs`
  conflicts silently shadow each other.
- 4 (`waf`) can land anytime; it touches `domain/waf.rs` and
  `infrastructure/http/waf_engine.rs` only.

### 2.4 Sizing and CI rule per PR

> **Note:** LOC figures in §2.3 are estimates — no -C provenance yet, except 3.B-1a/1b (measured at e9d9f2da / e428dcdf).

Every PR in §2.3 must satisfy **all four** before merge:

1. `git diff -C --shortstat` (copy detection for move+shim; `-M` over-reports
   by up to 1.9×) shows `insertions + deletions ≤ 400` or carries
   `size:exception` with explicit `domain/mod.rs` conflict note.
2. `cargo check --all-features` green.
3. `cargo clippy --all-targets --all-features -- -D warnings` green.
4. `cargo fmt --check` + `cargo nextest run` (or `cargo test` for Miri-gated
   paths) green.
5. `INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh` exits
   `0` and `allowlisted` count is monotonic non-increasing (drops or holds flat
   when narrowing a broad entry).

A PR that leaves its cited allowlist entry rotting (merged but entry still
present) is a **leak**, not a slice — DoD requires the entry removal in the same
commit.

---

## 3. Consequences

### Positive

- **Allowlist 10 → 2** over ~5 PRs remaining post-2026-09-01 (3.F, 3.G, 3.H, 3.I, 3.J/K, 4) — ~8 total before excluding 3 MERGED (#1064, #1065, #1066); plus #1055 partial. Each PR is independently reviewable (≤400L by `-C`).
- **Gate `strict` permanent** — flipped at `a6b931ab`, never reverts to `warn`.
  `ALLOWLIST_CAP=22` stays until 10→2 lands, then ratchets to `10` (warn at
  `8`), then `5` (warn at `3`) once only the two permanent entries remain.
- **Domain stays testable** — every new trait is `Arc<dyn Trait>`-ready,
  `Send+Sync` where the runtime requires it, dyn-compatible (no generics on
  method, no `Self` in return). `Container` is the single DI seam.
- **Prose removal-conditions fixed** (#1032 — now corrected in the file itself, not
  deferred): every allowlist comment cites a **symbol** (`DomainSessionPool`,
  `SessionPoolConfig`, `SystemRamProbe`, `asset_download::Downloader::new`,
  `elastic_ingestion.rs: CpuBridge`) and never a line number. This was claimed
  before it was true — `application/crawler/engine.rs` still carried an
  `engine.rs:57` line cite until the tooling-hygiene PR that closes #1032.
  Line numbers rot when sibling slices edit above them (e.g.
  `engine.rs:377` → `engine.rs:423` after 3.B-1b inserted the factory seam).

### Negative / costs

- `domain/` carries three accepted third-party leaks (`wreq::Jar`,
  `CancellationToken`, `ClientBuilder`/`Policy`, `oneshot`) with module-level
  `# Third-party types in domain — accepted deliberately` comments. A future
  newtype PR (tracked by #1045) can close them; until then the strict gate is
  **not** a purity proof.
- `domain/mod.rs` `pub use` re-exports grow (57 entries today → ~60 after
  3.F/3.I/3.H). Each PR touches `domain/mod.rs`, so merges must be sequential.
- `application/crawler/engine.rs` retains a temporary `SystemRamProbe` default
  ctor (`Engine::new()` builds `SystemRamProbe` without `Container`) because
  tests construct `Engine::new` directly. Hoisting to `cli` as sole ctor is
  deferred — the symbol stays allowlisted until that refactor.

### Neutral

- No public API break: every `domain::*` move leaves a `pub use` shim in
  `infrastructure::*` for one minor version.
- Inter-crate graph unchanged (`scripts/check_dependency_direction.sh` stays
  green; no `Cargo.toml` changes in this plan).
- No runtime behavior change: `Arc<dyn Port>` already used for
  `HttpClientPort`/`SemanticCleaner`; new ports follow the same pattern.

---

## 4. Alternatives Rejected

| Option | Why rejected |
|---|---|
| **Port all 8 remaining entries in one PR** (~1100L, even with `-C`) | Exceeds 400L budget by 2–3×, single review failure blocks all ports, forces `size:exception` without chunking benefit. Rejected in favour of §2.3's 8 sequential PRs. |
| **Keep broad `infrastructure::crawler` / `infrastructure::export` permanent** | Broad entries substring-match any future path under that subtree, hiding new violations. `webfang-architecture` skill rejects permanent debt; ADR-0010 explicitly rejected "documented exception is a permanent exception". Choke-point analysis (#1060/#1061) requires narrowing first. |
| **Apply `strict` without sub-slices** (flip gate with 10 entries still present) | Already done at `a6b931ab` — the 10 entries are the allowlist that makes strict green. Flipping without the allowlist would surface 71 `::error::` and block merge. The allowlist is the mechanism, not a bypass. |
| **Move concrete I/O types to `domain::*`** (e.g. `UrlQueue`, `RobotsFetcher`, `ResourceDownloader`, `CpuBridge` impls) | Layering violation: `domain` would then depend on `tokio`, `reqwest`, `lol_html`, `rayon`. Violates ADR-0010 §1 Clean Architecture (`infrastructure → adapters → application → domain` inward-only). The correct pattern is trait-in-domain, concrete-in-infra (§2.1). 2026-08-29 erratum rejected this; 0012-B reaffirms. |

---

## 5. References

- `scripts/check_intra_crate_direction.sh` — intra-crate gate (ADR-0010 + ADR-0010-A hardened, `ALLOWLIST_CAP=22`, `ALLOWLIST_WARN_AT=20`, default `strict` since `a6b931ab`)
- `scripts/check_intra_crate_direction_allowlist.txt` — **10 entries, 71 absorbed sites measured on `8dc58c6e`** (was 19/133 in ADR-0012; the 84 in earlier drafts of this document was the pre-#1069 regex-hit unit — see §1.1)
- `crates/webfang_core/src/domain/mod.rs` — 57 entries, re-exports, accepted-leak disclosures
- `crates/webfang_core/src/domain/downloader_factory.rs` — `DownloaderSpec` + `DownloaderFactory::build` (`wreq::Jar`, `CancellationToken` leak — precedent)
- `crates/webfang_core/src/domain/ssrf_guard.rs` — `is_forbidden_ip`, `redirect_policy`, `SsrfGuard`/`DefaultSsrfGuard` (`ClientBuilder`/`Policy` leak)
- `crates/webfang_core/src/domain/ram_probe_port.rs` — `RamProbePort`, `RamUsagePercent`
- `crates/webfang_core/src/domain/cpu_executor.rs` — `CpuExecutorPort` (`oneshot` leak)
- ADRs: `docs/adr/0010-intra-crate-direction-allowlist.md`, `docs/adr/0011-tighten-intra-crate-allowlist.md`, `docs/adr/0012-intra-crate-allowlist-roadmap.md` (frozen historical)
- PRs: #998 (sub-slice 1), #1002, #1005 (`e9d9f2da`, 3.B-0+1a), #1023 (`e428dcdf`, 3.B-1b), #1042 (3.B-1c `RamProbePort`), #1059 (3.C `ssrf_guard`), #1055 (3.D partial), **#1064 (choke-point #1060) / #1065 (Miri #1058) / #1066 (EnvGuard #1063) (sequential, MERGED at a32b2607/179be72a)**
- Issues: #994 (umbrella), **#1063 (EnvGuard — blocks 3.E)**, **#1058 (Miri pin — blocks 3.E)**, #1060 / #1061 (choke-point: broad `crawler`/`export` shadowing), **#1032 (prose removal-conditions rot — line→symbol)**, #1045 (pure-domain leak newtypes, out of scope for 10→2), #1012 (3.B erratum), #1022/#1024 (3.B-1b coverage gap)
- `.github/workflows/ci.yml` `toolchain` job — `INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh` (strict since `a6b931ab`)
- `crates/webfang_test_utils/src/lib.rs:EnvGuard` — unified env guard (issue #1063)
- Gate 0 drain #1015 (issue) / #1016 (PR, MERGED) — enabled breaking PR #1023 (FetchRouter removal) under freeze

---

## 6. Errata notes — what this ADR does to ADR-0012

- **ADR-0012 (2026-08-29) is frozen as historical.** Its base text (19/133, warn
  gate, sub-slices 1/2/3/4/5, ~1500L total churn) and its two errata
  (2026-08-29 design correction: trait-in-domain; 2026-08-30 measured reality:
  3.B four-PR decomposition, `-C` sizing, line-number citations rotted) are
  preserved verbatim in `docs/adr/0012-intra-crate-allowlist-roadmap.md`.
- **This ADR (0012-B) absorbs both errata.** The trait-in-domain principle,
  3.B-0/1a/1b/1c decomposition, `-C` sizing rule, and symbol-not-line citation
  rule are now normative in §2, not errata. No separate erratum file is needed
  — future corrections to 0012-B will be versioned as 0012-C or a successor ADR.
- **Numbers superseded:** wherever ADR-0012 says `19`/`133`/`warn`/`14 PRs`,
  read `10`/`71`/`strict`/`~10 PRs (5 done + ~5 remaining to 2, 3 MERGED excluded)` from this
  document.
- **No migration needed for consumers:** ADRs are planning artifacts; the only
  machine-readable artifact is `scripts/check_intra_crate_direction_allowlist.txt`
  (10 entries). Its per-line removal-conditions were rewritten to symbol-citations
  and migration-statements in the tooling-hygiene PR closing #1032 — absorbed count
  verified byte-identical at `allowlisted 71` before and after, patterns untouched.
  Future entries added by the 10→2 slices follow the same rule from the start.

---

## 7. Scope confirmation (webfang-architecture Output Contract)

- **Confirmed scope (in):** Freeze ADR-0012's numbers, re-measure the allowlist
  (10/71/strict/57-domain), decompose the remaining 10→2 path into reviewable
  PRs with symbol-cited removal-conditions, document accepted domain leaks, and
  ratchet the cap `22 → 10 → 5`. **Out:** actual port PRs (they land as §2.3),
  domain newtype replacement (#1045), export full port beyond the 5-site slice,
  `Container` hoist for `SystemRamProbe` default.
- **Filtered template + chapters:** `search-engine` (crawler/downloader/ingestion
  pipeline), tutorials 02 (scalability), 07 (queue/backpressure), 10
  (indexing/discovery). Others excluded (payment-system, ecommerce, social-feed).
- **Component sketch:** `domain::*` owns traits/VOs (`RamProbePort`,
  `SsrfGuard`, `DownloaderFactory`, `CpuExecutorPort`, `SessionPort`,
  `VaultNoteReader`, `WafInspectorPort`); `infrastructure::*` keeps concretes
  (`SystemRamProbe`, `ValidatingResolver`, `DefaultDownloaderFactory`,
  `CpuBridge`, `DomainSessionPool`, `WafEngine`); `application/*` depends on
  `domain::*` via `Arc<dyn Trait>`; `Container` (composition root) wires
  concretes — the only permanent `application→infrastructure` edge.
- **Tradeoffs:** (1) Broad entries narrow-before-delete vs. keep-broad: chosen
  narrow — avoids shadowing, costs one extra PR per broad. (2) Move+shim via
  `-C` vs. `-M`: chosen `-C` — reports logical churn, avoids 1.9× over-report.
  (3) Sequential `domain/mod.rs` merges vs. batch: chosen sequential — avoids
  silent `mod.rs` conflicts, costs more CI runs.
- **Next step:** 3.E/3.E.2 are unblocked (EnvGuard #1066 + Miri #1065 merged) and `application/elastic_ingestion.rs` is the single largest removable entry that needs no narrowing (8 exclusive sites, one module, no cap pressure) — start there. Then 3.I (1 site) and the `asset_download.rs` follow-up (1 site) as cheap wins. `infrastructure::export` (3.H) and `infrastructure::crawler` (narrow-then-3.F) are cap-bound and must be strictly sequential, one module per PR (§1.2.2). Entries 6 and 8 are free to drop today (§1.2.1). 3.G / 3.J / 3.K / 4 are off the 10→2 path (§5.1).

