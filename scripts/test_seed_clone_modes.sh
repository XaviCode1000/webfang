#!/usr/bin/env bash
set -uo pipefail
# test_seed_clone_modes.sh — a seeded clone must be writable AND executable,
# and the seed it came from must come out of the clone untouched.
#
# The contract is:
#
#     seed 555 → clone 755 (writable, executable) → seed still 555
#
# and not either of these:
#
#     seed 555 → clone 555   (cargo's first write fails with EACCES)
#     seed 555 → clone 644   (build scripts arrive without `x`; cargo fails the
#                            unit with "Permission denied (os error 13)")
#
# The second one is what shipped. `--no-preserve=mode` was documented as
# REQUIRED because a CoW clone does inherit the seed's mode bits, and that
# premise is correct — but dropping the mode entirely does not fix the write
# bit, it drops the execute bit too and cp re-derives the rest from the umask.
# On this machine that is 0644, so every build script in the seed landed
# unrunnable and `cargo check` died inside a build-script unit, which reads as a
# toolchain or filesystem fault rather than a seeding defect.
#
# `chmod -R u+w` is what turns the inherited 555 into 755. It is not a fallback
# for the failed-clone path — it is part of the success path.
#
# Manual: ~2 s.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
SEEDER="$SCRIPT_DIR/seed_target.sh"
KEYSH="$SCRIPT_DIR/seed_compat_key.sh"

export SEED_REPO_ROOT="$REPO_ROOT"
# Through the SAME recipe production uses, never a hand-built argument list: a
# test that assembles the key's inputs itself is a second opinion about what the
# key means, and a second opinion is what drifted in the first place.
# shellcheck source=scripts/seed_recipe.sh
. "$SCRIPT_DIR/seed_recipe.sh"
seed_recipe_parse "$REPO_ROOT" --features ''
seed_recipe_compute_key "$REPO_ROOT"
mapfile -t KEY_ARGS < <(seed_recipe_key_args)
KEY="$SEED_RECIPE_KEY" CARGO_INCREMENTAL=0
unset RUSTC_WRAPPER RUSTUP_TOOLCHAIN

# On the real cache filesystem, not tmpfs. This test is about CoW clone mode
# propagation, and /tmp is tmpfs: it has no reflink at all, so `cp --reflink=always`
# fails there and the clone this test is measuring never happens. Running it on
# tmpfs produces a green run against a branch that was never entered.
SANDBOX="${SEED_ROOT:-$HOME/.cache/cargo-target/seeds}/.test-modes.$$"
SEEDS="$SANDBOX/seeds"
trap 'chmod -R u+w "$SANDBOX" 2>/dev/null; rm -rf "$SANDBOX" 2>/dev/null' EXIT
mkdir -p "$SEEDS"

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL %s\n' "$1"; }

# ── a minimal but valid seed, published by production so the key is a real one
SEED="$SEEDS/$KEY"
BUILD_SCRIPT_REL="debug/build/some-crate-1.0.0/build-script-build"
mkdir -p "$SEED/$(dirname "$BUILD_SCRIPT_REL")"
# 555, exactly as seed_publish.sh leaves a published seed: read-only and
# executable. This is the state the clone has to survive.
printf '#!/usr/bin/sh\nexit 0\n' >"$SEED/$BUILD_SCRIPT_REL"
chmod 555 "$SEED/$BUILD_SCRIPT_REL"
mkdir -p "$SEED/debug/.fingerprint/some-crate-1.0.0"
printf 'content\n' >"$SEED/debug/.fingerprint/some-crate-1.0.0/lib-x"
# Emitted by production, never hand-written: a hand-written manifest would not
# verify, the consumer would report `incompatible-manifest` and never reach the
# clone, and the test would pass against a branch it never entered.
bash "$KEYSH" "${KEY_ARGS[@]}" --emit-manifest "$SEED" >/dev/null
bash "$KEYSH" "${KEY_ARGS[@]}" --verify "$SEED" >/dev/null \
  || { echo "FAIL: fixture seed does not verify; the test would measure nothing"; exit 1; }

