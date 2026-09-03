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
- `scripts/check_intra_crate_direction.sh` — intra-crate gate (this ADR), hardens `crate::ScraperConfig` alias; ADR-0010-A extends to inline qualified paths
- `scripts/check_intra_crate_direction_allowlist.txt` — versioned allowlist (≤22 during ADR-0010-A, warn at 20; cap reverts incrementally as #994 sub-slices 1, 3, 4 land; realistic post-#994 floor is ~10–13, not 5)
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
- ADR-0010-A (this addendum) — inline qualified-path detection (issue #995)
- Issues: #990, #984, #809, #994 (sub-slice 3 follow-up), #995 (this addendum)
- Tutorials: 02 scalability, 07 queue/backpressure, 10 indexing/discovery (filtered `search-engine` template)

## Addendum 0010-A: Inline Fully-Qualified Path Detection (issue #995)

### 1. Problem

The original lint (`scripts/check_intra_crate_direction.sh` per ADR-0010) only
matched `use crate::<layer>::...;` lines via a single `grep -E '^[[:space:]]*(pub[[:space:]]+)?use[[:space:]]+crate::[A-Za-z_]+(::|;)'`
pass. That scope missed **inline** fully-qualified paths that appear in any
position of any non-`use` line — function bodies, struct field defaults, trait
bounds, pattern matches, format strings, error variants, and so on. Empirically
(issue #995), a fresh `grep -nE 'crate::(infrastructure|adapters|application)::'`
across `crates/webfang_core/src/` surfaces **~63 inline sites in ~19 files** that
the original lint silently approved while the same module had the outward
import drawn on a different `use` line elsewhere. A new contributor could
`use`-clean their file, then smuggle the same violation inline, and the strict
gate would stay green.

The new pass is **mandatory** for the strict gate to be a real architectural
invariant, not a textual accident of `use`-line coverage.

### 2. Decision

Extend `scripts/check_intra_crate_direction.sh` with a **second scan pass** that
detects inline qualified `crate::<layer>::...` paths in any position. The two
passes share the same `#[cfg(test)]` / `mod tests` skip heuristic and the same
allowlist accounting.

**Layer regex** (inline pass only, no `use` prefix required):

```regex
crate::(infrastructure|adapters|application)::[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*
```

The regex captures the FULL qualified path (all `::Segment` components), not
just the layer + first segment — this is what allows narrow allowlist entries
(e.g. `infrastructure::http::waf_engine`) to substring-match the recorded
violation instead of forcing a broad `infrastructure::http` entry.

It deliberately does NOT match `crate::domain::` (domain is the innermost, so
domain→domain and outward imports of `domain::*` from any layer are caught
elsewhere or impossible by construction) and does NOT match `crate::<PascalCase>`
(those are the legacy alias re-exports — `crate::ScraperConfig`,
`crate::AutotuningConfig`, `crate::SitemapConfig`, `crate::ElasticConfig`,
`crate::ElasticOverrides` — already handled by `ALIAS_AS_INFRA_REGEX` in the
`use` pass and classified as `infrastructure` per ADR-0010 §3).

**Comment filter — awk state machine.** The inline pass routes every line
through a small awk filter (`filter_comments` in the script) that:

- Tracks an `in_block` flag for `/* ... */` block comments: opens on `/*` not
  preceded by `//`, closes on the next `*/`.
- Drops lines whose first non-whitespace token starts with `//` (covers `//`,
  `///`, `//!`).
- Extracts **every** occurrence of the layer regex on the surviving code —
  not just the first match per line — and emits one record per match.
- Preserves the original 1-indexed line number (`NR`), so the resulting
  `::error::$file:$lineno::` line number is correct for the user.
- Runs as a **single awk process per file** (the layer regex is passed via
  `awk -v` from `INLINE_LAYER_REGEX`, keeping one source of truth). The
  per-line-subshell variant was measured at >30s across the ~93k lines of
  `webfang_core` and is explicitly forbidden — the O(1)-processes-per-file
  contract is part of this addendum.

**Documented limitation (ADR-0010-A):** the awk filter does NOT parse Rust
string literals or handle backslash continuations. A path inside a `"..."`
literal or after a `\` continuation cannot be disambiguated by awk alone —
residual false positives are routed through the allowlist (which already
substring-matches on the full match), **not** through the regex. The regex
stays conservative; the allowlist absorbs noise. The user explicitly rejected
a full Rust parser in bash; this comment is the contract.

**Allowlist matching.** The existing `is_allowlisted($file, $match, $target)`
is reused as-is. For inline matches, `$match` is the full
`crate::infrastructure::X::Y...` substring from the line, `$file` is the source
file path, `$target` is `infrastructure` / `adapters` / `application`. Existing
broad entries (`infrastructure::crawler`, `infrastructure::downloader`,
`infrastructure::export`, `infrastructure::observability`,
`application/container.rs`) continue to absorb pre-existing inline sites by the
same substring match.

### 3. Allowlist cap revisited (5 → 22, temporary, with honest reversal floor)

The original ADR-0010 §2 capped the allowlist at **≤5 entries**. After the
inline pass is enabled, the empirical count of pre-existing inline sites not
already covered by the 5 broad entries is **43 violations across 13 files**.
The cap must be raised to keep the strict gate green.

**Honest cap-reversal floor (correction to the original addendum draft):**
the goal of "revert toward 5" stated in the first draft of this addendum
was wrong. The 5-entry cap assumed one broad `application/`-style entry; the
per-file shape (which we adopted to make cleanup mechanical) inherently
needs more entries. After all #994 sub-slices land (1, 3, 4), the realistic
floor is **~10–13 entries**, not 5. The remaining entries would be:
`application/container.rs` (composition root, never ported), the 5 broad
infrastructure entries (each with its own port candidate), and the per-file
stragglers whose porting depends on later architectural decisions beyond
#994. Future work that further reduces the floor must come with its own
ADR and explicit cost/benefit analysis — the 5-entry number is not a target
to blindly chase.

The cap is currently **≤22** (19 entries + headroom), with a **soft warn
threshold at 20 entries** (`::warning::` in CI, non-blocking) so a NEW
one-file violation does not force an ADR edit on every PR. The hard cap still
fails CI. The cap
reversal path is:

| After #994 sub-slice | Cap drops by | Reason |
|---|---|---|
| 1 (ScraperConfig family → `domain::config`) | -1 | `crate::ScraperConfig` alias entry removed |
| 1 (continued) | -3 | `application/crawler/discovery.rs`, `application/crawler_service.rs`, `application/extraction.rs` no longer need ScraperConfig-specific entries (they are absorbed by other broad entries) |
| 3 (persistence/scraper/converter ports → `domain::*`) | -3 | `application/elastic_ingestion.rs`, `application/scraper_service.rs`, `application/vault_search.rs` removed (or split per port) |
| 3 (continued) | -1 | `application/som_capture.rs` (axtree port) removed |
| 3 (continued) | -1 | `application/http_client/factory.rs` (SSRF port) removed |
| 3 (continued) | -1 | `application/llm_extraction.rs` (SSRF port) removed |
| 3 (continued) | -1 | `application/asset_download.rs` (Downloader port) removed |
| 3 (continued) | -1 | `application/crawler/engine.rs` (SessionPool port) removed |
| 3 (continued) | -1 | `adapters/downloader/mod.rs` (SSRF wiring) removed |
| 4 (WAF AC → `domain::waf`) | -1 | `infrastructure::http` entry removed |
| **Realistic post-#994 floor** | **~10** | `application/container.rs` + 5 broad infra entries + ~4 stragglers |

Three precision points govern the temporary cap (issue #995 user thread):

1. **Each new entry MUST cite ADR-0010 AND the specific #994 sub-slice in
   its reason comment** — "fecha de caducidad semántica" (semantic expiration
   date). When the cited sub-slice ports the offending file, the entry is
   removed in the same commit and the cap count drops by one. This is what
   makes cleanup mechanical (a `sed` per sub-slice, not a manual audit).
2. **Per-file entries are non-negotiable.** The first draft of this addendum
   used a broad `application/` substring entry. That was rejected because
   it froze debt (any new file under `application/` would silently match
   without per-file accountability) and made the cap-reversal unactionable
   (cleanup would require an audit of which files the broad entry matched).
3. **The cap is a hard gate in the script** (`ALLOWLIST_CAP=22`, with a soft
   non-blocking `::warning::` at `ALLOWLIST_WARN_AT=20`). Adding a 23rd entry
   fails CI; the warning fires when the allowlist reaches 20 entries so
   pruning is prompted BEFORE the hard gate bites. The 3-entry headroom
   exists so a NEW one-file violation does not force an ADR edit on every
   PR — the next entry must still wait for a sub-slice to remove an existing
   one or for a deliberate cap bump with its own ADR note.

### 4. Out of scope

Porting the 13 application/* files (top-3: `elastic_ingestion`,
`scraper_service`, `crawler/crawl_result_repository`; long tail: `som_capture`,
`vault_search`, `http_client/factory`, `llm_extraction`, `asset_download`,
`crawler/engine`, `crawler/discovery`, `crawler_service`, `extraction`, plus
`domain/waf.rs` and `adapters/downloader/mod.rs`) to `domain::*` is **not**
part of this slice. The intra-crate lint extension is bug-fix scope (closes
a CI gap that allowed architectural violations to slip through); the ports
are refactor scope and have their own design debate in #994 sub-slices 1, 3,
4:

- `CpuBridge` port shape — currently `infrastructure::bridge::CpuBridge` is
  used inline in `application/elastic_ingestion.rs` (8 sites). The port
  surface (`dispatch`, `dispatch_blocking`, `WorkerCount`) is non-trivial;
  sub-slice 3 weighs `Box<dyn CpuBridge>` vs. a generic `Port<CpuJob>`.
- `AutotuningConfig` ownership — `infrastructure::autotuning` defines
  `ElasticConfig` (now re-exported via `domain::config::ElasticOverrides` but
  the struct still lives in infrastructure). The full `ElasticConfig` →
  `domain::config` move is deferred to sub-slice 1.
- WAF AC automaton — `domain/waf.rs` is misfiled (delegates via qualified
  path to `infrastructure::http::waf_engine`). The full AC logic → `domain::waf`
  move is deferred to sub-slice 4.

This PR does NOT touch `Cargo.toml` (no new dependencies; the existing
`sysinfo` doc-link fix in `infrastructure/autotuning.rs` is a comment-only
change), does NOT touch `CHANGELOG.md` (AGENTS.md policy — consolidation PR
owns it), and does NOT modify any production file beyond the comment fix.

### 5. Update history

- **2026-08-29** — Addendum 0010-A issued with the inline-path scanner
  extension. Issue #995 closed. Allowlist cap raised 5 → 22 (19 entries +
  headroom, soft warn at 20). All 19 entries
  carry per-file inventory + cited #994 sub-slice (1, 3, or 4) for
  mechanical cleanup. Honest cap-reversal floor documented: ~10–13 after
  sub-slices 1+3+4 land, not 5. The "revert toward 5" language in the first
  draft was corrected because the per-file shape inherently needs more
  entries than the pre-#995 broad-entry model.
- **2026-08-29 (review fixes)** — Scanner hardened after external review:
  single-awk-pass matcher (one process per file; the per-line subshell
  variant measured >30s and is forbidden), every-match-per-line emission,
  full-qualified-path capture so allowlist entries stay narrow
  (`infrastructure::http::waf_engine`, not broad `infrastructure::http`),
  and the cap text in this document synchronized with the script
  (`ALLOWLIST_CAP=22`, warn at 20).

## Addendum 0010-B: Inline-Path Greenness Taxonomy (issue #1100)

### Context

The `use`-only lint missed `crate::` qualified paths inside function bodies;
Addendum 0010-A closed that hole with a second inline scan pass. Issue #1099
then deleted the legacy config shim (`lib.rs` re-export of
`infrastructure::config`) together with the alias regex machinery, so the
inline layer regex is now the single source of truth for path matching. What
remained undocumented was the greenness contract itself: which green states
are permanent, which are exclusions, and what a new inline path does. This
addendum pins that taxonomy so a green strict run is never accidental.

### Decision

- **Site definition.** Every non-comment occurrence of
  `crate::infrastructure|adapters|application::...` in a layer body is one
  site. Both scan passes emit records into a single stream deduped on the key
  (file, line, full-path) before anything is counted or reported.
- **Production green only via named entries.** A production site goes green
  ONLY through a named, ADR-reasoned, counted allowlist entry — today exactly
  the two permanent ADR-0011 entries (the `application/container.rs` DI root
  and the `infrastructure::observability` transversal). Each entry cites its
  removal condition by symbol, never by line number.
- **Exclusions are documented, never counted.** Test-only code (past the first
  `#[cfg(test)]` / `mod tests` marker), doc and block comments (dropped by the
  awk filter), lateral same-layer references (the rule fires outward only),
  and the `cli/` composition edge (rank -1, consumed from #1097, re-decided
  nothing here) stay green by documented rule and are pinned by
  `scripts/test_intra_crate_gate.sh`, never by allowlist entries.
- **Scanner shape retained.** One awk process per file per pass; Rust string
  literals are still not parsed — a path inside quotes is residual noise
  routed to the allowlist, never to a regex carve-out.
- `scripts/check_dependency_direction.sh` (inter-crate gate) is untouched.

### Consequences

- Strict mode is green with exactly the 2 permanent entries; the allowlist
  count is unchanged by this slice (scripts and docs only, zero production
  changes).
- Any new inline path in production code fails closed (`::error::`, exit 1);
  the four harness cases (body probe, lateral shim, `cli/` edge, doc comment)
  pin each taxonomy class.
- Future entries need their own ADR note with a symbol-cited removal
  condition; exclusions must gain a harness case, not an entry.

### Update history

- **2026-09-03** — Addendum 0010-B issued (issue #1100). Header taxonomy in
  the gate script, four harness cases, no logic or production change.
