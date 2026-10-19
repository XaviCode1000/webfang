#!/usr/bin/env bash
set -euo pipefail
# Compatibility harness — Sprint 0 Gate 0
# Loops 5 CI-required combos + 1 pairwise spot-check (local/nightly).
#
# The `ai` and `ai+persistence` combos were retired with the `ai` feature and
# the `webfang_ai` crate (ADR-0004 paso 6); COMPATIBILITY-MATRIX.md records
# the surviving matrix: 5 required combos + 1 pairwise.
# Usage: bash scripts/check_compatibility.sh --ci-required | --all | --self-test | --help
# Each combo verifies: compile | start | --help | crawl | resume | failure-path

MODE="ci-required"
for arg in "$@"; do
  case "$arg" in
    --ci-required) MODE="ci-required" ;;
    --all) MODE="all" ;;
    --self-test) MODE="self-test" ;;
    --help|-h) echo "Usage: $0 [--ci-required|--all|--self-test]"; echo "  --ci-required  5 required combos (CI)"; echo "  --all          5 + 1 pairwise (local/nightly)"; echo "  --self-test    unit-check the pure helpers (no cargo, no network)"; exit 0 ;;
    *) echo "Unknown arg: $arg" >&2; exit 2 ;;
  esac
done

COMBOS_CI=(
  "default:default"
  "no-default:--no-default-features"
  "chromium:chromium"
  "mcp:mcp"
  "full:full"
)
COMBOS_PAIRWISE=(
  "mcp+chromium:mcp,chromium"
)

if [ "$MODE" = "all" ]; then
  COMBOS=("${COMBOS_CI[@]}" "${COMBOS_PAIRWISE[@]}")
else
  COMBOS=("${COMBOS_CI[@]}")
fi

# --- helpers (fail-closed: any failure aborts combo) ---

# Map a combo flags token to cargo feature arguments for `-p webfang_core`,
# mirroring compile()/start_build(): `default` → no flags (the package
# default), `full` → --all-features, `--no-default-features` → the cargo
# switch itself, anything else → --features <list>. Emits one word per line
# (empty for `default`); callers collect with a read loop.
core_feature_args() {
  case "$1" in
    default) ;;
    --no-default-features) printf '%s\n' "--no-default-features" ;;
    full) printf '%s\n' "--all-features" ;;
    *) printf '%s\n' "--features" "$1" ;;
  esac
}

# The single decoder for that protocol: it fills the global
# CORE_FEATURE_ARGS, which every probe reads on its next line. The array-init
# plus read loop used to be inlined in each of the three probes, so changing
# the protocol meant three synchronized edits and one missed edit meant one
# probe interpreted feature arguments differently from the others
# (#1635 finding 4). Bash cannot return an array, hence the global.
decode_core_feature_args() {
  local flags="$1"
  local w
  CORE_FEATURE_ARGS=()
  while IFS= read -r w; do CORE_FEATURE_ARGS+=("$w"); done < <(core_feature_args "$flags")
}

# Path of the `webfang` binary this combo's `start_build` just produced.
# CARGO_TARGET_DIR is honoured exactly as scripts/gen-cli-reference.sh does —
# that is the convention every shell consumer in this repo already follows, not
# a third one. The hardcoded ./target/debug/webfang was wrong in the exact
# context this harness is designed to run in: per-worktree target-dir isolation
# (#1267) puts the build in ~/.cache/cargo-target/<tree>, so the probes looked
# for a binary that has never existed there and reported "exit 127 / command not
# found" — a resolution failure dressed up as a contract failure (#1698).
# Unset keeps the previous relative behaviour (cargo's own default, ./target).
webfang_bin() {
  printf '%s\n' "${CARGO_TARGET_DIR:-target}/debug/webfang"
}

# True when the resolved binary is missing or not executable. Reported as its
# own message so a resolution/never-built binary is never confused with a
# contract violation by whoever reads the log.
require_webfang_bin() {
  local bin
  bin="$(webfang_bin)"
  if [ ! -x "$bin" ]; then
    echo "FAIL $1: binary not built at $bin (resolved from CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-<unset>})" >&2
    return 1
  fi
}

# Run a command with its output captured; on success return 0 (output
# discarded), on failure print the last 40 lines to stderr and return the
# command's OWN exit status. Keeps CI logs small while staying fail-closed.
# The status is the command's, not a hardcoded 1: callers that branch on
# success/failure do not care, but a status-sensitive caller would otherwise
# silently lose the real code (#1635 finding 2).
run_logged() {
  local out rc
  if out=$("$@" 2>&1); then
    return 0
  else
    rc=$?
  fi
  printf '%s\n' "$out" | tail -40 >&2
  return "$rc"
}

