# ADR 0001: Unify Asset Downloading in the Adapters `Downloader`

- **Status:** Accepted (amended 2026-09-04 — see "Amendment")
- **Date:** 2026-07-10
- **Deciders:** Project Architect, `webfang` maintainers
- **Related issues:** #142 (timeout hardcoded), #143 (`--h2-profile` ignored), #144 (`--include-pattern` not applied to assets), #145 (asset naming = hash only), #1115 (amendment)
- **Supersedes:** —

## Context

`webfang` has accumulated **three parallel ways to download bytes**, and the one used in
production for assets is the weakest:

| Location | Kind | Role | Wired into production asset path? |
|----------|------|------|-----------------------------------|
| `src/infrastructure/downloader/mod.rs:44` | `Downloader` **trait** | Fetch **pages** (`fetch → FetchedPage`), connection pooling, DI | Yes (page crawling) |
| `src/adapters/downloader/mod.rs:74` | `Downloader` **struct** | Download **assets** with true streaming + atomic writes | **No — orphaned** (only `quick_download` + tests call it) |
| `src/infrastructure/scraper/asset_download.rs:131` | Inline `wreq::Client` per asset | Actual production asset download | **Yes — the "island"** |

The production island (`asset_download.rs`) builds a fresh `wreq::Client` for **every** asset with
two hardcoded literals:

- `asset_download.rs:133` → `.timeout(Duration::from_secs(30))` (ignores `--download-timeout`, which maps to `ScraperConfig.download_timeout_secs` but is never forwarded). → **Issue #142**
- `asset_download.rs:132` → `.emulation(Emulation::Chrome145)` (ignores `--h2-profile`). → **Issue #143**
- `asset_download.rs:144` → `response.bytes().await` loads the **entire file into RAM**, then `bytes.to_vec()` (l.163). Violates the `RAM_BUDGET`/`ElasticIngestion` memory contract for large files.
- `extract_documents` (`src/extractor/mod.rs:104`) returns all document URLs unfiltered; `--include-pattern` only applies to crawl discovery (`url_filter.rs`), so `--download-documents --include-pattern '*.pdf'` still downloads EPUB/MOBI/AZW3. → **Issue #144**
- Filenames are SHA-256 hashes (`asset_download.rs:148-155`); no title/slug strategy. → **Issue #145**
- No retry: `filter_map` silently drops failures (l.107-114).

The `Downloader` **struct** in `adapters/downloader/mod.rs` already implements the intended design:
true streaming to disk (~8KB RAM, hash on-the-fly, atomic temp-file rename), init-once directory
creation, and it **already honors `DownloadConfig.timeout_secs`** (l.108). It is simply not used by
the production path. (`True Streaming` is also referenced in `CHANGELOG.md:241`.)

This is **architectural drift**: a correctly-designed component exists but is bypassed by an
ad-hoc inline implementation.

## Decision

Adopt **Option B — Structural Unification**. Do **not** patch the island with parameter drilling.

1. **Eliminate the island.** Remove `src/infrastructure/scraper/asset_download.rs`. The asset
   download responsibility moves entirely to the `Downloader` struct in
   `src/adapters/downloader/mod.rs`.
2. **Inject, don't construct.** `ScraperService` (application layer) receives a configured
   `AssetDownloader` (the adapters `Downloader`) via constructor injection (REGLA_1: dependencies
   are provided, not built "al vuelo"). The orchestrator builds one `DownloadConfig` from
   `CrawlOptions`/`ScraperConfig` and passes the downloader down.
