#!/usr/bin/env bash
set -euo pipefail
# seed_publish.sh — build a seed and publish it as an immutable, portable target.
#
# Deliberate and explicit (D3 in odd/tasks/seed-target-bootstrap.md). Publishing is
# NOT automatic on the consumer path: if every worktree published whenever it
# failed to find a seed, the key space would fan back out and concurrent worktrees
# would race to write the same directory. The consumer (seed_target.sh) only reads.
#
# The seed's contract, and why it is not just "third-party crates":
#
#   included  build units whose fingerprints carry no workspace path
#   excluded  workspace members, any path-dependent unit, anything whose
#             fingerprint embeds an absolute path into this checkout
#
# "Third-party" is NOT the same as "portable". A registry dependency can have a
# build script or otherwise capture local paths, and the 2026-09-29 audit found
# exactly that: `ring`'s run-build-script fingerprint referenced
# `/home/xavi/Projects/webfang-worktrees/fix-ai-resume-duplicate-chunks/...`,
# a worktree that no longer exists. So the prune is by OBSERVED PATH EMBEDDING,
# with workspace members pruned unconditionally as belt-and-braces. That catches
# local dev-dependencies and path overrides that a name-based rule would miss.
#
# Pruning is what makes contamination impossible. Without it, seeding produces a
# wrong binary with exit 0 when the destination's sources are older than the
# seed's artifacts — Cargo decides freshness by mtime, and a CoW clone preserves
# mtimes. See scripts/test_seed_contamination.sh.
#
# Usage:
#   seed_publish.sh [--features <list>] [--profile <name>] [--target <triple>]
#                   [--seeds-root <dir>] [--force]
#
# Arbitrary cargo arguments after `--` are REFUSED, not passed through. An
# argument that changes which units get compiled has to be part of the recipe,
# or it changes the build without changing the SeedCompatibilityKey that is
# supposed to describe it. See scripts/seed_recipe.sh.
#
# Exit: 0 published · 1 usage/build error · 2 refused (seed exists, use --force)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SEED_REPO_ROOT:-$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)}"
# shellcheck source=scripts/seed_recipe.sh
. "$SCRIPT_DIR/seed_recipe.sh"
seed_recipe_parse "$REPO_ROOT" "$@"
# The key's arguments come from the recipe, through the same helper the consumer
# and the tests use. Assembling them here as well was a second, hand-maintained
# list of what the key means — and it had already drifted once, publishing a
# manifest with an empty workspace_contract.
mapfile -t KEY_ARGS < <(seed_recipe_key_args)
SEEDS_ROOT="$SEED_RECIPE_SEEDS_ROOT"
FORCE="$SEED_RECIPE_FORCE"
TEST_TX_DIR="$SEED_RECIPE_TEST_TX"
FAIL_AT="$SEED_RECIPE_FAIL_AT"
TEST_TX=0
[ -n "$SEED_RECIPE_TEST_TX" ] && TEST_TX=1

command -v cargo >/dev/null || { echo "seed_publish.sh: cargo not on PATH" >&2; exit 1; }
command -v jq    >/dev/null || { echo "seed_publish.sh: jq is required" >&2; exit 1; }
export SEED_REPO_ROOT="$REPO_ROOT"


# P1: the producer carries the SAME build-dir refusal the consumer has. It did
# not, and the gap was not cosmetic: build-dir is not in the key, so a seed
# published under a build-dir configuration could enter the store under a key
# that a consumer without one would accept. Shared implementation, so the two
# sides cannot drift apart again.
if seed_recipe_has_config_include "$REPO_ROOT"; then
  echo "seed_publish.sh: refusing to publish - a cargo config uses \`include\`." >&2
  echo "  the digest covers the config files cargo reads, not the files they" >&2
  echo "  pull in, so an included file could change the build without changing" >&2
  echo "  the key. Refused rather than guessed at." >&2
  exit 2
fi

if seed_recipe_has_build_dir "$REPO_ROOT"; then
  echo "seed_publish.sh: refusing to publish - build-dir is configured." >&2
  echo "  the consumer refuses to seed for the same reason, so publishing here" >&2
  echo "  would put a seed in the store that nobody is allowed to use." >&2
  exit 2
fi

seed_recipe_compute_key "$REPO_ROOT"
DEST="$SEEDS_ROOT/$SEED_RECIPE_KEY"
echo "==> key $SEED_RECIPE_KEY  (profile=$SEED_RECIPE_PROFILE target=${SEED_RECIPE_TARGET:-host} features=${SEED_RECIPE_FEATURES_CANON:-none} incremental=$SEED_RECIPE_INCREMENTAL toolchain=${SEED_RECIPE_TOOLCHAIN:-ambient})"

