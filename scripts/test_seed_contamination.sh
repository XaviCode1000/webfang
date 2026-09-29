#!/usr/bin/env bash
set -euo pipefail
# test_seed_contamination.sh — REGRESSION for the correctness failure of
# reflink-seeding a worktree CARGO_TARGET_DIR.
#
# What fails without this test (measured 2026-09-29, not theoretical):
#
#   W1 (reference) and W2 (destination) are both created BEFORE the reference
#   build — the natural bootstrap order: create the worktree, seed afterwards.
#       reference artifacts   mtime 18:37:41
#       W2 sources            mtime 18:35:09   <-- OLDER
#   cp --reflink=always  ref -> W2 ;  cargo build
#       Finished `dev` profile ... in 0.26s     0 crates compiled, exit 0
#   ./webfang  ->  W1's code, not W2's
#
# `cargo build` returns 0 and runs the wrong binary. This is the same class of
# failure as #1267 (an E2E run executing another tree's binary) arriving by a
# different route: Cargo decides freshness from mtimes, and here the mtime
# favours the seed. A seed cannot correct this on its own.
#
# What this test pins:
#
#   1. The primary assertion is the CONTENT of the executed binary, not how many
#      crates Cargo recompiled. A future change may alter that count without
#      breaking correctness, and must never let W2 execute W1's code.
#   2. A published seed carries no path-dependent WebFang build units. That is
#      the mitigation: with no unit that depends on the workspace there is
#      nothing to contaminate, by construction rather than by clock ordering.
#   3. A negative control (unpruned seed) proves the test is still sensitive to
#      the failure. If Cargo ever stops being vulnerable, the control says so
#      instead of leaving the test green for the wrong reason.
#   4. The test asserts its own precondition (sources older than artifacts).
#      Without it, the test could quietly exercise the SAFE path and check
#      nothing, which is how a regression test turns decorative.
#
# Usage:  scripts/test_seed_contamination.sh [--keep]
#
# Deliberately SLOW (~4-5 min: one cold workspace build including BoringSSL's
# C++). It is in no CI job for that reason; it is a manual / wide-cadence test.
# Exit 0 = invariants hold. Exit 1 = a check failed. Exit 0 with "SKIP" = the
# filesystem has no reflink support.
#
# Why the build recipe is pinned here instead of imported: the reference and
# the destination must compile with IDENTICAL flags. If they differ, the seed
# becomes a flags mismatch (the A3 case: zero benefit plus orphaned objects) and
# the test would be measuring something else. See AGENTS.md, worktree bootstrap.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
KEEP=0
[ "${1:-}" = "--keep" ] && KEEP=1

# Units a portable seed must NOT carry: the workspace members, and any other
# path-dependent unit. Their fingerprints embed absolute paths, which is what
# breaks freshness when the target moves to another worktree.
WORKSPACE_UNITS=(webfang_core webfang_cli webfang_ai webfang_mcp
                 webfang_benchmark webfang_test_utils)

CACHE_ROOT="${SEED_TEST_CACHE_ROOT:-$HOME/.cache/cargo-target}"
RUN_DIR="$(mktemp -d "$CACHE_ROOT/seed-contamination-test.XXXXXX")"
REF_TARGET="$RUN_DIR/ref"
SEED_RAW="$RUN_DIR/seed-raw"
SEED_PRUNED="$RUN_DIR/seed-pruned"
W1="$RUN_DIR/w1-reference"
W2="$RUN_DIR/w2-destination"

fail=0
ok()   { echo "OK:   $*"; }
bad()  { echo "FAIL: $*"; fail=1; }
info() { echo "INFO: $*"; }

# Invoked indirectly by the EXIT trap below. shellcheck cannot see that call
# when the script ends in an explicit `exit` (SC2329 false positive).
# shellcheck disable=SC2329
cleanup() {
  local rc=$?
  if [ "$KEEP" -eq 1 ]; then
    info "--keep: keeping $RUN_DIR"
    return "$rc"
  fi
  git -C "$REPO_ROOT" worktree remove --force "$W1" >/dev/null 2>&1 || true
  git -C "$REPO_ROOT" worktree remove --force "$W2" >/dev/null 2>&1 || true
  git -C "$REPO_ROOT" worktree prune >/dev/null 2>&1 || true
  rm -rf "$RUN_DIR"
  return "$rc"
}
trap cleanup EXIT

