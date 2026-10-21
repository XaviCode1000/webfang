#!/usr/bin/env bash
# test_mcp_public_surface.sh
#
# Semantics harness for scripts/check_mcp_public_surface.sh (issue #1614).
#
# The guard answers one question: did the advertised MCP public surface change
# without a contract_version bump (or the other way round)? It is the half of
# that decision a Rust test cannot make, because the marker and the fixture
# live in the same reach: re-recording the fixture answers "did the version
# move?" with "yes" automatically.
#
# What this harness proves, offline (mktemp git repos; no cargo, no network):
#
#   1. a tree where nothing changed PASSES;
#   2. a COMMITTED breaking fixture edit with no version bump FAILS, with the
#      DEFAULT base and with an explicit one. The default used to be the last
#      commit that touched the fixture, so once the edit was committed the
#      guard compared the fixture with itself and read it as "unchanged";
#   3. an UNCOMMITTED breaking edit FAILS (the pre-commit case, which the old
#      default did catch);
#   4. a row added under a section the reader does not know FAILS. Unknown
#      sections used to be dropped from BOTH sides of the comparison, so a
#      breaking addition there read as "the surface did not change";
#   5. a version bump WITHOUT a surface change FAILS (a gratuitous bump);
#   6. a surface change WITH a version bump PASSES (the decision was made);
#   7. a base that is not a commit is rc 2, never a green "bootstrap".
#
# Usage: bash scripts/test_mcp_public_surface.sh

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE="$SCRIPT_DIR/check_mcp_public_surface.sh"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

TSV="crates/webfang_mcp/tests/fixtures/mcp_public_surface.tsv"
VER="crates/webfang_mcp/tests/fixtures/mcp_public_surface.version"

passed=0
failed=0

ok() {
  printf '  ok   %-52s %s\n' "$1" "${2:-}"
  passed=$((passed + 1))
}

bad() {
  printf '  FAIL %-52s %s\n' "$1" "$2"
  if [ -n "${3:-}" ]; then
    printf '%s\n' "$3" | sed 's/^/        /' | head -20
  fi
  failed=$((failed + 1))
}

# base_fixture — the minimal advertised surface.
base_fixture() {
  cat <<'EOF'
[tools]
get_scrape	properties.json	additionalProperties=false
[properties]
get_scrape_page	properties.json	required=false	type=string
[defs]
policy	JsonValue	consts=
EOF
}

# new_repo <name> — a fixture repo with surface v1 and contract_version 1.
new_repo() {
  local dir="$WORK/$1"
  mkdir -p "$dir/$(dirname "$TSV")"
  git -C "$dir" init -q -b main
  base_fixture >"$dir/$TSV"
  printf '1\n' >"$dir/$VER"
  git -C "$dir" add -A
  git -C "$dir" -c user.email=harness@local -c user.name=harness commit -q -m "surface v1"
  printf '%s' "$dir"
}

commit_all() { # <dir> <message>
  git -C "$1" add -A
  git -C "$1" -c user.email=harness@local -c user.name=harness commit -q -m "$2"
}

OUT=""
RC=0
run_gate() { # <dir> [args...] — the guard runs INSIDE the fixture repo
  local dir="$1"
  shift
  OUT="$(cd "$dir" && bash "$GATE" "$@" 2>&1)"
  RC=$?
}

expect_pass() { # <label> <dir> [args...]
  local label="$1" dir="$2"
  shift 2
  run_gate "$dir" "$@"
  if [ "$RC" -eq 0 ]; then
    ok "$label" "rc=0"
  else
    bad "$label" "expected PASS, got rc=$RC" "$OUT"
  fi
}

expect_fail() { # <label> <dir> [args...]
  local label="$1" dir="$2"
  shift 2
  run_gate "$dir" "$@"
  if [ "$RC" -ne 0 ]; then
    ok "$label" "rc=$RC"
  else
    bad "$label" "expected FAIL, got rc=0" "$OUT"
  fi
}

echo "check_mcp_public_surface semantics harness"