3. **Extend `DownloadConfig`** with:
   - `include_patterns: Vec<glob>`, `exclude_patterns: Vec<glob>` → filtering in `download_batch`
     using the existing `url_filter::is_allowed` (reuses crawl-discovery logic for consistency). → **#144**
   - `h2_profile: wreq_util::Profile` → replaces the hardcoded `Emulation::Chrome145` (l.107). → **#143**
   - `asset_naming: AssetNamingStrategy { Hash | Slug | ContentDisposition }` → replaces the
     hardcoded `generate_filename_from_hash`. → **#145**
   - (`timeout_secs` already present → **#142 resolved for free** by routing through this struct.)
4. **Resilience.** `download` / `download_batch` gain retry-with-backoff + jitter on **transient
   network errors** (timeouts, connection resets); **fail-fast on 4xx** (terminal). Bounded by
   `--max-retries` / `--backoff-*`.
5. **Partial-failure semantics.** `download_batch` keeps returning `Vec<Result<DownloadedAsset>>`
   (collect, never abort the whole batch on one failure). The orchestrator aggregates
   successes/failures into the scrape summary.

## Trade-offs

| Quality attribute | Island (current) | Unified (Decision) |
|-------------------|------------------|--------------------|
| Memory efficiency | Poor (full-file buffer in RAM) | Excellent (8KB streaming) |
| Network resilience | Rate-limit risk (per-asset TLS handshake) | High (keep-alive, pooled client) |
| Maintainability | High debt (duplicated logic, 3 paths) | Solid (single change point) |
| Blast radius of change | Low (isolated) | Medium (touches `ScraperService` DI + deletes a module) |

The medium blast radius is accepted: the dependency graph (the asset-download entry point —
cited here at decision time as `download_all`, a name that never existed in the code — had
`impactedCount: 11`,
risk CRITICAL across `ScraperService`/`discovery`/`scrape_flow`/`mcp_server`) is concentrated and
well-understood; tests will be migrated to the adapters `Downloader`.

## Consequences

**Positive**
- RAM stays constant (~8KB) regardless of asset size → no collision with `RAM_BUDGET`/`ElasticIngestion`.
- Reused pooled `wreq::Client` → keep-alive removes repeated TLS handshakes (root cause of the
  intermittent GitHub Pages timeouts we observed).
- Issues #142, #143, #144, #145 all closed by one structural change.
- One source of truth for asset download configuration.

**Negative / costs**
- Refactor dependency injection in `ScraperService` (constructor signature change).
- Delete `asset_download.rs` and migrate its unit/integration tests to the adapters `Downloader`.
- Behavioral change: partial failures now surface in the summary instead of being silently dropped
  (must update any consumer that assumed "no error = all downloaded").

**Neutral**
- ~~`quick_download` (adapters `Downloader`'s own convenience fn) remains valid and becomes the
  canonical quick path.~~ **Amended (#1115):** `quick_download` never gained a caller and was
  removed; the canonical asset path is the `AssetDownloaderPort` (see Amendment).

## Clarifying questions resolved

1. **Naming strategy (#145):** The struct is currently hardcoded to hash
   (`generate_filename_from_hash`, l.235-242). Not slug/title capable. Decision: add
   `asset_naming` to `DownloadConfig`; default `Hash` (preserves current dedup behavior), with
   `Slug` (URL last segment) and `ContentDisposition` as options. Falls back to `Hash` when the
   source provides no usable name.
2. **Partial-error handling:** The struct already collects all results (`Vec<Result<…>>`, l.217-233)
   — i.e. collect-and-report, not fail-fast. Decision: keep collect semantics; add transient-retry
   with backoff; fail-fast only on 4xx; report per-asset status to the orchestrator summary.

## References

Paths are given as of the amendment (2026-09-04); the original ADR cited the pre-workspace
`src/...` layout, which maps to `crates/webfang_core/src/...` today.

- `crates/webfang_core/src/adapters/downloader/mod.rs` — target `Downloader` struct (streaming, init-once, `timeout_secs` honored); implements `AssetDownloaderPort`
- `crates/webfang_core/src/application/asset_download.rs` — application orchestration over the port (the island `infrastructure/scraper/asset_download.rs` was deleted as decided)
- `crates/webfang_core/src/extractor/mod.rs` — `extract_documents` (pattern filtering)
- `crates/webfang_core/src/application/url_filter.rs` — `is_allowed` (reuse for asset filtering)
- `crates/webfang_core/src/domain/config.rs` — `ScraperConfig.download_timeout_secs`
- `CHANGELOG.md` — "True Streaming: Constant ~8KB RAM, no OOM"
- Issues: #142, #143, #144, #145

## Amendment (2026-09-04, #1115)

Contract corrections after implementation — the decision itself stands unchanged:

1. **Canonical path is the port, not a convenience fn.** The production asset path is
   `AssetDownloaderPort` (`domain::ports`), implemented by the adapters `Downloader` and consumed
   by `application/asset_download.rs`. The `quick_download` convenience fn predicted as "the
   canonical quick path" was never wired into any caller and has been removed.
2. **`download_all` never existed.** The blast-radius citation named a function that was not in
   the code at decision time; the affected entry point was the asset-download call path through
   `ScraperService`/`scrape_flow`.
3. **Path updates.** All `src/...` references now live under `crates/webfang_core/src/...`
   (workspace split); `ScraperConfig::to_download_config` moved to the adapters module next to
   `DownloadConfig`, keeping `domain` free of `adapters` dependencies (ADR-0010).
