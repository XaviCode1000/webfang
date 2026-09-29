#!/usr/bin/env bash
set -euo pipefail
# ci_path_classifier.sh — changed-path semantics harness (RED before GREEN)
#
# Regression coverage for #1643: `git diff --name-only -z` captured through
# `$(...)` lost its NUL separators (bash drops NUL bytes in command
# substitutions), so every multi-file PR reported `1 file(s)` and classified
# one concatenated pseudo-path. When that concatenation happened to match a
# narrow category — a leading `.github/workflows/ci.yml` does — the unknown
# -surface widening never fired, `run_code_jobs` stayed false, and every code
# lane gated behind it was skipped in cascade while `CI Gate` stayed green.
#
# What is pinned here:
#   1. multi-file diffs report the real count, per path
#   2. CI + Rust in one diff keeps its code lanes (the live symptom)
#   3. the widen-to-everything net still fires on a genuinely unknown path
#   4. single-file, docs-only and spaced-path diffs keep their verdicts
#   5. the --files override still classifies a multi-line list independently
#   6. the output contract (key set, order, true/false shape) is unchanged
#      for its four consumers: .github/workflows/ci.yml,
#      scripts/ci_fast_gate.sh, scripts/ci_test_budget.sh,
#      scripts/ci_mutation_scope.sh
#
# Exit 0 = all checks pass. Exit 1 = at least one failed.
#
# Fixtures are throwaway `git init` trees under a mktemp dir, so this harness
# never reads or writes the repository, and it never invokes cargo.
# CI_PATH_CLASSIFIER overrides the script under test (used to run this same
# harness against a pre-fix copy and prove it fails there).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLASSIFIER="${CI_PATH_CLASSIFIER:-$SCRIPT_DIR/ci_path_classifier.sh}"
MUTATION_SCOPE="$SCRIPT_DIR/ci_mutation_scope.sh"

fail=0
ok() {
  PASSED=$((PASSED + 1))
  echo "OK: $1"
}
bad() {
  echo "FAIL: $1"
  fail=1
}
PASSED=0

[ -f "$CLASSIFIER" ] || {
  echo "FAIL: classifier script not found at $CLASSIFIER"
  exit 1
}

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

OUT=""
SUM=""

# --- helpers -------------------------------------------------------------------

# new_repo <name> -> echoes a fresh git repo path with one empty commit.
new_repo() {
  local d="$T/$1"
  mkdir -p "$d"
  git -C "$d" init -q
  git -C "$d" config user.email classifier-test@example.invalid
  git -C "$d" config user.name "Classifier Test"
  git -C "$d" config commit.gpgsign false
  echo "$d"
}

# commit_all <repo> <message>
commit_all() {
  git -C "$1" add -A
  git -C "$1" commit -q -m "$2"
}

# classify <repo> <base> <head> [extra args...] -> sets OUT (key=value lines)
# and SUM (the stderr summary). Never aborts the harness on a classifier
# failure: a missing verdict is a FAIL, not a crash.
classify() {
  local repo="$1" base="$2" head="$3"
  shift 3
  OUT="$(cd "$repo" && bash "$CLASSIFIER" --base "$base" --head "$head" \
    --format github "$@" 2>"$T/stderr")" || true
  SUM="$(cat "$T/stderr")"
}

# kv <key> -> value of the last `key=` line in OUT, or "" when the key is
# absent (an absent key is a FAIL for the caller, never an abort here: the
# `grep` miss must not trip `set -e` before the assertion can report it).
kv() {
  printf '%s\n' "$OUT" | grep -E "^$1=" | tail -1 | cut -d= -f2- || true
}

# file_count -> the `N file(s)` count the classifier reports on stderr.
file_count() {
  printf '%s\n' "$SUM" | sed -n 's/^classifier: \([0-9][0-9]*\) file(s).*/\1/p' || true
}

# expect_kv <key> <expected> <label>
expect_kv() {
  local got
  got="$(kv "$1")"
  if [[ "$got" == "$2" ]]; then
    ok "$3 ($1=$got)"
  else
    bad "$3: expected $1=$2, got '$got'"
    printf '%s\n' "$SUM" | sed 's/^/      /'
  fi
}

# expect_files <expected> <label>
expect_files() {
  local got
  got="$(file_count)"
  if [[ "$got" == "$1" ]]; then
    ok "$2 (${got} file(s))"
  else
    bad "$2: expected $1 file(s), got '${got:-none}'"
    printf '%s\n' "$SUM" | sed 's/^/      /'
  fi
}

