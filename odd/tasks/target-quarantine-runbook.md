# Runbook: reclamation of quarantined target dirs

**Status: the deletion in section 8 is NOT authorised and was NOT performed.** This
document defines the procedure so that a later, explicitly authorised step has
something to follow. Section 9 records the current quarantine state.

Scope note: `cargo clean` is deliberately not used anywhere in this procedure. It
removes artifacts from a target dir, and these target dirs are per-tree; cleaning one
that another worktree still shares would reach outside its own scope. Every step here
operates on a directory that has already been proven unreferenced.

---

## 1. Three categories, three different authorities

Do not merge these into one GC policy. Each has a different notion of "obsolete" and
a different party entitled to declare it.

| Category | Obsolete means | Authority | Location |
| :--- | :--- | :--- | :--- |
| **A. Seeds** | A `SeedCompatibilityKey` no tree computes any more | The key itself — self-describing | `~/.cache/cargo-target/seeds/v*` |
| **B. Retired main target** | No build path can name it, proven by a rename | Ownership + observation window | `~/.cache/cargo-target/quarantine/` |
| **C. Worktree targets** | The worktree no longer exists | Worktree list + ownership | `~/.cache/cargo-target/<tree-name>` |

An incompatible seed is **not** garbage. A consumer that finds no matching seed gets
`cold` and builds normally — that is a correct, designed outcome, not a leak. Seeds
reclaimed under category A must be reclaimed by an explicit seed policy, never as a
side effect of cleaning targets.

---

## 2. Identify the candidate exactly

A target dir candidate MUST be identified by **full canonical path**, never by a
prefix match and never by a name found by enumeration.

```bash
realpath -m "$CANDIDATE"     # resolves .. and symlinks
stat -f -c %d "$CANDIDATE"   # device id: proves same-filesystem for a rename
```

Enumeration is how a deletion once removed a live mission's 24 GB cache. The
filesystem never names a worktree's target — agents choose those paths freely, one
worktree may accumulate several, and the absence of a branch is *anti-correlated*
with being dead, because work not yet pushed has no branch anywhere by definition.

The only admissible origin for a category-B path is a path this repository's own
`.envrc` used to declare, or one this runbook has already quarantined.

---

## 3. Prove no worktree owns it

```bash
git worktree list
```

Every path appearing there is alive. A target dir referenced by a live worktree is
not a candidate, full stop.

The historical main target held **46 dead worktrees** referenced by live fingerprints
plus one still-running worktree. Fingerprints outlive the checkouts that produced
them, so a target dir's contents are evidence of nothing on their own.

---

## 4. Prove `main` no longer writes there

```bash
grep '^export CARGO_TARGET_DIR=' .envrc
```

`main` must declare a target of its own. Since the migration, it declares
`~/.cache/cargo-target/main`, and every tree — `main` included — has its own dir; the
shared target that made this undecidable no longer exists.

---

## 5. Observation window, with a moving test

A fixed "no writes for 30 minutes" test is **not sufficient**: it cannot distinguish a
finished build from an agent thinking between builds. The test is a *moving* window.

```bash
A=$(find "$D" -type f 2>/dev/null | wc -l); sleep 90
B=$(find "$D" -type f 2>/dev/null | wc -l)
[ "$A" -eq "$B" ]   # delta 0 required
```

Then, separately, process and open-file evidence:

```bash
for p in cargo rustc cargo-nextest sccache; do pgrep -x "$p"; done   # one name per call
lsof +D "$D" 2>/dev/null | grep -c "$D"                               # must be 0
find /proc/[0-9]*/cwd /proc/[0-9]*/exe -lname "$D*" 2>/dev/null
```

`pgrep -x 'a;b'` never matches — pass one name per call.

---

## 6. Quarantine by rename, same filesystem

```bash
Q="$HOME/.cache/cargo-target/quarantine"
ID="<owner>-<what>-<size>-$(date +%Y%m%d)"
[ "$(stat -f -c %d "$D")" = "$(stat -f -c %d "$Q")" ] || exit 1   # else it is a copy
mv "$D" "$Q/$ID"
```

A same-filesystem rename is a metadata operation: the 478 GB quarantine took
effectively no time and copied nothing. The original path immediately stops resolving,
which is what takes the directory out of the build system — and the contents remain
recoverable by renaming back.

**After the rename, rebuild the owning tree and confirm it still works.** That is the
proof the directory was not on any live path. A no-op build (measured: 0.28 s) is the
strongest form of that evidence.

---

## 7. What to record before any deletion

- canonical path, size, file count, and the observation window used;
- the `git worktree list` output showing no owner;
- the `lsof` / `/proc` result and the process check;
- the post-rename rebuild result;
- the date and the identity that authorised the quarantine.

---

## 8. Deletion — NOT AUTHORISED, NOT PERFORMED

Deletion becomes a separate decision, taken later, against a specific `--yes`
authorisation from the maintainer. It requires **all** of:

1. category confirmed (A, B or C) and the right authority invoked;
2. observation window satisfied, re-verified at deletion time — not carried over from
   the quarantine step;
3. ownership re-proven immediately before the removal;
4. explicit human authorisation naming the path;
5. a known restore path, which for category B is a rename back from `quarantine/`.

Abort conditions, any one of which stops the deletion:

- a live worktree appears in `git worktree list` for the path;
- any `cargo`/`rustc`/`cargo-nextest` process is live;
- `lsof` reports an open file under the path;
- the file count changes between the two samples;
- the maintainer has not authorised this specific path.

Expected result of aborting: nothing is removed. The directory stays in `quarantine/`.

---

## 9. Current quarantine state

| Path | Size | Quarantined | Authorised to delete |
| :--- | --- | --- | :--- |
| `~/.cache/cargo-target/quarantine/main-shared-478g-20260930` | 478 G apparent | 2026-09-30 | **No** |

Evidence at quarantine time: 545 907 files; moving window delta 0 over 90 s; zero
`cargo`/`rustc`/`cargo-nextest`/`sccache`; `lsof` 0; latest mtime 327 min before, the
tail of the migration's own builds. Post-rename `cargo build --workspace` on `main`
completed as a 0.28 s no-op and still reported `webfang 2.4.1`.

Restore path: `mv ~/.cache/cargo-target/quarantine/main-shared-478g-20260930
~/.cache/cargo-target/webfang` and restore the old `.envrc`. Nothing else references
that path any more.

### Not quarantined, deliberately

| Path | Size | Why it is still there |
| :--- | --- | :--- |
| `~/.cache/cargo-target/seeds/v1-4de4a216ec95753d` | 2.2 G | Category A. An incompatible seed is a correct `cold`, not garbage. Reclaiming it is an explicit seed policy. |
| `~/.cache/cargo-target/seeds/v1-b76756ec52d50dfc` | 2.2 G | Category A, orphaned when the key derivation changed. Same rule. |
| `~/.cache/cargo-target/seeds/v1-dfde71780c07a662` | 2.2 G | **Live.** The current key. |
| `~/.cache/cargo-target/fix-build-cache-leak` | 7.1 G | Category C, another mission's. Not ours to attribute. |

Note on the seeds: their contents are intentionally read-only, so `rm -rf` fails with
`EACCES` until permissions are restored. That is the protection working, not a fault.

---

## 10. Upstream context

Cargo is working toward a cross-workspace build cache with a central, granularly
managed location. Until that lands and stabilises, this policy should stay
conservative: a false deletion costs a rebuild, a false retention costs disk, and only
one of those two is recoverable by a person.