compile() {
  local flags="$1"
  local name="$2"
  echo "  [compile] $name ($flags)"
  if [ "$flags" = "--no-default-features" ]; then
    cargo check -p webfang_cli --no-default-features --tests
  elif [ "$flags" = "default" ]; then
    cargo check -p webfang_cli --tests
  elif [ "$flags" = "full" ]; then
    cargo check -p webfang_cli --all-features --tests
  else
    cargo check -p webfang_cli --features "$flags" --tests
  fi
}

start_build() {
  local flags="$1"
  local name="$2"
  echo "  [start] $name ($flags)"
  if [ "$flags" = "--no-default-features" ]; then
    cargo build -p webfang_cli --no-default-features
  elif [ "$flags" = "default" ]; then
    cargo build -p webfang_cli
  elif [ "$flags" = "full" ]; then
    cargo build -p webfang_cli --all-features
  else
    cargo build -p webfang_cli --features "$flags"
  fi
}

help_check() {
  local name="$1"
  local bin
  bin="$(webfang_bin)"
  echo "  [--help] $name ($bin)"
  if ! require_webfang_bin "--help $name"; then return 1; fi
  "$bin" --help >/dev/null
  local rc=$?
  if [ "$rc" -ne 0 ]; then echo "FAIL --help $name exit $rc (binary: $bin)" >&2; return 1; fi
}

crawl_check() {
  local flags="$1"
  local name="$2"
  echo "  [crawl] $name ($flags)"
  decode_core_feature_args "$flags"
  # Behavioral harness via wiremock, run as a runtime crawl assertion.
  # Availability is probed with the SAME package, target and feature
  # arguments the run below uses, and BOTH outcomes are fail-closed: a
  # listing that cannot be produced, or one that contains no `crawl` test,
  # FAILS this combo. The old branch probed `--lib` under DEFAULT features
  # (where no test is named `behavioral` outside the chromium-gated CDP
  # test) and degraded to a compile-only check of webfang_core, so the
  # feature-matrix job in .github/workflows/ci.yml could report success for
  # five of the six combos without a single crawl ever running (#1635
  # finding 5) — the same failure shape as the CRITICAL already fixed here:
  # a gate that reports success without exercising the thing it names.
  local listing
  if ! listing=$(cargo nextest list -p webfang_core "${CORE_FEATURE_ARGS[@]}" --test behavioral 2>&1); then
    printf '%s\n' "$listing" | tail -40 >&2
    echo "FAIL crawl $name: cannot list the behavioral suite for ($flags)" >&2
    return 1
  fi
  if ! grep -q "crawl" <<<"$listing"; then
    printf '%s\n' "$listing" | tail -40 >&2
    echo "FAIL crawl $name: the behavioral suite lists no crawl test for ($flags)" >&2
    return 1
  fi
  # --no-tests fail: a zero-match selector is a failure, never a vacuous pass.
  if ! run_logged cargo nextest run -p webfang_core "${CORE_FEATURE_ARGS[@]}" --no-tests fail --test behavioral crawl; then
    echo "FAIL crawl $name (behavioral crawl suite did not run and pass)" >&2
    return 1
  fi
  echo "  crawl ok ($name)"
}

resume_check() {
  local flags="$1"
  local name="$2"
  echo "  [resume] $name ($flags)"
  decode_core_feature_args "$flags"
  # Pre-seed StateStore and verify round-trip + corrupt degrade
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' RETURN
  mkdir -p "$tmp/webfang/state"
  # Valid v1 state
  cat > "$tmp/webfang/state/example.com.json" <<'JSON'
{"domain":"example.com","version":1,"processed_urls":["https://example.com/a"],"last_export":null,"total_exported":1}
JSON
  # Load via StateStore test harness (uses same serde path as --resume)
  local rc=0
  if ! run_logged cargo nextest run -p webfang_core "${CORE_FEATURE_ARGS[@]}" --no-tests fail --lib test_load_or_default_keeps; then
    echo "FAIL resume $name (fresh state round-trip)" >&2
    rc=1
  fi
  # Corrupted JSON — should degrade (propagate Serialization, filter returns all URLs)
  echo "not json {{{" > "$tmp/webfang/state/example.com.json"
  if ! run_logged cargo nextest run -p webfang_core "${CORE_FEATURE_ARGS[@]}" --no-tests fail --lib test_load_or_default_corrupt; then
    echo "FAIL resume $name (corrupt state degrade)" >&2
    rc=1
  fi
  if [ "$rc" -ne 0 ]; then
    return 1
  fi
  echo "  resume fresh+corrupt ok ($name)"
}