if [ -e "$DEST" ] && [ "$FORCE" -ne 1 ]; then
  echo "seed_publish.sh: a seed already exists at $DEST" >&2
  echo "  seeds are immutable after publication (D6). Pass --force to rebuild it." >&2
  exit 2
fi

# --- publish_atomically ------------------------------------------------------
# The whole publish transaction, as one unit. The seed must be atomic FROM THE
# CONSUMER'S POINT OF VIEW: a consumer resolves seeds/<key> and must find either
# nothing or a complete, self-describing, immutable seed. Never a partial one.
#
#   write manifest → validate manifest → mark read-only → rename (LAST)
#
# Everything a consumer could ever observe happens after the single rename.
publish_atomically() {
  local ref="$1" dest="$2" key="$3"   # profile comes from SEED_RECIPE_PROFILE

  [ "$FAIL_AT" = "before-manifest" ] && return 42

  # The manifest is part of the published OBJECT, so it is written and validated
  # while the seed is still invisible under .staging.*.
  #
  # Publishing first and writing the manifest afterwards was a real defect: the
  # seed became visible with no manifest inside it, and a process death in that
  # window left a PERMANENT, unmanifested seed squatting the key. Every later
  # consumer then failed verification and went cold, forever, with no recovery
  # short of deleting the directory by hand.
  bash "$SCRIPT_DIR/seed_compat_key.sh" "${KEY_ARGS[@]}" \
    --emit-manifest "$ref" >/dev/null
  bash "$SCRIPT_DIR/seed_compat_key.sh" "${KEY_ARGS[@]}" \
    --verify "$ref" >/dev/null || {
      echo "seed_publish.sh: staged seed failed its own manifest check; not publishing" >&2
      return 1
    }

  [ "$FAIL_AT" = "after-manifest" ] && return 42

  # Read-only BEFORE the rename, so the published reference has no observable
  # window in which it is mutable. This is the WEAK defence — the agent runs as
  # the same user and can chmod it back. The real control is the ci_fast_gate.sh
  # rule refusing any CARGO_TARGET_DIR under the seeds root (D6). This only makes
  # an accidental write fail loudly instead of silently poisoning the reference.
  #
  # What this does NOT claim, stated plainly so nobody later reads more into it:
  # the published seed is not filesystem-immutable. The top directory is left
  # writable, both because the rename requires it (see below) and because anyone
  # with write on the seeds root can unlink entries whose own mode is a-w —
  # unlinking is a parent-directory operation, not a file one. What the a-w bits
  # actually buy is that an accidental WRITE fails loudly. The authoritative
  # protection is the guard refusing seeds/<key> as a build target; the bits are
  # only there so a mistake is noisy instead of silent.
  #
  # `chmod u+w` on the top directory alone is REQUIRED, and the reason is
  # filesystem behaviour rather than principle: rename() can return EACCES when
  # the directory being renamed does not permit the `..` update, so the parent's
  # permissions are not the whole story. Measured on the real cache filesystem:
  # top 555 -> EACCES, top 755 -> renamed. Every path INSIDE stays a-w, which is
  # what the write-protection claim is about; the top entry is what has to be
  # movable. cargo needs it writable in the clone regardless (seed_target.sh
  # passes --no-preserve=mode for exactly that).
  chmod -R a-w "$ref" 2>/dev/null || echo "seed_publish.sh: warning: could not mark the seed read-only" >&2
  chmod u+w "$ref"

  [ "$FAIL_AT" = "before-rename" ] && return 42

  # The new seed is complete and validated above, so the old one is only now
  # disturbed. `mv SRC DEST` where DEST is an existing non-empty directory would
  # move SRC *inside* it, so the old seed is renamed aside first — into the same
  # SEEDS_ROOT, hence the same filesystem, hence a rename and not a copy.
  #
  # `--force` leaves a sub-millisecond no-seed window between the two renames.
  # That is accepted deliberately: it exists only under an explicit --force, the
  # replacement is already durable at that point, and the alternative — deleting
  # the only valid seed before building its successor — is what this ordering
  # exists to prevent.
  if [ -e "$dest" ]; then
    # The key goes IN the retired name. Two renames cannot be made atomic with
    # each other, so a crash between them leaves the key absent; without the key
    # in the name, that backup is an anonymous 2.2 G directory nobody can safely
    # attribute. With it, seed_recover.sh turns recovery into one rename.
    local retired="$SEEDS_ROOT/.retired.$key.$$"
    # Published seeds are read-only, and unlinking needs write permission on the
    # CONTAINING directory; chmod -R is belt-and-braces for manual cleanup.
    chmod -R u+w "$dest" 2>/dev/null || true
    mv "$dest" "$retired"
    if ! mv "$ref" "$dest"; then
      mv "$retired" "$dest" 2>/dev/null || true   # put the valid seed back
      echo "seed_publish.sh: publish failed; previous seed restored" >&2
      return 1
    fi
    rm -rf "$retired" 2>/dev/null || true
  else
    mv "$ref" "$dest"
  fi
}

