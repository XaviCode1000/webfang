#!/usr/bin/env bash
set -euo pipefail
# test_ci_plan.sh — semantics harness for scripts/ci_plan.sh (#2282).
#
# Pins the event -> comparison-SHA contract and the crossplatform table:
# pull_request / merge_group / push are path-aware, schedule/dispatch/unknown
# and every missing-or-zero SHA fall back to FULL scope. All fixtures are
# throwaway `git init` trees under mktemp; this harness never reads or writes
# the repository and never invokes cargo.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLAN="${CI_PLAN:-$SCRIPT_DIR/ci_plan.sh}"
CLASSIFIER="${CI_PLAN_CLASSIFIER:-$SCRIPT_DIR/ci_path_classifier.sh}"

fail=0
PASSED=0
ok() {
  PASSED=$((PASSED + 1))
  echo "OK: $1"
}
bad() {
  echo "FAIL: $1"
  fail=1
}

[ -f "$PLAN" ] || {
  echo "FAIL: plan script not found at $PLAN"
  exit 1
}
[ -f "$CLASSIFIER" ] || {
  echo "FAIL: classifier not found at $CLASSIFIER"
  exit 1
}

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

R="$T/repo"
mkdir -p "$R"
git -C "$R" init -q
git -C "$R" config user.email ci-plan-test@example.invalid
git -C "$R" config user.name "CI Plan Test"
git -C "$R" config commit.gpgsign false
mkdir -p "$R/crates/webfang_core/src"
printf '[workspace]\nmembers = ["crates/webfang_core"]\n' > "$R/Cargo.toml"
printf '[package]\nname = "webfang_core"\nversion = "0.0.0"\n' > "$R/crates/webfang_core/Cargo.toml"
printf 'pub fn plain() {}\n' > "$R/crates/webfang_core/src/lib.rs"
printf '# web fang\n' > "$R/README.md"
git -C "$R" add -A
git -C "$R" commit -q -m "base"
BASE="$(git -C "$R" rev-parse HEAD)"

OUT="$T/out"
ERR_FILE="$T/plan.err"
CLASSIFIER_OVERRIDE=""

run_plan() {
  local event="$1"
  shift
  : > "$OUT"
  (
    cd "$R"
    env GITHUB_OUTPUT="$OUT" \
      EVENT_NAME="$event" \
      PR_BASE_SHA="${1:-}" \
      PR_HEAD_SHA="${2:-}" \
      MERGE_GROUP_BASE_SHA="${3:-}" \
      MERGE_GROUP_HEAD_SHA="${4:-}" \
      PUSH_BEFORE_SHA="${5:-}" \
      PUSH_AFTER_SHA="${6:-}" \
      CI_PLAN_CLASSIFIER="${CLASSIFIER_OVERRIDE:-$CLASSIFIER}" \
      bash "$PLAN" 2>"$ERR_FILE"
  )
}

kv() {
  grep -E "^$1=" "$OUT" | tail -1 | cut -d= -f2- || true
}

expect_kv() {
  local got
  got="$(kv "$1")"
  if [[ "$got" == "$2" ]]; then
    ok "$3 ($1=$got)"
  else
    bad "$3: expected $1=$2, got '$got'"
    sed 's/^/      /' "$OUT"
    sed 's/^/      /' "$ERR_FILE"
  fi
}

expect_contains() {
  local got
  got="$(kv "$1")"
  if [[ "$got" == *"$2"* ]]; then
    ok "$3 ($1 contains $2)"
  else
    bad "$3: expected $1 to contain '$2', got '$got'"
  fi
}

commit_case() {
  local branch="$1" file="$2" content="$3" message="$4"
  git -C "$R" switch -q -c "$branch" "$BASE"
  mkdir -p "$R/$(dirname "$file")"
  printf '%s\n' "$content" > "$R/$file"
  git -C "$R" add -A
  git -C "$R" commit -q -m "$message"
  git -C "$R" rev-parse HEAD
}

DOCS_HEAD="$(commit_case docs-case README.md "# docs only" "docs")"

run_plan pull_request "$BASE" "$DOCS_HEAD"
expect_kv run_code_jobs false "PR docs-only skips code jobs"
expect_kv needs_crossplatform false "PR docs-only skips crossplatform"
expect_kv required_producers "toolchain,repo-guards,artifact-guards,merge-metrics-guards,security,doc-quality" "PR docs-only emits narrow producers"
expect_contains scope_reason "mode=path-aware" "PR docs-only is path-aware"
expect_kv base_sha "$BASE" "PR docs-only reports base sha"
expect_kv head_sha "$DOCS_HEAD" "PR docs-only reports head sha"

merge_group_docs() {
  run_plan merge_group "" "" "$BASE" "$DOCS_HEAD" "" ""
}
merge_group_docs
expect_kv run_code_jobs false "merge_group docs-only skips code jobs"
expect_kv needs_crossplatform false "merge_group docs-only skips crossplatform"

