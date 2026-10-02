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
# shellcheck source=scripts/seed_recipe.sh
. "$SCRIPT_DIR/seed_recipe.sh"

say() { printf 'seed: %s\n' "$1"; }
cold() { say "cold  reason=$1 key=$SEED_RECIPE_KEY target=$TARGET_DIR"; exit 0; }

# --target-dir is this script's own, not a contract field, so it is taken out
# before the recipe parses. The recipe rejects anything it does not recognise,
# which is correct: a contract field that quietly did nothing was one of the
# original defects.
TARGET_DIR="${CARGO_TARGET_DIR:-}"
_contract_args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --target-dir) TARGET_DIR="${2:-}"; shift 2 ;;
    *) _contract_args+=("$1"); shift ;;
  esac
done
seed_recipe_parse "$REPO_ROOT" "${_contract_args[@]+"${_contract_args[@]}"}"
SEEDS_ROOT="$SEED_RECIPE_SEEDS_ROOT"

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

seed_recipe_compute_key "$REPO_ROOT"
export SEED_REPO_ROOT="$REPO_ROOT"

# P1: the shared build-dir refusal, identical implementation to the producer's.
# See seed_recipe.sh for why this is a refusal rather than a key field.
if seed_recipe_has_config_include "$REPO_ROOT"; then
  cold "config-include-unsupported"
fi

if seed_recipe_has_build_dir "$REPO_ROOT"; then
  cold "build-dir-configured"
fi

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

SEED="$SEEDS_ROOT/$SEED_RECIPE_KEY"

[ -d "$SEED" ] || cold "no-seed"

# Defence in depth: the directory name already encodes the key, but a renamed or
# hand-edited seed must not be trusted on that alone.
mapfile -t KEY_ARGS < <(seed_recipe_key_args)
if ! bash "$SCRIPT_DIR/seed_compat_key.sh" "${KEY_ARGS[@]}" \
      --verify "$SEED" >/dev/null 2>&1; then
  cold "incompatible-manifest"
fi

if [ -e "$TARGET_DIR" ] && [ -n "$(ls -A "$TARGET_DIR" 2>/dev/null)" ]; then
  # Never seed over an existing target: the whole point is that each worktree owns
  # its dir exclusively, and a half-populated one belongs to a build in flight.
  say "cold  reason=target-not-empty key=$SEED_RECIPE_KEY target=$TARGET_DIR"
  exit 0
fi

# reflink=always, never =auto (D5): on a filesystem without CoW, =auto silently
# performs a full copy and exits 0 — measured, 64 MB copied for a 64 MB file on
# tmpfs. =always fails loudly, and failure here just means a cold build.
#
# The seed is published read-only (555) and a CoW clone inherits its mode bits,
# so the clone must have its read-only bits dropped or the worktree's target dir
# arrives unwritable and the first cargo write fails with EACCES.
#
# Drop only the READ bits, not the whole mode. `--no-preserve=mode` looks like
# it does the same thing, and it does not: cp then derives the copy's mode from
# the umask, so a 555 build script lands as 644 — no `x` for anyone — and cargo
# fails that unit with "Permission denied (os error 13)". That was measured on
# btrfs, on a real seed:
#     --no-preserve=mode            -> 644, not executable
#     --preserve=mode + chmod u+w   -> 755, executable
# `chmod -R u+w` is therefore not a fallback for the failure path alone; it is
# part of the success path, and it is what turns the inherited 555 into a
# writable-and-executable 755. Measured cost: 0.04 s over 8,541 files, against
# an 18 s seeded build. The seed itself stays 555 — only the copy is widened.
# Discard a cloned target that must not be built over, then report the outcome.
# Shared by both post-clone failure modes so they cannot drift apart: the contract
# for each is identical — the target is unusable, so it must not exist, and the
# caller gets a cold build from a clean dir. Handing back a target we cannot write
# inherits the exact hazard this mechanism exists to avoid.
#
# `rm -rf || true` on its own was a real bug in this path: a subdirectory the
# clone left without its write bit defeats the removal, the error was swallowed,
# and the script reported `cold` and exited 0 — handing the caller a target
# containing half a seed to build over. The silence was the defect; the debris in
# the build dir was the consequence. So the removal is VERIFIED, and a target that
# survives it is `refused` rather than `cold`.
discard_target() {
  local reason="$1" left
  rm -rf "$TARGET_DIR" 2>/dev/null || true
  if [ -e "$TARGET_DIR" ]; then
    # Second attempt with write bits restored, the usual reason the first failed.
    chmod -R u+w "$TARGET_DIR" 2>/dev/null || true
    rm -rf "$TARGET_DIR" 2>/dev/null || true
  fi
  if [ -e "$TARGET_DIR" ]; then
    left="$(find "$TARGET_DIR" -mindepth 1 2>/dev/null | wc -l)"
    # Same one-line verdict shape as every other outcome, so this state is
    # greppable the same way `seeded` and `cold` are — and so the word "cold"
    # never appears in a message that is denying it.
    say "refused reason=unremovable-leftover key=$SEED_RECIPE_KEY target=$TARGET_DIR entries=$left"
    echo "seed_target.sh: the clone failed AND its leftovers could not be removed." >&2
    echo "  not handing you a half-populated target to build over: a cold build" >&2
    echo "  assumes a clean target dir, and this one is not clean." >&2
    echo "  target: $TARGET_DIR  ($left entries left)" >&2
    echo "  fix: remove it by hand, then re-run." >&2
    echo "        chmod -R u+w '$TARGET_DIR' && rm -rf '$TARGET_DIR'" >&2
    exit 3
  fi
  cold "$reason"
}

if ! cp -a --reflink=always "$SEED" "$TARGET_DIR" 2>/dev/null; then
  discard_target "reflink-unavailable"
fi
# Widen the clone's read-only bits. 555 -> 755: writable so cargo can build, and
# still executable so the build scripts it inherits can run. Without this the
# clone is read-only (first cargo write fails) or, if the mode was dropped at
# clone time, unexecutable (build script fails to run). Both halves are needed.
#
# This is load-bearing, so its success is VERIFIED rather than assumed — the same
# discipline the removal path applies, and for the same reason. Swallowing a
# failed chmod with `|| true` and then reporting `seeded` would hand back a
# read-only target under a verdict that promises a usable one: not corruption,
# but cargo's first write would fail with an EACCES the caller has no way to
# connect to seeding. Verified against the target dir itself; whether the build
# scripts deeper in the tree kept their execute bit is asserted by
# test_seed_clone_modes.sh, which is where a per-file check belongs.
if ! chmod -R u+w "$TARGET_DIR" 2>/dev/null || [ ! -w "$TARGET_DIR" ]; then
  discard_target "clone-not-writable"
fi
# Apparent size, deliberately NOT a df delta: the delta is dominated by whatever
# else is touching the filesystem in the same second and was measured coming out
# NEGATIVE here. A number that can be negative is not a measurement.
say "seeded reason=reflink key=$SEED_RECIPE_KEY target=$TARGET_DIR apparent=$(du -sh --apparent-size "$TARGET_DIR" 2>/dev/null | cut -f1)"
exit 0
