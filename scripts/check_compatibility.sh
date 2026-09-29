#!/usr/bin/env bash
set -euo pipefail
# Compatibility harness — Sprint 0 Gate 0
# Loops 6 CI-required combos + 2 pairwise spot-checks (local/nightly).
# Usage: bash scripts/check_compatibility.sh --ci-required | --all | --self-test | --help
# Each combo verifies: compile | start | --help | crawl | resume | failure-path

MODE="ci-required"
for arg in "$@"; do
  case "$arg" in
    --ci-required) MODE="ci-required" ;;
    --all) MODE="all" ;;
    --self-test) MODE="self-test" ;;
    --help|-h) echo "Usage: $0 [--ci-required|--all|--self-test]"; echo "  --ci-required  6 required combos (CI)"; echo "  --all          6 + 2 pairwise (local/nightly)"; echo "  --self-test    unit-check the pure helpers (no cargo, no network)"; exit 0 ;;
    *) echo "Unknown arg: $arg" >&2; exit 2 ;;
  esac
done

COMBOS_CI=(
  "default:default"
  "no-default:--no-default-features"
  "ai:ai"
  "chromium:chromium"
  "mcp:mcp"
  "full:full"
)
COMBOS_PAIRWISE=(
  "ai+persistence:ai,persistence"
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
  echo "  [--help] $name"
  ./target/debug/webfang --help >/dev/null
  local rc=$?
  if [ "$rc" -ne 0 ]; then echo "FAIL --help $name exit $rc" >&2; return 1; fi
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
  #   65: --output-vectors <path> on a build WITH the `ai` feature but
  #       without --clean-ai (data-format error, #703);
  #   78: --output-vectors <path> on a build WITHOUT the `ai` feature
  #       (config error, #652).
  # Both gates fire in run() before any fetch, so they are feature- and
  # network-independent. (The old bad --state-dir probe is dropped: record
  # store persist failures are advisory by design (#1230/#1247) — logged,
  # never fatal — so that command's exit code is network-dependent, not a
  # state contract. The 69/74 I/O classes stay pinned by the error_path
  # suite, which runs against wiremock.)
  #
  # The expected code is DERIVED from whether the combo enables `ai`
  # (expected_vectors_exit below), never matched against a list of combos:
  # an allowlist silently asserted the wrong contract for any new
  # ai-bearing combination — `ai,persistence,chromium` selected the 78
  # branch and the gate passed on a build that must exit 65 (#1635
  # finding 3).
  local expected_vectors
  expected_vectors=$(expected_vectors_exit "$flags")
  decode_core_feature_args "$flags"
  if [ ! -x "./target/debug/webfang" ]; then
    echo "FAIL failure-path $name: binary not built at ./target/debug/webfang" >&2
    return 1
  fi
  local probe_out="" rc=0
  set +e
  probe_out=$(./target/debug/webfang --output-vectors vectors-compat.tmp --url https://example.com 2>&1)
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

# --- preflight (read-only, cheap) ---

# True when the combo's cargo invocation enables the `ai` feature: `full`
# passes --all-features (which includes `ai`), and any explicit feature list
# carrying the `ai` token does. One predicate, two consumers: the ONNX
# Runtime preflight below (the `ai` feature is what links ONNX Runtime) and
# expected_vectors_exit() (the --output-vectors exit contract). Classifying a
# combo by what it ENABLES is what keeps an unseen combination such as
# `ai,persistence,chromium` on the right side of both (#1635 finding 3).
combo_enables_ai() {
  local flags="$1"
  local -a tokens=()
  local token
  if [ "$flags" = "full" ]; then
    return 0
  fi
  IFS=',' read -r -a tokens <<<"$flags"
  for token in "${tokens[@]}"; do
    if [ "$token" = "ai" ]; then
      return 0
    fi
  done
  return 1
}

# The exit code `--output-vectors <path>` must produce for a combo, derived
# from the `ai` predicate. Consumed by failure_path_check() above.
expected_vectors_exit() {
  if combo_enables_ai "$1"; then printf '%s\n' 65; else printf '%s\n' 78; fi
}

# Directory the ort-sys build script downloads the ONNX Runtime static library
# into: $ORT_CACHE_DIR, else ${XDG_CACHE_HOME:-$HOME/.cache}/ort.pyke.io.
ort_cache_dir() {
  printf '%s\n' "${ORT_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/ort.pyke.io}"
}

# Preflight for the ai-bearing combos. The ONNX Runtime static library is
# downloaded by the ort-sys build script at build time, and it lands OUTSIDE
# target/ unless ORT_CACHE_DIR points inside it. rust-cache saves only target/,
# which is where the build script's recorded `cargo:rustc-link-search` lives, so
# a restored target/ can replay that link-search line while the library it names
# does not exist on this runner — and the link dies with "could not find native
# static library `onnxruntime`" (issue #1639). Naming the cause before the
# combos run is what makes the failure readable from the job log alone.
#
# Advisory only: it never fails the harness. The combo loop already reports the
# failure, and the library is legitimately absent on a cold cache, where the
# first ort-sys build downloads it.
preflight_ort_native_lib() {
  local combo name flags dir
  for combo in "${COMBOS[@]}"; do
    IFS=":" read -r name flags <<<"$combo"
    if ! combo_enables_ai "$flags"; then
      continue
    fi
    dir=$(ort_cache_dir)
    if [ -n "$(find "$dir" -type f -name libonnxruntime.a -print -quit 2>/dev/null)" ]; then
      return 0
    fi
    # Self-heal the half-restored cache. The download is gated on
    # `!bin_extract_dir.exists()` (ort-sys build/download/mod.rs), so a
    # restored-but-incomplete cache — extract dir present, library absent —
    # makes the build script skip the download and the link die. The
    # fingerprint is then Fresh, so nothing retries it.
    #
    # `cargo clean -p ort-sys` alone is NOT the remedy and must not be
    # presented as one: it drops the build-script unit but leaves
    # $ORT_CACHE_DIR untouched, so the gate above still sees the directory
    # and still skips. Verified on this repo: with the extract dir present
    # and the .a removed, clean+build fails with the same
    # "could not find native static library" and downloads nothing; clearing
    # the extract dir makes the next build download and link cleanly.
    echo "  [preflight] ONNX Runtime library missing with the extract dir present"
    echo "  [preflight]   clearing $dir so the ort-sys build script re-downloads"
    if [ -n "$dir" ] && [ -d "$dir" ]; then
      find "$dir" -mindepth 1 -maxdepth 1 -type d -name 'dfbin' -exec rm -rf {} + 2>/dev/null || true
    fi
    cargo clean -p ort-sys >/dev/null 2>&1 || true
    {
      echo "  [preflight] ONNX Runtime native library not found, but combo '$name' links it"
      echo "  [preflight]   missing file : libonnxruntime.a (host target)"
      echo "  [preflight]   searched dir : $dir"
      echo "  [preflight]   the ort-sys build script downloads that library at build"
      echo "  [preflight]   time into \$ORT_CACHE_DIR, else"
      echo "  [preflight]   \${XDG_CACHE_HOME:-\$HOME/.cache}/ort.pyke.io"
      echo "  [preflight]   A target/ restored from a CI cache can carry the cached"
      echo "  [preflight]   ort-sys build-script fingerprint, which records a"
      echo "  [preflight]   cargo:rustc-link-search naming that directory, without the"
      echo "  [preflight]   library itself. The link then fails with"
      echo "  [preflight]   \"error: could not find native static library \`onnxruntime\`\"."
      echo "  [preflight]   Expected on a cold cache (the first build downloads it)."
      echo "  [preflight]   The extract dir above has been cleared, so the next"
      echo "  [preflight]   ort-sys build re-downloads. If a link error still"
      echo "  [preflight]   follows, the cache restore is dropping the library"
      echo "  [preflight]   itself — see #1639."
    } >&2
    return 0
  done
  return 0
}

# --- self-test (--self-test) ---
# Unit checks for the pure helpers above: the feature-argument protocol, the
# `--output-vectors` exit contract, the `ai` predicate and run_logged's status
# contract. No cargo, no network, no binary. Driven by
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

self_test_yes_no() {
  if "$@"; then printf '%s\n' true; else printf '%s\n' false; fi
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
  decode_core_feature_args ai,persistence
  self_test_eq "decode ai,persistence" "--features ai,persistence" "${CORE_FEATURE_ARGS[*]-}"
  decode_core_feature_args ai,persistence,chromium
  self_test_eq "decode ai,persistence,chromium" "--features ai,persistence,chromium" "${CORE_FEATURE_ARGS[*]-}"

  # `ai` predicate: a combo is classified by what it enables, not by
  # membership in a list of combos someone remembered to add.
  for combo in default --no-default-features chromium mcp mcp,chromium; do
    self_test_eq "combo_enables_ai $combo" "false" "$(self_test_yes_no combo_enables_ai "$combo")"
  done
  for combo in ai full ai,persistence ai,persistence,chromium; do
    self_test_eq "combo_enables_ai $combo" "true" "$(self_test_yes_no combo_enables_ai "$combo")"
  done

  # Exit contract derived from that predicate. `ai,persistence,chromium` is
  # the combo the old allowlist got wrong (it selected 78).
  for combo in default --no-default-features chromium mcp mcp,chromium; do
    self_test_eq "expected_vectors_exit $combo" "78" "$(expected_vectors_exit "$combo")"
  done
  for combo in ai full ai,persistence ai,persistence,chromium; do
    self_test_eq "expected_vectors_exit $combo" "65" "$(expected_vectors_exit "$combo")"
  done

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
preflight_ort_native_lib
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
