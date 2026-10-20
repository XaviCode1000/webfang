#!/usr/bin/env bash
# test_dependency_direction.sh
#
# Semantics harness for scripts/check_dependency_direction.sh (issue #1825),
# following the repo convention of a companion harness per guard
# (test_ignored_guard.sh, test_concurrency_lints.sh). It proves, on mktemp
# fixtures and fully offline (no repo state is read beyond the gate script
# itself), that the dependency-direction gate:
#   - passes a clean five-crate baseline (post-ADR-0004 paso 6: the
#     webfang_ai crate is gone);
#   - FAILS on a deliberately introduced forbidden edge (negative control);
#   - accepts the [dev-dependencies] tier into webfang_test_utils from any
#     crate, while a [dependencies] edge into the test harness stays forbidden;
#   - checks webfang_test_utils and webfang_benchmark as sources too;
#   - extracts the dotted-key form (`webfang_core.workspace = true`) that the
#     pre-#1825 regex silently ignored;
#   - fails closed when a crate manifest is missing.
#
# Since the gate reads the REAL build graph (`cargo metadata --no-deps
# --offline`, see the gate header), the fixtures are cargo-metadata
# compatible: each one is a valid virtual workspace with resolvable path
# dependencies, a lib target per member, and NO external dependency (so the
# fixture never needs the registry and the run stays offline). A fixture that
# cannot produce metadata does NOT silently pass: the row asserts the gate's
# own fail-closed behaviour for that case.
#
# Rows added for the 2026-10-18 "scripts fail-open" audit (B33/B34/B35):
#   - B33: a dependency declared as a SUB-TABLE (`[dependencies.X]`) and the
#     target-specific variant (`[target.'cfg(unix)'.dependencies.X]`) are
#     rejected — the old header-matching extractor saw neither;
#   - B34: a renamed dependency (`key = { package = "real", … }`) is reported
#     under the REAL package, not under the manifest key;
#   - B35: a workspace member the policy does not know is a named error
#     (policy gap), and a [build-dependencies] edge is a violation like any
#     other build-graph edge.
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

# write_workspace_manifest — the fixture root: a virtual workspace over the
# five policy crates, with the internal path dependencies the members
# inherit through `= { workspace = true }`.
write_workspace_manifest() {
  cat > "$tmp/Cargo.toml" <<'EOF'
[workspace]
resolver = "2"
members = [
  "crates/webfang_core",
  "crates/webfang_mcp",
  "crates/webfang_cli",
  "crates/webfang_test_utils",
  "crates/webfang_benchmark",
]
exclude = ["fuzz"]

[workspace.dependencies]
webfang_core = { path = "crates/webfang_core" }
webfang_mcp = { path = "crates/webfang_mcp" }
webfang_cli = { path = "crates/webfang_cli" }
webfang_test_utils = { path = "crates/webfang_test_utils" }
webfang_benchmark = { path = "crates/webfang_benchmark" }
EOF
}

# write_clean_manifest <crate> — minimal manifest with no internal edges and
# no external dependency (the fixture must resolve offline).
write_clean_manifest() {
  local crate="$1"
  cat > "$tmp/crates/$crate/Cargo.toml" <<EOF
[package]
name = "$crate"
version = "0.1.0"
edition = "2021"
EOF
}

# add_crate <crate> — an extra crate directory with a lib target, used to
# model a brand-new internal crate the policy does not know.
add_crate() {
  local crate="$1"
  mkdir -p "$tmp/crates/$crate/src"
  : > "$tmp/crates/$crate/src/lib.rs"
  cat > "$tmp/crates/$crate/Cargo.toml" <<EOF
[package]
name = "$crate"
version = "0.1.0"
edition = "2021"
EOF
}