# expect_tracked <repo> <path> <label> — a fixture a developer's global
# gitignore drops would silently shrink the diff and make every count
# assertion below it vacuous, so pin the fixture is really committed.
expect_tracked() {
  if git -C "$1" ls-files --error-unmatch -- "$2" >/dev/null 2>&1; then
    ok "$3"
  else
    bad "$3: '$2' is not tracked (ignored?); the fixture diff would be smaller than intended"
  fi
}

# ---------------------------------------------------------------------------
# 1. The issue's own reproduction: three changed files, one per category.
# ---------------------------------------------------------------------------
R="$(new_repo repro3)"
mkdir -p "$R/.github/workflows" "$R/scripts" "$R/odd/tasks"
printf 'name: ci\n' >"$R/.github/workflows/ci.yml"
printf 'echo compat\n' >"$R/scripts/check_compatibility.sh"
printf 'base\n' >"$R/odd/tasks/repro.md"
commit_all "$R" base
BASE="$(git -C "$R" rev-parse HEAD)"
printf 'name: ci\n# touched\n' >>"$R/.github/workflows/ci.yml"
printf 'echo compat\n# touched\n' >>"$R/scripts/check_compatibility.sh"
printf 'base\ntouched\n' >>"$R/odd/tasks/repro.md"
commit_all "$R" repro

classify "$R" "$BASE" HEAD
expect_files 3 "multi-file diff reports the real count (#1643)"
expect_kv ci_only false "mixed ci+docs diff is not ci_only"
expect_kv docs_only false "mixed ci+docs diff is not docs_only"
expect_kv code false "no Rust file in the diff"
expect_kv all false "every path is a known surface"
expect_kv run_code_jobs false "nothing in the diff is a build input"
expect_kv affected none "no area flag is set"

# ---------------------------------------------------------------------------
# 2. The live symptom: CI file + Rust source in ONE diff. Pre-fix the
#    concatenated pseudo-path started with `.github/` and matched is_ci_file,
#    so ci_only=true, the unknown widening never fired, run_code_jobs stayed
#    false and clippy/fmt/test lanes were skipped in cascade.
# ---------------------------------------------------------------------------
R="$(new_repo ci_plus_code)"
mkdir -p "$R/.github/workflows" "$R/crates/webfang_core/src"
printf 'name: ci\n' >"$R/.github/workflows/ci.yml"
printf 'pub fn a() {}\n' >"$R/crates/webfang_core/src/lib.rs"
commit_all "$R" base
BASE="$(git -C "$R" rev-parse HEAD)"
printf 'name: ci\n# touched\n' >>"$R/.github/workflows/ci.yml"
printf 'pub fn a() {}\npub fn b() {}\n' >"$R/crates/webfang_core/src/lib.rs"
commit_all "$R" change
expect_tracked "$R" crates/webfang_core/src/lib.rs "fixture: Rust source is committed"

classify "$R" "$BASE" HEAD
expect_files 2 "ci + Rust diff counts both files"
expect_kv ci_only false "ci + Rust diff is not ci_only"
expect_kv code true "ci + Rust diff still runs the code lanes"
expect_kv lib_src_changed true "Rust library source detected"
expect_kv affected code,core "core area detected alongside code"
expect_kv all false "both paths are known surfaces"
expect_kv run_code_jobs true "the lost clippy/test lane is recovered"

# ---------------------------------------------------------------------------
# 3. The widen-to-everything net must still fire for a genuinely unknown
#    path (scripts/ci_path_classifier.sh, `if ! $known; then all=true; fi`).
#    The fixture is `odd/tasks/settings.toml`: not docs, not CI, not code.
#    Deliberately NOT `.envrc` — a developer's global gitignore can drop it
#    and silently shrink the diff to the CI file alone.
# ---------------------------------------------------------------------------
R="$(new_repo unknown)"
mkdir -p "$R/.github/workflows" "$R/odd/tasks"
printf 'name: ci\n' >"$R/.github/workflows/ci.yml"
printf 'key = 1\n' >"$R/odd/tasks/settings.toml"
commit_all "$R" base
BASE="$(git -C "$R" rev-parse HEAD)"
printf 'name: ci\n# touched\n' >>"$R/.github/workflows/ci.yml"
printf 'key = 1\n# touched\n' >>"$R/odd/tasks/settings.toml"
commit_all "$R" change
expect_tracked "$R" odd/tasks/settings.toml "fixture: the unknown path is committed"

