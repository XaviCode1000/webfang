#!/usr/bin/env bash
set -euo pipefail
# test_seed_contamination.sh — REGRESSION, as an integration test against the
# REAL seed mechanism.
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
# `cargo build` returns 0 and runs the wrong binary. Same class as #1267 (an E2E
# run executing another tree's binary), arriving by a different route: Cargo
# decides freshness from mtimes, and here the mtime favours the seed.
#
# INTEGRATION, NOT A RECREATION. The seed is produced by scripts/seed_publish.sh
# and consumed by scripts/seed_target.sh — the same two scripts a real worktree
# bootstrap runs. An earlier version of this file carried its own
# `prune_workspace_units` and poked at debug/.fingerprint directly, which made it
# a test of a COPY: the production pruning could rot completely while this test
# stayed green, because it pruned on its own and then congratulated itself. The
# pruning logic and the knowledge of cargo's internal layout now live only in
# production, and this file asserts the published artifact's observable
# behaviour. That inversion is the entire point of the rewrite.
#
# The contract asserted here is stated in observable terms, not cargo's:
#
#   "the published seed does not reference the checkout it was built from"
#
# A seed that carried a path-dependent WebFang unit would carry that path
# somewhere in its fingerprints and debug info. Not carrying it is the same
# property the old test measured by counting webfang_* fingerprint directories,
# obtained without the test knowing what a fingerprint directory is.
#
# Usage:  scripts/test_seed_contamination.sh [--keep]
#
# Deliberately SLOW: ~5-6 min. One cold workspace build including BoringSSL's C++
# (inside seed_publish.sh) plus the destination build. It is in no CI job and in
# no fast gate for that reason — this is a manual / wide-cadence test whose value
# is that it exercises the real mechanism, not that it is quick.
#
# Exit 0 = invariants hold. Exit 1 = a check failed. Exit 0 with "SKIP" = the
# filesystem has no reflink support, in which case there is nothing to prove.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"
PUBLISH="$SCRIPT_DIR/seed_publish.sh"
TARGETER="$SCRIPT_DIR/seed_target.sh"
KEEP=0
[ "${1:-}" = "--keep" ] && KEEP=1

# One build recipe, exported once, used by the publish, by the consume and by
# the destination build. The seed's KEY is derived from RUSTFLAGS and
# CARGO_INCREMENTAL among others, so a recipe that differed between the reference
# and the destination would simply produce a different key and the consume would
# report no-seed — the test would be measuring the wrong thing.
export CARGO_INCREMENTAL=0
export RUSTFLAGS="-C link-arg=-Wl,--no-keep-memory -C link-arg=-Wl,--reduce-memory-overheads"
export CARGO_BUILD_JOBS="${SEED_TEST_JOBS:-8}"
unset RUSTC_WRAPPER RUSTUP_TOOLCHAIN || true

# EVERYTHING lives on the real cache filesystem. Not /tmp: rename() of a
# read-only directory behaves differently on tmpfs, and a tmpfs sandbox would
# hide exactly the publication failures this test must be able to see.
CACHE_ROOT="${SEED_TEST_CACHE_ROOT:-$HOME/.cache/cargo-target}"
RUN_DIR="$(mktemp -d "$CACHE_ROOT/seed-contamination-test.XXXXXX")"
SEEDS_ROOT="$RUN_DIR/seeds"
W1="$RUN_DIR/w1-reference"
W2="$RUN_DIR/w2-destination"
W2_TARGET="$RUN_DIR/w2-target"     # deliberately NOT under the seeds root

fail=0
ok()   { echo "OK:   $*"; }
bad()  { echo "FAIL: $*"; fail=1; }
info() { echo "INFO: $*"; }

# shellcheck disable=SC2329  # invoked indirectly, by the EXIT trap set below
cleanup() {
  local rc=$?
  if [ "$KEEP" -eq 1 ]; then
    info "--keep: keeping $RUN_DIR"
    return "$rc"
  fi
  git -C "$REPO_ROOT" worktree remove --force "$W1" >/dev/null 2>&1 || true
  git -C "$REPO_ROOT" worktree remove --force "$W2" >/dev/null 2>&1 || true
  git -C "$REPO_ROOT" worktree prune >/dev/null 2>&1 || true
  chmod -R u+w "$RUN_DIR" 2>/dev/null || true
  rm -rf "$RUN_DIR"
  return "$rc"
}
trap cleanup EXIT

