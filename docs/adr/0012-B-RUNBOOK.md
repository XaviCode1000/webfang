# ADR-0012-B Runbook — Measured Operational State

- **Companion to:** `docs/adr/0012-intra-crate-allowlist-roadmap-b.md` (normative plan — not repeated here).
- **Purpose:** executable state of the 10→2 allowlist path as measured today. When this file and the ADR
  disagree on a number, this file's measurement wins until the next re-measure; the ADR's *rules*
  (§2.1 ports, §2.4 DoD, §1.2.2 one-module-at-a-time) still govern.
- **Measured on:** `refactor/adr-0012b-consolidate` @ `e4db6571`, 2026-09-03.
- **Status: the 10→2 path is COMPLETE.** 18 entries → **2**, both permanent by ADR-0011.

## 1. Allowlist state today (measured)

```bash
wc -l scripts/check_intra_crate_direction_allowlist.txt        # → 8 lines = 2 entries + 6 header comments
INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh
# → allowlisted 30 (max 22, ..., entries: 2)
# → OK: intra-crate Clean Architecture layering is inward-only (ADR-0010, strict mode)   exit 0
```

Per-entry absorption, measured by dropping exactly one entry via `INTRA_CRATE_ALLOWLIST`:

| Entry | Entries | Sites | Verdict |
|---|---:|---:|---|
| `application/container.rs` | 1 | 24 | **Permanent** — ADR-0011 DI root, names concretes |
| `infrastructure::observability` | 1 | 6 | **Permanent** — ADR-0011 transversal tracing |
| **Total** | **2** | **30** | additive — no double-covering remains |

The 24 + 6 = 30 sum being exact is itself a signal: every site is now covered by exactly one entry.
While the campaign ran, columns were non-additive (ADR §1.2.1 warning) because broad and narrow entries
overlapped; that is gone.

## 2. Remaining entries and their fate

Both remaining entries are permanent and are **never** removed. There is no removal work left on this path.

Gate constants (`scripts/check_intra_crate_direction.sh`): `ALLOWLIST_CAP=22`, `ALLOWLIST_WARN_AT=20`.
At 2 entries the cap is now far above need — the ratchet is tracked in **#1095** (22 → 10 → 5, warn at cap−2).

## 3. Landed slices — normalized counters

Endpoints measured directly; intermediates are PR-recorded deltas applied in merge order. Rows marked
**[M]** were measured by this file's author on that exact tree; rows without it are PR-recorded and should
be re-measured before being trusted.

| PR | Commit | What | Entries → | Allowlisted → |
|---|---|---|---:|---:|
| #1084 | `005f654e` | Crawler narrow: 1 broad → 15 per-symbol entries. Absorption-neutral. | 8 → 18 | 56 (flat) |
| #1085 | `a0348d2d` | Repoint 3 application imports from crawler shims to domain; 3 entries dropped. | → 15 | → 53 |
| #1086 | `19ffdef1` | Export narrow: 1 broad → 8 per-symbol entries (the last broad entry). | → 22 | 53 (flat) |
| #1087 **[M]** | `709d2e31` | 3.H port: record DTOs + `RecordStorePort` into `domain::exporter`. Entries unchanged — the port landed, the entries did not. The PR body estimated `~48` sites; **measured 53**, so the port moved fewer sites than claimed. | 22 | → 53 |
| #1088 **[M]** | `1c430e71` | Sitemap port: −3 entries, +1 (`SitemapConfig`) = net −2. | → 20 | → 50 |
| #1089 **[M]** | `980f68db` | Robots port: −2 entries (same concrete, two paths). | → **18** | → **48** |
| #1096 | `396555ae` | ADR §2.3 retirement of rows 3.G/3.J/3.K/4 (#1093). Docs-only. | 18 | 48 |
| Lane A **[M]** | `eea97c6e`…`79e44354` | Export drain, 6 commits: all 8 `infrastructure::export::*` entries removed. | → 10 | → 36 |
| Lane B **[M]** | `3de02b30`…`d172a19f` | Crawler drain, 8 commits: all 8 `infrastructure::crawler::*` entries removed. | → 10 | → 42 |
| Consolidation **[M]** | `e4db6571` | Lane A + Lane B merged; the two lanes' conflicts resolved in `container.rs` and the allowlist. | → **2** | → **30** |

## 4. What is left

1. **#1095 — cap ratchet.** `ALLOWLIST_CAP` 22 → 10 → 5 with warn at cap−2. Mechanical; gated on this landing.
2. **#1097 — the deferred state-store port.** Lane A removed the last `state_store::StateStore` entry by
   relocating `create_state_store` into `cli/`, which `LAYER_RANK` does not rank — so the site left the
   gate's scope rather than passing behind a domain port. The entry's own removal-condition required a real
   port. This is genuine purity debt, and #1097 also asks the project to decide whether `cli/` should be a
   ranked layer at all. **Do not treat "the gate is green" as "3.H is architecturally finished."**
3. **Off-path purity backlog (NOT on this path):** 3.G / 3.J / 3.K / 4 — retired from the actionable table
   by #1096, zero gate delta, no entry to remove (ADR §5.1).

## 5. Acceptance criteria per slice type

Retained for the next time this file is needed — the campaign is closed, but the method is reusable.

```bash
INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh; echo "exit=$?"
```

| Slice type | Gate criteria (all must hold) |
|---|---|
| **Narrow** (broad → per-symbol) | exit 0; `allowlisted` count **flat** vs base; entries ≤ cap; every recorded module path covered — verify by re-running the §1 probe with the broad line removed and diffing `::error::` paths against the new entries. |
| **Port** (repoint + delete entry) | exit 0; `allowlisted` drops by exactly the measured exclusive sites of the deleted entry(ies); zero new `::error::`; deleted entry's symbols have no remaining non-test reference outside `container.rs`. |
| **Dead-weight drop** | exit 0; count flat; each dropped entry re-measured at 0 exclusive sites on the PR's own base (method: §1 probe). |
| **Any** | Each new/kept entry cites its removal condition **by symbol**, never by line number (#1032). `CHANGELOG.md` untouched. PR body `Closes part of #994` — never a bare-close of the umbrella. One `type:*` label; conventional branch. |

### Method lessons worth keeping

- **A commit message is not evidence.** One Lane B commit claimed to drop an allowlist entry whose line was
  absent from its own diff; entry counts and gate exit were both green, so every numeric check passed on a
  false claim. Audit claims against `git show --name-only`, not against counts.
- **Diff against the merge-base, never the live tip.** A reviewer reported a "revert" that was only the
  branch being behind main.
- **`git stash` is forbidden here for a reason.** One lane used a `stash push/pop` round-trip to test whether
  warnings pre-existed on base. `refs/stash` is shared across every worktree; a pop in one session applies
  another session's work. Use `git show main:<path>` or a temp copy outside the repo.
- **`rust-analyzer` E0308/E0605 are noise** (spurious, tracked in #1034). `cargo check` is the authority.
- **The pi-lens automated runner's 60s timeout kills cargo on this repo** and reports it as test failure;
  no test ever ran. Run gates explicitly instead.

---

**Re-measure triggers:** any merge touching `crates/webfang_core/src/**` or the allowlist invalidates §1/§3/§4.
Re-run §1's probe commands and update this file in the same PR that changes the allowlist.