if [ "$TEST_TX" -eq 1 ]; then
  # Test seam: no build, no prune, no self-check — just the transaction.
  [ -d "$TEST_TX_DIR" ] || { echo "seed_publish.sh: --test-transaction needs an existing directory" >&2; exit 1; }
  mkdir -p "$SEEDS_ROOT"
  publish_atomically "$TEST_TX_DIR" "$DEST" "$SEED_RECIPE_KEY"
  exit $?
fi


# Stage on the SAME filesystem as the destination. Staging in /tmp would put a
# full workspace target (3+ G, BoringSSL included) on a 16 G tmpfs, and the
# final move would degrade from an atomic rename to a 3+ G cross-device copy.
mkdir -p "$SEEDS_ROOT"
STAGE="$(mktemp -d "$SEEDS_ROOT/.staging.XXXXXX")"
# The staged tree is marked read-only before publication, so a plain `rm` cannot
# unlink it: it needs write permission on every entry. Without this, a failed run
# leaves a ~2.2 G read-only orphan under the seeds root — which is exactly what
# happened the first time this transaction was reordered.
cleanup() { chmod -R u+w "$STAGE" 2>/dev/null || true; rm -rf "$STAGE" 2>/dev/null || true; }
trap cleanup EXIT

REF="$STAGE/target"
# The isolated recipe from AGENTS.md. It must match what consumers use, or the
# seed is a flags mismatch: A3 measured zero hits and orphaned build-script
# outputs from exactly that.
echo "==> building the reference (cold; includes BoringSSL's C++)..."
  # The build runs the SAME RECIPE the key was computed from: same profile,
  # same target, same features, same flags, same pinned toolchain. The key used
  # to be derived from the caller's ambient environment while the build ran under
  # a different one, so --profile / --target / --features could change the key
  # without ever reaching cargo.
  mapfile -t CARGO_ARGS < <(seed_recipe_cargo_args)
  mapfile -t BUILD_ENV  < <(seed_recipe_build_env "$REF")
  echo "    recipe: ${CARGO_ARGS[*]}"
  ( cd "$REPO_ROOT" \
    && env "${BUILD_ENV[@]}" \
         "$SEED_RECIPE_CARGO_BIN" "${CARGO_ARGS[@]}" \
  ) >"$STAGE/build.log" 2>&1 \
  || { echo "seed_publish.sh: reference build failed" >&2; tail -20 "$STAGE/build.log" >&2; exit 1; }

BEFORE_O="$(find "$REF" -name '*.o' 2>/dev/null | wc -l)"
echo "==> reference built ($(du -sh --apparent-size "$REF" | cut -f1) apparent, $BEFORE_O objects)"

# --- prune: every unit whose fingerprint embeds this checkout ---------------
MEMBERS="$(cd "$REPO_ROOT" && cargo metadata --no-deps --format-version 1 --offline 2>/dev/null \
  | jq -r '.packages[].name')"
# Workspace TARGET names too, not just package names: a bin named `bench_live`
# or `webfang_mcp_stdio` would otherwise survive the prune, and so would cargo's
# uplifted debug/lib<pkg>.rlib copies, which carry the workspace path through
# debug info. Both are workspace products, never seed material.
TARGET_NAMES="$(cd "$REPO_ROOT" && cargo metadata --no-deps --format-version 1 --offline 2>/dev/null \
  | jq -r '.packages[].targets[].name')"
