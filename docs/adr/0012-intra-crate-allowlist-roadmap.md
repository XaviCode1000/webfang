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

## Erratum (2026-08-29) — Sub-slice 3 design correction

### What the original said

The "Sub-slice 3" section above (line 89) described the work as porting
concrete types from `infrastructure::*` to `domain::*` ports:

> `infrastructure::crawler::{UrlQueue, RobotsFetcher, SitemapParser, extract_links, UrlSource}` → `domain::crawler` port
> `infrastructure::downloader::{Wreq, Obscura, Chromium, ResourceGovernor, CookieBridge}` → `domain::downloader` port
> `infrastructure::ssrf::{redirect_policy, ValidatingResolver, is_forbidden_literal_host}` → `domain::ssrf` port
> `infrastructure::bridge::CpuBridge` (8 sites in `elastic_ingestion.rs`) → `domain::bridge` port

### Why that was wrong

`UrlQueue`, `RobotsFetcher`, `SitemapParser`, `extract_links`, the `Wreq`/
`Obscura`/`Chromium` downloaders, and the `CpuBridge` concrete are **not
domain primitives**. They are infra with real I/O or mutable state:

- `RobotsFetcher` makes HTTP requests to fetch `robots.txt`.
- `SitemapParser` parses remote XML, possibly with HTTP recursion.
- `UrlQueue` is an async, mutable priority queue.
- `ResourceDownloader` opens sockets, manages byte-weighted semaphores, writes to disk.
- `CpuBridge` dispatches to a Rayon pool with internal locking.

Moving any of these to `domain::*` would put the innermost layer in
charge of the outer layer's I/O concerns — the opposite of Clean
Architecture. It would also make `domain::*` depend on `tokio`, `reqwest`,
`lol_html`, `rayon`, etc., violating the inward-only rule that ADR-0010
§1 and ADR-0009 already established.

The original sub-slice 3 conflated *"move so the static scanner stops
flagging it"* with *"move by Clean Architecture"*. The former is a
mechanical lint fix; the latter is a layering principle. They diverge
for concrete infra types — the scanner fix must follow the layering
principle, not override it.

### Corrected decision

For each `infrastructure::*` type that `application::*` consumes, the
correct porting pattern is **trait in domain, concrete in infra, DI
through container**:

1. **Define the trait in `domain::*`** with the public surface that
   `application::*` actually needs. The trait depends on no infra type.
2. **Keep the concrete implementation in `infrastructure::*`**, impl-ing
   the new `domain::*` trait.
3. **Migrate call sites in `application/*` and `adapters/*`** to depend
   on the trait, not the concrete. Use `Arc<dyn Trait>` if the call site
   stores it in a struct field.
4. **Container wires the concrete** as the trait implementation (this
   already happens for `WafInspector`; the rest follow the same pattern).

The shim pattern from PR #993 (WafInspector, ScraperConfig family)
applies to every port: the canonical type lives in `domain::*`, the
`infrastructure::*` module is a thin `pub use` re-export for backwards
compatibility.

This is the only valid approach. Moving the concrete types to
`domain::*` is rejected as a layering violation; bypassing the scanner
by re-exporting from `domain::*` to `infrastructure::*` and keeping
call sites on the infra path is rejected as lint evasion (the scanner
is intra-crate, not path-based — it sees the use line, not the resolved
symbol).

### Sub-slice 3 re-breakdown — 14 small PRs

Sub-slice 3 is not one big slice (~1100L, exceeds 400L budget, needs
`size:exception` or chained feature-branch-chain). It is **13 small
sub-slices (3.A through 3.K) plus sub-slice 4 plus sub-slice 2** = 14
PRs total. Each PR:

- Stays within the 400L budget (typical 80–250L).
- Migrates call sites for one or two related `infrastructure::*` modules
  to depend on a `domain::*` trait/port.
- Removes the corresponding entry (or entries) from
  `scripts/check_intra_crate_direction_allowlist.txt` as part of the
  PR's DoD — not as a follow-up. A merged PR that leaves the entry
  rotting is a hidden leak.
