# ADR 0010: Intra-Crate Direction Strict Gate — Hybrid Ports + Allowlist

- **Status:** Accepted
- **Date:** 2026-08-28
- **Deciders:** Project Architect, `webfang` maintainers
- **Related issues:** #990 (71 intra-crate violations), #984 (lint warn), #809 (domain-owned config precedent)
- **Supersedes:** —
- **Depends on:** ADR-0009 (persistence mode input struct)

## Context

`webfang_core` enforces Clean Architecture layering `infrastructure → adapters → application → domain` (inward only) per `AGENTS.md`. The inter-crate gate `scripts/check_dependency_direction.sh` checks `Cargo.toml` only and cannot see module-level violations. The intra-crate lint `scripts/check_intra_crate_direction.sh` (PR #984, ADR-0009 follow-up) scans `use crate::<layer>::...` and maps the source file's layer to the target's layer via `LAYER_RANK` (`infrastructure 0 < adapters 1 < application 2 < domain 3`). A violation is any `target_rank < src_rank` (outward import).

PR #984 shipped the lint in **warn** mode: 71 `application→infrastructure` (+2 `adapters→infrastructure`) violations were reported as `::warning::` with exit 0. Flipping to `INTRA_CRATE_MODE=strict` would yield 71 `::error::` and block merge, but the violations pre-exist and cannot be fixed by a single mechanical rename — they span configs, crawler helpers, downloader traits, observability, and DI wiring.

Additional lint holes were discovered:

- `use crate::ScraperConfig` (re-export alias via `lib.rs:118` → `infrastructure::config::ScraperConfig`) bypasses the regex `^use crate::([a-z_]+)::` because the first segment is `ScraperConfig`, not `infrastructure`. Three application files use the alias and were not counted in the 71.
- `#[cfg(test)]` / `mod tests` imports inflate the count with test-only debt (e.g. `pipeline/mod.rs`, `som_capture.rs`).
- No allowlist mechanism existed, so any strict flip would either require fixing all 71 at once or a permanent exception.

ADR-0009 proved the pattern: `domain::persistence::ResumeConfig` is owned by domain, application depends on the port, infrastructure implements. The same pattern applies to `ScraperConfig` and crawler helpers.

## Decision

**Adopt a hybrid slice** — ports for ~80% of violations, narrow allowlist for the remainder, and a hardened lint that is flipped to strict next to `check_dependency_direction.sh` in the `toolchain` job.

### 1. Domain-owned ports and value objects

Move the trait/DTO/VO for the three highest fan-in categories into `domain`:

| Category | Example types | New home | Infra keeps |
|----------|---------------|----------|-------------|
| Config VOs | `ScraperConfig`, `SitemapConfig`, `ElasticConfig`/`ElasticOverrides`, `AutotuningConfig`, `AssetNamingStrategy` | `domain::config` (or `domain::site` per open question; shim covers either) | `pub use domain::config::ScraperConfig` shim |
| Downloader port | `Downloader` trait + `FetchedPage`, `Cookie`, `DownloadError`, `ResourceDownloader`/`DownloadConfig` DTOs | `domain::downloader_port` (re-export via `domain::ports`) — `BoxFuture` dyn-compat like `VectorRepository` | `WreqDownloader`, `ObscuraDownloader`, `ChromiumoxideDownloader`, `HybridRouter` implement the trait |
| Crawler helpers | `extract_links`, `is_internal_link`, `normalize_url`, `derive_filename_from_response`, `SitemapConfig` surface | `domain::{link_extractor,url_validation,crawler_port}` — pure helpers; `LinkProcessor` already in domain | `infrastructure::crawler::{link_extractor,crawler_utils}` implements `LinkExtractor` |

Only `Container::new` (the composition root) constructs concrete infra types (`CpuBridge`, `RayonCpuPool`, `DomainSessionPool`, `ResourceDownloader::new`) and stores them as `Arc<dyn Port>`. Application orchestrates via traits; `domain` has zero outer imports.

### 2. Narrow allowlist (≤5 entries)

A versioned file `scripts/check_intra_crate_direction_allowlist.txt` lists the only permitted outward imports, each line with an ADR-referenced reason. CI prints `allowlisted N` and fails if `N > 5`.

Justified entries (≤5, each referenced here):

1. `application/container.rs` — `CpuBridge`, `RayonCpuPool`, `DomainSessionPool`, `ResourceDownloader::new` — **composition root must know concretes** (ADR-0009 Out-of-Scope explicitly names `application::container` as exception example; pattern `HttpClientPort` already uses `Container` as single DI point).
2. `observability::log_scrape_error` / `memory_probe` (`rss_bytes`, `append_report`, `fmt_rss`) — **transversal cross-cutting concern**; no domain value, `tracing` is the stack. Alternative was a domain `ObservabilityPort` that adds indirection with no business value — deferred.
3. `#[cfg(test)]` imports — **lint-excluded, not debt** (e.g. `pipeline/mod.rs`, `pipeline/stages/clean.rs`, `adapters/downloader/mod.rs`, `som_capture.rs` inner `mod tests`). The lint skips `use` lines under `#[cfg(test)]` / `mod tests` so strict count reflects prod code only.

Categories explicitly **not** allowlisted and must be ported:

- `StateStore`/`RecordStore`, `WafInspector`, `UserAgentCache`, `llm::validation`, `obsidian::read_vault_notes`, `axtree::fetch_raw_axtree`, `converter::html_cleaner` — low fan-in (1–2 files each); deferred to follow-up slice but **not** permanently excepted.

### 3. Lint hardening

`scripts/check_intra_crate_direction.sh` is hardened to close the bypass:

- Canonicalize `crate::ScraperConfig` (and any `crate::<PascalCase>`) alias as `infrastructure` — flagged as `::error::` in strict mode.
- Skip `use` lines inside `#[cfg(test)]` / `mod tests` blocks (heuristic: if the preceding 3 lines contain `#[cfg(test)]` or the file path contains `mod tests`, exclude).
- Enforce allowlist file ≤5 entries; each entry must have an ADR reason comment; print `allowlisted N`.
- Keep skipping crate root `lib.rs` / `main.rs` and `tests/` directories.

### 4. CI gate

Add `INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh` to `.github/workflows/ci.yml` `toolchain` job **next to** `check_dependency_direction.sh` with propagated exit. Any new `use crate::infrastructure::...` in `application/` (or `crate::ScraperConfig` alias) blocks merge. The check is not placed in `pr-validation.yml` because it is an architectural invariant, not a metadata check.

`CHANGELOG.md` is untouched (AGENTS.md policy — consolidation PR owns it). No public API rename: `pub use domain::config::ScraperConfig` shim in `infrastructure::config` + `lib.rs:118` preserves `webfang_core::ScraperConfig` and `crate::ScraperConfig` (the latter is now flagged by the lint, but still compiles via the shim until removed).

## Consequences

**Positive**

- Strict gate reaches 71 → 0 (`::error::` count) with `allowlisted ≤5` printed; new violations block merge.
- `domain` has zero outer imports; `Container` remains the single DI point (`Arc<dyn Port>`); `Engine` avoids holding `RwLock` across `.await` by cloning `Arc<dyn Downloader>` before `await` (`async-no-lock-await`, `async-clone-before-await`).
- Re-export shim keeps `webfang_core::ScraperConfig` stable — no downstream migration in this slice.
- Allowlist drift is bounded: versioned file, ADR reason per entry, CI prints count and fails if >5.

**Negative / costs**

- `ScraperConfig` move touches `infrastructure::config` ↔ `domain::config` boundary; `to_download_config()` (which builds `adapters::downloader::DownloadConfig`) must remain in the infrastructure shim to avoid `domain → adapters` outward dependency — adds a small indirection.
- `Engine` trait wiring touches hot path concurrency code; requires `CodeGraph explore` + `codedb_callers` before each edit to avoid lock-across-await.
- Allowlist is new mechanism to maintain (≤5, ADR-referenced) — but intentionally small and reviewed.

**Neutral**

- Inter-crate graph unchanged (`check_dependency_direction.sh` stays green; no `Cargo.toml` changes).
- Snapshots stable (`redact_nondeterministic()` already sanitizes TempDir/timestamps/ports).
- No runtime behavior change — `Arc<dyn Port>` already used for `HttpClientPort`/`SemanticCleaner`.

## Alternatives rejected

| Option | Pros | Cons | Verdict |
|--------|------|------|---------|
| **Pure domain ports** (200–400 lines, 10 file moves + 25 import fixups) | Fully strict, matches `HttpClientPort` pattern | Largest scope, engine lock-across-await risk, ~10 moves | Rejected — too large for a single 400-line review budget; deferred as follow-up for `StateStore` etc. |
| **Documented exception for 71 files** | Near-zero code change | Permanent debt; `inward only` becomes `inward only modulo 71`; `webfang-architecture` skill rejects permanent exception; lint stays silent on new violations | Rejected |
| **Hybrid — ports + narrow allowlist (chosen)** | Fixes ~80% debt, keeps DI/observability pragmatic, still strict | Needs ADR allowlist governance | **Chosen** — smallest viable slice that makes strict lint meaningful |

The 71-file permanent exception was explicitly rejected by the `webfang-architecture` skill (filtered `search-engine` template, tutorials 02/07/10) and by ADR-0009's own warning: "documented exception is a permanent exception; the next slice will copy the pattern."

## References

- `AGENTS.md` — crate dependency allow-matrix and Clean Architecture layers (`infrastructure → adapters → application → domain`)
- `scripts/check_dependency_direction.sh` — CI gate for inter-crate direction (Cargo.toml)
- `scripts/check_intra_crate_direction.sh` — intra-crate gate (this ADR), hardens `crate::ScraperConfig` alias
- `scripts/check_intra_crate_direction_allowlist.txt` — versioned allowlist (≤5)
- `crates/webfang_core/src/domain/config.rs` — `ScraperConfig` family now owned by domain
- `crates/webfang_core/src/domain/downloader_port.rs` — `Downloader` + `FetchedPage`/`Cookie`/`DownloadError` (BoxFuture dyn-compat)
- `crates/webfang_core/src/domain/crawler_port.rs` — `SitemapConfig` + helpers surface
- `crates/webfang_core/src/infrastructure/config.rs` — `pub use domain::config::ScraperConfig` shim
- `crates/webfang_core/src/lib.rs:118` — `pub use domain::config::ScraperConfig` re-export
- `crates/webfang_core/src/domain/ports.rs` — `AssetDownloaderPort`, `VectorRepository` (BoxFuture precedent)
- `crates/webfang_core/src/domain/link_extractor.rs` — `LinkExtractor` trait + `LinkProcessor`
- `crates/webfang_core/src/domain/url_validation.rs` — `is_internal_link`, `normalize_url`, `NormalizeConfig`
- `.github/workflows/ci.yml` — `toolchain` job next to `check_dependency_direction.sh`
- ADR-0009 — `domain::persistence::ResumeConfig` precedent for domain-owned config
- Issues: #990, #984, #809
- Tutorials: 02 scalability, 07 queue/backpressure, 10 indexing/discovery (filtered `search-engine` template)
