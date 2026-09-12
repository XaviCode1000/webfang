#!/usr/bin/env bash
#
# ci_fast_gate.sh — local equivalent of the PR Tier 1 fast gate.
#
# Design source: docs/research/webfang-workflow-transformation-blueprint.md
# §9 "Local pre-push gate" (./scripts/ci-fast-gate.sh: fmt, repo guards,
# affected-crate check/clippy, affected tests, mock integration tests —
# no AI/ONNX, no coverage, no release build by default).
#
# Usage: scripts/ci_fast_gate.sh [--base-ref <ref>] [--head-ref <ref>] [--dry-run]
#
# Behaviour: calls ci_path_classifier.sh, then runs exactly one lane:
#   all=true          -> FULL local gate (fail to full, never skip).
#   docs_only=true    -> cheap docs validation only (NO cargo).
#   ci_only=true      -> shell syntax + workflow validation only (NO cargo).
#   otherwise         -> `cargo fmt --check` + repo guards, then
#                        crate-targeted check/clippy/build/tests for the
#                        changed areas. Never AI inference, coverage,
#                        release builds, or broad mutation.
#
# Scope notes:
#   - The classifier sees committed range refs; uncommitted worktree edits
#     (staged, unstaged, untracked) are unioned in via --files so a local
#     run never misses work in progress. Read-only git commands only.
#   - No destructive commands anywhere: no `git reset/checkout/switch/stash`,
#     no `rm -rf`, no release, no push. Temp files use mktemp + trap.
#   - Steps are fail-open on MISSING tools (warn + skip) but fail-closed on
#     real findings. A missing linter must never greenwash, nor red-block.
#
# Timing log (Phase 6 observability, best-effort, never fails the gate):
#   on every non-dry-run finish this script appends one CSV line to
#   docs/ci-metrics/fast-gate.log:
#     date,branch,lane,result,seconds
#   date    UTC ISO-8601 of gate finish (`date` only, no new dependencies).
#   branch  `git branch --show-current` (or `unknown` when unreadable).
#   lane    one of: docs, ci, docs+ci, code, full, unknown.
#   result  `green` (FAIL=0) or `red` (FAIL>0).
#   seconds wall-clock seconds for the whole gate invocation.
#   The append is `|| true`-guarded: logging must never turn green red, and
#   dry runs are never logged (they would pollute the series).
#   The summary also echoes the `scripts/ci_metrics.sh` regeneration command
#   (echo-only, no behaviour change; see docs/ci-slo.md).

# NOTE: `set -e` is DELIBERATELY absent — the run_step collector owns
# failure handling so one red step never hides the rest. `-u` + pipefail
# stay on for robustness.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLASSIFIER="$SCRIPT_DIR/ci_path_classifier.sh"

# Phase 6 timing: start stamp + lane label (set in the dispatch below).
FAST_GATE_START_SECONDS="$(date +%s)"
FAST_GATE_LANE="unknown"

BASE_REF="origin/main"
HEAD_REF="HEAD"
DRY_RUN=false

usage() {
  sed -n '2,22p' "$0"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base-ref) BASE_REF="${2:?missing value for --base-ref}"; shift 2 ;;
    --head-ref) HEAD_REF="${2:?missing value for --head-ref}"; shift 2 ;;
    --dry-run) DRY_RUN=true; shift ;;
    -h | --help) usage; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 2 ;;
  esac
done

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo "$SCRIPT_DIR/..")"
cd "$ROOT" || exit 1

# --- step runner (no `set -e`: collect failures, report a summary) ------------
PASS=0
FAIL=0
SKIPPED=0
FAILED_STEPS=()

run_step() {
  local name="$1"
  shift
  if $DRY_RUN; then
    echo "[dry-run] would run: $name :: $*"
    return 0
  fi
  echo "==> $name"
  echo "    $ $*"
  if "$@"; then
    echo "    PASS: $name"
    PASS=$((PASS + 1))
  else
    echo "::error::FAIL: $name"
    FAIL=$((FAIL + 1))
    FAILED_STEPS+=("$name")
  fi
}

skip_step() {
  echo "    SKIP: $1 ($2)"
  SKIPPED=$((SKIPPED + 1))
}

