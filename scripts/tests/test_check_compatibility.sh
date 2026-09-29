#!/usr/bin/env bash
#
# test_check_compatibility.sh — semantics harness for
# scripts/check_compatibility.sh (#1635).
#
# Hermetic and cargo-free: the harness under test is driven against stub
# `cargo` and stub `webfang` binaries in a temp sandbox, so every failure path
# of the compatibility probe is exercised in milliseconds and without a build,
# a network, or the ONNX Runtime cache.
#
# What it pins:
#   section A  the pure helpers, via `check_compatibility.sh --self-test`
#              (feature-argument decoder, the `ai` predicate, the derived
#              --output-vectors exit contract, run_logged's status contract);
#   section B  a mismatching exit code prints the binary's own output BEFORE
#              the FAIL line (finding 1);
#   section C  the crawl availability branch is fail-closed — a listing that
#              fails, or lists no crawl test, FAILS the combo and never
#              degrades into a compile-only pass (finding 5);
#   section D  the --output-vectors exit contract is derived per combo from
#              whether `ai` is enabled, not from a fixed branch: a binary that
#              honours the contract passes all six combos, while one that
#              always exits 78 fails the ai-bearing ones and vice versa
#              (finding 3).
#
# Exit code: 0 when every assertion holds, 1 otherwise.
#
# Usage: bash scripts/tests/test_check_compatibility.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HARNESS="$ROOT/scripts/check_compatibility.sh"

FAILED=0
assert() {
  local label="$1" ok="$2" detail="${3:-}"
  if [ "$ok" = "1" ]; then
    echo "  ok   $label"
  else
    echo "  FAIL $label${detail:+: $detail}" >&2
    FAILED=1
  fi
}
assert_eq() {
  local label="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    echo "  ok   $label ($actual)"
  else
    echo "  FAIL $label: expected '$expected', got '$actual'" >&2
    FAILED=1
  fi
}
assert_contains() {
  local label="$1" haystack="$2" needle="$3"
  case "$haystack" in
    *"$needle"*) echo "  ok   $label contains '$needle'" ;;
    *) echo "  FAIL $label: '$needle' not found in output" >&2; FAILED=1 ;;
  esac
}
assert_not_contains() {
  local label="$1" haystack="$2" needle="$3"
  case "$haystack" in
    *"$needle"*) echo "  FAIL $label: '$needle' unexpectedly present" >&2; FAILED=1 ;;
    *) echo "  ok   $label without '$needle'" ;;
  esac
}

if [ ! -r "$HARNESS" ]; then
  echo "harness not found: $HARNESS" >&2
  exit 1
fi

# --- sandbox ---------------------------------------------------------------
# Stubs stand in for the toolchain so the probe failure paths are reachable
# without cargo. `cargo` records every invocation and answers per phase;
# `webfang` prints a recognisable line on each stream and exits with a status
# chosen by STUB_WEBFANG_MODE.
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT
mkdir -p "$SANDBOX/bin" "$SANDBOX/target/debug"

cat > "$SANDBOX/bin/cargo" <<'STUB'
#!/usr/bin/env bash
# Stub cargo: never compiles, never reaches the network.
printf '%s\n' "$*" >>"$STUB_LOG"
case "${1:-}" in
clean)
  exit 0
  ;;
check)
  exit 0
  ;;
build)
  # start_build() runs just before the binary is probed, so the argv it got is
  # an independent record of the feature set this combo actually builds.
  printf '%s\n' "$*" >"$STUB_FLAGS"
  exit 0
  ;;
nextest)
  case "${2:-}" in
  list)
    case "${STUB_LISTING_MODE:-crawl}" in
    crawl)
      echo "        cli::crawl_test::max_pages_limits_crawl_output [bin]"
      echo "        cli::crawl_test::crawl_js_strategy_respects_timeout_secs [bin]"
      exit 0
      ;;
    empty)
      echo "        cli::other_test::some_unrelated_name [bin]"
      exit 0
      ;;
    *)
      echo "stub nextest list: cannot list target" >&2
      exit 1
      ;;
    esac
    ;;
  run)
    exit 0
    ;;
  esac
  ;;