failure_path_check() {
  local flags="$1"
  local name="$2"
  echo "  [failure-path] $name ($flags)"
  # Deterministic, pre-network exit contracts, asserted per combo:
  #   65: --output-vectors <path> without --clean-ai (data-format error,
  #       #703). ONE answer for every build: the local inference stack that
  #       used to make a build unable to produce vectors AT ALL is gone
  #       (ADR-0004 paso 6), so the gate asks the same question on every
  #       build — "did the operator ask for vectors without asking for the
  #       thing that produces them?" — and answers 65. The retired
  #       build-capability arm (78) described a build that no longer exists,
  #       so it is not a second answer any more.
  # The gate fires in run() before any fetch, so it is network-independent.
  # (The old bad --state-dir probe is dropped: record
  # store persist failures are advisory by design (#1230/#1247) — logged,
  # never fatal — so that command's exit code is network-dependent, not a
  # state contract. The 69/74 I/O classes stay pinned by the error_path
  # suite, which runs against wiremock.)
  local expected_vectors="$EXPECTED_VECTORS_EXIT"
  decode_core_feature_args "$flags"
  local bin
  bin="$(webfang_bin)"
  if ! require_webfang_bin "failure-path $name"; then return 1; fi
  local probe_out="" rc=0
  set +e
  probe_out=$("$bin" --output-vectors vectors-compat.tmp --url https://example.com 2>&1)
  rc=$?
  set -e
  if [ "$rc" -ne "$expected_vectors" ]; then
    # The binary's own output goes to the log BEFORE the FAIL line: "expected
    # 78 got 64" alone cannot tell an application regression from a
    # toolchain or environment problem, which is the whole point of a
    # compatibility gate (#1635 finding 1).
    printf '%s\n' "$probe_out" | tail -40 >&2
    echo "FAIL failure-path $name: --output-vectors expected $expected_vectors got $rc" >&2
    return 1
  fi
  if ! run_logged cargo nextest run -p webfang_core "${CORE_FEATURE_ARGS[@]}" --no-tests fail --test behavioral error_path; then
    echo "FAIL failure-path $name (error_path tests)" >&2
    return 1
  fi
  echo "  failure-path ok ($name)"
}

# --- the --output-vectors exit contract --------------------------------------

# The exit code `--output-vectors <path>` must produce, for EVERY combo.
#
# Since ADR-0004 paso 6 there is exactly ONE contract, and it does not depend
# on the build: the local inference stack that used to make a build unable to
# produce vectors AT ALL is gone, so the gate asks every build the same
# question — "did the operator ask for vectors without asking for the thing
# that produces them?" — and answers with a data-format error (65).
#
# The retired second answer (78, for a build WITHOUT the `ai` feature) is not
# a variant of the current one: it described a build that cannot be produced
# any more. Keeping a predicate that classified combos by whether they
# enabled `ai` would keep a decision that no longer has a question behind it.
EXPECTED_VECTORS_EXIT=65

# --- self-test (--self-test) ---
# Unit checks for the pure helpers above: the feature-argument protocol, the
# `--output-vectors` exit contract and run_logged's status contract. No cargo,
# no network, no binary. Driven by
# scripts/tests/test_check_compatibility.sh, which also exercises the probe
# failure paths end to end with stub binaries.
#
# self_test_fail is global on purpose: the assert helpers below are called
# from loops and must not abort the run under `set -e`.
self_test_fail=0

self_test_eq() {
  local label="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    echo "  ok   $label = $actual"
  else
    echo "  FAIL $label: expected '$expected', got '$actual'" >&2
    self_test_fail=1
  fi
}

self_test_contains() {
  local label="$1" needle="$2" haystack="$3"
  case "$haystack" in
    *"$needle"*) echo "  ok   $label contains '$needle'" ;;
    *) echo "  FAIL $label: '$needle' not found in: $haystack" >&2; self_test_fail=1 ;;
  esac
}