# --- the build recipe, identical for reference and destination ---------------
build() {  # $1=worktree  $2=target-dir
  ( cd "$1" \
    && env -u RUSTC_WRAPPER -u RUSTUP_TOOLCHAIN -u SCCACHE_DIR -u SCCACHE_BASEDIRS \
         CARGO_TARGET_DIR="$2" \
         CARGO_INCREMENTAL=0 \
         CARGO_BUILD_JOBS="${SEED_TEST_JOBS:-8}" \
         RUSTFLAGS="-C link-arg=-Wl,--no-keep-memory -C link-arg=-Wl,--reduce-memory-overheads" \
         cargo build --workspace --offline >"$RUN_DIR/build.log" 2>&1 )
}

binary_marker() {  # $1=binary -> the marker it EMITS when executed
  "$1" --help 2>&1 | grep -oE 'SEEDTEST_[A-Z]+_[a-z0-9]+' | head -1 || true
}

inject_marker() {  # $1=worktree  $2=marker
  python3 - "$1" "$2" <<'PY'
import pathlib, sys
wt, marker = sys.argv[1], sys.argv[2]
p = pathlib.Path(wt) / "crates/webfang_cli/src/main.rs"
s = p.read_text()
anchor = "pub async fn main() -> CliExit {\n"
if anchor not in s:
    sys.exit(f"anchor not found in {p}")
if "SEEDTEST_" not in s:
    p.write_text(s.replace(anchor, anchor + f'    eprintln!("{marker}");\n', 1))
PY
}

prune_workspace_units() {  # $1=seed target dir
  local seed="$1" u
  for u in "${WORKSPACE_UNITS[@]}"; do
    rm -rf "${seed:?}/debug/.fingerprint/${u}-"* \
           "${seed:?}/debug/build/${u}-"* \
           "${seed:?}/debug/deps/lib${u}"* \
           "${seed:?}/debug/deps/${u}"* 2>/dev/null || true
  done
  rm -f "$seed/debug/webfang" 2>/dev/null || true
}

# --- preconditions ----------------------------------------------------------
mkdir -p "$CACHE_ROOT" || { echo "FAIL: could not create $CACHE_ROOT"; exit 1; }

if ! cp --reflink=always "$0" "$RUN_DIR/.reflink-probe" 2>/dev/null; then
  info "SKIP: this filesystem does not support reflink, and the contamination"
  info "      reproduced here depends on one. On a non-CoW filesystem the"
  info "      correct path is the cold-build fallback, and this test has"
  info "      nothing to prove."
  exit 0
fi
rm -f "$RUN_DIR/.reflink-probe"
ok "reflink available under $CACHE_ROOT"

command -v python3 >/dev/null || { echo "FAIL: python3 is required"; exit 1; }

HEAD_SHA="$(git -C "$REPO_ROOT" rev-parse HEAD)"
info "repo $REPO_ROOT @ ${HEAD_SHA:0:8}"
info "run dir $RUN_DIR"

# --- 1. two throwaway worktrees with DIFFERENT content -----------------------
# The order is what triggers the failure: both are created BEFORE the reference
# build, exactly as in the real bootstrap (worktree first, seed afterwards).
git -C "$REPO_ROOT" worktree add --detach "$W1" "$HEAD_SHA" >/dev/null 2>&1 \
  || { echo "FAIL: could not create the reference worktree"; exit 1; }
git -C "$REPO_ROOT" worktree add --detach "$W2" "$HEAD_SHA" >/dev/null 2>&1 \
  || { echo "FAIL: could not create the destination worktree"; exit 1; }

REF_MARKER="SEEDTEST_ALPHA_a1a1"
DST_MARKER="SEEDTEST_BRAVO_b2b2"
inject_marker "$W1" "$REF_MARKER"
inject_marker "$W2" "$DST_MARKER"
ok "worktrees created; W1 carries $REF_MARKER, W2 carries $DST_MARKER"

# --- 2. build the reference --------------------------------------------------
info "building the reference (cold, includes BoringSSL's C++)..."
if ! build "$W1" "$REF_TARGET"; then
  echo "FAIL: reference build failed; log at $RUN_DIR/build.log"; exit 1
fi
REF_BIN="$REF_TARGET/debug/webfang"
[ "$(binary_marker "$REF_BIN")" = "$REF_MARKER" ] \
  || { echo "FAIL: the reference binary does not emit its own marker"; exit 1; }