build_w2() {  # the destination build, with the same recipe as the seed
  ( cd "$W2" \
    && env -u RUSTC_WRAPPER -u RUSTUP_TOOLCHAIN -u SCCACHE_DIR -u SCCACHE_BASEDIRS \
         CARGO_TARGET_DIR="$W2_TARGET" \
         cargo build --workspace --offline >"$RUN_DIR/build.log" 2>&1 )
}

binary_marker() { "$1" --help 2>&1 | grep -oE 'SEEDTEST_[A-Z]+_[a-z0-9]+' | head -1 || true; }

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

# --- preconditions ----------------------------------------------------------
mkdir -p "$CACHE_ROOT" || { echo "FAIL: could not create $CACHE_ROOT"; exit 1; }
if ! cp --reflink=always "$0" "$RUN_DIR/.reflink-probe" 2>/dev/null; then
  info "SKIP: $CACHE_ROOT has no reflink support, and the contamination"
  info "      reproduced here depends on one. On a non-CoW filesystem the"
  info "      correct path is the cold-build fallback and there is nothing to prove."
  exit 0
fi
rm -f "$RUN_DIR/.reflink-probe"
ok "reflink available under $CACHE_ROOT"
command -v python3 >/dev/null || { echo "FAIL: python3 is required"; exit 1; }
for f in "$PUBLISH" "$TARGETER"; do
  [ -x "$f" ] || { echo "FAIL: $f is missing or not executable"; exit 1; }
done

HEAD_SHA="$(git -C "$REPO_ROOT" rev-parse HEAD)"
info "repo $REPO_ROOT @ ${HEAD_SHA:0:8}"
info "run dir $RUN_DIR (on $(df --output=fstype "$RUN_DIR" 2>/dev/null | tail -1 | tr -d ' '), same fs as the real cache)"

# --- 1. two throwaway worktrees, created BEFORE the reference build ----------
# The order is the failure: worktree first, seed afterwards, which is the order
# a real bootstrap uses.
git -C "$REPO_ROOT" worktree add --detach "$W1" "$HEAD_SHA" >/dev/null 2>&1 \
  || { echo "FAIL: could not create the reference worktree"; exit 1; }
git -C "$REPO_ROOT" worktree add --detach "$W2" "$HEAD_SHA" >/dev/null 2>&1 \
  || { echo "FAIL: could not create the destination worktree"; exit 1; }

REF_MARKER="SEEDTEST_ALPHA_a1a1"
DST_MARKER="SEEDTEST_BRAVO_b2b2"
inject_marker "$W1" "$REF_MARKER"
inject_marker "$W2" "$DST_MARKER"
ok "worktrees created; W1 carries $REF_MARKER, W2 carries $DST_MARKER"

# --- 2. publish the seed with PRODUCTION, built from W1 ---------------------
info "publishing the seed via seed_publish.sh (cold build of W1, BoringSSL included)…"
if ! SEED_REPO_ROOT="$W1" bash "$PUBLISH" --seeds-root "$SEEDS_ROOT" --features '' \
     >"$RUN_DIR/publish.log" 2>&1; then
  echo "FAIL: seed_publish.sh failed; log at $RUN_DIR/publish.log"
  tail -20 "$RUN_DIR/publish.log" >&2
  exit 1
fi
sed -n 's/^==> \(.*\)$/INFO: publish: \1/p' "$RUN_DIR/publish.log"
SEED="$SEEDS_ROOT/$(SEED_REPO_ROOT="$W1" bash "$SCRIPT_DIR/seed_compat_key.sh" --features '')"
[ -d "$SEED" ] || { echo "FAIL: no seed published at $SEED"; exit 1; }
[ -f "$SEED/manifest.toml" ] || { echo "FAIL: published seed carries no manifest"; exit 1; }
ok "seed published at ${SEED#"$SEEDS_ROOT"/}"

# --- 3. THE CONTRACT: the seed does not reference its own checkout -----------
# Stated observably. This is the same property the old test measured by counting
# webfang_* fingerprint directories; here the test does not know what a
# fingerprint is, only that the artifact carries no trace of the tree it was
# built from. If that ever becomes non-empty, some unit is path-dependent again
# and the contamination window is open.
STRAY="$(grep -rl -- "$W1" "$SEED" 2>/dev/null | wc -l || true)"
if [ "$STRAY" -eq 0 ]; then
  ok "the published seed references no path from its source checkout"
