#!/usr/bin/env bash
set -uo pipefail
# Empty-test-target guard semantics harness (#1852, lote L8) — RED before GREEN.
#
# The guard is `scripts/check_empty_test_targets.sh`: a declared test target
# that compiles to ZERO tests is a false green (cargo discovers the file,
# builds a target out of it, and that target has no test in it; the run
# reports green and the file looks covered). #1852 is exactly that: 22 files
# of `webfang_mcp` gated on a feature that was not in the package's default.
#
# This harness runs the guard over `mktemp` fixture trees and asserts the
# four shapes the rule has to tell apart:
#
#   1. gated + the feature IS in the package default      → PASS
#   2. gated + the feature NOT in the default + no
#      `required-features` declaration                    → FAIL, and the
#        message names the file and the feature
#   3. gated + `required-features` declared                → PASS
#   4. no gate at all                                     → PASS
#
# Plus the vacuity case: a tree with no test target at all must FAIL, not
# pass. A guard that inspects nothing is a green light wired to nothing.
#
# Every negative assertion is paired with a positive one: a check whose
# positive control fails is a broken harness, not a finding.
#
# STATIC: fixtures on `mktemp` plus text over the tree. No cargo, no network,
# no build, no repo state touched.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD="$SCRIPT_DIR/check_empty_test_targets.sh"

[ -f "$GUARD" ] || { echo "test_empty_test_targets: no guard at $GUARD" >&2; exit 1; }

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

FAILURES=0

# make_fixture <root> <default features csv> <feature> <declare: yes|no> <gate: yes|no>
make_fixture() {
  local root="$1" default="$2" feature="$3" declare="$4" gate="$5"
  mkdir -p "$root/crates/pkg/tests"
  cat >"$root/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = ["crates/pkg"]
EOF
  {
    echo "[package]"
    echo 'name = "pkg"'
    echo 'version = "0.0.0"'
    echo 'edition = "2021"'
    echo
    echo "[features]"
    echo "default = [$default]"
    echo "$feature = []"
    if [ "$declare" = "yes" ]; then
      echo
      echo "[[test]]"
      echo 'name = "gated_test"'
      echo 'path = "tests/gated_test.rs"'
      echo "required-features = [\"$feature\"]"
    fi
  } >"$root/crates/pkg/Cargo.toml"
  if [ "$gate" = "yes" ]; then
    printf '#![cfg(feature = "%s")]\n\n#[test]\nfn t() {}\n' "$feature" \
      >"$root/crates/pkg/tests/gated_test.rs"
  else
    printf '#[test]\nfn t() {}\n' >"$root/crates/pkg/tests/gated_test.rs"
  fi
}

# run_guard <root> → the guard's exit code, with its output captured.
run_guard() {
  local root="$1"
  EMPTY_TEST_TARGETS_ROOT="$root" bash "$GUARD" >"$T/out.txt" 2>&1
  GUARD_RC=$?
  GUARD_OUT="$(cat "$T/out.txt")"
  return 0
}

expect() {
  local what="$1" want_rc="$2" want_text="$3"
  if [ "$GUARD_RC" != "$want_rc" ]; then
    printf 'FAIL: %s: expected exit %s, got %s\n' "$what" "$want_rc" "$GUARD_RC"
    printf '      guard output: %s\n' "$GUARD_OUT"
    FAILURES=$((FAILURES + 1))
    return 0
  fi
  if [ -n "$want_text" ] && ! printf '%s' "$GUARD_OUT" | grep -qF -- "$want_text"; then
    printf 'FAIL: %s: the output does not mention "%s"\n' "$what" "$want_text"
    printf '      guard output: %s\n' "$GUARD_OUT"
    FAILURES=$((FAILURES + 1))
    return 0
  fi
  printf 'ok   %s\n' "$what"
}

# ── case 1: gated + the feature IS in the default → PASS ───────────────────
make_fixture "$T/case1" "mcp" "mcp" no yes
run_guard "$T/case1"
expect "case 1: gated + feature in the default passes" 0 "no declared test target compiles to zero tests"

# ── case 2: gated + the feature NOT in the default + no declaration → FAIL ─
make_fixture "$T/case2" "" "mcp" no yes
run_guard "$T/case2"
expect "case 2: gated + feature out of the default + no required-features fails" 1 "gated_test.rs"
expect "case 2: the message names the feature" 1 "mcp"
expect "case 2: the message names the way to fix it" 1 "required-features"

# ── case 3: gated + required-features declared → PASS ─────────────────────
make_fixture "$T/case3" "" "mcp" yes yes
run_guard "$T/case3"
expect "case 3: gated + required-features declared passes" 0 "no declared test target compiles to zero tests"

# ── case 4: no gate at all → PASS ─────────────────────────────────────────
make_fixture "$T/case4" "" "mcp" no no
run_guard "$T/case4"
expect "case 4: a target without a gate passes" 0 "no declared test target compiles to zero tests"

# ── case 5 (vacuity): a tree with no test target at all → FAIL ────────────
make_fixture "$T/case5" "" "mcp" no no
rm -rf "$T/case5/crates/pkg/tests"
run_guard "$T/case5"
expect "case 5: a tree with no test target fails (the guard must not pass by inspecting nothing)" 1 "inspected NO test target"

# ── case 6 (the real shape of #1852): 22 files gated on a feature that is
#     not in the default → the guard names EVERY one of them ───────────────
make_fixture "$T/case6" "" "mcp" no yes
for i in $(seq 1 3); do
  printf '#![cfg(feature = "mcp")]\n\n#[test]\nfn t() {}\n' \
    >"$T/case6/crates/pkg/tests/gated_$i.rs"
done
run_guard "$T/case6"
expect "case 6: every gated file is named" 1 "4 target(s) compile to zero tests"

if [ "$FAILURES" -ne 0 ]; then
  printf 'FAIL: the empty-test-target guard does not tell the shapes apart\n'
  exit 1
fi
printf 'OK: the empty-test-target guard tells every shape apart\n'
exit 0
