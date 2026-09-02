# ADR-0012-B Runbook — Measured Operational State

- **Companion to:** `docs/adr/0012-intra-crate-allowlist-roadmap-b.md` (normative plan — not repeated here).
- **Purpose:** executable state of the 10→2 allowlist path as measured today. When this file and the ADR disagree on a number, this file's measurement wins until the next re-measure; the ADR's *rules* (§2.1 ports, §2.4 DoD, §1.2.2 one-module-at-a-time) still govern.
- **Measured on:** `main` @ `98273348`, 2026-09-02 (re-verified: 18 entries / `allowlisted 56` / strict exit 0). Every number below was produced by the commands in §1/§4 — do not trust any of them after a merge that touches `crates/webfang_core/src` or the allowlist.

## 1. Allowlist state today (measured)

```bash
wc -l scripts/check_intra_crate_direction_allowlist.txt        # → 23 lines = 18 entries + 5 header comments
INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh
# → allowlisted 56 (max 22, ..., entries: 18)
# → OK: intra-crate Clean Architecture layering is inward-only (ADR-0010, strict mode)   exit 0
```

Gate constants (`scripts/check_intra_crate_direction.sh`): `ALLOWLIST_CAP=22` (hard fail above), `ALLOWLIST_WARN_AT=20` (::warning:: at/above). Today: 18 entries — **2 below warn, 4 below cap**. The next narrow (+7 worst case) crosses both; see §4.

Per-entry absorption, measured by dropping exactly one entry via `INTRA_CRATE_ALLOWLIST` (no repo file touched):

```bash
D=$(mktemp -d); grep -vF 'infrastructure::crawler' scripts/check_intra_crate_direction_allowlist.txt > "$D/probe.txt"
INTRA_CRATE_ALLOWLIST="$D/probe.txt" INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh
```

