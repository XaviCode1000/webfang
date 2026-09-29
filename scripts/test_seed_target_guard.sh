#!/usr/bin/env bash
set -uo pipefail
# test_seed_target_guard.sh — the build-cache gate must refuse any CARGO_TARGET_DIR
# under the seed store, before Cargo is ever invoked.
#
# The requirement being tested is not "exits 2". It is "stops before Cargo", so
# the assertions are about a Cargo that DID NOT RUN: a stub `cargo` on PATH
# writes a marker every time it is called. A reject case passes only if the gate
# exited non-zero AND the marker is absent. An allow case is the converse — the
# marker must EXIST, which proves the guard let the build through rather than
# failing for some unrelated reason.
#
# The decision is on canonical path IDENTITY and nothing else. Case 9 is the one
# that pins this down: a path under the store that contains no seed at all is
# still refused, because a rule that peeked at the contents would be satisfied by
# an empty directory and would have to be re-argued every time the store changed.
#
# Manual: ~1 min. Not in CI, alongside the other seed tests.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE="$SCRIPT_DIR/ci_fast_gate.sh"
REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel)"

SANDBOX="$(mktemp -d)"
# The gate picks its lane from the changed paths, and an unmodified tree selects
# the CI-only lane — which never invokes cargo. That would make "the marker is
# absent" vacuous: cargo would be absent whether or not the guard fired, and the
# test would pass for the wrong reason. Staging one scratch .rs forces the CODE
# lane so that cargo IS reached whenever the guard lets the build through, which
# is what gives the marker its meaning in both directions. Untracked, and removed
# by the trap; nothing here touches git state.
SCRATCH="$REPO_ROOT/crates/webfang_core/src/zz_seed_guard_probe.rs"
trap 'rm -f "$SCRATCH"; chmod -R u+w "$SANDBOX" 2>/dev/null; rm -rf "$SANDBOX"' EXIT
printf '// scratch file: forces the CODE lane so cargo is reachable. Removed by the test.\n' >"$SCRATCH"
STORES="$SANDBOX/seeds"
MARKER="$SANDBOX/cargo-was-called"
mkdir -p "$SANDBOX/bin" "$STORES"

# A Cargo that announces itself instead of building.
cat >"$SANDBOX/bin/cargo" <<STUB
#!/usr/bin/env bash
echo "cargo \$*" >> "$MARKER"
exit 0
STUB
chmod +x "$SANDBOX/bin/cargo"

PASS=0; FAIL=0
ok()  { PASS=$((PASS+1)); printf '  ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL+1)); printf '  FAIL %s\n' "$1"; }

# Reject: must exit 2 AND must not have invoked Cargo.
expect_reject() {
  local label="$1" target="$2" out rc
  : >"$MARKER"
  out="$(cd "$REPO_ROOT" && env \
      PATH="$SANDBOX/bin:$PATH" \
      CARGO_TARGET_DIR="$target" \
      WEBFANG_SEEDS_ROOT="$STORES" \
      bash "$GATE" 2>&1)"; rc=$?
  if [ "$rc" -ne 2 ]; then
    bad "$label — expected exit 2, got $rc"; return
  fi
  if [ -s "$MARKER" ]; then
    bad "$label — rejected, but Cargo RAN first ($(head -1 "$MARKER"))"; return
  fi
  ok "$label → exit 2, Cargo never invoked"
}

# Allow: the guard must not fire. Proven by Cargo actually being reached.
expect_allow() {
  local label="$1" target="$2" out rc
  : >"$MARKER"
  out="$(cd "$REPO_ROOT" && env \
      PATH="$SANDBOX/bin:$PATH" \
      CARGO_TARGET_DIR="$target" \
      WEBFANG_SEEDS_ROOT="$STORES" \
      bash "$GATE" 2>&1)"; rc=$?
  if printf '%s' "$out" | grep -q 'points into the seed store'; then
    bad "$label — wrongly refused by the seed store guard"; return
  fi
  if [ ! -s "$MARKER" ]; then
    bad "$label — not refused by this guard, but Cargo was never reached either (rc=$rc); the test cannot tell allow from a different failure"
    printf '        last gate output: %s\n' "$(printf '%s' "$out" | tail -3 | tr '\n' ' ')"
    return
  fi
  ok "$label → allowed, Cargo reached ($(wc -l <"$MARKER") invocations)"
}

# A healthy, publishable-looking seed at the store root. Case 9 must be refused
# even when this exists, and refused just as firmly when it does not.
KEY="v1-deadbeefdeadbeef"
mkdir -p "$STORES/$KEY/debug"
printf 'key = "%s"\n' "$KEY" >"$STORES/$KEY/manifest.toml"

echo "seed store guard (identity only, pre-Cargo)"
echo
echo "reject:"
expect_reject "1. target = the store root"            "$STORES"
expect_reject "2. target = a seed"                    "$STORES/$KEY"
expect_reject "3. target = a child of a seed"          "$STORES/$KEY/debug"
expect_reject "4. target = not-yet-existing under seed" "$STORES/$KEY/future/nested"
# Created BEFORE the case, not after: a symlink that does not exist yet
# canonicalises to itself, which is under no store, and the case would pass for
# the wrong reason — exactly the "judged by accident of filesystem state" hole
# realpath -m is there to close.
ln -sfn "$STORES/$KEY" "$SANDBOX/link-to-seed"
expect_reject "5. target = symlink into a seed"        "$SANDBOX/link-to-seed"
expect_reject "6. target = ../ that normalises in"     "$STORES/../seeds/$KEY"
expect_reject "9. store holds no seed at all"          "$STORES/never-published"
# `..` that walks back INTO the store, over a leaf that was never created. Kept
# because a textual `..` test would get this backwards, and because it is the
# shape most likely to appear by accident in a hand-written .envrc.
#
# This case was originally added to separate realpath -m from readlink -f, and it
# does not: on this coreutils both resolve it identically, and mutating the guard
# to readlink -f leaves all ten cases green. The comment in ci_fast_gate.sh says
# the same. Recorded here so the next reader does not assume the two are pinned
# apart by the suite.
expect_reject "10. dotdot into the store, nothing created" "$STORES/../seeds/never-created-xyz"
echo
echo "allow:"
expect_allow "7. target = seeds-webfang (prefix only)" "$SANDBOX/seeds-webfang"
expect_allow "8. custom target named webfang elsewhere" "$SANDBOX/elsewhere/webfang"
echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
echo "identity-only, pre-Cargo: the guard cannot be satisfied by an empty directory"