else
  bad "$STRAY file(s) in the seed still name the reference checkout, e.g."
  grep -rl -- "$W1" "$SEED" 2>/dev/null | head -3 | sed 's/^/        /'
fi

# Pruning must not cost the benefit: third-party units must survive it. Counted
# as files, not as cargo's internal unit directories.
REF_O="$(sed -n 's/.*reference built (.*, \([0-9]*\) objects).*/\1/p' "$RUN_DIR/publish.log" | head -1)"
KEPT_O="$(sed -n 's/.*C++ objects kept: \([0-9]*\).*/\1/p' "$RUN_DIR/publish.log" | head -1)"
SEED_O="$(find "$SEED" -name '*.o' 2>/dev/null | wc -l)"
if [ "${KEPT_O:-0}" -gt 0 ] && [ "${REF_O:-0}" = "${KEPT_O:-x}" ]; then
  ok "all $KEPT_O BoringSSL C++ objects survived pruning"
else
  bad "BoringSSL objects: built ${REF_O:-?}, publish kept ${KEPT_O:-?}, seed now holds $SEED_O"
fi
if [ -f "$SEED/debug/webfang" ]; then
  bad "the published seed still carries the final webfang binary"
else
  ok "the published seed carries no final binary"
fi

# --- 4. the failure's precondition, asserted --------------------------------
# W2's sources must be OLDER than the seed's artifacts, or Cargo takes the safe
# path and this test would verify nothing while reporting success. The manifest
# is written last during publication, so being older than it means being older
# than every artifact in the tree.
DST_SRC_MTIME="$(stat -c %Y "$W2/crates/webfang_cli/src/main.rs")"
SEED_MTIME="$(stat -c %Y "$SEED/manifest.toml")"
if [ "$DST_SRC_MTIME" -lt "$SEED_MTIME" ]; then
  ok "failure precondition present (W2 sources $DST_SRC_MTIME < seed artifacts $SEED_MTIME)"
else
  echo "FAIL: precondition not met — W2 sources ($DST_SRC_MTIME) are not older than"
  echo "      the seed's artifacts ($SEED_MTIME). The test would exercise the SAFE"
  echo "      path and verify nothing. Retry, or check the clock resolution."
  exit 1
fi

# --- 5. consume with PRODUCTION --------------------------------------------
info "seeding W2 via seed_target.sh…"
SEED_OUT="$(SEED_REPO_ROOT="$W2" bash "$TARGETER" --seeds-root "$SEEDS_ROOT" \
              --features '' --target-dir "$W2_TARGET" 2>&1)"
echo "      $SEED_OUT"
case "$SEED_OUT" in
  *"seed: seeded"*) ok "seed_target.sh seeded the destination" ;;
  *) bad "seed_target.sh did not seed the destination: $SEED_OUT" ;;
esac

# --- 6. the primary assertion: the binary that RUNS is W2's own -------------
build_w2 || { echo "FAIL: destination build failed; log at $RUN_DIR/build.log"; tail -20 "$RUN_DIR/build.log" >&2; exit 1; }
DST_BIN="$W2_TARGET/debug/webfang"
[ -x "$DST_BIN" ] || { echo "FAIL: no binary produced at $DST_BIN"; exit 1; }
GOT="$(binary_marker "$DST_BIN")"

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
if strings "$DST_BIN" 2>/dev/null | grep -q "$REF_MARKER"; then
  bad "the destination binary contains the reference marker ($REF_MARKER)"
else
  ok "strings: the destination binary does not contain the reference marker"
fi

# The benefit must still be there. A correct test that no longer exercises the
# fast path would be testing the cold fallback and calling it a seeding result.
COMPILED="$(grep -c '^ *Compiling' "$RUN_DIR/build.log" 2>/dev/null || true)"
if [ "${COMPILED:-0}" -lt 100 ]; then
  ok "the destination reused the seed ($COMPILED crates compiled, not a cold 493)"
else
  info "destination compiled $COMPILED crates — seeding saved nothing measurable here"
fi

echo "----------------------------------------"
if [ "$fail" -eq 0 ]; then
  echo "test_seed_contamination: GREEN"
else
  echo "test_seed_contamination: RED"
fi
exit "$fail"
