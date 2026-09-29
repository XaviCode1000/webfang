#!/usr/bin/env bash
set -uo pipefail
# test_seed_publish_atomicity.sh — the seed must be atomic from the CONSUMER's
# point of view: resolving seeds/<key> yields either nothing or a complete,
# self-describing, immutable seed. Never a partial one.
#
# These are the three properties that were FALSE before the publish transaction
# was reordered. They are tested against the real publish function through
# --test-transaction, not against a reimplementation of it, and without a
# reference build — the transaction is what is under test, not the compiler.
#
# Manual: ~2 s. Not in CI, alongside scripts/test_seed_contamination.sh.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
PUBLISH="$SCRIPT_DIR/seed_publish.sh"
KEYSH="$SCRIPT_DIR/seed_compat_key.sh"

export SEED_REPO_ROOT="$REPO_ROOT"
export CARGO_INCREMENTAL=0
unset RUSTC_WRAPPER RUSTUP_TOOLCHAIN

# Stage on the SAME filesystem production uses, never on mktemp's tmpfs.
# This is not tidiness: renaming a directory whose mode is a-w to a new name
# fails with EACCES on the real cache filesystem and succeeds on tmpfs. A test
# sandboxed on tmpfs therefore cannot observe the failure mode that actually
# broke a real publish — it would have gone green while production was broken.
SANDBOX="${SEED_ROOT:-$HOME/.cache/cargo-target/seeds}/.test-atomicity.$$"
SEEDS="$SANDBOX"
trap 'chmod -R u+w "$SANDBOX" 2>/dev/null; rm -rf "$SANDBOX" 2>/dev/null' EXIT
KEY="$(bash "$KEYSH" --features '')"
DEST="$SEEDS/$KEY"

PASS=0; FAIL=0
STAGE_N=0
ok()   { PASS=$((PASS+1)); printf '  ok   %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  FAIL %s\n' "$1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (expected '$3', got '$2')"; fi; }

# A staging tree shaped enough for the transaction: the transaction only writes
# a manifest, chmods, and renames. Each call gets its OWN directory — a published
# seed is left read-only, so reusing one path fails with EACCES on the next call.
# Sets the global $S rather than echoing: called via $( ), a function runs in a
# subshell and its STAGE_N increment would be discarded, handing the next call
# the same path — which the previous publish has just made read-only.
#
# Staged INSIDE the seeds root, exactly as production does. This is not cosmetic:
# `mv` of a directory whose own mode is a-w to a new name in a DIFFERENT parent
# fails with EACCES (reproduced), while the same move within one parent works.
# The seed transaction only ever renames within SEEDS_ROOT, and the test has to
# exercise that topology rather than a convenient one.
mkstage() {
  STAGE_N=$((STAGE_N+1))
  S="$SEEDS/.staging-test.$STAGE_N"
  mkdir -p "$S/debug/.fingerprint/dep-foo"
  printf 'marker-%s\n' "$1" >"$S/debug/.fingerprint/dep-foo/marker.txt"
}

# Verify a published seed is genuinely usable, not merely present.
seed_is_valid() {
  bash "$KEYSH" --features '' --verify "$1" >/dev/null 2>&1
}

echo "seed publish atomicity (key=$KEY)"

# --- 1. failure before the manifest is written ------------------------------
mkstage pre-manifest
bash "$PUBLISH" --seeds-root "$SEEDS" --test-transaction "$S" --fail-at before-manifest >/dev/null 2>&1
if [ -e "$DEST" ]; then bad "fail before manifest: seeds/<key> appeared"
else ok "fail before manifest: seeds/<key> never appeared"; fi
if ls -A "$SEEDS" 2>/dev/null | grep -qv '^\.'; then bad "fail before manifest: something published"
else ok "fail before manifest: nothing published at all"; fi

# --- 2. failure after the manifest, before the rename ------------------------
mkstage pre-rename
bash "$PUBLISH" --seeds-root "$SEEDS" --test-transaction "$S" --fail-at before-rename >/dev/null 2>&1
if [ -e "$DEST" ]; then bad "fail before rename: seeds/<key> appeared"
else ok "fail before rename: seeds/<key> never appeared"; fi

# --- 3. a VALID seed survives a failed refresh (the D-1 property) -----------
# The old, good seed must still be there AND still verify after a --force
# refresh dies before the rename. Deleting the only valid seed before its
# successor is ready is exactly what the old ordering did.
mkstage replacement
bash "$PUBLISH" --seeds-root "$SEEDS" --test-transaction "$S" >/dev/null 2>&1
check "baseline: seed published" "$([ -d "$DEST" ] && echo yes || echo no)" "yes"
check "baseline: seed verifies" "$(seed_is_valid "$DEST" && echo yes || echo no)" "yes"
GOOD_INODE_CONTENT="$(cat "$DEST/debug/.fingerprint/dep-foo/marker.txt")"

mkstage doomed-refresh
bash "$PUBLISH" --seeds-root "$SEEDS" --force --test-transaction "$S" --fail-at before-rename >/dev/null 2>&1
check "failed refresh: old seed still present" "$([ -d "$DEST" ] && echo yes || echo no)" "yes"
check "failed refresh: old seed still valid" "$(seed_is_valid "$DEST" && echo yes || echo no)" "yes"
check "failed refresh: old content untouched" \
  "$(cat "$DEST/debug/.fingerprint/dep-foo/marker.txt" 2>/dev/null)" "$GOOD_INODE_CONTENT"

# --- 4. the published seed is complete the instant it becomes visible ---------
# Regression for the ordering bug: the manifest used to be written AFTER the
# rename, leaving a visible seed with no manifest inside it.
if [ -f "$DEST/manifest.toml" ]; then ok "published seed contains its manifest"
else bad "published seed missing manifest"; fi
check "published seed verifies" "$(seed_is_valid "$DEST" && echo yes || echo no)" "yes"
# The top directory is deliberately left WRITABLE: renaming a directory whose own
# mode is a-w fails with EACCES on the real cache filesystem, so the publish
# transaction restores write on the top and nowhere else. Asserting it read-only
# would be asserting a bug. The immutability claim lives in the CONTENTS.
if [ -w "$DEST" ]; then ok "published seed top level is writable (required for the rename)"
else bad "published seed top level is read-only — the rename could not have worked"; fi
if [ -w "$DEST/debug" ]; then bad "published seed CONTENTS are writable (immutability is only cosmetic)"
else ok "published seed contents are read-only"; fi

# --- 5. a successful --force refresh actually replaces ------------------------
mkstage good-refresh
bash "$PUBLISH" --seeds-root "$SEEDS" --force --test-transaction "$S" >/dev/null 2>&1
check "successful refresh: replaced" \
  "$(cat "$DEST/debug/.fingerprint/dep-foo/marker.txt" 2>/dev/null)" "marker-good-refresh"
check "successful refresh: still valid" "$(seed_is_valid "$DEST" && echo yes || echo no)" "yes"
check "no .retired left behind" "$(ls -A "$SEEDS" | grep -c '^\.retired' || true)" "0"

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
echo "publish is atomic: seeds/<key> is never observable in a partial state"