# --- 1. an untouched tree passes --------------------------------------------
# NEGATION: a guard that cannot pass anything is not a gate; this is the
# negative control for every FAIL case below.
D="$(new_repo clean)"
expect_pass "untouched surface passes" "$D"

# --- 2. a COMMITTED breaking edit without a version bump fails --------------
# NEGATION: the default base was the last commit that touched the fixture, so
# a committed edit compared the fixture with itself and the guard stayed green
# on a breaking change.
D="$(new_repo committed_edit)"
sed -i 's/^get_scrape\t/get_scrape_renamed\t/' "$D/$TSV"
commit_all "$D" "BREAKING: rename a tool without a version bump"
expect_fail "committed breaking edit fails (default base)" "$D"

# The same edit, compared against an explicit base: the decision must not
# depend on how the base was chosen.
expect_fail "committed breaking edit fails (explicit base)" "$D" --base HEAD~1

# --- 3. an UNCOMMITTED breaking edit fails ----------------------------------
# NEGATION: the pre-commit case is the one the local fast gate hits, and it
# must not depend on the base resolution either.
D="$(new_repo uncommitted_edit)"
sed -i 's/^get_scrape\t/get_scrape_renamed\t/' "$D/$TSV"
expect_fail "uncommitted breaking edit fails" "$D"

# --- 4. a row under an UNKNOWN section fails --------------------------------
# NEGATION: unknown sections used to be dropped from BOTH sides of the
# comparison, so a breaking addition there left SURFACE_CHANGED=0.
D="$(new_repo unknown_section)"
BASE="$(git -C "$D" rev-parse HEAD)"
printf '\n[policies]\nbudget_scopes\tproperties.json\n' >>"$D/$TSV"
commit_all "$D" "BREAKING: add a new section"
expect_fail "row under an unknown section fails" "$D" --base "$BASE"

# Control: the same shape inside a KNOWN section is caught as well.
D="$(new_repo known_section)"
BASE="$(git -C "$D" rev-parse HEAD)"
printf '\n[tools]\nbudget_snapshot\tproperties.json\tadditionalProperties=false\n' >>"$D/$TSV"
commit_all "$D" "add a tool row"
expect_fail "row under a known section fails" "$D" --base "$BASE"

# --- 5. a version bump without a surface change fails -----------------------
# NEGATION: a bump with no change silences this guard without a reason, so it
# must be reported rather than accepted.
D="$(new_repo gratuitous_bump)"
printf '2\n' >"$D/$VER"
expect_fail "version bump without a surface change fails" "$D"

# --- 6. a surface change WITH a version bump passes -------------------------
# NEGATION: the guard must not reject the legitimate path, or it teaches
# everyone to bump nothing.
D="$(new_repo declared_change)"
sed -i 's/^get_scrape\t/get_scrape_renamed\t/' "$D/$TSV"
printf '2\n' >"$D/$VER"
expect_pass "surface change with a version bump passes" "$D"

# --- 7. a base that is not a commit is rc 2, never a green bootstrap -------
# NEGATION: an unresolvable base used to read as "nothing to compare" — a
# green gate on a change nobody looked at.
D="$(new_repo bad_base)"
expect_fail "a base that is not a commit fails" "$D" --base origin/does-not-exist
run_gate "$D" --base origin/does-not-exist
if [ "$RC" -eq 2 ]; then
  ok "a base that is not a commit is rc 2" "rc=2"
else
  bad "a base that is not a commit is rc 2" "expected rc=2, got rc=$RC" "$OUT"
fi

# --- 8. a missing fixture is a failure, not a bootstrap --------------------
# NEGATION: an absent fixture and an unreadable one are different answers,
# and neither is a pass.
D="$(new_repo missing_fixture)"
mv "$D/$TSV" "$WORK/moved.tsv"
expect_fail "a missing fixture fails" "$D"

echo
echo "check_mcp_public_surface semantics harness: $passed passed, $failed failed"
[ "$failed" -eq 0 ]