esac
exit 0
STUB

cat > "$SANDBOX/target/debug/webfang" <<'STUB'
#!/usr/bin/env bash
# Stub webfang: `--help` succeeds (help_check must keep passing), anything
# else probes the --output-vectors exit contract.
case "${1:-}" in
--help | -h | --version)
  echo "STUB-HELP-OK"
  exit 0
  ;;
esac
echo "STUB-PROBE-STDOUT: webfang $*"
echo "STUB-PROBE-STDERR: simulated application failure" >&2
# The oracle: independent re-derivation of the contract, from the argv the
# build actually received (start_build runs just before this probe, so the
# file is current for the combo under test). `--all-features` enables `ai`; an
# explicit feature list enables it iff one of its comma-separated tokens is
# `ai`.
STUB_BUILD_ARGV="$(cat "$STUB_FLAGS" 2>/dev/null || true)"
ai_enabled=0
prev=""
for tok in $STUB_BUILD_ARGV; do
  [ "$tok" = "--all-features" ] && ai_enabled=1
  if [ "$prev" = "--features" ]; then
    IFS=',' read -r -a feats <<<"$tok"
    for f in "${feats[@]}"; do [ "$f" = "ai" ] && ai_enabled=1; done
  fi
  prev="$tok"
done
case "${STUB_WEBFANG_MODE:-exit}" in
predicate) [ "$ai_enabled" = "1" ] && exit 65 || exit 78 ;;
always78) exit 78 ;;
always65) exit 65 ;;
*) exit "${STUB_WEBFANG_EXIT:-0}" ;;
esac
STUB
chmod +x "$SANDBOX/bin/cargo" "$SANDBOX/target/debug/webfang"

# run_harness <label> [env assignments...]: run the harness under the stubs
# inside the sandbox and echo its combined output plus a HARNESS_EXIT line.
CALL_LOG="$SANDBOX/calls.log"
run_harness() {
  # $1 is a label for the caller's own bookkeeping; the rest are env
  # assignments handed straight to `env`.
  shift
  (
    cd "$SANDBOX" || exit 1
    : >"$SANDBOX/calls.log"
    env PATH="$SANDBOX/bin:$PATH" \
      STUB_LOG="$SANDBOX/calls.log" \
      STUB_FLAGS="$SANDBOX/build-argv" \
      ORT_CACHE_DIR="$SANDBOX/ort-cache" \
      "$@" \
      bash "$HARNESS" --ci-required 2>&1
    echo "HARNESS_EXIT=$?"
  )
}

# --- section A: helper units ------------------------------------------------
echo "A: helper units (check_compatibility.sh --self-test)"
A_OUT="$(bash "$HARNESS" --self-test 2>&1)"
A_RC=$?
assert_eq "self-test exit status" "0" "$A_RC"
assert_contains "self-test verdict" "$A_OUT" "self-test: PASS"
assert_contains "unseen ai combo asserted as 65" "$A_OUT" "expected_vectors_exit ai,persistence,chromium = 65"
assert_contains "non-ai combo asserted as 78" "$A_OUT" "expected_vectors_exit mcp,chromium = 78"
assert_contains "run_logged returns the real status" "$A_OUT" "run_logged exit 42 = 42"

# --- section B: opaque exit-contract diagnostics (finding 1) -----------------
echo "B: mismatching --output-vectors exit code is diagnosable (finding 1)"
B_OUT="$(run_harness b STUB_WEBFANG_MODE=exit STUB_WEBFANG_EXIT=64)"
assert_contains "harness fails" "$B_OUT" "HARNESS_EXIT=1"
assert_contains "FAIL line names the contract" "$B_OUT" "FAIL failure-path default: --output-vectors expected 78 got 64"
assert_contains "binary stdout reaches the log" "$B_OUT" "STUB-PROBE-STDOUT"
assert_contains "binary stderr reaches the log" "$B_OUT" "STUB-PROBE-STDERR"
B_PROBE_LINE="$(grep -n "STUB-PROBE-STDERR" <<<"$B_OUT" | head -1 | cut -d: -f1)"
B_FAIL_LINE="$(grep -n "FAIL failure-path" <<<"$B_OUT" | head -1 | cut -d: -f1)"
if [ -n "$B_PROBE_LINE" ] && [ -n "$B_FAIL_LINE" ] && [ "$B_PROBE_LINE" -lt "$B_FAIL_LINE" ]; then
  assert "binary output precedes the FAIL line ($B_PROBE_LINE < $B_FAIL_LINE)" "1"