SEED_MODE_BEFORE="$(stat -c '%a' "$SEED/$BUILD_SCRIPT_REL")"
[ "$SEED_MODE_BEFORE" = "555" ] \
  || { echo "FAIL: fixture is $SEED_MODE_BEFORE, expected 555; the test would measure nothing"; exit 1; }

# ── consume it, exactly as a fresh worktree would
TARGET="$SANDBOX/worktree-target"
OUT="$(SEED_ROOT="$SEEDS" CARGO_TARGET_DIR="$TARGET" bash "$SEEDER" 2>&1)"
case "$OUT" in
  *"seeded"*) ok "consumer reports seeded" ;;
  *) bad "consumer did not report seeded: $OUT"; printf '\n  %d passed, %d failed\n' "$PASS" "$FAIL"; exit 1 ;;
esac

# ── the clone must be executable: this is the regression
CLONE="$TARGET/$BUILD_SCRIPT_REL"
if [ ! -e "$CLONE" ]; then
  bad "clone is missing $BUILD_SCRIPT_REL"
else
  CLONE_MODE="$(stat -c '%a' "$CLONE")"
  if [ -x "$CLONE" ]; then
    ok "build script is executable in the clone (mode $CLONE_MODE)"
  else
    bad "build script lost its execute bit in the clone (mode $CLONE_MODE)"
  fi
  if [ -w "$CLONE" ]; then
    ok "build script is writable in the clone (mode $CLONE_MODE)"
  else
    bad "build script is read-only in the clone (mode $CLONE_MODE); cargo's first write would fail with EACCES"
  fi
fi

# ── and the seed must come out of the consumer untouched: the widening applies
# to the copy, never to the published reference.
SEED_MODE_AFTER="$(stat -c '%a' "$SEED/$BUILD_SCRIPT_REL")"
if [ "$SEED_MODE_AFTER" = "$SEED_MODE_BEFORE" ]; then
  ok "seed left read-only (still $SEED_MODE_AFTER)"
else
  bad "consumer mutated the seed: $SEED_MODE_BEFORE -> $SEED_MODE_AFTER"
fi

# ── 5. a chmod that fails must not be reported as `seeded` ───────────────────
#
# `chmod -R u+w` is what makes the clone usable, so reporting `seeded` without
# verifying it hands back a read-only target under a verdict that promises a
# working one. This is the same defect shape the removal path already documents:
# a swallowed error plus a success verdict. There the consequence was debris; here
# it is cargo's first write failing with an EACCES the caller cannot connect to
# seeding.
#
# The stub fails ONCE and delegates every later call to the real chmod, so the
# cleanup path is unaffected and the failure is isolated to the load-bearing call.
mkdir -p "$SANDBOX/bin"
cat >"$SANDBOX/bin/chmod" <<STUB
#!/usr/bin/env bash
if [ ! -e "$SANDBOX/chmod-already-failed" ]; then
  : >"$SANDBOX/chmod-already-failed"
  exit 1
fi
exec $(command -v chmod) "\$@"
STUB
chmod +x "$SANDBOX/bin/chmod"

TARGET2="$SANDBOX/worktree-target-chmod-fails"
OUT2="$(PATH="$SANDBOX/bin:$PATH" SEED_ROOT="$SEEDS" CARGO_TARGET_DIR="$TARGET2" \
        bash "$SEEDER" 2>&1)"
case "$OUT2" in
  *"seeded"*)
    bad "chmod failure is reported as seeded — a read-only target under a success verdict" ;;
  *cold*clone-not-writable*)
    ok "chmod failure is reported cold with reason=clone-not-writable" ;;
  *)
    bad "chmod failure produced an unexpected verdict: $OUT2" ;;
esac
if [ -e "$TARGET2" ]; then
  bad "chmod failure left a target dir behind; a cold build assumes a clean one"
else
  ok "chmod failure left no target dir to build over"
fi

printf '\n  %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
