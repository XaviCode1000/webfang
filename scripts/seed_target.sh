#!/usr/bin/env bash
set -euo pipefail
# seed_target.sh — prepare a worktree's CARGO_TARGET_DIR from a seed, if one fits.
#
# This is the consumer half. It never publishes and never writes into a seed.
# Its job is narrow: decide `seeded` or `cold`, do it, and say which and why.
#
# Failure is never fatal. Every path that cannot seed ends in a cold build, and
# says so on stdout in a single greppable line:
#
#   seed: cold  reason=no-seed key=v1-xxxx target=/path
#   seed: cold  reason=reflink-unavailable key=v1-xxxx target=/path
#   seed: cold  reason=incompatible-manifest key=v1-xxxx target=/path
#   seed: seeded reason=reflink key=v1-xxxx target=/path bytes=<n>
#
# Rationale for logging rather than staying silent: a seed that quietly stops
# matching (a toolchain bump, a new flag) degrades to a 2 m 23 s cold build with
# no symptom. One line per run makes that visible instead of mysterious.
#
# Usage:
#   seed_target.sh [--features <list>] [--profile <n>] [--target <triple>]
#                  [--seeds-root <dir>] [--target-dir <path>]
#
# --target-dir defaults to $CARGO_TARGET_DIR.
# Exit: 0 seeded or cold (both fine) · 1 usage error · 2 refused to touch a seed

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${SEED_REPO_ROOT:-$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)}"
SEEDS_ROOT="${SEED_ROOT:-$HOME/.cache/cargo-target/seeds}"
KEY_ARGS=(--features "")
PROFILE="dev"
TARGET=""
TARGET_DIR="${CARGO_TARGET_DIR:-}"

while [ $# -gt 0 ]; do
  case "$1" in
    --features)   KEY_ARGS=(--features "${2:-}"); shift 2 ;;
    --profile)    PROFILE="${2:-}"; shift 2 ;;
    --target)     TARGET="${2:-}"; shift 2 ;;
    --seeds-root) SEEDS_ROOT="${2:-}"; shift 2 ;;
    --target-dir) TARGET_DIR="${2:-}"; shift 2 ;;
    -h|--help)    sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 0 ;;
    *) echo "seed_target.sh: unknown argument '$1'" >&2; exit 1 ;;
  esac
done

say() { printf 'seed: %s\n' "$1"; }
cold() { say "cold  reason=$1 key=$KEY target=$TARGET_DIR"; exit 0; }

[ -n "$TARGET_DIR" ] || { echo "seed_target.sh: no target dir (set CARGO_TARGET_DIR or pass --target-dir)" >&2; exit 1; }

