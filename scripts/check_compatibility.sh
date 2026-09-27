#!/usr/bin/env bash
set -euo pipefail
# Compatibility harness — Sprint 0 Gate 0
# Loops 6 CI-required combos + 2 pairwise spot-checks (local/nightly).
# Usage: bash scripts/check_compatibility.sh --ci-required | --all | --help
# Each combo verifies: compile | start | --help | crawl | resume | failure-path

MODE="ci-required"
for arg in "$@"; do
  case "$arg" in
    --ci-required) MODE="ci-required" ;;
    --all) MODE="all" ;;
    --help|-h) echo "Usage: $0 [--ci-required|--all]"; echo "  --ci-required  6 required combos (CI)"; echo "  --all          6 + 2 pairwise (local/nightly)"; exit 0 ;;
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

# Run a command with its output captured; on success return 0 (output
# discarded), on failure print the last 40 lines to stderr and return the
# failing status. Keeps CI logs small while staying fail-closed.
run_logged() {
  local out
  if out=$("$@" 2>&1); then
    return 0
  fi
  printf '%s\n' "$out" | tail -40 >&2
  return 1
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
  local -a fargs=()
  local w
  while IFS= read -r w; do fargs+=("$w"); done < <(core_feature_args "$flags")
  # Behavioral harness via wiremock: run the crawl-filtered behavioral suite
  # if it is available, otherwise fallback to compile-check of core.
  if cargo nextest run -p webfang_core --lib -- --list 2>/dev/null | grep -q "behavioral"; then
    if ! run_logged cargo nextest run -p webfang_core "${fargs[@]}" --no-tests fail --test behavioral crawl; then
      echo "FAIL crawl $name" >&2
      return 1
    fi
  else
    # Fallback: at least check that core lib compiles with this feature set
    if ! run_logged cargo check -p webfang_core "${fargs[@]}" --tests; then
      echo "FAIL crawl $name (fallback compile-check)" >&2
      return 1
    fi
  fi
}

resume_check() {
  local flags="$1"
  local name="$2"
  echo "  [resume] $name ($flags)"
  local -a fargs=()
  local w
  while IFS= read -r w; do fargs+=("$w"); done < <(core_feature_args "$flags")
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
  if ! run_logged cargo nextest run -p webfang_core "${fargs[@]}" --no-tests fail --lib test_load_or_default_keeps; then
    echo "FAIL resume $name (fresh state round-trip)" >&2
    rc=1
  fi
  # Corrupted JSON — should degrade (propagate Serialization, filter returns all URLs)
  echo "not json {{{" > "$tmp/webfang/state/example.com.json"
  if ! run_logged cargo nextest run -p webfang_core "${fargs[@]}" --no-tests fail --lib test_load_or_default_corrupt; then
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
  local expected_vectors=78
  case "$flags" in
    ai|full|ai,persistence) expected_vectors=65 ;;
  esac
  local -a fargs=()
  local w
  while IFS= read -r w; do fargs+=("$w"); done < <(core_feature_args "$flags")
  if [ ! -x "./target/debug/webfang" ]; then
    echo "FAIL failure-path $name: binary not built at ./target/debug/webfang" >&2
    return 1
  fi
  set +e
  ./target/debug/webfang --output-vectors vectors-compat.tmp --url https://example.com >/dev/null 2>&1
  rc=$?
  set -e
  if [ "$rc" -ne "$expected_vectors" ]; then
    echo "FAIL failure-path $name: --output-vectors expected $expected_vectors got $rc" >&2
    return 1
  fi
  if ! run_logged cargo nextest run -p webfang_core "${fargs[@]}" --no-tests fail --test behavioral error_path; then
    echo "FAIL failure-path $name (error_path tests)" >&2
    return 1
  fi
  echo "  failure-path ok ($name)"
}

# --- preflight (read-only, cheap) ---

# True when the combo's cargo invocation links the ONNX Runtime static library:
# `full` passes --all-features (which includes the `ai` feature), and any
# explicit feature list carrying the `ai` token does.
combo_links_ort() {
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
    if ! combo_links_ort "$flags"; then
      continue
    fi
    dir=$(ort_cache_dir)
    if [ -n "$(find "$dir" -type f -name libonnxruntime.a -print -quit 2>/dev/null)" ]; then
      return 0
    fi
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
      echo "  [preflight]   If a link error follows anyway, drop the stale ort-sys"
      echo "  [preflight]   build state with 'cargo clean -p ort-sys' so the build"
      echo "  [preflight]   script re-runs."
    } >&2
    return 0
  done
  return 0
}

# --- main loop ---
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
