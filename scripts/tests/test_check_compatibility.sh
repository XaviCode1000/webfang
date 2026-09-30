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
# Extra scratch dirs, each created by new_sandbox, all removed on exit.
EXTRA_SANDBOXES=()
# shellcheck disable=SC2329 # invoked by the EXIT trap, not by name
cleanup() { rm -rf "$SANDBOX" ${EXTRA_SANDBOXES[@]+"${EXTRA_SANDBOXES[@]}"}; }
trap cleanup EXIT
mkdir -p "$SANDBOX" 

# install_stubs <root> <target-subdir>
# Writes the stub `cargo` into <root>/bin and the stub `webfang` into
# <root>/<target-subdir>/debug. The target dir is a PARAMETER, not a constant:
# the behaviour under test (#1698) is exactly "which directory does the harness
# look in", and a fixture with one single layout baked in cannot detect that
# look-up being hardcoded to the wrong one.
install_stubs() {
  local root="$1" subdir="$2"
  mkdir -p "$root/bin" "$root/$subdir/debug"

  cat > "$root/bin/cargo" <<'STUB'
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

cat > "$root/$subdir/debug/webfang" <<'STUB'
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
  chmod +x "$root/bin/cargo" "$root/$subdir/debug/webfang"
}

# The in-repo default layout, used by sections A-D.
install_stubs "$SANDBOX" "target"

# run_in <root> <target-dir|-> [env assignments...]: run the harness under the
# stubs from inside <root> and echo its combined output plus a HARNESS_EXIT
# line. <target-dir> is exported verbatim; `-` leaves CARGO_TARGET_DIR
# UNSET, which is the only way to exercise the ./target fallback — the ambient
# environment of whoever runs this test must never leak in, or the suite would
# silently depend on a direnv'd worktree (#1698).
run_in() {
  local root="$1" target="$2"
  shift 2
  # The ambient CARGO_TARGET_DIR must never leak in, or the suite would depend
  # on a direnv'd worktree and the `-` (fallback) case would silently test the
  # caller's target dir instead of ./target. The unset happens in the subshell
  # rather than through `env -u`: GNU env stops parsing options at the first
  # assignment, so an option placed after PATH=… is taken as the command.
  local -a target_args=()
  (
    cd "$root" || exit 1
    if [ "$target" = "-" ]; then
      unset CARGO_TARGET_DIR
    else
      target_args=("CARGO_TARGET_DIR=$target")
    fi
    : >"$root/calls.log"
    env PATH="$root/bin:$PATH" \
      STUB_LOG="$root/calls.log" \
      STUB_FLAGS="$root/build-argv" \
      ORT_CACHE_DIR="$root/ort-cache" \
      ${target_args[@]+"${target_args[@]}"} \
      "$@" \
      bash "$HARNESS" --ci-required 2>&1
    echo "HARNESS_EXIT=$?"
  )
}

