# ADR 0012-B: Intra-Crate Allowlist 10→2 — Corrected Plan 2026-09-01

- **Status:** Accepted
- **Date:** 2026-09-01
- **Deciders:** Project Architect, webfang maintainers
- **Related:** ADR-0010, ADR-0010-A, ADR-0011, ADR-0012 (superseded for numbers)
- **Closes:** nothing (planning artifact — execution happens in sub-slices 3.D–3.K and 4)
- **Supersedes:** ADR-0012 for all counts, mode, and breakdown (0012 remains historical)

> **Normative status.** This document is the single normative plan for the
> intra-crate allowlist. ADR-0012 (2026-08-29) and its two errata are frozen as
> historical record. For counts, gate mode, slice decomposition, and
> removal-conditions, this ADR (0012-B) governs. Where the two disagree, 0012-B
> wins.

## 1. Context

### 1.1 What ADR-0012 said vs. measured reality (2026-09-01)

ADR-0012 (and its 2026-08-29 / 2026-08-30 errata) described the state as
`19` allowlist entries, `133` absorbed sites (strict), `warn` gate, cap `22`.

**Reality measured on `main` at `a6b931ab` (2026-09-01):**

| Dimension | ADR-0012 (2026-08-29) | Measured 2026-09-01 | Delta |
|---|---:|---:|---|
| Allowlist entries | 19 | **10** | −9 |
| Absorbed `allowlisted` sites (`INTRA_CRATE_MODE=strict`) | 133 | **84** | −49 |
| Gate default (`scripts/check_intra_crate_direction.sh:43`) | `warn` | **`strict`** | flipped at `a6b931ab` |
| Hard cap (`ALLOWLIST_CAP`) | 22 | **22** (unchanged) | — |
| Soft warn threshold (`ALLOWLIST_WARN_AT`) | 20 | **20** (unchanged) | — |
| `domain/` entries (`ls crates/webfang_core/src/domain/`) | ~30 | **48** | +18 ports/modules |
| `crate::infrastructure` in `domain/` (prod) | — | **0** (`grep -rn crate::infrastructure domain/` → 0 productive hits) | clean |

Cited measurement commands (reproducible on `main`):

```bash
# 1. Strict gate — authoritatively counts allowlisted sites
INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh
# → allowlisted 84 (max 22, file: scripts/check_intra_crate_direction_allowlist.txt, entries: 10)
# → OK: intra-crate Clean Architecture layering is inward-only (ADR-0010, strict mode)

# 2. Same count with allowlist physically removed (proves scanner sees 84 violations)
#    (temporarily move allowlist aside; gate then reports 84 violations as ::error::)
grep -rn 'crate::infrastructure' crates/webfang_core/src/domain/ --include='*.rs' | grep -v '//' | grep -v 'test'
# → 0 productive hits (only doc-comments referencing the gate)

ls crates/webfang_core/src/domain/ | wc -l
# → 48 entries (files + dirs)
```

> **84-site provenance.** `84` is the scanner's `allowlisted` count in `strict`
> mode with the 10-entry allowlist present. Removing the allowlist makes the
> same 84 sites surface as `::error::` violations — the count is not an
> estimate. `INTRA_CRATE_MODE=warn` on the same commit also reports `84`, but
> as `::warning::` with exit `0`; the gate has been `strict` as default since
> `a6b931ab`, so `warn` is no longer the CI mode.

### 1.2 What already landed (and why the counts moved)