else
  assert "binary output precedes the FAIL line" "0" "probe line '$B_PROBE_LINE' vs fail line '$B_FAIL_LINE'"
fi

# --- section C: crawl availability is fail-closed (finding 5) ---------------
echo "C: crawl availability never degrades to a compile-only pass (finding 5)"
C_OUT="$(run_harness c STUB_WEBFANG_MODE=exit STUB_WEBFANG_EXIT=0 STUB_LISTING_MODE=empty)"
assert_contains "harness fails when no crawl test is listed" "$C_OUT" "HARNESS_EXIT=1"
assert_contains "no-crawl listing fails the combo" "$C_OUT" "FAIL crawl default: the behavioral suite lists no crawl test"
assert_not_contains "no compile-only fallback" "$C_OUT" "fallback compile-check"
assert_not_contains "fallback cargo check absent from the log" "$(cat "$CALL_LOG")" "check -p webfang_core"
assert_not_contains "crawl suite run absent from the log" "$(cat "$CALL_LOG")" "nextest run -p webfang_core --test behavioral crawl"
assert_contains "availability probe asks for the behavioral target" "$(cat "$CALL_LOG")" "nextest list -p webfang_core --test behavioral"

C2_OUT="$(run_harness c2 STUB_WEBFANG_MODE=exit STUB_WEBFANG_EXIT=0 STUB_LISTING_MODE=error)"
assert_contains "harness fails when the listing cannot be produced" "$C2_OUT" "HARNESS_EXIT=1"
assert_contains "listing error fails the combo" "$C2_OUT" "FAIL crawl default: cannot list the behavioral suite for (default)"
assert_contains "listing error output is shown" "$C2_OUT" "cannot list target"

# --- section D: exit contract is derived, not branch-selected (finding 3) ----
echo "D: --output-vectors contract discriminates per combo (finding 3)"
D_OUT="$(run_harness d STUB_WEBFANG_MODE=predicate)"
assert_contains "a contract-honouring binary passes all 6 combos" "$D_OUT" "Compatibility harness: PASS (ci-required)"
assert_contains "ai combo passed" "$D_OUT" "PASS ai"
assert_contains "full combo passed" "$D_OUT" "PASS full"
assert_contains "default combo passed" "$D_OUT" "PASS default"

D2_OUT="$(run_harness d2 STUB_WEBFANG_MODE=always78)"
assert_contains "always-78 binary fails the harness" "$D2_OUT" "HARNESS_EXIT=1"
assert_contains "ai combo asserts 65" "$D2_OUT" "FAIL failure-path ai: --output-vectors expected 65 got 78"
assert_contains "full combo asserts 65" "$D2_OUT" "FAIL failure-path full: --output-vectors expected 65 got 78"
assert_contains "no-default combo still asserts 78 (accepted)" "$D2_OUT" "PASS no-default"

D3_OUT="$(run_harness d3 STUB_WEBFANG_MODE=always65)"
assert_contains "always-65 binary fails the harness" "$D3_OUT" "HARNESS_EXIT=1"
assert_contains "default combo asserts 78" "$D3_OUT" "FAIL failure-path default: --output-vectors expected 78 got 65"
assert_contains "no-default combo asserts 78" "$D3_OUT" "FAIL failure-path no-default: --output-vectors expected 78 got 65"
assert_contains "chromium combo asserts 78" "$D3_OUT" "FAIL failure-path chromium: --output-vectors expected 78 got 65"
assert_contains "mcp combo asserts 78" "$D3_OUT" "FAIL failure-path mcp: --output-vectors expected 78 got 65"

# --- summary ----------------------------------------------------------------
if [ "$FAILED" -ne 0 ]; then
  echo "test_check_compatibility: FAIL"
  exit 1
fi
echo "test_check_compatibility: PASS"
exit 0