# Phase 6: append one `date,branch,lane,result,seconds` line to
# docs/ci-metrics/fast-gate.log. Shell built-ins + `date` only, fully
# `|| true`-guarded: best-effort, never fails the gate.
log_fast_gate_timing() {
  local result="$1"
  local now elapsed branch finished_at
  now="$(date +%s)"
  elapsed=$((now - FAST_GATE_START_SECONDS))
  branch="$(git branch --show-current 2>/dev/null || echo unknown)"
  [[ -z "$branch" ]] && branch="unknown"
  finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  mkdir -p docs/ci-metrics 2>/dev/null || true
  printf '%s,%s,%s,%s,%s\n' "$finished_at" "$branch" \
    "$FAST_GATE_LANE" "$result" "$elapsed" \
    >> docs/ci-metrics/fast-gate.log 2>/dev/null || true
}

# --- classify ------------------------------------------------------------------
# Range files (committed) UNION worktree files (staged + unstaged + untracked)
# so uncommitted work is never invisible to the lane decision.

range_files() {
  local base="$1" head="$2" resolved=""
  local candidate
  for candidate in "$base" "main" "HEAD~1"; do
    if git rev-parse --verify --quiet "$candidate" >/dev/null 2>&1; then
      resolved="$candidate"
      break
    fi
  done
  [[ -z "$resolved" ]] && return 0
  git diff --name-only "${resolved}...${head}" -- 2>/dev/null \
    || git diff --name-only "$resolved" "$head" -- 2>/dev/null \
    || true
}

worktree_files() {
  {
    git diff --name-only -- 2>/dev/null || true
    git diff --cached --name-only -- 2>/dev/null || true
    git ls-files --others --exclude-standard 2>/dev/null || true
  }
}

UNION_TMP="$(mktemp)"
trap 'rm -f "$UNION_TMP"' EXIT
{
  range_files "$BASE_REF" "$HEAD_REF"
  worktree_files
} | grep -v '^$' | sort -u > "$UNION_TMP" || true

echo "fast-gate: base=$BASE_REF head=$HEAD_REF worktree files considered:"
sed 's/^/  changed: /' "$UNION_TMP"

CLASS_TMP="$(mktemp)"
trap 'rm -f "$UNION_TMP" "$CLASS_TMP"' EXIT
bash "$CLASSIFIER" --base-ref "$BASE_REF" --head-ref "$HEAD_REF" \
  --files "$(cat "$UNION_TMP")" --github-output "$CLASS_TMP"

# Load only the 13 known keys (never blind-source tool output).
# Pre-initialised so `set -u`-style readers and shellcheck (SC2154) see
# real assignments; printf -v below only overwrites them.
docs_only=false ci_only=false code_changed=false ai_changed=false
mcp_changed=false cli_changed=false core_changed=false crawler_changed=false
downloader_changed=false tests_changed=false release_changed=false
lock_changed=false all=false
for key in docs_only ci_only code_changed ai_changed mcp_changed cli_changed \
  core_changed crawler_changed downloader_changed tests_changed \
  release_changed lock_changed all; do
  val="$(grep -E "^${key}=" "$CLASS_TMP" | tail -1 | cut -d= -f2)"
  printf -v "$key" '%s' "${val:-false}"
done

echo "fast-gate: lane inputs: docs_only=$docs_only ci_only=$ci_only code=$code_changed all=$all"