classify "$R" "$BASE" HEAD
expect_files 2 "unknown-surface diff counts both files"
expect_kv all true "an unknown path still widens to everything"
expect_kv ci_only false "widening supersedes the ci_only claim"
expect_kv run_code_jobs true "unknown surface fails closed to the code lanes"
expect_kv needs_ai true "unknown surface fails closed to the AI lane"
expect_kv needs_mcp true "unknown surface fails closed to the MCP lane"
expect_kv affected all "affected reports the widening"

# ---------------------------------------------------------------------------
# 4. Single-file diff keeps its narrow verdict (no false widening).
# ---------------------------------------------------------------------------
R="$(new_repo single)"
mkdir -p "$R/crates/webfang_core/src"
printf 'pub fn a() {}\n' >"$R/crates/webfang_core/src/lib.rs"
commit_all "$R" base
BASE="$(git -C "$R" rev-parse HEAD)"
printf 'pub fn a() {}\npub fn b() {}\n' >"$R/crates/webfang_core/src/lib.rs"
commit_all "$R" change

classify "$R" "$BASE" HEAD
expect_files 1 "single-file diff reports one file"
expect_kv code true "single Rust file is code"
expect_kv all false "a single known code path does not widen"
expect_kv run_code_jobs true "single Rust file runs the code lanes"

# ---------------------------------------------------------------------------
# 5. Docs-only diff keeps the docs lane and skips the code lanes.
# ---------------------------------------------------------------------------
R="$(new_repo docs)"
mkdir -p "$R/docs"
printf 'guide\n' >"$R/README.md"
printf 'guide\n' >"$R/docs/guide.md"
commit_all "$R" base
BASE="$(git -C "$R" rev-parse HEAD)"
printf 'guide\ntouched\n' >>"$R/README.md"
printf 'guide\ntouched\n' >>"$R/docs/guide.md"
commit_all "$R" change

classify "$R" "$BASE" HEAD
expect_files 2 "docs-only diff reports both files"
expect_kv docs_only true "markdown-only diff stays docs_only"
expect_kv ci_only false "docs-only diff is not ci_only"
expect_kv code false "markdown is not a build input"
expect_kv all false "docs-only diff does not widen"
expect_kv run_code_jobs false "docs-only diff skips the code lanes"

# ---------------------------------------------------------------------------
# 6. NUL separation is preserved: a path containing a space is its own
#    element (proves the fix did not fall back to a newline split).
# ---------------------------------------------------------------------------
R="$(new_repo spaced)"
mkdir -p "$R/odd dir"
printf 'one\n' >"$R/odd dir/one file.md"
printf 'two\n' >"$R/odd dir/two file.md"
commit_all "$R" base
BASE="$(git -C "$R" rev-parse HEAD)"
printf 'one\ntouched\n' >>"$R/odd dir/one file.md"
printf 'two\ntouched\n' >>"$R/odd dir/two file.md"
commit_all "$R" change
expect_tracked "$R" "odd dir/one file.md" "fixture: a path with a space is committed"

classify "$R" "$BASE" HEAD
expect_files 2 "paths with spaces are split on NUL, not on spaces"
expect_kv docs_only true "both spaced markdown paths classify as docs"

# ---------------------------------------------------------------------------
# 7. The --files override (used by ci_fast_gate.sh and
#    ci_mutation_scope.sh via CI_*_FILES) classifies a multi-line list
#    independently too.
# ---------------------------------------------------------------------------
OUT="$(bash "$CLASSIFIER" --files "scripts/x.sh
.github/workflows/ci.yml
crates/webfang_core/src/lib.rs" --format github 2>"$T/stderr")" || true
SUM="$(cat "$T/stderr")"
expect_files 3 "--files list of 3 paths reports 3 files"
expect_kv code true "--files list keeps the code lane"
expect_kv all false "--files list of known paths does not widen"
expect_kv affected code,core "affected lists the code and core areas"

# ---------------------------------------------------------------------------
# 8. Output contract. .github/workflows/ci.yml and scripts/ci_test_budget.sh
#    read --format github; ci_fast_gate.sh and ci_mutation_scope.sh read the
#    human --github-output. Both key sets must stay exactly as documented —
#    the fix must not rename, add or drop a key.
# ---------------------------------------------------------------------------
GITHUB_KEYS="docs_only ci_only code all affected run_code_jobs needs_ai needs_mcp needs_mutation_hotpath snapshot_changed lib_src_changed"
HUMAN_KEYS="docs_only ci_only code_changed ai_changed mcp_changed cli_changed core_changed crawler_changed downloader_changed tests_changed release_changed lock_changed all needs_ai needs_mcp needs_mutation_hotpath snapshot_changed lib_src_changed"