| Slice | PR(s) | What it did | Allowlist effect |
|---|---|---|---|
| **Sub-slice 1** — `domain::config` (`ScraperConfig` family) | #998 | Moved `ScraperConfig`/`AutotuningConfig`/etc. VOs to `domain::config`; infra shim | 19 → 18 |
| **3.A / 3.A.2 / 3.B-0 / 3.B-1a / 3.B-1b / 3.B-1c / 3.C** | #1002, #1005 (`e9d9f2da`), #1023 (`e428dcdf`), #1042, #1059 | 3.B decomposed into 4 PRs (see §2.3); 3.C created `domain::ssrf_guard` (`is_forbidden_ip`, `redirect_policy`, `SsrfGuard`/`DefaultSsrfGuard` + `OnceLock` registry) and `domain::ram_probe_port` (`RamProbePort`, `SystemRamProbe` shim) | 18 → 16 → 10 |
| **3.D (partial)** | #1055 (partial), #1064/#1065/#1066 (sequential, landing now) | `domain::scraper_port` / `domain::html_cleaner` / `domain::content_processor` extraction | 10 → (shrinking; see §2.3) |
| **Gate flip** | `a6b931ab` | `INTRA_CRATE_MODE` default `warn` → `strict`; CI `toolchain` job runs `INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh` | Strict is permanent |

Current allowlist (`scripts/check_intra_crate_direction_allowlist.txt`, 10 entries, 84 sites, 2026-09-01):

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

### 1.3 Preconditions already in flight

Two preconditions cited as critical in #994 are **merged as sequential PRs
#1064 / #1065 / #1066 (landing now, strict gate green per PR)**:

- **#1063 — `EnvGuard` unification** (`crates/webfang_test_utils/src/lib.rs:EnvGuard`): centralizes `WEBFANG_DISABLE_SSRF_*` and other env-var test guards. Blocks `3.E`/`3.E.2` (`domain::bridge`/`CpuBridge`) which needs deterministic env isolation for `CpuExecutorPort` wiring.
- **#1058 — Miri pin** (`nightly-2026-08-27` in `.github/workflows/ci.yml`, `MIRIFLAGS=-Zmiri-tree-borrows`): pins the nightly that produces green `miri-infra-*`. Without it, `CpuBridge`/`ResourceGovernor` thread-pool tests are flaky under Tree Borrows.

Both are **not deferred** — they gate `3.E` and must be green before the bridge slice lands. This ADR freezes the dependency as explicit.

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