RUST_HEAD="$(commit_case rust-case crates/webfang_core/src/plain.rs 'pub fn plain_two() {}' "plain rust")"
run_plan pull_request "$BASE" "$RUST_HEAD"
expect_kv run_code_jobs true "Rust-only PR runs code jobs"
expect_kv needs_crossplatform false "Rust-only PR without markers skips crossplatform"
expect_contains required_producers "test-core" "Rust-only PR emits full producers"

run_plan merge_group "" "" "$BASE" "$RUST_HEAD" "" ""
expect_kv run_code_jobs true "merge_group code PR runs code jobs"
expect_kv needs_crossplatform false "merge_group Rust-only without markers skips crossplatform"

push_docs() {
  run_plan push "" "" "" "" "$BASE" "$DOCS_HEAD"
}
push_docs
expect_kv run_code_jobs false "push docs-only skips code jobs"
expect_kv needs_crossplatform false "push docs-only skips crossplatform"

run_plan push "" "" "" "" "0000000000000000000000000000000000000000" "$RUST_HEAD"
expect_kv run_code_jobs true "push with zero before is full scope"
expect_kv needs_crossplatform true "push with zero before needs crossplatform"
expect_contains scope_reason "reason=push-before-zero-or-missing" "push zero before is explained"

run_plan schedule
expect_kv run_code_jobs true "schedule is full scope"
expect_kv needs_crossplatform true "schedule needs crossplatform"
expect_kv base_sha "" "schedule has no base sha"
expect_contains required_producers "test-full" "schedule emits full producers"

run_plan workflow_dispatch
expect_kv run_code_jobs true "workflow_dispatch is full scope"

run_plan unknown_event
expect_kv run_code_jobs true "unknown event is full scope"
expect_contains scope_reason "reason=unknown-event" "unknown event is explained"

run_plan pull_request "" "$DOCS_HEAD"
expect_kv run_code_jobs true "missing PR base is full scope"
expect_contains scope_reason "reason=pull_request-missing-sha" "missing PR base is explained"

# Filename surface: Cargo.toml, Cargo.lock, build.rs, .cargo/**, rust-toolchain.toml.
path_cases=(
  "Cargo.toml|[workspace]\nmembers = [\"crates/webfang_core\"]\n# platform"
  "Cargo.lock|version = 3"
  "crates/webfang_core/build.rs|fn main() {}"
  ".cargo/config.toml|[build]\nrustflags = []"
  "rust-toolchain.toml|[toolchain]\nchannel = \"1.88\""
)
i=0
for entry in "${path_cases[@]}"; do
  file="${entry%%|*}"
  content="${entry#*|}"
  i=$((i + 1))
  HEAD="$(commit_case "path-case-$i" "$file" "$content" "path $file")"
  run_plan pull_request "$BASE" "$HEAD"
  expect_kv needs_crossplatform true "crossplatform path: $file"
done

# Rust content table. Each marker gets its own branch and assertion.
marker_labels=(
  "cfg"
  "cfg_attr"
  "cfg_bang"
  "target_family"
  "target_pointer_width"
  "target_arch"
  "std_os_windows"
  "windows_hyphen"
  "windows_underscore"
  "winapi"
)
marker_bodies=(
  '#[cfg(target_os = "windows")] pub fn platform() {}'
  '#[cfg_attr(windows, allow(dead_code))] pub fn platform() {}'
  'pub const PLATFORM: bool = cfg!(windows);'
  '#[cfg(target_family = "windows")] pub fn platform() {}'
  '#[cfg(target_pointer_width = "64")] pub fn platform() {}'
  '#[cfg(target_arch = "x86_64")] pub fn platform() {}'
  'use std::os::windows::process::CommandExt;'
  '// windows-sys dependency'
  'use windows_sys::Win32;'
  '// winapi dependency'
)
for idx in "${!marker_labels[@]}"; do
  label="${marker_labels[$idx]}"
  body="${marker_bodies[$idx]}"
  HEAD="$(commit_case "marker-$label" crates/webfang_core/src/platform.rs "$body" "marker $label")"
  run_plan pull_request "$BASE" "$HEAD"
  expect_kv needs_crossplatform true "crossplatform marker: $label"
done

printf '#!/usr/bin/env bash\nexit 1\n' > "$T/fail-classifier.sh"
chmod +x "$T/fail-classifier.sh"
CLASSIFIER_OVERRIDE="$T/fail-classifier.sh"
run_plan pull_request "$BASE" "$DOCS_HEAD"
CLASSIFIER_OVERRIDE=""
expect_kv run_code_jobs true "classifier failure is full scope"
expect_kv needs_crossplatform true "classifier failure needs crossplatform"
expect_contains scope_reason "reason=classifier-failed" "classifier failure is explained"

if [[ "$fail" -ne 0 ]]; then
  echo "FAILED: $PASSED assertion(s) passed, at least one failed"
  exit 1
fi
echo "OK: $PASSED assertions passed"
