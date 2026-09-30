# Runbook: reclamation of build seeds

Companion to `target-quarantine-runbook.md`, category **A**. The two are separate
because the objects are genuinely different: a target dir holds state whose usefulness
cannot be established without proving ownership, and a seed is a *cache* whose
incompatibility is self-describing.

**The deletion in section 6 is NOT authorised and was NOT performed.** Section 7 records
the state after the first classification pass.

---

## 1. Why a seed needs no analysis

A target dir cannot be declared obsolete by inspecting it: 545 907 files, dozens of
generations, no reliable way to say which are still needed. A seed can. Its identity is
the `SeedCompatibilityKey`, and a consumer whose key finds no matching seed does not
fail — it gets `cold` and builds normally, which is a designed outcome.

That asymmetry is the whole reason this policy can be mechanical where the target
policy cannot. It is also why "a seed directory exists" must never mean "keep it
forever": **a seed is not a source of functional truth, it is optimised cache.**

## 2. States

| State | Meaning |
| :--- | :--- |
| `ACTIVE` | Its key is the one the current recipe computes. Never a candidate. |
| `RETAINED` | Deliberately kept. Its key no longer matches, but a near neighbour absorbs a recipe change without a cold build. |
| `QUARANTINED` | Out of normal lookup, physically intact, reversible by rename. |
| `DELETED` | Reclaimed. |

The transition is driven by **key identity, not age**. "Older than N days" is the wrong
test here: a recipe change today can orphan a seed published an hour ago, and a
long-lived toolchain can leave a day-old seed perfectly current.

## 3. Retention rule

```
ACTIVE          the seed whose key == the key the current recipe computes
RETAINED        at most ONE predecessor of the active key
QUARANTINE      every other seed
DELETE          only after the quarantine window
```

One predecessor, not several. Its purpose is to absorb an immediate recipe change — a
toolchain bump, a lock update — without a cold build. Beyond one, the older seeds
describe contracts nothing in the tree produces, and they cost 2.2 G each.

The predecessor is identified by the **most recent `built_at` in its manifest that is
not the active key**, never by directory name or mtime.

## 4. Classification procedure

```bash
source scripts/seed_recipe.sh
seed_recipe_parse "$PWD" --features ""; seed_recipe_compute_key "$PWD"   # the active key

for s in ~/.cache/cargo-target/seeds/*/; do
  sed -n 's/^built_at = "\(.*\)"/\1/p' "$s/manifest.toml"
done | sort
```

A seed with **no readable `manifest.toml`** is a damaged seed, not a current one. It
cannot be classified by identity at all, so it goes straight to `QUARANTINED`; the
name in its directory is a claim, not evidence. Verify before trusting it:

```bash
bash scripts/seed_compat_key.sh "${KEY_ARGS[@]}" --verify "$SEED"   # exit 3 = incompatible
```

## 5. Quarantine by rename, never `rm`

Seed contents are intentionally read-only, so `rm -rf` fails with `EACCES` until
permissions are restored. That protection is working, and it is also why quarantine
happens by rename:

```bash
Q=~/.cache/cargo-target/quarantine/seeds
[ "$(stat -f -c %d "$SEEDS")" = "$(stat -f -c %d "$Q")" ] || exit 1
mkdir -p "$Q"
mv "$SEEDS/$KEY" "$Q/$KEY.$(date +%Y%m%d)"
```

A same-filesystem rename is metadata only — no blocks copied — and it takes the seed
out of lookup immediately while leaving it recoverable. Confirm the active seed still
resolves and that a fresh worktree still reports `seeded` afterwards.

## 6. Deletion — NOT AUTHORISED, NOT PERFORMED

Separate decision, separate authorisation, naming specific keys. Requires all of:

1. the seed has been `QUARANTINED` for the observation window;
2. it is neither `ACTIVE` nor the retained predecessor;
3. a recomputed key confirms nothing in the tree produces it;
4. explicit human authorisation naming the path;
5. a restore path, which for a quarantined seed is a rename back into `seeds/`.

Abort conditions: the seed's key starts matching again; it becomes the only copy of a
contract some tree still needs; a worktree build reports `cold` right after the rename,
which would mean the classification was wrong.

Permissions must be restored before removal, deliberately and audibly:

```bash
chmod -R u+w "$Q/$KEY" && rm -rf "$Q/$KEY"
```

## 6b. Minimum retention â the eligibility gate

`minimum_quarantine_age` for a quarantined seed is **48 hours** from
`quarantine_started_at`. Full rationale and the two-object table live in
`target-quarantine-runbook.md` §8; same gate, shorter window, because a seed is
rebuildable cache and a target dir is not.

The gate is necessary and not sufficient: it makes a deletion *decidable*, and fresh
ownership/use evidence collected at the moment of deletion is what actually decides
it. Age never substitutes for that evidence, and an old `built_at` is not the gate —
a seed published an hour ago can be quarantined today.

| Seed | `quarantine_started_at` | Minimum | Eligible from |
| :--- | :--- | :--- | :--- |
| `v1-b76756ec52d50dfc.20260930` | 2026-09-30 01:45 | 48 h | **2026-10-02 01:45** |
| `v1-4de4a216ec95753d.20260930` | 2026-09-30 01:45 | 48 h | **2026-10-02 01:45** |

## 7. Current state after the first pass

| Seed | `built_at` | State | Evidence |
| :--- | :--- | :--- | :--- |
| `v1-da8aa82dd29c4018` | 2026-09-30T00:38:46Z | `ACTIVE` | key computed by the current recipe from `9252e314` |
| `v1-dfde71780c07a662` | 2026-09-29T23:58:08Z | `RETAINED` | most recent predecessor, absorbs a recipe change |
| `v1-b76756ec52d50dfc` | — | `QUARANTINED` | **no readable manifest**: 7 300 of ~7 303 files remain after a partial `rm -rf` that failed on read-only permissions. `verify` → exit 3. |
| `v1-4de4a216ec95753d` | 2026-09-29T20:41:19Z | `QUARANTINED` | oldest; key no longer produced by any tree |

The damaged seed is the reason the rule is written the way it is. Its directory name
still said `v1-b767…`, and taken at face value it would have been filed as a normal
predecessor. It is not one: a seed you cannot read its identity from is a seed you
cannot reason about, and that is a quarantine candidate regardless of what its name
claims.

## 8. Steady state

A dependency change must not become a disk-administration problem:

```
new Cargo.lock / toolchain / recipe change
    → new key
    → publish a new seed           (ACTIVE)
    → previous ACTIVE becomes the retained predecessor
    → the seed that held RETAINED is quarantined
```

At most three seeds exist at any time — one active, one retained, one mid-quarantine —
so the store stays bounded without anyone scheduling a cleanup.
