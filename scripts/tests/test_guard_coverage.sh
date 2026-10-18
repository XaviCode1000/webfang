#!/usr/bin/env bash
# Negation harness for scripts/test_guard_coverage.sh — issue #2056.
#
# A meta-guard that only ever prints OK is the defect it was written to catch.
# Every case below builds a throwaway repository in a mktemp directory and runs
# a COPY of the meta-guard against it, so the real tree is never mutated.
#
# What is proven:
#   - a wired guard with no harness fails the meta-guard
#   - a harness no workflow step runs fails it (#1707 shape)
#   - a harness with no `# NEGATION:` marker fails it
#   - a harness that is already red on an intact tree fails it
#   - a harness that stays green against a broken guard fails it
#   - a baseline entry that no longer reproduces fails it (the ratchet)
#   - a fully compliant fixture passes
#
# Usage: bash scripts/tests/test_guard_coverage.sh
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GUARD="$ROOT/scripts/test_guard_coverage.sh"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "ok   $1"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL $1"; }

# Builds a fixture repository. Every fixture gets its own copy of the meta-guard
# so ROOT resolves inside the fixture, never to the real tree.
new_fixture() { # name
  local dir="$SCRATCH/$1"
  mkdir -p "$dir/scripts/tests" "$dir/.github/workflows"
  cp "$GUARD" "$dir/scripts/test_guard_coverage.sh"
  printf '%s\n' "$dir"
}

# The guard under test: green on a clean tree, red when a `forbidden` file is
# present. Small on purpose — the fixture proves the meta-guard's logic, not
# this guard's.
write_guard() { # fixture-dir
  cat > "$1/scripts/check_foo.sh" <<'GUARD'
#!/usr/bin/env bash
set -uo pipefail
[ -f "forbidden" ] && { echo "forbidden present" >&2; exit 1; }
exit 0
GUARD
}

# A harness that catches a broken guard: asserts the clean case passes AND the
# forbidden case fails. Against `exit 0` the second case goes green, so the
# harness turns red — which is exactly what the mutation check looks for.
write_catching_harness() { # fixture-dir
  cat > "$1/scripts/test_foo.sh" <<'HARNESS'
#!/usr/bin/env bash
set -uo pipefail
G="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/check_foo.sh"
d="$(mktemp -d)"
trap 'rm -rf "$d"' EXIT
# NEGATION: a forbidden file must make the guard fail
( cd "$d" && bash "$G" ) >/dev/null 2>&1 || { echo "clean tree must pass" >&2; exit 1; }
touch "$d/forbidden"
if ( cd "$d" && bash "$G" ) >/dev/null 2>&1; then
  echo "guard must fail when forbidden is present" >&2
  exit 1
fi
echo "ok"
HARNESS
}

# A harness that asserts nothing negative: green no matter what the guard does.
write_toothless_harness() { # fixture-dir
  cat > "$1/scripts/test_foo.sh" <<'HARNESS'
#!/usr/bin/env bash
# NEGATION: declared, but the case below never checks for a failure
bash "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/check_foo.sh" >/dev/null 2>&1 || true
echo "ok"
HARNESS
}

# A harness that is red regardless of the guard's behaviour.
write_broken_harness() { # fixture-dir
  cat > "$1/scripts/test_foo.sh" <<'HARNESS'
#!/usr/bin/env bash
# NEGATION: never reached, this harness always fails
echo "always red" >&2
exit 1
HARNESS
}

# Wires only the guard, the way a real workflow step does. Used by the cases
# that assert the harness side is missing or unwired.
wire_guard_only() { # fixture-dir
  cat > "$1/.github/workflows/ci.yml" <<'YML'
name: fixture
jobs:
  guards:
    runs-on: ubuntu-latest
    steps:
      - name: guard step
        run: bash scripts/check_foo.sh
YML
}

# Wires the guard and its harness, the shape a compliant entry point has.
wire_both() { # fixture-dir
  cat > "$1/.github/workflows/ci.yml" <<'YML'
name: fixture
jobs:
  guards:
    runs-on: ubuntu-latest
    steps:
      - name: guard step
        run: bash scripts/check_foo.sh
      - name: harness step
        run: bash scripts/test_foo.sh
YML
}

