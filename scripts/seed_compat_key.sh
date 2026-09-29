#!/usr/bin/env bash
set -euo pipefail
# seed_compat_key.sh — SeedCompatibilityKey: decide whether a seed may be reused.
#
# A key answers one question: "is a seed built under this build contract reusable by
# this invocation?" It deliberately does NOT answer "is this the same source".
#
# NOT in the key, on purpose (D4 in odd/tasks/seed-target-bootstrap.md):
#   commit, branch, worktree path, workspace identity
# Those are exactly the things a seed must be able to span. Two worktrees at
# different commits on the same dependency graph MUST resolve to the same key —
# otherwise the seed is one-per-worktree and the whole mechanism buys nothing.
#
# IN the key:
#   rustc        the compiler, semantically ("1.88.0 (6b00bc38 2025-06-23)").
#                `rustc -V` already carries the commit hash, so this is exact
#                without depending on Cargo's internal hash layout.
#   target       host triple, or the explicit --target when cross-building.
#   profile      profile name (dev/release/...).
#   rustflags    RUSTFLAGS verbatim. #1267/A3 measured that a flags mismatch
#                scores zero hits AND strands orphaned build-script outputs
#                (btls-sys went 669 -> 2914 .o), so this is load-bearing.
#   incremental  CARGO_INCREMENTAL. Same reasoning, cheaper to exclude than to
#                debug later.
#   config       digest of the repo's .cargo/config.toml, or "absent". The file is
#                CWD-scoped (the cause fixed in #1678), so a seed built from
#                outside the repo can otherwise be mismatched against one built
#                inside it.
#   features     the caller's feature selection, sorted. Supplied by the
#                invocation, not read from the repo: the same worktree built
#                with --features ai is a different build contract.
#   lockfile     sha256 of Cargo.lock. Deliberately conservative — a lockfile can
#                change in ways that do not affect every unit, so this loses some
#                reuse. Losing reuse is cheap; an incompatible seed is not.
#
# Why semantic fields instead of Cargo's internal fingerprint hashes: the 2026-09-29
# audit of main's shared target found 461 distinct `profile` hashes in real use,
# because that hash varies per unit ROLE (a build script compiles for host, a lib
# for target). Using them as an external contract would yield 461 "recipes" and
# zero seeds. Measured recipe count with these semantic fields: 7, of which 2 are
# seed-relevant.
#
# Usage:
#   seed_compat_key.sh --features <comma-list> [--profile <name>] [--target <triple>]
#       prints the key on stdout, nothing else
#   seed_compat_key.sh ... --emit-manifest <dir>
#       additionally writes <dir>/manifest.toml
#   seed_compat_key.sh --verify <dir> ...
#       exits 0 if <dir>/manifest.toml matches the key computed right now,
#       3 if it does not, 1 on usage/environment error
#
# Exit codes: 0 ok · 1 usage or environment error · 3 incompatible/missing manifest.

KEY_SCHEMA=1
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SEED_REPO_ROOT:-$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)}"

usage() { sed -n '2,50p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 1; }

FEATURES=""
PROFILE="dev"
TARGET=""
EMIT=""
VERIFY=""
CONFIG_DIGEST_ARG=""
WORKSPACE_DIGEST_ARG=""
RUSTC_BIN=""
RECIPE_SCHEMA=""
WRAPPER_POLICY=""
TOOLCHAIN_ID=""
KEY_SCHEMA="${RECIPE_SCHEMA:-1}"

while [ $# -gt 0 ]; do
  case "$1" in
    --features) FEATURES="${2:-}"; shift 2 ;;
    --profile)  PROFILE="${2:-}";  shift 2 ;;
    --target)   TARGET="${2:-}";   shift 2 ;;
    --config-digest) CONFIG_DIGEST_ARG="${2:-}"; shift 2 ;;
    --workspace-digest) WORKSPACE_DIGEST_ARG="${2:-}"; shift 2 ;;
    --rustc-bin) RUSTC_BIN="${2:-}"; shift 2 ;;
    --recipe-schema) RECIPE_SCHEMA="${2:-}"; shift 2 ;;
    --wrapper-policy) WRAPPER_POLICY="${2:-}"; shift 2 ;;
    --toolchain-id)  TOOLCHAIN_ID="${2:-}"; shift 2 ;;
    --emit-manifest) EMIT="${2:-}"; shift 2 ;;
    --verify)   VERIFY="${2:-}";   shift 2 ;;
    -h|--help)  usage ;;
    *) echo "seed_compat_key.sh: unknown argument '$1'" >&2; exit 1 ;;
  esac
done

# The recipe pins the exact compiler, and the build runs that same binary, so
# the version hashed here is the version that compiles. Resolving rustc from PATH
# independently would let the two sides describe different compilers.
RUSTC_BIN="${RUSTC_BIN:-rustc}"
[ -x "$RUSTC_BIN" ] || command -v "$RUSTC_BIN" >/dev/null 2>&1 \
  || { echo "seed_compat_key.sh: no rustc at '$RUSTC_BIN'" >&2; exit 1; }