self_test() {
  local combo rc captured
  echo "self-test: compatibility harness helpers"
  self_test_fail=0

  # Feature-argument protocol: one decoder, one wire format (#1635 finding 4).
  decode_core_feature_args default
  self_test_eq "decode default" "" "${CORE_FEATURE_ARGS[*]-}"
  decode_core_feature_args --no-default-features
  self_test_eq "decode --no-default-features" "--no-default-features" "${CORE_FEATURE_ARGS[*]-}"
  decode_core_feature_args full
  self_test_eq "decode full" "--all-features" "${CORE_FEATURE_ARGS[*]-}"
  decode_core_feature_args mcp,persistence
  self_test_eq "decode mcp,persistence" "--features mcp,persistence" "${CORE_FEATURE_ARGS[*]-}"
  decode_core_feature_args mcp,persistence,chromium
  self_test_eq "decode mcp,persistence,chromium" "--features mcp,persistence,chromium" "${CORE_FEATURE_ARGS[*]-}"

  # Exit contract: ONE answer for every combo since ADR-0004 paso 6. The list
  # includes an UNSEEN combination (not one of the matrix's combos) on
  # purpose: the contract is asserted per combination, not per list entry.
  for combo in default --no-default-features chromium mcp mcp,chromium full mcp,persistence,chromium; do
    self_test_eq "expected_vectors_exit $combo" "65" "$EXPECTED_VECTORS_EXIT"
  done

  # Binary resolution: honour CARGO_TARGET_DIR, fall back to ./target (#1698).
  # Asserted on the VALUE, so a future edit that reintroduces a hardcoded
  # relative path is caught by the self-test rather than by a lost build.
  # The `unset` runs inside the command substitution's subshell, so it cannot
  # leak into the rest of the self-test.
  self_test_eq "webfang_bin with CARGO_TARGET_DIR unset" "target/debug/webfang" \
    "$(unset CARGO_TARGET_DIR; webfang_bin)"
  self_test_eq "webfang_bin with external CARGO_TARGET_DIR" "/home/x/.cache/cargo-target/tree/debug/webfang" \
    "$(CARGO_TARGET_DIR=/home/x/.cache/cargo-target/tree webfang_bin)"
  self_test_eq "webfang_bin with empty CARGO_TARGET_DIR" "target/debug/webfang" \
    "$(CARGO_TARGET_DIR='' webfang_bin)"

  # run_logged returns the command's own status, and still tails the output.
  rc=0; run_logged true || rc=$?
  self_test_eq "run_logged true" "0" "$rc"
  rc=0; run_logged false || rc=$?
  self_test_eq "run_logged false" "1" "$rc"
  rc=0; run_logged bash -c 'exit 42' || rc=$?
  self_test_eq "run_logged exit 42" "42" "$rc"
  captured=$(run_logged bash -c 'echo SELF-TEST-TAIL-MARKER >&2; exit 7' 2>&1) || true
  self_test_contains "run_logged tail" "SELF-TEST-TAIL-MARKER" "$captured"

  if [ "$self_test_fail" -ne 0 ]; then
    echo "self-test: FAIL"
    return 1
  fi
  echo "self-test: PASS"
  return 0
}

# --- main loop ---
if [ "$MODE" = "self-test" ]; then
  self_test
  exit $?
fi

echo "Compatibility harness: mode=$MODE combos=${#COMBOS[@]}"
echo "Retention: cargo hack --each-feature (isolated) stays in ci.yml feature-matrix"
overall_fail=0
for c in "${COMBOS[@]}"; do
  IFS=":" read -r name flags <<<"$c"
  echo "== $name ($flags) =="
  if ! compile "$flags" "$name"; then echo "FAIL $name compile"; overall_fail=1; continue; fi
  if ! start_build "$flags" "$name"; then echo "FAIL $name start"; overall_fail=1; continue; fi
  if ! help_check "$name"; then echo "FAIL $name --help"; overall_fail=1; continue; fi
  if ! crawl_check "$flags" "$name"; then echo "FAIL $name crawl"; overall_fail=1; continue; fi
  if ! resume_check "$flags" "$name"; then echo "FAIL $name resume"; overall_fail=1; continue; fi
  if ! failure_path_check "$flags" "$name"; then echo "FAIL $name failure-path"; overall_fail=1; continue; fi
  echo "PASS $name"
done

if [ "$overall_fail" -ne 0 ]; then
  echo "Compatibility harness: FAIL"
  exit 1
fi
echo "Compatibility harness: PASS ($MODE)"
