# Migrate `main` to a seeded, exclusive target

## Goal

`main` stops being the one tree with a monolithic shared `CARGO_TARGET_DIR` and becomes
an ordinary seed consumer like every other worktree. The 478 GB historical target then
receives no writes, which is the precondition that makes it reclaimable.

## Why now

PR #1721 (merged, `4c03aa12`) proved the mechanism end to end on a real worktree: seed
published from `main`, `seed_target.sh` returning `seeded`, workspace build in 18 s, and
`webfang 2.4.1` running. What it did not change is `main`'s own target, which still holds
46 generations of dead worktrees and 198 generations of `btls-sys`. The audit showed the
growth is not "old seeds nobody reclaims" — it is references to checkouts that no longer
exist, inside a target Cargo cannot attribute by ownership.

## Decisions taken

| Decision | Choice | Consequence |
| :--- | :--- | :--- |
| Worktree `.envrc` generation | Generate it from the worktree's own basename, never by rewriting `main`'s | `main`'s target name stops being an input to any other tree's policy |
| `main`'s `CARGO_INCREMENTAL` | Keep `1` | First seeded build costs one workspace rebuild (measured 20 s); the 669 BoringSSL objects survive either way |
| 478 GB | Quarantine now, delete in a separate authorized step | Reversible; the runbook's ownership evidence is recorded |

## Tasks

### T1 — Policy: `main` is a seed consumer, not the shared cache

Rewrite the worktree bootstrap recipe in `AGENTS.md` to write `.envrc` directly instead of
`sed`-ing `main`'s, and restate `main` as one isolated target among many.

Why the recipe is load-bearing: with `main`'s target renamed, the existing `sed` still
produces a **valid** `CARGO_TARGET_DIR` — pointing at `main`'s target. Every new worktree
would inherit it, and the only thing preventing damage is PR C's guard refusing the build.
A bootstrap recipe that fails silently into an isolation violation is worse than one that
fails loudly.

Also correct the guard's wording in `ci_fast_gate.sh` ("main's shared cache" is no longer
what it is) and the `.envrc` note that says only `main` uses the shared cache.

### T2 — Migrate `main`

Create `~/.cache/cargo-target/main` from the current seed, point `main`'s `.envrc` at it,
`direnv allow`, then a real build and the real binary.

`.envrc` is untracked, so T2 is a local environment change; the policy in T1 is what ships.

### T3 — Quarantine the historical target (DONE)

Verify no writers, sample twice 60 s apart requiring delta 0, confirm ownership, then rename
to a quarantine name. No deletion in this work.

### T4 — Documented, not executed (DONE)

Runbook written to `odd/tasks/target-quarantine-runbook.md`. The deletion it describes is
**not authorised and was not performed**. Quarantine state recorded in its section 9.

## Non-goals

- Deleting the quarantined target.
- Deleting the orphaned seed `v1-4de4a216ec95753d`.
- Touching `~/.cache/cargo-target/fix-build-cache-leak`.
- PR #1704, issue #1679.

## Evidence log

- Seed contract: `v1-b76756ec52d50dfc`, published from `main` after `4c03aa12`.
- Real consumption: `seed: seeded reason=reflink`, workspace build 18 s, `webfang 2.4.1`.
- Incremental toggle: 0 → 1 costs one 20 s workspace rebuild; subsequent toggles free;
  669 C++ objects preserved across both settings.
- Historical target: 478 GB, 46 dead worktrees referenced by live fingerprints.
- Quarantine: `quarantine/main-shared-478g-20260930`, rename same-filesystem, 545 907 files,
  moving window delta 0 over 90 s, 0 build processes, `lsof` 0, latest mtime 327 min prior.
  Post-rename `cargo build --workspace` on `main` = 0.28 s no-op, still `webfang 2.4.1`.
- T1 also uncovered `scripts/ci_batch_branch.sh` printing the removed sed recipe; fixed in
  `77d39f08`.