# --- the build contract, as semantic values ---------------------------------
RUSTC_V="$("$RUSTC_BIN" -V)"
if [ -n "$TARGET" ]; then
  TARGET_TRIPLE="$TARGET"
else
  TARGET_TRIPLE="$("$RUSTC_BIN" -vV | sed -n 's/^host: //p')"
fi
[ -n "$TARGET_TRIPLE" ] || { echo "seed_compat_key.sh: could not determine host triple" >&2; exit 1; }

# Sorting makes the feature list canonical: "ai,mcp" and "mcp,ai" are the same
# build contract and must not produce two seeds.
FEATURES_CANON="$(printf '%s' "$FEATURES" | tr ',' '\n' | sed '/^$/d' | LC_ALL=C sort | paste -sd, -)"

# The configuration surface is supplied by the recipe, not discovered here.
#
# This used to hash only $REPO_ROOT/.cargo/config.toml. Cargo also reads
# $CARGO_HOME/config.toml and every .cargo/config.* from the working directory
# up to the root, and any of them can set build.rustflags, build.incremental,
# target or rustc-wrapper. Hashing one file meant a change elsewhere moved the
# build and left the key alone — the silent incompatibility this whole contract
# is supposed to make impossible. seed_recipe.sh computes a conservative digest
# over the whole chain and passes it in.
if [ -z "$CONFIG_DIGEST_ARG" ]; then
  echo "seed_compat_key.sh: --config-digest is required." >&2
  echo "  the key is a pure function of the recipe; it does not go looking for" >&2
  echo "  config sources itself, because that is how the two sides drifted." >&2
  exit 1
fi
CONFIG_DIGEST="$CONFIG_DIGEST_ARG"

LOCK_DIGEST="absent"
if [ -f "$REPO_ROOT/Cargo.lock" ]; then
  LOCK_DIGEST="sha256:$(sha256sum "$REPO_ROOT/Cargo.lock" | cut -d' ' -f1)"
fi

# CARGO_INCREMENTAL as a canonical yes/no: "1" and "true" mean the same thing
# to cargo, so they must not produce two keys.
case "${CARGO_INCREMENTAL:-0}" in
  1|true|TRUE|True) INCREMENTAL="on" ;;
  *)               INCREMENTAL="off" ;;
esac

# Canonical serialisation. Field order is fixed and every value is on its own
# line; a change to this layout is a key_schema bump, not a silent re-key.
MANIFEST="$(
  cat <<EOF
key_schema = $KEY_SCHEMA

[key]
rustc = "$RUSTC_V"
target = "$TARGET_TRIPLE"
profile = "$PROFILE"
rustflags = "${RUSTFLAGS:-}"
incremental = "$INCREMENTAL"
config = "$CONFIG_DIGEST"
workspace_contract = "$WORKSPACE_DIGEST_ARG"
features = "$FEATURES_CANON"
cargo_lock = "$LOCK_DIGEST"
recipe_schema = "$RECIPE_SCHEMA"
wrapper_policy = "$WRAPPER_POLICY"
toolchain_id = "$TOOLCHAIN_ID"
EOF
)"

KEY="v${KEY_SCHEMA}-$(printf '%s' "$MANIFEST" | sha256sum | cut -c1-16)"

# --- outputs ----------------------------------------------------------------
if [ -n "$EMIT" ]; then
  mkdir -p "$EMIT"
  printf '%s\n' "$MANIFEST" > "$EMIT/manifest.toml"
  {
    echo ""
    echo "[provenance]"
    echo "# Informational only. Deliberately NOT part of [key]: a seed is meant to"
    echo "# be shared across commits, so the source that built it is a fact about"
    echo "# the seed, not a compatibility input."
    echo "built_from_commit = \"$(git -C "$REPO_ROOT" rev-parse HEAD)\""
    echo "built_at = \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\""
  } >> "$EMIT/manifest.toml"
fi

if [ -n "$VERIFY" ]; then
  M="$VERIFY/manifest.toml"
  if [ ! -f "$M" ]; then
    echo "seed_compat_key.sh: no manifest at $M" >&2
    exit 3
  fi
  WANT="$(printf '%s' "$MANIFEST" | sed -n '/^\[key\]/,/^$/p' | grep -v '^\[provenance\]')"
  HAVE="$(sed -n '/^\[key\]/,/^$/p' "$M")"
  if [ "$WANT" = "$HAVE" ]; then
    echo "$KEY"
    exit 0
  fi
  echo "seed_compat_key.sh: $VERIFY is not compatible with this build contract" >&2
  diff <(printf '%s\n' "$HAVE") <(printf '%s\n' "$WANT") >&2 || true
  exit 3
fi

printf '%s\n' "$KEY"