# reset_fixture — a clean five-crate tree, so every case starts from scratch.
reset_fixture() {
  local crate
  for crate in webfang_core webfang_mcp webfang_cli webfang_test_utils webfang_benchmark; do
    mkdir -p "$tmp/crates/$crate/src"
    : > "$tmp/crates/$crate/src/lib.rs"
    write_clean_manifest "$crate"
  done
  write_workspace_manifest
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

# expect_gate <ok|fail> <must-appear-substring or -> <label> [must-not-appear]
# Runs the gate inside the fixture. The explicit `|| rc=$?` capture matters:
# under `set -e`, a bare `out=$(cmd)` would abort this script at the first
# expected-failure case instead of recording it.
expect_gate() {
  local expect="$1" needle="$2" label="$3" absent="${4:--}" out rc
  rc=0
  out="$(cd "$tmp" && bash "$GATE" 2>&1)" || rc=$?
  if [[ "$expect" == "ok" ]]; then
    if [[ $rc -ne 0 ]]; then
      echo "  FAIL: $label (expected exit 0, got $rc)"
      case_fail=1
    elif [[ "$needle" != "-" && "$out" != *"$needle"* ]]; then
      echo "  FAIL: $label (gate passed but output lacks '$needle')"
      case_fail=1
    else
      echo "  ok: $label"
    fi
  elif [[ $rc -eq 0 ]]; then
    echo "  FAIL: $label (expected nonzero exit, gate passed)"
    case_fail=1
  elif [[ "$needle" != "-" && "$out" != *"$needle"* ]]; then
    echo "  FAIL: $label (gate failed as expected but output lacks '$needle')"
    case_fail=1
  elif [[ "$absent" != "-" && "$out" == *"$absent"* ]]; then
    echo "  FAIL: $label (gate failed as expected but output contains '$absent')"
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
expect_gate ok - "gate exits 0 on a clean five-crate tree (no internal edges)"
finish_case "clean baseline passes"

# Case 2: negative control (#1825 acceptance criterion) — a deliberately
# introduced forbidden edge FAILS the gate. The pair changed with ADR-0004
# paso 6 (webfang_ai no longer exists): webfang_core -> webfang_mcp keeps the
# probe symmetric (core has no allowed edge at all).
reset_fixture
overlay_manifest webfang_core $'[dependencies]\nwebfang_mcp = { workspace = true }'
expect_gate fail "must NOT depend on webfang_mcp" "forbidden edge webfang_core -> webfang_mcp fails the gate (negative control)"
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
rm "$tmp/crates/webfang_benchmark/Cargo.toml"
expect_gate fail "missing manifest" "absent manifest fails the gate closed"
finish_case "missing manifest fails closed"

# Case 8 (audit B33): a dependency declared as a SUB-TABLE is a real edge.
# The old extractor only matched the exact headers `[(dev-)?dependencies]`
# and `[target.*dependencies]`, so `[dependencies.X]` was invisible.
reset_fixture
overlay_manifest webfang_core $'[dependencies.webfang_benchmark]\npath = "../webfang_benchmark"'
expect_gate fail "must NOT depend on webfang_benchmark" "sub-table form [dependencies.webfang_benchmark] is rejected"
finish_case "B33: dependency sub-table form rejected"

# Case 9 (audit B33): the target-specific sub-table is a real edge too.
reset_fixture
overlay_manifest webfang_core $'[target.\'cfg(unix)\'.dependencies.webfang_benchmark]\npath = "../webfang_benchmark"'
expect_gate fail "must NOT depend on webfang_benchmark" "target-specific sub-table [target.'cfg(unix)'.dependencies.webfang_benchmark] is rejected"
finish_case "B33: target-specific dependency sub-table rejected"

# Case 10 (audit B34): the REAL package is the forbidden one, not the
# manifest key. `webfang_core = { package = "webfang_benchmark", … }` links
# webfang_benchmark; the old extractor reported the allowed key
# webfang_core and the edge passed.
reset_fixture
overlay_manifest webfang_test_utils $'[dependencies]\nwebfang_core = { package = "webfang_benchmark", path = "../webfang_benchmark" }'
expect_gate fail "must NOT depend on webfang_benchmark" "renamed dependency is reported under its real package" "must NOT depend on webfang_core"
finish_case "B34: renamed dependency reported under the real package"

# Case 11 (audit B35): a brand-new internal crate is a POLICY GAP — the
# gate names it instead of silently allowing it (and never auto-allows it).
reset_fixture
add_crate webfang_web
overlay_manifest webfang_test_utils $'[dependencies]\nwebfang_web = { path = "../webfang_web" }'
expect_gate fail "unknown workspace member 'webfang_web'" "unknown workspace member webfang_web is a named error"
finish_case "B35: unknown workspace member reported by name"

# Case 12 (audit B35): a [build-dependencies] edge links the crate into the
# build graph exactly like a normal edge, so it is a violation too (the
# documented build-dependency decision).
reset_fixture
overlay_manifest webfang_test_utils $'[build-dependencies]\nwebfang_cli = { path = "../webfang_cli" }'
expect_gate fail "build-dependency" "build-dependencies edge webfang_test_utils -> webfang_cli is rejected"
finish_case "B35: build-dependencies edge rejected"

# Case 13: CONTROL — the healthy workspace still passes, and says so.
reset_fixture
expect_gate ok "OK: inter-crate dependency direction matches policy" "healthy workspace passes with the OK marker"
finish_case "control: healthy workspace passes"

# Case 14: the webfang_ai ghost (removed in ADR-0004 paso 6) is not
# invisible: a surviving webfang_ai edge is reported, never silently OK.
reset_fixture
add_crate webfang_ai
overlay_manifest webfang_core $'[dependencies]\nwebfang_ai = { path = "../webfang_ai" }'
expect_gate fail "webfang_ai" "a surviving webfang_ai edge is reported, not silently allowed"
finish_case "webfang_ai ghost is not invisible"

# Case 15: a crate directory OUTSIDE the workspace (fuzz/ is excluded in the
# real root manifest) is not a member, so it does not reach the policy.
reset_fixture
mkdir -p "$tmp/fuzz/src"
: > "$tmp/fuzz/src/lib.rs"
cat > "$tmp/fuzz/Cargo.toml" <<'EOF'
[workspace]

[package]
name = "webfang_fuzz"
version = "0.1.0"
edition = "2021"
EOF
expect_gate ok - "a crate outside the workspace does not affect the policy"
finish_case "crate outside the workspace stays outside the policy"

# Case 16: the graph cannot be produced -> the gate fails CLOSED, it never
# degrades to "no edges found" (the fail-open shape this rewrite removes).
reset_fixture
overlay_manifest webfang_core $'[dependencies]\nwebfang_ghost = { path = "../does-not-exist" }'
expect_gate fail "cargo metadata failed" "an unresolvable path dependency fails the gate closed"
finish_case "unresolvable manifest fails closed"

if [[ $cases_failed -eq 0 ]]; then
  echo "OK: $cases_run/16 dependency-direction gate semantics cases passed"
  exit 0
fi
echo "FAILED: $cases_failed/$cases_run dependency-direction gate semantics cases failed"
exit 1