# --- lane: docs-only (NO cargo) -------------------------------------------------
lane_docs() {
  local strict_docs_only="${1:-true}"
  echo "fast-gate lane: docs (cheap validation, no cargo)"
  run_step "whitespace (git diff --check)" git diff --check
  if [[ "$strict_docs_only" == "true" ]]; then
    # Re-assert the docs-only claim against the live worktree: a stray .rs or
    # manifest edit must flip the lane, not slip through.
    run_step "assert no code files in worktree diff" bash -c "
      ! grep -Eq '\\\\.rs$|Cargo\\\\.toml|Cargo\\\\.lock|^crates/|^tests/|^benches/|^examples/|^fuzz/|\\\\.github/|scripts/' '$UNION_TMP'
    "
  else
    skip_step "assert docs-only" "mixed known non-code scope"
  fi
  MD_FILES="$(grep -E '\.md$' "$UNION_TMP" || true)"
  if [[ -z "$MD_FILES" ]]; then
    skip_step "markdownlint" "no markdown files changed"
  elif command -v markdownlint >/dev/null 2>&1; then
    # shellcheck disable=SC2086
    run_step "markdownlint (changed markdown)" markdownlint --dot $MD_FILES
  else
    skip_step "markdownlint" "not installed (warn-only; docs lane stays cheap)"
  fi
  if grep -Eq '\.md$' "$UNION_TMP" 2>/dev/null && command -v python3 >/dev/null 2>&1; then
    run_step "docs link sanity (relative links resolve)" python3 - "$UNION_TMP" <<'EOF'
import os, re, sys
missing = []
for path in open(sys.argv[1], encoding="utf-8", errors="replace").read().splitlines():
    if not path.endswith(".md") or not os.path.isfile(path):
        continue
    base = os.path.dirname(path)
    for m in re.finditer(r"\]\((?!https?://|#|mailto:)([^)#\s]+)", open(path, encoding="utf-8", errors="replace").read()):
        target = os.path.normpath(os.path.join(base, m.group(1)))
        if not os.path.exists(target):
            missing.append(f"{path}: {m.group(1)}")
if missing:
    print("\n".join(missing)); sys.exit(1)
print("OK: relative doc links resolve")
EOF
  else
    skip_step "docs link sanity" "no markdown changed or no python3"
  fi
  # Phase 3: orphan snapshots rot silently, so the check runs even on the
  # cheap docs lane. Blocking run_step like the surrounding steps (never
  # warn-only); markdownlint above stays warn-only as is.
  run_guard "orphan snapshot guard" scripts/check_orphan_snapshots.sh \
    bash scripts/check_orphan_snapshots.sh
}