PRUNE_NAMES="$(printf '%s\n%s\n' "$MEMBERS" "$TARGET_NAMES" | sed '/^$/d' | LC_ALL=C sort -u)"
# Cargo writes a dashed target name (`webfang-mcp-stdio`) uplifted in debug/ but
# underscored in debug/deps/ (`webfang_mcp_stdio-<hash>`). Globbing only one
# spelling leaves the other behind — caught by the path-free self-check below.
# tr '-' '_', NOT tr -d '-': deleting the dashes yields `webfangmcpstdio`,
# which matches nothing.
PRUNE_NAMES="$PRUNE_NAMES
$(printf '%s\n' "$PRUNE_NAMES" | tr '-' '_')"
FP="$REF/debug/.fingerprint"
[ -d "$FP" ] || { echo "seed_publish.sh: no debug/.fingerprint in $REF" >&2; exit 1; }

PRUNED=0
PRUNED_MEMBERS=0
while IFS= read -r dir; do
  [ -d "$dir" ] || continue
  name="$(basename "$dir")"
  pkg="${name%-*}"
  hash="${name##*-}"
  is_member=0
  if printf '%s\n' "$MEMBERS" | grep -qx -- "$pkg"; then is_member=1; fi
  # grep -a: fingerprint files contain binary bytes, and without -a grep
  # silently skips them — a mistake that undercounted dead worktree references
  # by 6x during the audit.
  embeds=0
  if grep -aq -- "$REPO_ROOT" "$dir"/* 2>/dev/null; then embeds=1; fi
  if [ "$is_member" -eq 1 ] || [ "$embeds" -eq 1 ]; then
    rm -rf "${dir:?}"
    rm -rf "${REF:?}/debug/build/${pkg}-${hash}"*
    rm -f  "${REF:?}/debug/deps/lib${pkg}-${hash}".* \
           "${REF:?}/debug/deps/${pkg}-${hash}".* 2>/dev/null || true
    PRUNED=$((PRUNED + 1))
    if [ "$is_member" -eq 1 ]; then PRUNED_MEMBERS=$((PRUNED_MEMBERS + 1)); fi
  fi
done < <(find "$FP" -mindepth 1 -maxdepth 1 -type d)

# Final binaries are workspace products, never seed material. Match on the
# 16-hex hash suffix so a package whose name is a prefix of another cannot be
# caught by the glob.
for n in $PRUNE_NAMES; do
  rm -f  "$REF/debug/$n" "$REF/debug/$n.d" "$REF/debug/lib$n".* 2>/dev/null || true
  rm -f  "$REF/debug/deps/$n-"[0-9a-f]* "$REF/debug/deps/lib$n-"[0-9a-f]* 2>/dev/null || true
done

AFTER_O="$(find "$REF" -name '*.o' 2>/dev/null | wc -l)"
echo "==> pruned $PRUNED unit dirs ($PRUNED_MEMBERS of them workspace members)"
echo "    C++ objects kept: $AFTER_O (of $BEFORE_O)"
if [ "$AFTER_O" -ne "$BEFORE_O" ]; then
  echo "    note: build-script output was pruned; that unit is registry-path" >&2
  echo "    embedding and the consumer will rebuild it (a full BoringSSL rebuild)." >&2
fi

LEFT=0
for u in $MEMBERS; do
  n=$(find "$FP" -maxdepth 1 -name "${u}-*" 2>/dev/null | wc -l)
  LEFT=$((LEFT + n))
done

if [ "$LEFT" -ne 0 ]; then
  echo "seed_publish.sh: refusing to publish, $LEFT workspace fingerprints survived the prune" >&2
  exit 1
fi

# The general form of the same contract: nothing in the seed may still point at
# this checkout. The fingerprint check above only sees .fingerprint; this one
# sees the whole tree, which is how the 24 stragglers were found the first time
# (uplifted rlibs and differently-named bins).
# `|| true` is load-bearing: grep exits 1 when it finds nothing, which is the
# SUCCESS case here, and under `set -o pipefail` that would abort the script.
STRAY="$( { grep -ral "$REPO_ROOT" "$REF" 2>/dev/null || true; } | wc -l)"
if [ "$STRAY" -ne 0 ]; then
  echo "seed_publish.sh: refusing to publish, $STRAY file(s) still reference this checkout:" >&2
  grep -ral "$REPO_ROOT" "$REF" 2>/dev/null | head -10 | sed "s#$REF/#    #" >&2
  echo "  a seed must be path-free; anything still naming the build tree is a" >&2
  echo "  candidate for cross-worktree contamination." >&2
  exit 1
fi

publish_atomically "$REF" "$DEST" "$SEED_RECIPE_KEY"

echo "==> published $DEST"
echo "    apparent: $(du -sh --apparent-size "$DEST" | cut -f1)   units kept: $(find "$DEST/debug/.fingerprint" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)"
echo "    manifest: $DEST/manifest.toml"