run_guard() { # fixture-dir [args...]
  local dir="$1"
  shift
  ( cd "$dir" && bash scripts/test_guard_coverage.sh "$@" ) >"$dir/out.log" 2>&1
}

expect_fail_naming() { # fixture-dir needle label
  local dir="$1" needle="$2" label="$3"
  if run_guard "$dir" --no-mutation; then
    fail "$label: expected non-zero exit, got 0"
  elif grep -qF -- "$needle" "$dir/out.log"; then
    pass "$label"
  else
    fail "$label: exit was non-zero but no '$needle' in output"
    sed 's/^/       /' "$dir/out.log"
  fi
}

# --- case 1: wired guard, no harness ----------------------------------------
fx="$(new_fixture no-harness)"
write_guard "$fx"
wire_both "$fx"
expect_fail_naming "$fx" "no-harness: check_foo.sh" "wired guard without a harness fails"

# --- case 2: harness exists, no workflow step runs it ------------------------
fx="$(new_fixture unwired)"
write_guard "$fx"
write_catching_harness "$fx"
wire_guard_only "$fx"
expect_fail_naming "$fx" "unwired-harness: scripts/test_foo.sh" "unwired harness fails"

# --- case 3: harness with no negation marker --------------------------------
fx="$(new_fixture no-marker)"
write_guard "$fx"
write_catching_harness "$fx"
sed -i '/# NEGATION:/d' "$fx/scripts/test_foo.sh"
wire_both "$fx"
expect_fail_naming "$fx" "no-marker: scripts/test_foo.sh" "missing marker fails"

# --- case 4: harness already red on an intact tree --------------------------
fx="$(new_fixture broken-harness)"
write_guard "$fx"
write_broken_harness "$fx"
wire_both "$fx"
if run_guard "$fx"; then
  fail "broken harness: expected non-zero exit, got 0"
elif grep -qF "broken-harness: scripts/test_foo.sh" "$fx/out.log"; then
  pass "harness red on an intact tree fails"
else
  fail "broken harness: wrong violation reported"
  sed 's/^/       /' "$fx/out.log"
fi

# --- case 5: harness stays green against a broken guard ---------------------
fx="$(new_fixture no-mutation)"
write_guard "$fx"
write_toothless_harness "$fx"
wire_both "$fx"
if run_guard "$fx"; then
  fail "toothless harness: expected non-zero exit, got 0"
elif grep -qF "no-mutation: scripts/test_foo.sh" "$fx/out.log"; then
  pass "harness green against a broken guard fails"
else
  fail "toothless harness: wrong violation reported"
  sed 's/^/       /' "$fx/out.log"
fi

# --- case 6: baseline entry that no longer reproduces -----------------------
fx="$(new_fixture stale-baseline)"
write_guard "$fx"
write_catching_harness "$fx"
wire_both "$fx"
printf 'no-harness check_bar.sh no longer true\n' > "$fx/scripts/guard_coverage_baseline.txt"
expect_fail_naming "$fx" "stale-baseline: 'no-harness check_bar.sh'" "stale baseline entry fails"

# --- case 7: fully compliant fixture passes ---------------------------------
fx="$(new_fixture compliant)"
write_guard "$fx"
write_catching_harness "$fx"
wire_both "$fx"
: > "$fx/scripts/guard_coverage_baseline.txt"
if run_guard "$fx"; then
  pass "compliant fixture passes the mutation check"
else
  fail "compliant fixture: expected 0, got non-zero"
  sed 's/^/       /' "$fx/out.log"
fi

# --- case 8: the mutation is restored, never left in place ------------------
fx="$(new_fixture restore)"
write_guard "$fx"
write_catching_harness "$fx"
wire_both "$fx"
before="$(sha256sum "$fx/scripts/check_foo.sh" | cut -d' ' -f1)"
run_guard "$fx" >/dev/null 2>&1
after="$(sha256sum "$fx/scripts/check_foo.sh" | cut -d' ' -f1)"
if [ "$before" = "$after" ]; then
  pass "guard bytes are restored after the mutation run"
else
  fail "guard bytes changed after the mutation run"
fi

# --- summary ----------------------------------------------------------------

echo ""
if [ "$FAIL" -gt 0 ]; then
  echo "FAIL: $FAIL case(s) failed, $PASS passed"
  exit 1
fi
echo "OK: $PASS case(s) passed"
exit 0
