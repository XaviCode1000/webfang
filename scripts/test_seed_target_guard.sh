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
QSTORE="$SANDBOX/quarantine"
mkdir -p "$QSTORE"
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
        WEBFANG_QUARANTINE_ROOT="$QSTORE" \
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
        WEBFANG_QUARANTINE_ROOT="$QSTORE" \
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
  # --- quarantine store: the same rule, enforced rather than documented ------
  # Until this existed, AGENTS.md was the only thing stopping an agent from
  # pointing CARGO_TARGET_DIR at a quarantined object. The entries are real Cargo
  # target dirs, so cargo would have compiled into one without complaint, and a
  # rule that exists only in prose is one an agent can ignore by writing a
  # different .envrc. Measured before the check: exit 0, accepted.
  mkdir -p "$QSTORE/main-shared-478g" "$QSTORE/seeds/v1-deadbeef"
  expect_reject "11. target = quarantine root"               "$QSTORE"
  expect_reject "12. target = quarantined main target"       "$QSTORE/main-shared-478g"
  expect_reject "13. target = quarantined seed"              "$QSTORE/seeds/v1-deadbeef"
  expect_reject "14. dotdot into quarantine, nothing created" "$QSTORE/../quarantine/never-created"
  ln -sfn "$QSTORE/main-shared-478g" "$SANDBOX/q-link"
  expect_reject "15. symlink resolving into quarantine"      "$SANDBOX/q-link"
  expect_allow "16. target = quarantine-webfang (prefix only)" "$SANDBOX/quarantine-webfang"
  expect_allow "17. quarantine-named target elsewhere"         "$SANDBOX/elsewhere/quarantine"
echo "allow:"
expect_allow "7. target = seeds-webfang (prefix only)" "$SANDBOX/seeds-webfang"
expect_allow "8. custom target named webfang elsewhere" "$SANDBOX/elsewhere/webfang"
# --- 10. a path that cannot be canonicalised is refused, not guessed ---------
# The guard's previous canonicalisation fell back to the raw input when realpath
# failed, so a path it could not classify was compared as a plain string against
# a canonical root — a fail-OPEN guard on exactly the input it could not
# understand. Forcing realpath to fail is the only way to reach that branch.
cat >"$SANDBOX/bin/realpath" <<'STUB'
#!/usr/bin/env bash
exit 1          # canonicalisation is impossible in this scenario
STUB
chmod +x "$SANDBOX/bin/realpath"
: >"$MARKER"
out="$(cd "$REPO_ROOT" && env \
    PATH="$SANDBOX/bin:$PATH" \
    CARGO_TARGET_DIR="$SANDBOX/elsewhere/webfang" \
    WEBFANG_SEEDS_ROOT="$STORES" \
        WEBFANG_QUARANTINE_ROOT="$QSTORE" \
    bash "$GATE" 2>&1)"; rc=$?
if [ "$rc" -eq 2 ]; then
  ok "10. unresolvable path → exit 2 (fail-closed, not fail-open)"
else
  bad "10. unresolvable path → exit $rc; it must be 2"
fi
case "$out" in
  *"could not canonicalise"*) ok "10. it says why: identity could not be established" ;;
  *) bad "10. no explanation: $(printf '%s' "$out" | head -1)" ;;
esac
if [ -s "$MARKER" ]; then
  bad "10. Cargo ran despite the unresolvable path"
else
  ok "10. Cargo never invoked"
fi
rm -f "$SANDBOX/bin/realpath"

# ── 11. the remediation must not hand back the bootstrap we removed ──────────
#
# The guard caught a worktree pointing at main's target and told the operator to
# recover with a `sed` over main's .envrc. That recipe is gone. AGENTS.md records
# why it had to go: it made main's target name a load-bearing input to every
# future worktree, and when main moved off `cargo-target/webfang` the substitution
# silently stopped matching and emitted a perfectly VALID CARGO_TARGET_DIR
# pointing at main's target. Silent, and caught only here, at the build.
#
# So the message printed on refusal is part of what this gate is worth. A correct
# verdict paired with a remediation that reintroduces the failure is not a pass,
# which is why this is asserted rather than assumed.
MAIN_ROOT_T="$(dirname "$(git -C "$REPO_ROOT" rev-parse --path-format=absolute --git-common-dir)")"
MAIN_TARGET="$(sed -n 's/^export CARGO_TARGET_DIR=//p' "$MAIN_ROOT_T/.envrc" 2>/dev/null | tail -1)"
if [ -z "$MAIN_TARGET" ]; then
  echo "SKIP 11. main's checkout is not bootstrapped; this suite already depends on it"
else
  : >"$MARKER"
  out="$(cd "$REPO_ROOT" && env \
      PATH="$SANDBOX/bin:$PATH" \
      CARGO_TARGET_DIR="$MAIN_TARGET" \
      WEBFANG_SEEDS_ROOT="$STORES" \
        WEBFANG_QUARANTINE_ROOT="$QSTORE" \
      bash "$GATE" 2>&1)"; rc=$?
  if [ "$rc" -ne 2 ]; then
    bad "11. main's target → expected exit 2, got $rc"
  elif [ -s "$MARKER" ]; then
    bad "11. rejected, but Cargo RAN first"
  else
    ok "11. main's target → exit 2, Cargo never invoked"
  fi
  case "$out" in
    *"main's target dir"*) ok "11. it names the collision it found" ;;
    *) bad "11. no explanation: $(printf '%s' "$out" | head -1)" ;;
  esac
  # The regression itself: any remediation that rewrites main's .envrc.
  case "$out" in
    *"sed -e"*".envrc"*|*"'\$MAIN_ROOT/.envrc'"*)
      bad "11. remediation tells the operator to rewrite main's .envrc — the recipe that caused this failure" ;;
    *)
      ok "11. remediation does not rewrite main's .envrc" ;;
  esac
  case "$out" in
    *"Do NOT derive it by rewriting main's .envrc"*)
      ok "11. remediation says why, not only what" ;;
    *)
      bad "11. remediation forbids the old recipe without explaining the failure it caused" ;;
  esac
  case "$out" in
    *"cargo-target/$(basename "$REPO_ROOT")"*)
      ok "11. remediation names this tree's own target dir" ;;
    *)
      bad "11. remediation does not name a concrete target dir for this tree" ;;
  esac
fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
echo "identity-only, pre-Cargo: the guard cannot be satisfied by an empty directory"