REF_O="$(find "$REF_TARGET" -name '*.o' | wc -l)"
ok "reference built ($(du -sh --apparent-size "$REF_TARGET" | cut -f1) apparent, $REF_O C++ objects)"

# --- 3. the failure's precondition: sources OLDER than artifacts ------------
DST_SRC_MTIME="$(stat -c %Y "$W2/crates/webfang_cli/src/main.rs")"
ART_MTIME="$(stat -c %Y "$REF_BIN")"
if [ "$DST_SRC_MTIME" -ge "$ART_MTIME" ]; then
  echo "FAIL: precondition not met — destination sources ($DST_SRC_MTIME) are"
  echo "      not older than the reference artifacts ($ART_MTIME). The test"
  echo "      would exercise the SAFE path and verify nothing. Retry, or check"
  echo "      the system clock resolution."
  exit 1
fi
ok "failure precondition present (sources $DST_SRC_MTIME < artifacts $ART_MTIME)"

# --- 4. negative control: seed WITHOUT pruning -------------------------------
cp -a --reflink=always "$REF_TARGET" "$SEED_RAW"
if ! build "$W2" "$SEED_RAW"; then
  echo "FAIL: destination build failed; log at $RUN_DIR/build.log"; exit 1
fi
RAW_MARKER="$(binary_marker "$SEED_RAW/debug/webfang")"
if [ "$RAW_MARKER" = "$DST_MARKER" ]; then
  info "negative control: the UNPRUNED seed produced the correct binary."
  info "  Cargo no longer reproduces this (or the clock did not recreate the"
  info "  ordering). The pruned phase below is still the one that matters, but"
  info "  the negative control no longer demonstrates sensitivity."
else
  info "negative control: UNPRUNED seed produced '$RAW_MARKER' (W1's code)."
  info "  Confirms the test reproduces the failure, and that what avoids it is"
  info "  the mitigation below rather than the luck of a fresh build."
fi

# --- 5. the invariant: PRUNED seed ------------------------------------------
cp -a --reflink=always "$REF_TARGET" "$SEED_PRUNED"
prune_workspace_units "$SEED_PRUNED"

LEFTOVER=0
for u in "${WORKSPACE_UNITS[@]}"; do
  n=$(find "$SEED_PRUNED/debug/.fingerprint" -maxdepth 1 -name "${u}-*" 2>/dev/null | wc -l)
  LEFTOVER=$((LEFTOVER + n))
done
if [ "$LEFTOVER" -eq 0 ]; then
  ok "the published seed carries no path-dependent WebFang build unit"
else
  bad "the seed still holds $LEFTOVER workspace fingerprints"
fi

if [ -f "$SEED_PRUNED/debug/webfang" ]; then
  bad "the seed still carries the final webfang binary"
else
  ok "the seed does not carry the final binary"
fi

build "$W2" "$SEED_PRUNED" || { echo "FAIL: destination build failed; log at $RUN_DIR/build.log"; exit 1; }
DST_BIN="$SEED_PRUNED/debug/webfang"
GOT="$(binary_marker "$DST_BIN")"

# Primary assertion: the executed binary is the destination's own.
if [ "$GOT" = "$DST_MARKER" ]; then
  ok "the destination binary emits $DST_MARKER (its own content)"
else
  bad "the destination binary emitted '$GOT'; expected $DST_MARKER"
fi
if [ "$GOT" != "$REF_MARKER" ]; then
  ok "the destination binary does NOT run the reference's code"
else
  bad "CONTAMINATION: the destination ran W1's code"
fi

if strings "$DST_BIN" | grep -q "$REF_MARKER"; then
  bad "the destination binary contains the reference marker ($REF_MARKER)"
else
  ok "strings: the destination binary does not contain the reference marker"
fi

# Pruning must not cost the benefit: third-party units must still be reused.
PRUNED_O="$(find "$SEED_PRUNED" -name '*.o' | wc -l)"
if [ "$PRUNED_O" -eq "$REF_O" ]; then
  ok "all $PRUNED_O BoringSSL C++ objects were reused (pruning left them alone)"
else
  bad "C++ objects went from $REF_O to $PRUNED_O; pruning damaged third-party"
fi

echo "----------------------------------------"
if [ "$fail" -eq 0 ]; then
  echo "test_seed_contamination: GREEN"
else
  echo "test_seed_contamination: RED"
fi
exit "$fail"