- Keeps `INTRA_CRATE_MODE=strict` green.

| PR    | Módulos                                           | Sitios | LOC est. | Crea port nuevo?      | Depende de |
|-------|---------------------------------------------------|-------:|---------:|-----------------------|------------|
| 3.A   | crawler call sites → `domain::crawler_port`       |     28 |    ~180  | NO (ya existe)        | —          |
| 3.A.2 | axtree call sites → `domain::axtree_port`         |      6 |     ~60  | NO (ya existe)        | —          |
| 3.B ⚠️ | downloader → `domain::downloader_port`            |     23 |    ~200  | NO (ya existe)        | —          |
| 3.B.2 | cpu_pool → `domain::cpu_executor`                 |      4 |     ~40  | NO (ya existe)        | —          |
| 3.C   | **Crear `domain::ssrf_guard`** + migrar 17 sitios |     17 |    ~250  | **SÍ**                | —          |
| 3.D   | scraper + converter → `domain::scraper_port` / `domain::html_cleaner` | 21 | ~180 | NO (ya existen) | — |
| 3.E   | **`domain::bridge` trait + DTO `ProcessedChunk`** + shim   |     10 |    ~180  | **SÍ (diseño)**       | —          |
| 3.E.2 | `application/elastic_ingestion.rs` field `Arc<dyn CpuBridgePort>` + container wiring | 0 (rewrite) | ~200 | NO (3.E provides) | 3.E |
| 3.F   | network/session_pool → `domain::session_port`     |      7 |    ~120  | NO (ya existe)        | —          |
| 3.G   | autotuning helpers (`from_elastic`/`resolve`) → `domain::config` | 8 | ~120 | NO (mover impls) | **sub-slice 1** (sub-slice 1 ya mergeado en #998) |
| 3.H   | export (ADR-0011 next) → `domain::export_port` (parcial) | 7 | ~150 | **SÍ (parcial)** | — |
| 3.I   | **Crear `domain::obsidian`** + migrar; **Crear `domain::content_processing`** + migrar | 11 | ~250 | **SÍ (×2)** | — |
| 3.J   | http misc → `domain::http_port`                   |      5 |    ~100  | NO (ya existe)        | —          |
| 3.K   | persistence → `domain::persistence`               |      5 |    ~120  | NO (ya existe)        | —          |
| 4     | `http::waf_engine` lógica → `domain::waf`        |      7 |    ~250  | parcial               | —          |
| 2     | strict mode default flip (`check_intra_crate_direction.sh:43`) | 0 | 1 | NO | sub-slice 1 (merging only) |
| **Total** |                                              |   ~173 |  ~2230  | 4 ports nuevos        |            |

> ⚠️ **Row `3.B` is stale.** Its three numbers — 23 sitios, ~200L, "NO (ya existe)" —
> were all wrong, and the third one is what made it dangerous. See
> **Erratum (2026-08-30) — Sub-slice 3.B measured reality** below for the measured
> scope, the four-PR decomposition actually used, and why `3.B-1c` no longer has a
> purpose. Do not size a slice from this table without re-measuring it.

**Operational notes:**

- **3.E is split** because the field change in `elastic_ingestion.rs`
  (`bridge: CpuBridge` → `bridge: Arc<dyn CpuBridgePort>`) touches the
  struct constructor, the DI wiring in `application/container.rs`, and
  every `ElasticIngestion::new` call site. Splitting 3.E (DTO + trait +
  shim) from 3.E.2 (field rewrite + container + call sites) keeps each
  PR reviewable and isolates the lock-across-await risk to 3.E.2.
- **3.C crosses `application/` and `adapters/downloader/`** in the
  same crate. Verify with `codedb_deps` that no shared files outside
  the listed ones are touched, so the PR can be batch-merged with
  others (3.A, 3.A.2, 3.I, etc., that touch disjoint files).
- **3.G depends on sub-slice 1** because the `from_elastic`/`resolve`
  impls on `AutotuningConfig` are currently in the infra shim (added
  by PR #993). Sub-slice 1 migrated the *call sites* to
  `domain::config::AutotuningConfig`; the impls stayed in infra. 3.G
  moves the impls themselves. Sub-slice 1 is already in flight (PR
  #998); 3.G can land any time after.
- **Batch-merge optimization:** with `strict: true` on `main`, 14
  sequential merges re-run CI ~14×. For the small migration-only
  PRs (3.A, 3.A.2, ~~3.B~~, 3.B.2, 3.D, 3.F, 3.J, 3.K) that touch
  disjoint files in `application/` and `adapters/`, batch-merge via
  `fix/batch-3-migrations-<date>` saves ~7× CI runs. The 4 port-creating
  PRs (3.C, 3.E, 3.I, 3.H) merge sequentially because each introduces
  a new domain module and could conflict on `domain/mod.rs`.
  *(3.B was struck from the "small migration-only" list by the 2026-08-30
  erratum: it creates a new port and moves concrete types, so it belongs
  with the sequential group.)*

### Allowlist language sync

The 11 per-file allowlist entries in
`scripts/check_intra_crate_direction_allowlist.txt` currently say
`"Remove after sub-slice 3 ports X to domain"`. The word *ports* is
already aligned with the corrected decision (port = trait in domain +
concrete in infra). No change required to the entry text. The
sub-slice 3.x numbering in this erratum is a planning refinement;
the existing per-file entries continue to track which 3.x removes them.

> ⚠️ **The last sentence of that paragraph held until 3.B-1b landed.** Once
> `domain::downloader_factory` existed, the `application/asset_download.rs` entry's
> removal condition became readable as *satisfied* while its call site was still
> there. See **An allowlist entry is now self-contradictory** in the 2026-08-30
> erratum. Prose removal-conditions go stale silently — they are not a substitute
> for re-checking the site.

### What this erratum does NOT change

- Sub-slice 1 (already landed in PR #998) — the ScraperConfig family
  migration is unaffected. The infra shim is unchanged in shape; the
  call-site migration is correct under both the original and corrected
  decisions because `ScraperConfig` and `AutotuningConfig` are pure
  DTOs and legitimately live in `domain::config`.
- Sub-slice 2 (strict mode flip) — the 1-line gate default change.
- Sub-slice 4 (WAF AC automaton → `domain::waf`) — the WAF work is
  intra-domain (logic, not infra), so the original description stands.
- Sub-slice 5 (final cleanup) — unchanged.
- The shim pattern (canonical in `domain::*`, re-export in
  `infrastructure::*`) — unchanged. The erratum only rejects the
  interpretation that "porting" means moving the concrete type.

## Erratum (2026-08-30) — Sub-slice 3.B measured reality

Filed as #1012. This corrects the **2026-08-29 re-breakdown table above**, not the
original sub-slice 3 prose (which the 2026-08-29 erratum already replaced).

### What row 3.B claimed

```
| 3.B | downloader → domain::downloader_port | 23 sitios | ~200L | NO (ya existe) | — |
```

Three separate false claims in one row.

### Why each was wrong

**"23 sitios"** — the violations were spread across **4 `application/` files** for
`CookieBridge` alone (`crawl_task_ctx.rs`, `engine.rs`, `fetch_router.rs` in production,
plus `crawl_task.rs:432` which is test-only — its `mod tests` opens at line 411), and
the work landed across **9 files in #1005 plus 15 files in #1023**. The row's number
conflated `use` lines with inline qualified paths and counted test-only modules the
gate does not police.

**"~200L"** — the two PRs that actually executed 3.B changed **366+/332-** (#1005) and
**387+/75-** (#1023). The estimate assumed a call-site repoint; the work required
creating a port, moving concrete types, and threading a DI seam through `EngineOptions`.

**"NO (ya existe)"** — this was the damaging one. `domain::downloader_port`
existed, but the thing `application/` actually needed was a **`DownloaderFactory`**,
which **did not exist anywhere**. Verified against history: `git cat-file -e
e428dcdf~1:crates/webfang_core/src/domain/downloader_factory.rs` fails — the file was
first added by 3.B-1b itself. The row told the next implementer "no design work
needed, just repoint imports." Anyone who trusted that would have started a
~200L mechanical PR and discovered mid-flight that it was an architecture change.

### Decomposition actually used

3.B was executed as **four** PRs, not one:

| Slice | Qué hizo | PR | Resultado |
|-------|----------|----|-----------|
| 3.B-0  | Repoint 4 imports que apuntaban al shim hacia `domain::downloader_port` | #1005 (`e9d9f2da`) | mecánico, como se vendió |
| 3.B-1a | `CookieBridge` movido a `domain::cookie_bridge` vía `git mv` + shim en infra | #1005 (`e9d9f2da`) | prerequisite real |
| 3.B-1b | **Crea** `domain::downloader_factory` (`DownloaderSpec` + `DownloaderFactory`), mueve `FetchRouter` + `build_fetch_router` a `infrastructure/downloader/`, seam `EngineOptions.downloader_factory` | #1023 (`e428dcdf`) | `feat`-sized, etiquetado `type:breaking-change` |
| 3.B-1c | ~~Delete the broad `infrastructure::downloader` entry~~ | — | **degradada — ver abajo** |

3.B-1a had to precede 3.B-1b: `CookieBridge` is referenced by three **production**
`application/` files (`crawl_task_ctx.rs:52`, `engine.rs:108`, `fetch_router.rs:89`),
each storing it as `Arc<RwLock<CookieBridge>>` independently of any factory. No port
signature could name it without leaking an infra type into a `domain` trait.

Allowlist effect: entries **17 → 16**, absorbed sites **115 → 104**.

3.B-1b removed the **first broad module-level entry** (`infrastructure::downloader`) in
the chain. Two entries had already been deleted, and the distinction matters:

| Commit | Slice | Entries | Absorbed | Entry removed | Calibre |
|--------|-------|--------:|---------:|---------------|---------|
| `6894f18a` | #997 | 19 | — | *(none — 14 added to absorb pre-existing)* | — |
| `09d8d2cd` | sub-slice 1 | 18 | 128 | `crate::ScraperConfig` | alias family |
| `2e1379f1` | 3.A.2-followup.B | 17 | — | `application/som_capture.rs` | per-file |
| `e9d9f2da` | 3.B-0 + 3.B-1a | 17 | 115 | *(none)* | — |
| `e428dcdf` | **3.B-1b** | **16** | **104** | **`infrastructure::downloader`** | **broad module** |

A broad entry is the one that actually hides things: the gate matches allowlist
patterns as substrings, so `infrastructure::downloader` silently covers every module
beneath it — which is why entry 15 of the file is deliberately scoped to
`infrastructure::http::waf_engine` with the note *"the broad form would also
substring-match `infrastructure::http_client`"*. Deleting a per-file entry relocates
debt; deleting a broad entry removes a blind spot.

### 3.B-1c is demoted — its stated purpose is already gone

3.B-1c was scoped as *"delete the broad `infrastructure::downloader` entry"*. That
entry **was already deleted by 3.B-1b**, deliberately and verified empirically (gate
exits 0, absorbed count unchanged at 104). Removing a broad umbrella while the
per-file entries survive cannot hide violations, so there was no reason to wait.

What actually remains under the 3.B label is two narrow sites, and they are **not**
the same job:

1. **`application/crawler/engine.rs:56`** — `use crate::infrastructure::downloader::
resource_governor::ResourceGovernor`, feeding the static `ram_usage_percent()` call
in the autoscale loop (`engine.rs:423`). This is a genuine port-extraction: a domain
RAM-usage probe. Small, and it is the only live `infrastructure::downloader` reference
left in `application/`.
2. **`application/asset_download.rs:106`** — `crate::adapters::downloader::
Downloader::new(...)`. **The original row 3.B never counted this correctly.** It goes
through `adapters::`, not `infrastructure::`, and it **bypasses the `DownloaderFactory`
that 3.B-1b just created**. The port exists and `engine.rs` uses it; this call site
was simply never migrated to it.

### An allowlist entry is now self-contradictory

The `asset_download.rs` entry still reads:

> *"Remove after sub-slice 3 ports the Downloader factory call to a domain
> DownloaderFactory port."*

`DownloaderFactory` **now exists**. Read literally, the entry's removal condition is
met, but the site is still there — because the promise was written when the port did
not exist and describes a migration, not a precondition. This is the failure mode of
prose removal-conditions: they go stale silently. The entry text should be corrected
to name the *call-site migration* as the condition, in the slice that does it.

### Line-number citations in the allowlist already rotted

The `application/crawler/engine.rs` entry points at:

> *"...the `ResourceGovernor::ram_usage_percent()` static call in the autoscale loop
> (`engine.rs:377`)"*

`engine.rs:377` is `timeout_secs: timeout,` inside a `DownloaderSpec` literal. The
actual call is at **`engine.rs:423`**. The citation was correct when written; 3.B-1b
inserted the factory seam into the same file and shifted everything below it by 46
lines.

The lesson generalises to the remaining 13 slices: **cite symbols, not line numbers.**
Every sibling slice edits these same files above the cited line, so a numeric anchor in
a removal-condition is guaranteed to drift before the slice that honours it lands.


### Review-size note for the remaining slices

3.B-1a moved a file and left a `pub use` shim at the old path —
`infrastructure/downloader/cookie_bridge.rs` is now a documented shim. That defeats
`git -M` rename detection: the source path is **modified**, not deleted, so Git cannot
pair it and the diff renders as a near-full delete plus a near-full add. Measured on
the `cookie_bridge.rs` paths of #1005:

```
git diff -M --shortstat e9d9f2da~1 e9d9f2da -- '*cookie_bridge.rs'
  2 files changed, 353 insertions(+), 323 deletions(-)   # 676 changed lines

git diff -C --shortstat e9d9f2da~1 e9d9f2da -- '*cookie_bridge.rs'
  2 files changed,  29 insertions(+), 325 deletions(-)   # 354 changed lines
```

`git diff -C` (content-copy detection) recovers the truth: the move is ~1.9× smaller
than `-M` reports. Anyone sizing a move+shim slice from a `-M` diff will over-report
review workload by roughly double. 3.B-1b's move had **no** shim, so `-M` paired it
cleanly: `.../downloader}/fetch_router.rs — 1 file changed, 59 insertions(+), 4
deletions(-)`.

### What this erratum does NOT change

- Rows 3.A, 3.A.2, 3.B.2, 3.C … 3.K, 4, 2 — only 3.B was measured against real work.
  The same class of error may well exist in the others; they are unverified until
  someone starts them, and that is the honest status of this table.
- The 2026-08-29 corrected decision (trait in `domain`, concrete in `infra`, DI
  through container) — 3.B-1b followed it exactly, which is evidence the pattern works.
- Sub-slice 5 (final cleanup) — unchanged.


## References

- `scripts/check_intra_crate_direction.sh` (gate, ADR-0010 + ADR-0010-A hardened)
- `scripts/check_intra_crate_direction_allowlist.txt` (**16 entries, 104 absorbed sites
  as of 2026-08-30**, each cites its sub-slice; was 18 entries / 128 absorbed sites
  after sub-slice 1 `09d8d2cd`, measured with `INTRA_CRATE_MODE=strict`)
- `docs/adr/0010-intra-crate-direction-allowlist.md`
- `docs/adr/0011-tighten-intra-crate-allowlist.md`
- PRs #993 (squash `d442119f`), #997 (squash `6894f18a`), #1005 (squash `e9d9f2da`,
  3.B-0 + 3.B-1a), #1023 (squash `e428dcdf`, 3.B-1b)
- Issue #994 (tracking), #1012 (this erratum), #1022 (3.B-1b), #1024 (3.B-1b coverage gap)