R="$(new_repo contract)"
mkdir -p "$R/crates/webfang_core/src"
printf 'pub fn a() {}\n' >"$R/crates/webfang_core/src/lib.rs"
commit_all "$R" base
BASE="$(git -C "$R" rev-parse HEAD)"
printf 'pub fn a() {}\npub fn b() {}\n' >"$R/crates/webfang_core/src/lib.rs"
commit_all "$R" change

GOT_OUT="$T/gh.out"
HUM_OUT="$T/human.out"
if (cd "$R" && bash "$CLASSIFIER" --base "$BASE" --head HEAD --format github) \
  >"$GOT_OUT" 2>/dev/null; then
  ok "classifier exits 0 on a successful classification"
else
  bad "classifier exited non-zero on a successful classification"
fi
got_keys="$(cut -d= -f1 "$GOT_OUT" | tr '\n' ' ' | sed 's/ $//')"
if [[ "$got_keys" == "$GITHUB_KEYS" ]]; then
  ok "github format emits the documented 11 keys in order"
else
  bad "github format keys changed"
  printf '      expected: %s\n      got:      %s\n' "$GITHUB_KEYS" "$got_keys"
fi
# Every flag is true/false; only `affected` is a CSV.
if grep -v '^affected=' "$GOT_OUT" | cut -d= -f2 | sort -u | tr '\n' ' ' | sed 's/ $//' \
  | grep -qx 'false true'; then
  ok "every non-affected value is true/false"
else
  bad "github format emitted a value outside true/false"
  sed 's/^/      /' "$GOT_OUT"
fi

(cd "$R" && bash "$CLASSIFIER" --base "$BASE" --head HEAD \
  --github-output "$HUM_OUT") 2>/dev/null
got_keys="$(cut -d= -f1 "$HUM_OUT" | tr '\n' ' ' | sed 's/ $//')"
if [[ "$got_keys" == "$HUMAN_KEYS" ]]; then
  ok "human format emits the documented 18 keys in order"
else
  bad "human format keys changed"
  printf '      expected: %s\n      got:      %s\n' "$HUMAN_KEYS" "$got_keys"
fi
if grep -q '^core_changed=true$' "$HUM_OUT" && grep -q '^lib_src_changed=true$' "$HUM_OUT"; then
  ok "human format reports the core/library change consumed by ci_fast_gate.sh"
else
  bad "human format lost core_changed/lib_src_changed"
  sed 's/^/      /' "$HUM_OUT"
fi

# 8b. A real consumer, run through its own --files path (no cargo):
#     ci_mutation_scope.sh must recommend the core hot path for a
#     multi-file list containing Rust source.
if [ -x "$MUTATION_SCOPE" ] || [ -f "$MUTATION_SCOPE" ]; then
  mut_out="$(bash "$MUTATION_SCOPE" --files "scripts/x.sh
crates/webfang_core/src/lib.rs" 2>/dev/null || true)"
  if printf '%s\n' "$mut_out" | grep -q 'needs_mutation_hotpath=true'; then
    ok "ci_mutation_scope.sh still reads needs_mutation_hotpath for a multi-file list"
  else
    bad "ci_mutation_scope.sh lost the hot-path recommendation"
    printf '%s\n' "$mut_out" | sed 's/^/      /'
  fi
else
  echo "SKIP: ci_mutation_scope.sh not present"
fi

# ---------------------------------------------------------------------------
# 9. A diff that cannot run fails closed, exactly as before the fix.
# ---------------------------------------------------------------------------
R="$(new_repo empty)"
printf 'x\n' >"$R/README.md"
commit_all "$R" base
BASE="$(git -C "$R" rev-parse HEAD)"

classify "$R" "$BASE" HEAD
expect_files 0 "an empty diff reports 0 files"
expect_kv all true "an empty diff fails closed to all=true"
expect_kv run_code_jobs true "an empty diff runs the code lanes"

echo
if [ "$fail" = "0" ]; then
  echo "All ci_path_classifier changed-path checks passed ($PASSED checks)."
  exit 0
fi
echo "Some ci_path_classifier changed-path checks FAILED."
exit 1