| Entry group | Entries | Sites absorbed (exclusive-ish) |
|---|---:|---:|
| `application/container.rs` (permanent, DI root) | 1 | 12 |
| `infrastructure::observability` (permanent, transversal) | 1 | 6 |
| `infrastructure::crawler::*` per-symbol (narrowed by `42b06421`, #1082) | 15 | 22 |
| `infrastructure::export` (broad — last broad entry) | 1 | 14 |
| **Total** | **18** | **56** (sum is 54; 2 sites are double-covered, e.g. container.rs naming `crawler::ResourceDownloader`) |

## 2. Remaining entries and their fate

| Entry | Fate | Removal condition (symbol, never line — #1032) |
|---|---|---|
| `application/container.rs` | **Permanent** | Never removed (ADR-0011 DI root). |
| `infrastructure::observability` | **Permanent** | Never removed (ADR-0011 transversal). |
| 15 × `infrastructure::crawler::<sym>` | **Port one at a time** | Each entry's own comment names its port slice + home module. All 15 absorb ≥1 site today — no dead weight to drop for free. |
| `infrastructure::export` (broad) | **Narrow+port in ONE PR (#1083)** | Cannot land as pure narrow — cap math in §4. |

Crawler port grouping (from the landed per-symbol entries):

- **Cheap (pure moves / shim repoint):** `normalize_url`, `is_internal_link` (link_extractor), `binary_utils::derive_filename_from_response`, `parse_sitemap`, `SitemapConfig` (already `domain::crawler_port::SitemapConfig` — repoint call sites, delete shim).
- **Trait slices (concrete stays infra, ADR-0012-B §2.1):** `UrlQueue`, `RobotsFetcher` + `robots_utils::RobotsFetcher` (same concrete, two paths — delete together), `SitemapParser`/`SitemapUrl`/`SitemapError`, `fetch_url`, `extract_links`, `resource_downloader::ResourceDownloader`, `FsBinaryWriter`.

## 3. Landed slices — normalized counters

Effective state on `main` after each merge. PR bodies measured against their own branch base; the chain below is normalized to main. Endpoints (`71`, `56`) measured directly; intermediates are PR-recorded deltas applied in merge order (chain is self-consistent).

| PR | Commit | What | Entries → | Allowlisted → |
|---|---|---|---:|---:|
| #1069 | `8dc58c6e` | Scanner unit fix: sites not regex hits (84→52→**71**), brace expansion, segment-boundary matching. Narrowing became implementable. | 10 | 71 |
| #1074 | `b75a4bdc` | Prose hygiene (#1032): symbol-cited removal conditions. Patterns untouched. | 10 → | 71 → |
| #1079 | `7905e72c` | Dropped 2 zero-absorption entries (`discovery.rs`, `crawler_service.rs`). Free step. | → 8 | 71 (flat) |
| #1073 | `9399855e` | 3.I — `vault_search` → `domain::note_repository::VaultNoteReader`. | → 7 | → 70 (−1) |
| #1077 | `07d6f7cb` | 3.F — Engine consumes `domain::session_port` (3 sites). `engine.rs` entry KEPT for `SystemRamProbe` residual. | 7 | → 67 (−3) |
| #1076 | `04f57a65` | 3.E — `CpuExecutorPort` grows `dispatch_resource` + `ProcessedChunk`. No allowlist change. | 7 | 67 |
| #1080 | `3a0ec39f` | 3.E.2 — `ElasticIngestion` → `Arc<dyn CpuExecutorPort>`; entry dropped. Delta −9 (8 bridge + 1 collapsed crawler record; ADR's −8 estimate missed brace expansion). | → 6 | → 58 |
| #1081 | `7768f23b` | Cheap wins — `asset_download` port + `engine.rs` `SystemRamProbe` → `domain::ram_probe_port::system_default()`; 2 entries dropped. Also corrected #1081's own finding: the ADR §5 removal-condition for `asset_download` was wrong (two different downloaders). | → 4 | → **56** |
| #1082 | `42b06421` | Crawler narrow: 1 broad → 15 per-symbol (measured; ADR §1.2.2's "9" was stale at `8dc58c6e` — per-file entry drops exposed 6 more paths). Absorption-neutral by design. Merged into local main; **origin/main still `7768f23b` — push pending; issue #1082 stays OPEN on GitHub until the push.** | → **18** | 56 (flat) ✓ |
| — | `98273348` | Runbook merged into local main (docs-only; no allowlist/code change). | 18 | 56 (flat) |

## 4. Pending slices

**Order is forced by the cap, not by preference.** After the crawler narrow, `17 + E_export_entries ≤ 22` → **E ≤ 5**. The pure 8-entry export narrow (branch `refactor/narrow-export-allowlist` @ `22bb5752`, rebased onto `42b06421`) **cannot land on today's main** — its tree carries 25 entries and the gate hard-fails on entry count. It waits for 3.H.

1. **#1082 follow-up (decision, not code):** the narrow is merged on local main (`42b06421`, runbook on top at `98273348`); **the push is still pending** — origin/main is `7768f23b`, so #1082 stays OPEN on GitHub until `git push`.
2. **#1083 — 3.H export (narrow+port in ONE PR):** grow the parked `22bb5752` narrow until ≥3 of the 8 export symbols are ported away in the same PR. Files with the 14 sites (measured): `application/export_factory.rs` ×8, `application/resume.rs` ×5, `application/export_utils.rs` ×1. The `cli/` files cited in issue #1083 are outside the gate's `ROOT` (`crates/webfang_core/src`) — they don't move the counter. Fitting path: porting `DomainRecords` + `RawRecord` (−2 entries) and letting `Container` absorb a `StateStore`-family site into the permanent file entry (−1) reaches E=5 at exactly cap 22 (warn fires at 20 — allowed, non-blocking).
3. **Crawler port slices (post-#1082):** one symbol-group per PR, cheapest first (§2 grouping). **In flight:** `refactor/sitemap-port` @ `42b06421` (sitemap group: `SitemapParser`/`SitemapUrl`/`SitemapError`/`parse_sitemap` — 5 sites measured). Each PR: repoint sites to the `domain::crawler_port` surface, delete exactly its entries, count drops by that group's sites. Re-measure absorption per group before claiming a delta — double-covering makes columns non-additive (ADR §1.2.1 warning still true).
4. **Off-path purity backlog (NOT on the 10→2 path):** 3.G / 3.J / 3.K / 4 — zero gate delta, no entry to remove (ADR §5.1). Do not attach allowlist acceptance tests to them.
5. **Cap ratchet (after 10→2 lands):** 22 → 10 → 5 with warn at cap−2 (ADR §2.2). Until then cap stays 22.

## 5. Acceptance criteria per slice type

Every PR runs the full §2.4 DoD (≤400L by `git diff -C`, `cargo check/clippy/fmt/nextest` green). Gate-specific checks by slice type:

```bash
INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh; echo "exit=$?"
```

| Slice type | Gate criteria (all must hold) |
|---|---|
| **Narrow** (broad → per-symbol) | exit 0; `allowlisted` count **flat** vs base (narrowing must not change absorption); entries ≤ 22; every recorded module path covered — verify by re-running the §1 probe with the broad line removed and diffing `::error::` paths against the new entries. |
| **Port** (repoint + delete entry) | exit 0; `allowlisted` drops by exactly the measured exclusive sites of the deleted entry(ies); zero new `::error::`; deleted entry's symbols have no remaining non-test reference outside `container.rs`. |
| **Dead-weight drop** | exit 0; count flat; each dropped entry re-measured at 0 exclusive sites on the PR's own base (method: §1 probe). |
| **Any** | Each new/kept entry cites its removal condition **by symbol**; no line numbers (#1032). `CHANGELOG.md` untouched. PR body `Closes part of #994` (never bare-closes the umbrella). One `type:*` label; conventional branch. |

## 6. Active worktrees (measured 2026-09-02)

```bash
git worktree list   # candidate-views omitted; they are gentle-ai RDD internals — never touch
```

| Worktree | Branch / HEAD | Role | State |
|---|---|---|---|
| `~/Projects/Rust/webfang` | `main` @ `98273348` | 3 ahead / 0 behind origin | #1082 + runbook merged locally, unpushed (§4.1) |
| `webfang-worktrees/feat-3h-export-port` | `refactor/3h-export-port` @ `7768f23b` | 3.H port work | **Stale base** (predates the #1082 narrow); overlaps #1083 scope — coordinate before either launches |
| `webfang-worktrees/refactor-narrow-export-allowlist` | `refactor/narrow-export-allowlist` @ `22bb5752` | export narrow | Rebased onto `42b06421`; pure narrow = 25 entries > cap → **parked, waiting on 3.H** (§4.2) |
| `webfang-worktrees/refactor-sitemap-port` | `refactor/sitemap-port` @ `42b06421` | sitemap crawler port (§4.3) | **Working** (0 commits ahead of its base so far) |

Cleanup note: the crawler-narrow worktree/branch and the unrelated worktrees (`fix-1062-rustdoc`, `fix-rust-analyzer-boxfuture`, `wip-1034-partial`) were removed after landing. Post-merge runbook (AGENTS.md) applies per slice as each lands.

---

**Re-measure triggers:** any merge touching `crates/webfang_core/src/**` or the allowlist invalidates §1/§2/§4 numbers. Re-run §1's probe commands and update this file in the same PR that changes the allowlist.
