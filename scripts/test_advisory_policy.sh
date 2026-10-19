#!/usr/bin/env bash
# Negation harness for scripts/check_advisory_policy.sh — issue #2057 (S3).
#
# The defect this pins: `collect_duplicates` ran `cargo deny check bans ... ||
# true` and then `return 0`, so a broken producer and a clean tree produced the
# same empty set. The ratchet at the call site (`if ! collect_duplicates ...;
# then exit 1; fi`) was dead code and a failed `cargo deny` read as "no
# duplicates".
#
# No real cargo and no network: a fake `cargo` on PATH drives the producer, and
# the guard's own env overrides (`ADVISORY_POLICY_BASELINE`,
# `ADVISORY_POLICY_DENY_BANS_OUTPUT`) keep the real tree untouched.
#
# Usage: bash scripts/test_advisory_policy.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="$ROOT/scripts/check_advisory_policy.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

PASS=0
FAIL=0
pass() { PASS=$((PASS + 1)); echo "ok   $1"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL $1"; }

# fake_cargo <exit-code> [stdout]
# A PATH entry with a `cargo` that answers `deny check bans` without cargo.
# The prefix also carries DEFAULT stubs for the standalone binaries, so the
# guard's two tool gates (`command -v cargo-deny`, `command -v cargo-audit`)
# resolve to the sandbox and the guard reaches the fake `cargo` on EVERY
# machine. Without them, a runner with neither binary installed fails at the
# tool gate — showing the fail-closed refusal instead of driving the fake
# producer — the exact mismatch observed on the first CI run of this step. Silent no-ops: the guard invokes the standalone binaries
# nowhere; all tool output travels through `cargo deny` / `cargo audit`.
fake_cargo() { # name exit-code body
  local dir="$SCRATCH/$1"
  mkdir -p "$dir/bin"
  {
    printf '#!/usr/bin/env bash\n'
    printf 'printf %%s %q\n' "$3"
    printf 'exit %s\n' "$2"
  } > "$dir/bin/cargo"
  chmod +x "$dir/bin/cargo"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$dir/bin/cargo-deny"
  printf '#!/usr/bin/env bash\nexit 0\n' > "$dir/bin/cargo-audit"
  chmod +x "$dir/bin/cargo-deny" "$dir/bin/cargo-audit"
  printf '%s\n' "$dir/bin"
}

run_guard() { # path-prefix [args...]
  local prefix="$1"
  shift
  ( cd "$ROOT" && PATH="$prefix:$PATH" bash "$GUARD" "$@" ) >"$SCRATCH/out" 2>&1
}

# --- case 1: a FAILED producer fails the guard ------------------------------
# NEGATION: `cargo deny` exiting non-zero must NOT read as "no duplicates".
prefix="$(fake_cargo failing 1 'error: failed to fetch advisory database')"
if run_guard "$prefix"; then
  fail "a failed cargo deny must fail the guard"
elif grep -qE 'cargo deny check bans falló|no es confiable' "$SCRATCH/out"; then
  pass "a failed cargo deny fails the guard, loudly"
else
  fail "a failed cargo deny failed the guard but without explaining why"
  sed 's/^/       /' "$SCRATCH/out"
fi

# --- case 2: a NEW duplicate fails the guard -------------------------------
# NEGATION: a duplicate absent from the baseline must fail, not warn.
printf 'known-crate\n' > "$SCRATCH/baseline.txt"
prefix="$(fake_cargo dup 0 "warning[duplicate]: found 2 duplicate entries for crate 'surprise-crate'
warning[duplicate]: found 2 duplicate entries for crate 'known-crate'")"
if ADVISORY_POLICY_BASELINE="$SCRATCH/baseline.txt" run_guard "$prefix"; then
  fail "a duplicate missing from the baseline must fail the guard"
elif grep -qF 'surprise-crate' "$SCRATCH/out"; then
  pass "a duplicate missing from the baseline fails the guard and names it"
else
  fail "a new duplicate failed the guard without naming the crate"
  sed 's/^/       /' "$SCRATCH/out"
fi

# --- case 3: a baselined duplicate still passes ----------------------------
prefix="$(fake_cargo dup-ok 0 "warning[duplicate]: found 2 duplicate entries for crate 'known-crate'")"
if ADVISORY_POLICY_BASELINE="$SCRATCH/baseline.txt" run_guard "$prefix"; then
  pass "a duplicate already in the baseline passes"
else
  fail "a baselined duplicate must not fail the guard"
  sed 's/^/       /' "$SCRATCH/out"
fi

# --- case 4: a missing producer file fails closed --------------------------
# NEGATION: the override path naming a file that does not exist must fail.
prefix="$(fake_cargo unused 0 '')"
if ADVISORY_POLICY_DENY_BANS_OUTPUT="$SCRATCH/does-not-exist.txt" run_guard "$prefix"; then
  fail "a missing ADVISORY_POLICY_DENY_BANS_OUTPUT file must fail the guard"
elif grep -qF 'no existe' "$SCRATCH/out"; then
  pass "a missing producer file fails the guard, loudly"
else
  fail "a missing producer file failed the guard without explaining why"
  sed 's/^/       /' "$SCRATCH/out"
fi

echo ""
if [ "$FAIL" -gt 0 ]; then
  echo "FAIL: $FAIL case(s) failed, $PASS passed"
  exit 1
fi
echo "OK: $PASS case(s) passed"
exit 0