| Slice | Symbol / Port (cite, not line) | Sites | Est. LOC (`-C`) | New port? | Allowlist entries removed / narrowed | Depends | Status |
|---|---|---:|---:|---|---|---|---|
| **3.B-0** | `domain::downloader_port` repoint (4 crawler imports → port) | 4 | ~80 | NO | — (repoint) | — | **DONE** (#1005) |
| **3.B-1a** | `CookieBridge` → `domain::cookie_bridge` (`git mv` + `pub use` shim) | 3 | ~180 (`-C`: 354 counted as 180 logical) | **YES** | — | 3.B-0 | **DONE** (#1005) |
| **3.B-1b** | `DownloaderFactory` (`DownloaderSpec` + `DownloaderFactory::build`) + `fetch_router::DefaultDownloaderFactory` seam `EngineOptions::downloader_factory` | 23 | ~320 (`-M` would report ~460) | **YES** | `infrastructure::downloader` (broad, first in chain) | 3.B-1a | **DONE** (#1023) |
| **3.B-1c** | `RamProbePort` (`RamProbePercent`, `ram_usage_percent()`) + `Engine::with_ram_probe()` injection; default `SystemRamProbe` stays in infra | 1 prod + 1 `use SystemRamProbe` default | ~120 | **YES** | — (autoscale loop now reads `RamProbePort`; `engine.rs: RamProbePort` symbol, not `engine.rs:377`) | 3.B-1b | **DONE** (#1059) |
| **3.C** | `domain::ssrf_guard` (`is_forbidden_ip`, `is_forbidden_literal_host`, `redirect_policy()`, `SsrfGuard`/`DefaultSsrfGuard` + `OnceLock` registry) | 17 | ~250 | **YES** | `infrastructure::ssrf` residuals absorbed; `application/*` SSRF sites | — | **DONE** (#1042) |
| **3.D** | `domain::scraper_port` / `domain::html_cleaner` / `domain::content_processor` (pure) — `ScraperPort`, `AuthorExtractor`, `DomPruner`, `clean_html`, `ContentProcessor` | ~10 residual (`elastic_ingestion.rs: ContentProcessor` still infra-bound via bridge) | ~180 | NO (reuse) | — (partial; `scraper`/`converter` broad residuals) | — | **DONE partial** (#1055) → **closing in #1064/#1065/#1066 sequential** |
| **3.E** | `domain::cpu_executor::CpuExecutorPort` trait + `ProcessedChunk` DTO + `infrastructure::bridge` shim (`CpuBridge` implements `CpuExecutorPort`) | 8 | ~180 | **YES** (trait) | — | **#1063 EnvGuard green** | **BLOCKED until EnvGuard** |
| **3.E.2** | `application/elastic_ingestion.rs: ElasticIngestion { bridge: Arc<dyn CpuExecutorPort> }` field rewrite + `Container` wiring + call sites | 0 new (rewrite) | ~200 | NO (uses 3.E) | `application/elastic_ingestion.rs: CpuBridge` (symbol, not line) | 3.E | **BLOCKED until EnvGuard** |
| **3.F** | `domain::session_port` (`SessionPort`, `SessionId`, `SessionPoolConfig`, `DomainSessionPool` trait) | 3 (`engine.rs: DomainSessionPool`) | ~120 | NO | `application/crawler/engine.rs: DomainSessionPool` (narrow, not broad) | — | TODO |
| **3.G** | `domain::config::AutotuningConfig::from_elastic` / `resolve` impls moved from `infrastructure::autotuning` shim into `domain::config` | 8 | ~120 | NO (move impls) | — (post-1 cleanup) | Sub-slice 1 (#998) | TODO |
| **3.H** | `domain::exporter` / `domain::export` port (`ExportState`, `Exporter`, `DomainRecords`, `RawRecord` — partial, state-store stays infra) | 5 | ~150 | **YES (partial)** | `infrastructure::export` (narrowed first: `infrastructure::export::state_store` etc., then removed) | — | TODO |
| **3.I** | `domain::note_repository::VaultNoteReader` + `domain::content_processor` already covers; `infrastructure::obsidian::read_vault_notes` → port | 1 (`vault_search.rs: read_vault_notes`) | ~120 | **YES** (`VaultNoteReader`) | `application/vault_search.rs: read_vault_notes` (symbol) | — | TODO |
| **3.J** | `domain::http_port` / `domain::user_agent` misc (`HttpClientPort`, `UserAgentProvider`) | ~3 | ~100 | NO | `infrastructure::http` residuals | — | TODO |
| **3.K** | `domain::persistence` (`PersistenceMode`, `ResumeConfig`) | ~2 | ~120 | NO | `infrastructure::persistence` residuals | — | TODO |
| **4** | `domain::waf` full port — move `infrastructure::http::waf_engine` AC automaton into `domain::waf` (`WafInspectorPort`, `WafVerdict`, `EvidenceSource`); infra becomes `pub use` shim | 7 | ~250 | — (intra-domain logic move) | `domain/waf.rs` shim delegation removed; `infrastructure::http::waf_engine` entry deleted (narrow, not `infrastructure::http` broad) | — | TODO |
| **Perm** | `application/container.rs` + `infrastructure::observability` remain | 0 | 0 | — | **Never removed** | — | Permanent |

**How the 8 removable entries map to slices:**

| Allowlist entry | Removal slice | Removal-condition (symbol, not line) | Sites |
|---|---|---|---|
| `infrastructure::crawler` (broad) | **Narrow then 3.F/3.D** | `engine.rs: DomainSessionPool` + `discovery.rs: crawl_task_ctx::CrawlTaskCtx` → `domain::session_port` / `domain::crawler_port` | broad covers ~13 |
| `infrastructure::export` (broad) | **3.H** | `export: ExportState` / `DomainRecords` → `domain::exporter` (narrowed first) | 5 |
| `application/asset_download.rs` | **Follow-up to 3.B-1b** | `asset_download::Downloader::new` → `DownloaderFactory::build` via injected `Arc<dyn DownloaderFactory>` | 1 (`crate::adapters::downloader::Downloader::new`) |
| `application/crawler/discovery.rs` | **3.F** | `discovery.rs: CrawlTaskCtx` residuals → `domain::session_port` / `domain::crawler_port` | broad residual |
| `application/crawler/engine.rs` | **3.F** (then `SystemRamProbe` default hoist if `cli` becomes sole ctor) | `engine.rs: DomainSessionPool` + `engine.rs: SystemRamProbe` → `domain::session_port` + `domain::ram_probe_port` | 3 + 1 default |
| `application/crawler_service.rs` | **3.F / 3.D** | `crawler_service.rs: CrawlerService` crawler residuals → `domain::crawler_port` | broad residual |
| `application/elastic_ingestion.rs` | **3.E + 3.E.2** | `elastic_ingestion.rs: CpuBridge` → `domain::cpu_executor::CpuExecutorPort` | 8 |
| `application/vault_search.rs` | **3.I** | `vault_search.rs: read_vault_notes` → `domain::note_repository::VaultNoteReader` | 1 |

**Ordering constraints (why not all in parallel):**

- 3.E → 3.E.2 is sequential (trait must exist before field rewrite).
- 3.F, 3.G, 3.H, 3.I, 3.J/K can land in any order **except** they must serialize
  through `domain/mod.rs` (each adds a `pub mod`). PRs #1064/#1065/#1066
  demonstrate the pattern: **sequential merge, one at a time, strict green per
  PR** — batch-merge via `fix/batch-3-*` is forbidden because `domain/mod.rs`
  conflicts silently shadow each other.
- 4 (`waf`) can land anytime; it touches `domain/waf.rs` and
  `infrastructure/http/waf_engine.rs` only.

### 2.4 Sizing and CI rule per PR

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

- **Allowlist 10 → 2** over ~8 PRs post-2026-09-01 (plus 3.D tail #1064/#1065/#1066
  already sequential). Each PR is independently reviewable (≤400L by `-C`).
- **Gate `strict` permanent** — flipped at `a6b931ab`, never reverts to `warn`.
  `ALLOWLIST_CAP=22` stays until 10→2 lands, then ratchets to `10` (warn at
  `8`), then `5` (warn at `3`) once only the two permanent entries remain.
- **Domain stays testable** — every new trait is `Arc<dyn Trait>`-ready,
  `Send+Sync` where the runtime requires it, dyn-compatible (no generics on
  method, no `Self` in return). `Container` is the single DI seam.
- **Prose removal-conditions fixed** (closes #1032): every allowlist comment
  now cites a **symbol** (`engine.rs: RamProbePort`,
  `asset_download::Downloader::new`, `elastic_ingestion.rs: CpuBridge`) not a
  line number. Line numbers rot when sibling slices edit above them (e.g.
  `engine.rs:377` → `engine.rs:423` after 3.B-1b inserted the factory seam).

### Negative / costs

- `domain/` carries three accepted third-party leaks (`wreq::Jar`,
  `CancellationToken`, `ClientBuilder`/`Policy`, `oneshot`) with module-level
  `# Third-party types in domain — accepted deliberately` comments. A future
  newtype PR (tracked by #1045) can close them; until then the strict gate is
  **not** a purity proof.
- `domain/mod.rs` `pub use` re-exports grow (48 entries today → ~50 after
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
| **Apply `strict` without sub-slices** (flip gate with 10 entries still present) | Already done at `a6b931ab` — the 10 entries are the allowlist that makes strict green. Flipping without the allowlist would surface 84 `::error::` and block merge. The allowlist is the mechanism, not a bypass. |
| **Move concrete I/O types to `domain::*`** (e.g. `UrlQueue`, `RobotsFetcher`, `ResourceDownloader`, `CpuBridge` impls) | Layering violation: `domain` would then depend on `tokio`, `reqwest`, `lol_html`, `rayon`. Violates ADR-0010 §1 Clean Architecture (`infrastructure → adapters → application → domain` inward-only). The correct pattern is trait-in-domain, concrete-in-infra (§2.1). 2026-08-29 erratum rejected this; 0012-B reaffirms. |

---

## 5. References

- `scripts/check_intra_crate_direction.sh` — intra-crate gate (ADR-0010 + ADR-0010-A hardened, `ALLOWLIST_CAP=22`, `ALLOWLIST_WARN_AT=20`, default `strict` since `a6b931ab`)
- `scripts/check_intra_crate_direction_allowlist.txt` — **10 entries, 84 absorbed sites as of 2026-09-01** (was 19/133 in ADR-0012, 16/104 after 3.B-1b `e428dcdf`)
- `crates/webfang_core/src/domain/mod.rs` — 48 entries, re-exports, accepted-leak disclosures
- `crates/webfang_core/src/domain/downloader_factory.rs` — `DownloaderSpec` + `DownloaderFactory::build` (`wreq::Jar`, `CancellationToken` leak — precedent)
- `crates/webfang_core/src/domain/ssrf_guard.rs` — `is_forbidden_ip`, `redirect_policy`, `SsrfGuard`/`DefaultSsrfGuard` (`ClientBuilder`/`Policy` leak)
- `crates/webfang_core/src/domain/ram_probe_port.rs` — `RamProbePort`, `RamUsagePercent`
- `crates/webfang_core/src/domain/cpu_executor.rs` — `CpuExecutorPort` (`oneshot` leak)
- ADRs: `docs/adr/0010-intra-crate-direction-allowlist.md`, `docs/adr/0011-tighten-intra-crate-allowlist.md`, `docs/adr/0012-intra-crate-allowlist-roadmap.md` (frozen historical)
- PRs: #998 (sub-slice 1), #1002, #1005 (`e9d9f2da`, 3.B-0+1a), #1023 (`e428dcdf`, 3.B-1b), #1042 (3.C `ssrf_guard`), #1055 (3.D partial), #1059 (3.B-1c `RamProbePort`), **#1064 / #1065 / #1066 (sequential, landing now — 3.D tail)**
- Issues: #994 (umbrella), **#1063 (EnvGuard — blocks 3.E)**, **#1058 (Miri pin — blocks 3.E)**, #1060 / #1061 (choke-point: broad `crawler`/`export` shadowing), **#1032 (prose removal-conditions rot — line→symbol)**, #1045 (pure-domain leak newtypes, out of scope for 10→2), #1012 (3.B erratum), #1022/#1024 (3.B-1b coverage gap)
- `.github/workflows/ci.yml` `toolchain` job — `INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh` (strict since `a6b931ab`)
- `crates/webfang_test_utils/src/lib.rs:EnvGuard` — unified env guard (issue #1063)

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
  read `10`/`84`/`strict`/`~10 PRs (5 done + ~8 remaining to 2)` from this
  document.
- **No migration needed for consumers:** ADRs are planning artifacts; the only
  machine-readable artifact is `scripts/check_intra_crate_direction_allowlist.txt`
  (10 entries). Its per-line `Remove after sub-slice …` comments will be
  updated to symbol-citations (fix #1032) in the PR that removes each entry —
  not retroactively in this ADR.

---

## 7. Scope confirmation (webfang-architecture Output Contract)

- **Confirmed scope (in):** Freeze ADR-0012's numbers, re-measure the allowlist
  (10/84/strict/48-domain), decompose the remaining 10→2 path into reviewable
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
- **Next step:** Land #1064/#1065/#1066 sequential (3.D tail, strict green per
  PR), then unblock 3.E/3.E.2 after #1063/#1058 green, then 3.F/3.H/3.I/4 in
  any order via sequential PRs.

