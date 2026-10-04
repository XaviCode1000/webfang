#!/usr/bin/env bash
# test_dependency_direction.sh
#
# Semantics harness for scripts/check_dependency_direction.sh (issue #1825),
# following the repo convention of a companion harness per guard
# (test_ignored_guard.sh, test_concurrency_lints.sh). It proves, on mktemp
# fixtures and fully offline (no repo state is read beyond the gate script
# itself), that the dependency-direction gate:
#   - passes a clean six-crate baseline;
#   - FAILS on a deliberately introduced forbidden edge (negative control);
#   - accepts the [dev-dependencies] tier into webfang_test_utils from any
#     crate, while a [dependencies] edge into the test harness stays forbidden;
#   - checks webfang_test_utils and webfang_benchmark as sources too;
#   - extracts the dotted-key form (`webfang_core.workspace = true`) that the
#     pre-#1825 regex silently ignored;
#   - fails closed when a crate manifest is missing.
#
# Case bodies are inline and sequenced directly (no function tables invoked
# through "$@"): CI runs shellcheck with no severity filter, and indirect
# invocation makes every case function read as dead code (SC2329).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GATE="$SCRIPT_DIR/check_dependency_direction.sh"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

declare -i cases_run=0 cases_failed=0 case_fail=0

# write_clean_manifest <crate> — minimal manifest with no internal edges.
write_clean_manifest() {
  local crate="$1"
  cat > "$tmp/crates/$crate/Cargo.toml" <<EOF
[package]
name = "$crate"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1"
EOF
}

# reset_fixture — a clean six-crate tree, so every case starts from scratch.
reset_fixture() {
  local crate
  for crate in webfang_core webfang_ai webfang_mcp webfang_cli webfang_test_utils webfang_benchmark; do
    mkdir -p "$tmp/crates/$crate"
    write_clean_manifest "$crate"
  done
}

# overlay_manifest <crate> <body> — replace one crate's manifest with a
# minimal [package] header plus the given body (dependencies tables etc.).
# Bodies are passed as $'...\n...' strings so the unquoted heredoc
# interpolates them as multi-line content.
overlay_manifest() {
  local crate="$1" body="$2"
  cat > "$tmp/crates/$crate/Cargo.toml" <<EOF
[package]
name = "$crate"
version = "0.1.0"
edition = "2021"

$body
EOF
}

# expect_gate <ok|fail> <must-appear-substring or -> <label>
# Runs the gate inside the fixture. The explicit `|| rc=$?` capture matters:
# under `set -e`, a bare `out=$(cmd)` would abort this script at the first
# expected-failure case instead of recording it.
expect_gate() {
  local expect="$1" needle="$2" label="$3" out rc
  rc=0
  out="$(cd "$tmp" && bash "$GATE" 2>&1)" || rc=$?
  if [[ "$expect" == "ok" ]]; then
    if [[ $rc -eq 0 ]]; then
      echo "  ok: $label"
    else
      echo "  FAIL: $label (expected exit 0, got $rc)"
      case_fail=1
    fi
  elif [[ $rc -eq 0 ]]; then
    echo "  FAIL: $label (expected nonzero exit, gate passed)"
    case_fail=1
  elif [[ "$needle" != "-" && "$out" != *"$needle"* ]]; then
    echo "  FAIL: $label (gate failed as expected but output lacks '$needle')"
    case_fail=1
  else
    echo "  ok: $label"
  fi
}

# finish_case <label> — account and report one completed case.
finish_case() {
  local label="$1"
  cases_run+=1
  if [[ $case_fail -eq 0 ]]; then
    echo "ok: case $cases_run — $label"
  else
    cases_failed+=1
    echo "FAIL: case $cases_run — $label"
  fi
  case_fail=0
}

# Case 1: a fully clean workspace passes.
reset_fixture
expect_gate ok - "gate exits 0 on a clean six-crate tree (no internal edges)"
finish_case "clean baseline passes"

# Case 2: negative control (#1825 acceptance criterion) — a deliberately
# introduced forbidden edge FAILS the gate.
reset_fixture
overlay_manifest webfang_ai $'[dependencies]\nwebfang_mcp = { workspace = true }'
expect_gate fail "must NOT depend on webfang_mcp" "forbidden edge webfang_ai -> webfang_mcp fails the gate (negative control)"
finish_case "negative control: forbidden prod edge fails"

# Case 3: a production edge into the test harness stays forbidden.
reset_fixture
overlay_manifest webfang_mcp $'[dependencies]\nwebfang_test_utils = { workspace = true }'
expect_gate fail "webfang_test_utils" "prod edge webfang_mcp -> webfang_test_utils stays forbidden"
finish_case "prod edge into webfang_test_utils forbidden"

# Case 4: the dev tier — any crate may dev-depend on webfang_test_utils.
reset_fixture
overlay_manifest webfang_core $'[dev-dependencies]\nwebfang_test_utils = { workspace = true }'
expect_gate ok - "dev-dependencies edge webfang_core -> webfang_test_utils accepted"
overlay_manifest webfang_benchmark $'[dev-dependencies]\nwebfang_test_utils = { workspace = true }'
expect_gate ok - "dev-dependencies edge webfang_benchmark -> webfang_test_utils accepted"
finish_case "dev-dependencies tier into test harness accepted"

# Case 5: webfang_test_utils is itself a checked source now.
reset_fixture
overlay_manifest webfang_test_utils $'[dependencies]\nwebfang_cli = { workspace = true }'
expect_gate fail "webfang_cli" "webfang_test_utils -> webfang_cli prod edge is rejected (test_utils is a checked source)"
finish_case "webfang_test_utils checked as a source"

# Case 6: the dotted-key form is extracted, not silently invisible.
# webfang_core -> webfang_cli is a FORBIDDEN pair (webfang_mcp would be
# allowed for webfang_cli and would not exercise rejection).
reset_fixture
overlay_manifest webfang_core $'[dependencies]\nwebfang_cli.workspace = true'
expect_gate fail "webfang_cli" "dotted-key form webfang_cli.workspace = true is extracted, not invisible"
finish_case "dotted-key form extracted"

# Case 7: a missing manifest fails closed.
reset_fixture
rm "$tmp/crates/webfang_ai/Cargo.toml"
expect_gate fail "missing manifest" "absent manifest fails the gate closed"
finish_case "missing manifest fails closed"

if [[ $cases_failed -eq 0 ]]; then
  echo "OK: $cases_run/7 dependency-direction gate semantics cases passed"
  exit 0
fi
echo "FAILED: $cases_failed/$cases_run dependency-direction gate semantics cases failed"
exit 1