# --- lane: ci-only (NO cargo) ----------------------------------------------------
lane_ci() {
  echo "fast-gate lane: CI-ONLY (shell + workflow validation, no cargo)"
  # Same ignore vocabulary as the repo-guards job in .github/workflows/ci.yml
  # (#1070): structural findings stay fatal, style debt stays ignored.
  local ignore="SC2010,SC2012,SC2027,SC2034,SC2046,SC2086,SC2126,SC2129"
  local -a script_files=()
  local -a workflow_files=()
  # mapfile returns non-zero on empty input, hence `|| true` under `set -u`.
  mapfile -t script_files < <(grep -E '^scripts/[^/]*\.sh$' "$UNION_TMP" || true) || true
  mapfile -t workflow_files < <(grep -E '^\.github/workflows/[^/]*\.ya?ml$' "$UNION_TMP" || true) || true
  if [[ ${#script_files[@]} -gt 0 ]]; then
    run_step "bash syntax (changed scripts)" bash -n "${script_files[@]}"
    if command -v shellcheck >/dev/null 2>&1; then
      run_step "shellcheck (changed scripts, shared ignore list)" shellcheck -x --exclude="$ignore" --format=gcc "${script_files[@]}"
    else
      skip_step "shellcheck" "not installed"
    fi
  else
    skip_step "shell syntax/shellcheck" "no changed scripts"
  fi
  if [[ ${#workflow_files[@]} -gt 0 ]]; then
    if command -v actionlint >/dev/null 2>&1; then
      run_step "actionlint (changed workflows)" actionlint -ignore SC2010 -ignore SC2012 -ignore SC2027 -ignore SC2034 -ignore SC2046 -ignore SC2086 -ignore SC2126 -ignore SC2129 "${workflow_files[@]}"
    elif python3 -c "import yaml" 2>/dev/null; then
      run_step "workflow YAML parse (actionlint absent)" python3 - "${workflow_files[@]}" <<'EOF'
import glob, sys, yaml
for p in sys.argv[1:]:
    yaml.safe_load(open(p))
print('OK: changed workflow YAML parses')
EOF
    else
      skip_step "workflow validation" "neither actionlint nor python3-yaml available"
    fi
  else
    skip_step "workflow validation" "no changed workflows"
  fi
}

# --- shared: fmt + repo guards ----------------------------------------------------
# Mirrors the `repo-guards` job + `fmt` tier of .github/workflows/ci.yml.
# Cheap greps/scripts only; every guard present in scripts/ runs, missing
# ones warn-skip (never silently dropped: the skip is logged).

# run_guard <name> <script> <cmd...>: run_step when the guard script
# exists and is readable, else a logged skip (never a silent drop).
# Readability, NOT executability (#1302): every wrapped command invokes
# the script through `bash`, exactly like CI does, so a missing exec bit
# (e.g. check_dependency_direction.sh is -rw-r--r-- in main) must not
# skip the guard locally while CI still runs it. Avoids the
# `A && B || C` idiom (shellcheck SC2015).
run_guard() {
  local name="$1" script="$2"
  shift 2
  if [[ -f "$script" && -r "$script" ]]; then
    run_step "$name" "$@"
  else
    skip_step "$name" "script absent or unreadable ($script)"
  fi
}

lane_fmt_and_guards() {
  run_step "cargo fmt --check" cargo fmt --all -- --check
  run_guard "dead test detection" scripts/check_dead_tests.sh \
    bash scripts/check_dead_tests.sh
  run_step "forbid anyhow in webfang_core (#428)" bash -c "
    if grep -rn 'use anyhow' crates/webfang_core/src/ --include='*.rs'; then
      echo 'webfang_core must use typed errors, not anyhow'; exit 1
    fi
    echo 'OK: no anyhow imports in webfang_core/src'"
  run_guard "dependency direction gate (#513)" scripts/check_dependency_direction.sh \
    bash scripts/check_dependency_direction.sh
  run_guard "intra-crate direction gate, strict (ADR-0010)" scripts/check_intra_crate_direction.sh \
    env INTRA_CRATE_MODE=strict bash scripts/check_intra_crate_direction.sh
  run_guard "ignored-test inventory guard" scripts/check_ignored_guard.sh \
    bash scripts/check_ignored_guard.sh
  # Phase 3: same guard as the docs lane — blocking here because every
  # surrounding repo-guard step is blocking (fail-closed on findings,
  # warn-skip only when the script itself is absent).
  run_guard "orphan snapshot guard" scripts/check_orphan_snapshots.sh \
    bash scripts/check_orphan_snapshots.sh
  run_step "forbid sitemap string-match coupling" bash -c "
    if grep -rn 'contains(\"no URLs found\")' crates/*/src/ --include='*.rs'; then
      echo 'use ScraperError::SitemapEmpty, never message matching'; exit 1
    fi
    echo 'OK: no string-match sitemap coupling'"
}

# --- shared: crate-targeted check/clippy/build/tests ------------------------------
# Packages derive from the classifier flags. crawler/downloader live inside
# webfang_core, so those flags fold into -p webfang_core. No package matched
# but code changed (e.g. only root Cargo.toml/clippy.toml) -> workspace scope
# (fail to full at the check level, still no AI/coverage/release/mutation).

targeted_cargo() {
  local -a pkgs=()
  [[ "$core_changed" == "true" || "$crawler_changed" == "true" || "$downloader_changed" == "true" ]] && pkgs+=(webfang_core)
  [[ "$cli_changed" == "true" ]] && pkgs+=(webfang_cli)
  [[ "$mcp_changed" == "true" ]] && pkgs+=(webfang_mcp)
  [[ "$ai_changed" == "true" ]] && pkgs+=(webfang_ai)
  if [[ "$code_changed" == "true" && ${#pkgs[@]} -eq 0 ]]; then
    echo "fast-gate: code changed outside known crates -> workspace scope"
    run_step "cargo check (workspace, all targets+features)" cargo check --workspace --all-targets --all-features
    run_step "clippy strict (workspace)" cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines
    run_step "nextest unit (workspace lib)" cargo nextest run --workspace --lib --test-threads 4 --retries 2
    return 0
  fi
  if [[ ${#pkgs[@]} -eq 0 ]]; then
    # tests_changed alone (e.g. only snapshots touched): still compile the
    # workspace default surface so the harness stays green.
    echo "fast-gate: no crate flags; defaulting test scope to webfang_core"
    pkgs+=(webfang_core)
  fi
  echo "fast-gate: targeted packages: ${pkgs[*]}"
  # shellcheck disable=SC2068
  run_step "cargo check (-p ${pkgs[*]}, all targets+features)" cargo check $(printf -- '-p %s ' ${pkgs[@]}) --all-targets --all-features
  # shellcheck disable=SC2068
  run_step "clippy strict (-p ${pkgs[*]})" cargo clippy $(printf -- '-p %s ' ${pkgs[@]}) --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines
  if [[ "$cli_changed" == "true" || "$core_changed" == "true" ]]; then
    # Pre-build mirrors ci.yml test-core: behavioral tests resolve the
    # binary via tests/common/cli_harness.rs webfang_path().
    run_step "pre-build webfang binary" cargo build -p webfang_cli --bin webfang
  fi
  # Unit + mock integration/behavioral for the affected crates. Default
  # nextest runs NEVER include #[ignore] model tests, so no AI inference
  # happens here; coverage/release/mutation are main-tier concerns.
  # shellcheck disable=SC2068
  run_step "nextest unit (-p ${pkgs[*]} --lib)" cargo nextest run $(printf -- '-p %s ' ${pkgs[@]}) --lib --test-threads 4 --retries 2
  if [[ "$tests_changed" == "true" || "$crawler_changed" == "true" || "$downloader_changed" == "true" || "$cli_changed" == "true" || "$mcp_changed" == "true" ]]; then
    # shellcheck disable=SC2068
    run_step "nextest integration (-p ${pkgs[*]} --tests)" cargo nextest run $(printf -- '-p %s ' ${pkgs[@]}) --tests --test-threads 4 --retries 2
  else
    skip_step "nextest integration" "no runtime-area flags (crawler/downloader/cli/mcp/tests)"
  fi
}

# --- lane: full local gate (unknown scope — fail to full, never skip) --------------
# Mirrors the AGENTS.md pre-commit gate plus the workspace test surface
# (test-core + test-full minus AI inference). Still no coverage, release,
# cross-platform, sanitizers, fuzz, or mutation: those are main/nightly tiers.

lane_full() {
  echo "fast-gate lane: FULL (unknown scope — nothing skipped)"
  lane_fmt_and_guards
  run_step "cargo check (workspace, all targets+features)" cargo check --workspace --all-targets --all-features
  run_step "clippy strict (workspace)" cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines
  run_step "pre-build webfang binary" cargo build -p webfang_cli --bin webfang --all-features
  run_step "nextest (workspace, all features)" cargo nextest run --workspace --all-features --test-threads 4 --retries 2
}

# --- dispatch -----------------------------------------------------------------------

if [[ "$all" == "true" ]]; then
  FAST_GATE_LANE="full"
  lane_full
elif [[ "$docs_only" == "true" && "$ci_only" == "true" ]]; then
  FAST_GATE_LANE="docs+ci"
  echo "fast-gate note: change is docs+CI only; running docs and CI lanes."
  lane_docs true
  lane_ci
elif [[ "$docs_only" == "true" || "$ci_only" == "true" ]]; then
  if [[ "$release_changed" == "true" ]]; then
    echo "fast-gate note: non-code change BUT release files touched (e.g. CHANGELOG) — release validation stays a main-tier concern; running the cheap lane."
  fi
  if [[ "$docs_only" == "true" ]]; then
    FAST_GATE_LANE="docs"
    lane_docs true
  fi
  if [[ "$ci_only" == "true" ]]; then
    FAST_GATE_LANE="ci"
    lane_ci
  fi
elif [[ "$code_changed" != "true" ]]; then
  FAST_GATE_LANE="docs+ci"
  echo "fast-gate note: known non-code scope; running cheap docs+CI lanes."
  lane_docs false
  lane_ci
elif [[ "$code_changed" == "true" ]]; then
  FAST_GATE_LANE="code"
  echo "fast-gate lane: CODE (fmt + guards + targeted cargo)"
  lane_fmt_and_guards
  targeted_cargo
  # Phase 4 (advisory only): regression naming convention check. Runs
  # outside run_step and can never fail the lane — findings are
  # ::notice::/NOTICE lines for the author.
  if [[ -x scripts/check_regression_naming.sh ]]; then
    echo "fast-gate: regression naming check (advisory, never red)"
    bash scripts/check_regression_naming.sh || true
  fi
fi

# --- summary --------------------------------------------------------------------------

echo "----------------------------------------"
echo "fast-gate summary: PASS=$PASS FAIL=$FAIL SKIP=$SKIPPED"
FAST_GATE_RESULT="green"
if [[ $FAIL -gt 0 ]]; then
  printf 'failed steps:\n'
  printf '  - %s\n' "${FAILED_STEPS[@]}"
  FAST_GATE_RESULT="red"
fi
# Phase 6 observability: best-effort timing log (never fails the gate; dry
# runs are skipped) + metrics command hint (echo only, no behaviour change).
if ! $DRY_RUN; then
  log_fast_gate_timing "$FAST_GATE_RESULT"
fi
echo "metrics: regenerate the SLO snapshot with: bash scripts/ci_metrics.sh [--days N] [--workflow ci.yml] [--branch main] (see docs/ci-slo.md)"
if [[ $FAIL -gt 0 ]]; then
  exit 1
fi
echo "fast-gate: GREEN"