# run_harness <label> [env assignments...]: the default layout (binary under
# the in-repo ./target, exported as CARGO_TARGET_DIR so the suite is
# independent of the caller's environment). $1 is a label for the caller's own
# bookkeeping; the rest are env assignments handed straight to `env`.
run_harness() {
  shift
  run_in "$SANDBOX" "$SANDBOX/target" "$@"
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
assert_not_contains "fallback cargo check absent from the log" "$(cat "$SANDBOX/calls.log")" "check -p webfang_core"
assert_not_contains "crawl suite run absent from the log" "$(cat "$SANDBOX/calls.log")" "nextest run -p webfang_core --test behavioral crawl"
assert_contains "availability probe asks for the behavioral target" "$(cat "$SANDBOX/calls.log")" "nextest list -p webfang_core --test behavioral"

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

# --- section E: binary resolution honours CARGO_TARGET_DIR (#1698) ---------
# The bug: the probes ran a hardcoded ./target/debug/webfang, so every
# worktree — all of which build into ~/.cache/cargo-target/<tree> under the
# repo's isolated-target-dir policy (#1267) — reported "exit 127, command not
# found". 127 is a look-up failure, not a contract failure, and the harness
# printed it as if the contract had been broken. Each case below places the
# binary in exactly ONE of the two locations, so a resolver that looks in the
# wrong one cannot pass.
echo "E: binary resolution follows CARGO_TARGET_DIR (#1698)"

# new_sandbox: a second scratch dir, so a case can hold a layout the default
# one does not (notably: no in-repo ./target at all). Sets NEW_SANDBOX rather
# than echoing it, so the dir joins the EXIT cleanup list — a command
# substitution would run in a subshell and lose the bookkeeping.
NEW_SANDBOX=""
new_sandbox() {
  NEW_SANDBOX="$(mktemp -d)"
  EXTRA_SANDBOXES+=("$NEW_SANDBOX")
}

# E1: external target dir, NO in-repo ./target anywhere. This is the reported
# bug: the binary exists exactly where cargo put it and the harness must pass.
new_sandbox
FAR="$NEW_SANDBOX"
install_stubs "$FAR" "target-ext"
E1_OUT="$(run_in "$FAR" "$FAR/target-ext" STUB_WEBFANG_MODE=predicate)"
assert_contains "external CARGO_TARGET_DIR passes all 6 combos" "$E1_OUT" "Compatibility harness: PASS (ci-required)"
assert_contains "resolved path is the external one" "$E1_OUT" "[--help] default ($FAR/target-ext/debug/webfang)"
assert_contains "failure-path probe ran the external binary" "$E1_OUT" "failure-path ok (default)"
assert_not_contains "no in-repo target path is referenced" "$E1_OUT" "[--help] default (target/debug/webfang)"

# E2: same sandbox, CARGO_TARGET_DIR unset. Nothing was built under ./target,
# so the harness must FAIL — and name the path it resolved, so the log says
# WHERE it looked instead of a bare 127.
E2_OUT="$(run_in "$FAR" "-" STUB_WEBFANG_MODE=predicate)"
assert_contains "missing in-repo binary fails the harness" "$E2_OUT" "HARNESS_EXIT=1"
assert_contains "failure names the resolved path" "$E2_OUT" "binary not built at target/debug/webfang"
assert_contains "failure names the environment it resolved from" "$E2_OUT" "resolved from CARGO_TARGET_DIR=<unset>"
assert_not_contains "no bare command-not-found status is reported as a contract failure" "$E2_OUT" "exit 127"

# E3: default behaviour is unchanged — CARGO_TARGET_DIR unset AND the binary in
# the in-repo ./target, exactly as before the fix.
install_stubs "$FAR" "target"
E3_OUT="$(run_in "$FAR" "-" STUB_WEBFANG_MODE=predicate)"
assert_contains "unset CARGO_TARGET_DIR still resolves ./target" "$E3_OUT" "Compatibility harness: PASS (ci-required)"
assert_contains "default resolution is the relative ./target path" "$E3_OUT" "[--help] default (target/debug/webfang)"
rm -rf "$FAR"

# --- section F: a column can actually go red (M2 of #1607) -----------------
# A gate that cannot fail is the defect. Each of the six columns is driven to
# a genuine failure in a scratch copy and the harness must return non-zero —
# an un-failable column is what made this harness decorative.
echo "F: every column is fail-closed (M2, #1607)"
# F1: compile fails (stub cargo exits non-zero on `check`).
new_sandbox
FC="$NEW_SANDBOX"
install_stubs "$FC" "target"
{
  echo '#!/usr/bin/env bash'
  # shellcheck disable=SC2016 # ${1} belongs to the GENERATED stub, not to this shell
  echo 'case "${1:-}" in check) exit 101 ;; esac'
  echo 'exit 0'
} >"$FC/bin/cargo"
chmod +x "$FC/bin/cargo"
F1_OUT="$(run_in "$FC" "$FC/target" STUB_WEBFANG_MODE=predicate)"
assert_contains "compile column fails the harness" "$F1_OUT" "HARNESS_EXIT=1"
assert_contains "compile failure is reported" "$F1_OUT" "FAIL default compile"
rm -rf "$FC"

# F2: help column fails — the binary exists but --help exits non-zero.
new_sandbox
FH="$NEW_SANDBOX"
install_stubs "$FH" "target"
{
  echo '#!/usr/bin/env bash'
  # shellcheck disable=SC2016 # ${1} belongs to the GENERATED stub, not to this shell
  echo 'case "${1:-}" in --help|-h) echo "stub: help is broken" >&2; exit 2 ;; esac'
  echo 'exec true'
} >"$FH/target/debug/webfang"
chmod +x "$FH/target/debug/webfang"
F2_OUT="$(run_in "$FH" "$FH/target" STUB_WEBFANG_MODE=predicate)"
assert_contains "help column fails the harness" "$F2_OUT" "HARNESS_EXIT=1"
assert_contains "help failure is reported" "$F2_OUT" "FAIL default --help"
rm -rf "$FH"

# F3: crawl column fails — the behavioral suite lists no crawl test.
F3_OUT="$(run_harness f3 STUB_WEBFANG_MODE=exit STUB_WEBFANG_EXIT=0 STUB_LISTING_MODE=empty)"
assert_contains "crawl column fails the harness" "$F3_OUT" "HARNESS_EXIT=1"

# F4: resume column fails — the state round-trip suite does not pass.
new_sandbox
FS="$NEW_SANDBOX"
install_stubs "$FS" "target"
# Same stub as every other case, with ONE rule added: the two state round-trip
# selectors fail. Editing the body rather than replacing the stub keeps the
# crawl listing intact, so the case fails on the resume column and not on an
# earlier one — otherwise it would prove nothing about the column it names.
{
  echo '#!/usr/bin/env bash'
  # shellcheck disable=SC2016 # "$@" belongs to the GENERATED stub, not to this shell
  echo 'for a in "$@"; do case "$a" in test_load_or_default_keeps|test_load_or_default_corrupt) exit 1 ;; esac; done'
  tail -n +2 "$FS/bin/cargo"
} >"$FS/bin/cargo.new"
mv "$FS/bin/cargo.new" "$FS/bin/cargo"
chmod +x "$FS/bin/cargo"
F4_OUT="$(run_in "$FS" "$FS/target" STUB_WEBFANG_MODE=predicate)"
assert_contains "resume column fails the harness" "$F4_OUT" "HARNESS_EXIT=1"
assert_contains "resume failure is reported" "$F4_OUT" "FAIL default resume"
rm -rf "$FS"

# F5: failure-path column fails — the --output-vectors contract is violated.
F5_OUT="$(run_harness f5 STUB_WEBFANG_MODE=exit STUB_WEBFANG_EXIT=64)"
assert_contains "failure-path column fails the harness" "$F5_OUT" "HARNESS_EXIT=1"
assert_contains "failure-path contract breach is reported" "$F5_OUT" "FAIL failure-path default: --output-vectors expected 78 got 64"

# --- summary ----------------------------------------------------------------
if [ "$FAILED" -ne 0 ]; then
  echo "test_check_compatibility: FAIL"
  exit 1
fi
echo "test_check_compatibility: PASS"
exit 0
