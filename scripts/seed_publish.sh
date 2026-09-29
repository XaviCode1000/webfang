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
#                   [--seeds-root <dir>] [--force] [-- <extra cargo args>]
#
# Exit: 0 published · 1 usage/build error · 2 refused (seed exists, use --force)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SEED_REPO_ROOT:-$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)}"
SEEDS_ROOT="${SEED_ROOT:-$HOME/.cache/cargo-target/seeds}"
KEY_ARGS=(--features "")
PROFILE="dev"
TARGET=""
FORCE=0
CARGO_EXTRA=()

while [ $# -gt 0 ]; do
  case "$1" in
    --features) KEY_ARGS=(--features "${2:-}"); shift 2 ;;
    --profile)  PROFILE="${2:-}"; shift 2 ;;
    --target)   TARGET="${2:-}"; shift 2 ;;
    --seeds-root) SEEDS_ROOT="${2:-}"; shift 2 ;;
    --force)    FORCE=1; shift ;;
    --) shift; CARGO_EXTRA=("$@"); break ;;
    *) echo "seed_publish.sh: unknown argument '$1'" >&2; exit 1 ;;
  esac
done

command -v cargo >/dev/null || { echo "seed_publish.sh: cargo not on PATH" >&2; exit 1; }
command -v jq    >/dev/null || { echo "seed_publish.sh: jq is required" >&2; exit 1; }
export SEED_REPO_ROOT="$REPO_ROOT"
if [ -n "$TARGET" ]; then KEY_ARGS+=(--target "$TARGET"); fi

KEY="$(bash "$SCRIPT_DIR/seed_compat_key.sh" "${KEY_ARGS[@]}" --profile "$PROFILE")"
DEST="$SEEDS_ROOT/$KEY"

if [ -e "$DEST" ] && [ "$FORCE" -ne 1 ]; then
  echo "seed_publish.sh: a seed already exists at $DEST" >&2
  echo "  seeds are immutable after publication (D6). Pass --force to rebuild it." >&2
  exit 2
fi

# Stage on the SAME filesystem as the destination. Staging in /tmp would put a
# full workspace target (3+ G, BoringSSL included) on a 16 G tmpfs, and the
# final move would degrade from an atomic rename to a 3+ G cross-device copy.
mkdir -p "$SEEDS_ROOT"
STAGE="$(mktemp -d "$SEEDS_ROOT/.staging.XXXXXX")"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

REF="$STAGE/target"
# The isolated recipe from AGENTS.md. It must match what consumers use, or the
# seed is a flags mismatch: A3 measured zero hits and orphaned build-script
# outputs from exactly that.
echo "==> building the reference (cold; includes BoringSSL's C++)..."
( cd "$REPO_ROOT" \
  && env -u RUSTC_WRAPPER -u RUSTUP_TOOLCHAIN -u SCCACHE_DIR -u SCCACHE_BASEDIRS \
       CARGO_TARGET_DIR="$REF" \
       CARGO_INCREMENTAL=0 \
       RUSTFLAGS="${RUSTFLAGS:-}" \
       cargo build --workspace --offline ${CARGO_EXTRA[@]+"${CARGO_EXTRA[@]}"} \
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

# --- publish ----------------------------------------------------------------
if [ -e "$DEST" ]; then
  # Published seeds are marked read-only, and `rm` needs write permission on
  # the CONTAINING directory to unlink. Without this, --force (and any manual
  # cleanup) fails with a wall of "Permission denied" on a 3 G tree.
  chmod -R u+w "$DEST" 2>/dev/null || true
  rm -rf "$DEST"
fi
mv "$REF" "$DEST"
bash "$SCRIPT_DIR/seed_compat_key.sh" "${KEY_ARGS[@]}" --profile "$PROFILE" \
  --emit-manifest "$DEST" >/dev/null

# Read-only is the WEAK defence: the agent runs as the same user and can chmod
# it back. The real control is the ci_fast_gate.sh rule that refuses any
# CARGO_TARGET_DIR under the seeds root (D6). This only makes an accidental
# write fail loudly instead of silently poisoning the reference.
chmod -R a-w "$DEST" 2>/dev/null || echo "seed_publish.sh: warning: could not mark the seed read-only" >&2

echo "==> published $DEST"
echo "    apparent: $(du -sh --apparent-size "$DEST" | cut -f1)   units kept: $(find "$DEST/debug/.fingerprint" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)"
echo "    manifest: $DEST/manifest.toml"