# D6: a seed is an immutable source, never a build output. The fast gate refuses
# this too; refusing here as well means a direct invocation cannot bypass it.
case "$(readlink -f "$TARGET_DIR" 2>/dev/null || echo "$TARGET_DIR")" in
  "$(readlink -f "$SEEDS_ROOT" 2>/dev/null || echo "$SEEDS_ROOT")"/*)
    echo "seed_target.sh: refusing to use a seed directory as a build target" >&2
    echo "  target: $TARGET_DIR" >&2
    echo "  seeds are read-only sources; each worktree needs its own target dir." >&2
    exit 2 ;;
esac

export SEED_REPO_ROOT="$REPO_ROOT"

if [ -n "$TARGET" ]; then KEY_ARGS+=(--target "$TARGET"); fi
KEY="$(bash "$SCRIPT_DIR/seed_compat_key.sh" "${KEY_ARGS[@]}" --profile "$PROFILE")"

# NOTE: this check must stay AFTER the key is computed. `cold` interpolates
# $KEY, and under `set -u` calling it earlier aborts the script with
# "KEY: variable sin asignar" instead of degrading to a cold build — which
# is the exact failure the whole seed mechanism is built to avoid.
# build-dir: refused when configured, on purpose — it is NOT a key field.
#
# Cargo stores build-script outputs in build-dir, separately from target-dir, and
# BoringSSL's output — the single largest thing this seed exists for — is exactly
# build-script output. Relocating build-dir means the tree the pruning measures
# (`<target>/debug/build/`) is not the tree that gets populated. Hashing it would
# only manufacture a compatibility claim out of a gap in what we can reason about.
#
# build-dir is STABLE, not nightly-only: it shipped in Rust 1.91 and its layout is
# still internal and subject to change. WebFang is on 1.88, so on our toolchain
# the flag is inert today — which is precisely why refusing it is a policy
# decision rather than a workaround for a broken build. A build-dir shared across
# worktrees is also a known Cargo footgun (rust-lang/cargo#17312: cargo can
# conclude nothing needs rebuilding), the same contamination class this whole
# mechanism exists to prevent.
#
# Detection is deliberately BROAD rather than exact: any `build-dir` in any config
# source cargo could read refuses the seed. A refusal must fail closed. A precise
# parser would have to model cargo's entire config precedence chain to be narrower
# than the truth, and being wrong here means silently seeding from a tree whose
# layout we never modelled.
if [ -n "${CARGO_BUILD_BUILD_DIR:-}" ]; then
  cold "build-dir-configured"
fi
for cfg in "${CARGO_HOME:-$HOME/.cargo}/config.toml" "${CARGO_HOME:-$HOME/.cargo}/config"; do
  if [ -f "$cfg" ] && grep -q 'build-dir' "$cfg" 2>/dev/null; then
    cold "build-dir-configured"
  fi
done
# cargo also walks from the CWD up to the root looking for .cargo/config.*, so a
# config ABOVE the repository applies to our build without living in it.
dir="$REPO_ROOT"
while [ "$dir" != "/" ]; do
  for cfg in "$dir/.cargo/config.toml" "$dir/.cargo/config"; do
    if [ -f "$cfg" ] && grep -q 'build-dir' "$cfg" 2>/dev/null; then
      cold "build-dir-configured"
    fi
  done
  dir="$(dirname "$dir")"
done

SEED="$SEEDS_ROOT/$KEY"

[ -d "$SEED" ] || cold "no-seed"

# Defence in depth: the directory name already encodes the key, but a renamed or
# hand-edited seed must not be trusted on that alone.
if ! bash "$SCRIPT_DIR/seed_compat_key.sh" "${KEY_ARGS[@]}" --profile "$PROFILE" \
      --verify "$SEED" >/dev/null 2>&1; then
  cold "incompatible-manifest"
fi

if [ -e "$TARGET_DIR" ] && [ -n "$(ls -A "$TARGET_DIR" 2>/dev/null)" ]; then
  # Never seed over an existing target: the whole point is that each worktree owns
  # its dir exclusively, and a half-populated one belongs to a build in flight.
  say "cold  reason=target-not-empty key=$KEY target=$TARGET_DIR"
  exit 0
fi

# reflink=always, never =auto (D5): on a filesystem without CoW, =auto silently
# performs a full copy and exits 0 — measured, 64 MB copied for a 64 MB file on
# tmpfs. =always fails loudly, and failure here just means a cold build.
#
# --no-preserve=mode is REQUIRED, not cosmetic: the seed is published read-only
# and a CoW clone inherits its mode bits, so without this the worktree's own
# target dir arrives unwritable and the first cargo write fails with EACCES.
# Dropping the read-only bits on the COPY is correct — the seed stays read-only,
# and each worktree needs a target it can actually write.
if ! cp -a --reflink=always --no-preserve=mode "$SEED" "$TARGET_DIR" 2>/dev/null; then
  rm -rf "$TARGET_DIR" 2>/dev/null || true
  cold "reflink-unavailable"
fi
# Apparent size, deliberately NOT a df delta: the delta is dominated by whatever
# else is touching the filesystem in the same second and was measured coming out
# NEGATIVE here. A number that can be negative is not a measurement.
say "seeded reason=reflink key=$KEY target=$TARGET_DIR apparent=$(du -sh --apparent-size "$TARGET_DIR" 2>/dev/null | cut -f1)"
exit 0
