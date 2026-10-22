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
#        [--allow-unregistered-target]
#   --allow-unregistered-target accepts a target outside the worktree registry; it never allows sharing another worktree's target (#1679).
#
# Behaviour: calls ci_path_classifier.sh, then runs exactly one lane:
#   all=true          -> FULL local gate (fail to full, never skip).
#   docs_only=true    -> cheap docs validation only (NO cargo).
#   ci_only=true      -> shell syntax + workflow validation only (NO cargo).
#   otherwise         -> `cargo fmt --check` + repo guards, then
#                        crate-targeted check/clippy/rustdoc/build/tests for
#                        the changed areas. The rustdoc step mirrors the CI
#                        `doc-quality` job (`cargo doc --workspace
#                        --all-features --no-deps` with `RUSTDOCFLAGS=-D
#                        warnings`) and runs only when library source
#                        (`crates/*/src/**.rs`, classifier lib_src_changed)
#                        changed; the FULL lane always runs it. Never AI
#                        inference, coverage, release builds, or broad
#                        mutation.
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
#   lane    one of: docs, ci, docs+ci, code, full, unknown
#           (`unknown` when the gate aborts before the lane dispatch).
#   result  `green` (FAIL=0), `red` (FAIL>0), or `early-exit`
#           (fail-closed abort before the summary, via the EXIT trap).
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
FAST_GATE_LOGGED=false
FAST_GATE_LOG_FILE=""

# Phase 6: append one `date,branch,lane,result,seconds` line to
# docs/ci-metrics/fast-gate.log. Shell built-ins + `date` only, fully
# `|| true`-guarded: best-effort, never fails the gate.
# result is `green`/`red` (summary path) or `early-exit` (EXIT-trap path
# for fail-closed aborts before the summary). Sets FAST_GATE_LOGGED so the
# trap never double-logs a run the summary already recorded.
log_fast_gate_timing() {
  local result="$1"
  local now elapsed branch finished_at
  FAST_GATE_LOGGED=true
  if [[ -z "$FAST_GATE_LOG_FILE" ]]; then
    return 0
  fi
  now="$(date +%s)"
  elapsed=$((now - FAST_GATE_START_SECONDS))
  branch="$(git branch --show-current 2>/dev/null || echo unknown)"
  [[ -z "$branch" ]] && branch="unknown"
  finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  mkdir -p "$(dirname "$FAST_GATE_LOG_FILE")" 2>/dev/null || true
  printf '%s,%s,%s,%s,%s\n' "$finished_at" "$branch" \
    "$FAST_GATE_LANE" "$result" "$elapsed" \
    >> "$FAST_GATE_LOG_FILE" 2>/dev/null || true
}

# Early-exit coverage (#1789): the fail-closed guards below `exit 2` before
# the lane dispatch, so without this trap those aborts skip logging entirely.
# The trap is installed only after FAST_GATE_START_SECONDS and the validated
# absolute FAST_GATE_LOG_FILE exist (`set -u` safety: the handler never
# references an unset variable). Consequences, declared: the trap precedes
# arg parsing, so usage-error exits (code 2) are covered; clean exits
# (`--help`, exit 0) intentionally leave no row.
#
# Worktree safety (load-bearing): the path is absolute, anchored at this
# script's own worktree — never relative, because pre-`cd` exits run under
# the caller's CWD and a relative append there would dirty whatever tree
# invoked the gate (including `main`). When the path cannot be validated to
# live inside the current worktree, it stays empty and the handler — like
# every logging path here — writes nothing.
_FAST_GATE_SCRIPT_TOP="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null || true)"
if [[ -n "$_FAST_GATE_SCRIPT_TOP" ]]; then
  _FAST_GATE_CANDIDATE="$_FAST_GATE_SCRIPT_TOP/docs/ci-metrics/fast-gate.log"
  _FAST_GATE_CANDON="$(realpath -m -- "$_FAST_GATE_CANDIDATE" 2>/dev/null || true)"
  _FAST_GATE_TOPCANON="$(realpath -m -- "$_FAST_GATE_SCRIPT_TOP" 2>/dev/null || true)"
  if [[ -n "$_FAST_GATE_TOPCANON" && "$_FAST_GATE_CANDON" == "$_FAST_GATE_TOPCANON"* ]]; then
    FAST_GATE_LOG_FILE="$_FAST_GATE_CANDIDATE"
  fi
fi
unset _FAST_GATE_SCRIPT_TOP _FAST_GATE_CANDIDATE _FAST_GATE_CANDON _FAST_GATE_TOPCANON

fast_gate_early_exit_trap() {
  local rc=$?
  # Owns temp cleanup: this is the only EXIT trap in this script.
  rm -f "${UNION_TMP:-}" "${CLASS_TMP:-}" 2>/dev/null || true
  # Exactly-once: the summary path already logged.
  if [[ "${FAST_GATE_LOGGED:-false}" == "true" ]]; then
    return "$rc"
  fi
  # Clean exits (e.g. `--help`) leave no row; only failures are traced.
  if [[ "$rc" -eq 0 ]]; then
    return "$rc"
  fi
  # Dry runs never pollute the series (current behavior, kept).
  if [[ "${DRY_RUN:-false}" == "true" ]]; then
    return "$rc"
  fi
  # Unvalidated log path: write nothing (worktree safety).
  if [[ -z "${FAST_GATE_LOG_FILE:-}" ]]; then
    return "$rc"
  fi
  # `|| true`-guarded like the summary append: logging never turns any
  # outcome redder or greener. No `exit`, no PASS/FAIL touch — the script
  # keeps the exit code captured on entry.
  log_fast_gate_timing "early-exit" || true
  return "$rc"
}
trap fast_gate_early_exit_trap EXIT
# Signal deaths (#1789): INT/TERM would otherwise kill the gate with no
# row. Convert to the conventional codes (130/143); the EXIT trap above
# then logs the single `early-exit` row and the script keeps that code.
# Installed alongside the EXIT trap so pre-dispatch deaths are covered too.
trap 'exit 130' INT
trap 'exit 143' TERM

BASE_REF="origin/main"
HEAD_REF="HEAD"
DRY_RUN=false

usage() {
  # Design-source + Usage + Behaviour paragraphs. Range was 2,22 when
  # introduced (#1262, then the end of the Behaviour paragraph); Scope notes
  # were appended later without updating it. Fixed back to the paragraph
  # boundary and extended for the two header lines added by #1679.
  sed -n '2,27p' "$0"
}

BASE_REF="origin/main"
HEAD_REF="HEAD"
DRY_RUN=false
ALLOW_UNREGISTERED_TARGET=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base-ref) BASE_REF="${2:?missing value for --base-ref}"; shift 2 ;;
    --head-ref) HEAD_REF="${2:?missing value for --head-ref}"; shift 2 ;;
    --dry-run) DRY_RUN=true; shift ;;
    --allow-unregistered-target) ALLOW_UNREGISTERED_TARGET=true; shift ;;
    -h | --help) usage; exit 0 ;;
    *) echo "error: unknown argument '$1' (see --help)" >&2; exit 2 ;;
  esac
done

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || echo "$SCRIPT_DIR/..")"
cd "$ROOT" || exit 1

# --- build-cache isolation guard (#1679) ---------------------------------------
# The whole target policy (unset, seeds, quarantine, main bootstrap, live
# worktree registry, unregistered opt-out) lives in
# scripts/check_target_isolation.sh so its decision matrix is hermetically
# testable. Runs before any lane; no dry-run carve-out (a lane decision
# reported from an env that cannot build is itself misleading); fail-closed.
TARGET_GUARD_ARGS=(--root "$ROOT")
if [[ "${ALLOW_UNREGISTERED_TARGET:-false}" == "true" ]]; then
  TARGET_GUARD_ARGS+=(--allow-unregistered-target)
fi
if ! bash "$SCRIPT_DIR/check_target_isolation.sh" "${TARGET_GUARD_ARGS[@]}"; then
  exit 2
fi

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

# --- classify ------------------------------------------------------------------
# Range files (committed) UNION worktree files (staged + unstaged + untracked)
# so uncommitted work is never invisible to the lane decision.

range_files() {
  local base="$1" head="$2" resolved=""
  # The REQUESTED base is the only candidate (B45/S1). It is the PR base, and
  # nothing else is:
  #   - `main` is not a substitute: a local main can be stale (behind the real
  #     base, so the diff is wider than the PR — harmless) or ahead of it (so
  #     the diff MISSES the PR's own commits, which is the dangerous case);
  #   - HEAD~1 is not a base at all (#2058): it is the branch tip, so the diff
  #     would be the last commit instead of the branch — a branch that also
  #     touched Rust read as documentation-only and skipped cargo.
  # A base that does not resolve must WIDEN the run, never narrow it.
  if git rev-parse --verify --quiet "$base" >/dev/null 2>&1; then
    resolved="$base"
  fi
  if [[ -z "$resolved" ]]; then
    echo "fast-gate: cannot resolve base '$base' — widening to FULL" >&2
    return 1
  fi
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
# Temp cleanup is owned by fast_gate_early_exit_trap (the single EXIT trap).
BASE_UNRESOLVED=false
RANGE_FILES=""
if ! RANGE_FILES="$(range_files "$BASE_REF" "$HEAD_REF")"; then
  BASE_UNRESOLVED=true
fi
{
  printf '%s\n' "$RANGE_FILES"
  worktree_files
} | grep -v '^$' | sort -u > "$UNION_TMP" || true

echo "fast-gate: base=$BASE_REF head=$HEAD_REF worktree files considered:"
sed 's/^/  changed: /' "$UNION_TMP"

CLASS_TMP="$(mktemp)"
# (No per-file EXIT trap here either — see above.)
#
# The classifier's exit status is CHECKED (#2057, S2). It used to run as a bare
# command, so a crash left $CLASS_TMP empty, every key below defaulted to
# `false`, and the dispatch fell through to the cheap docs+CI lanes: no cargo,
# green gate, nothing verified. A failed classification is a failed step AND a
# widening — never a silent downgrade.
CLASSIFIER_OK=true
if ! bash "$CLASSIFIER" --base-ref "$BASE_REF" --head-ref "$HEAD_REF" \
  --files "$(cat "$UNION_TMP")" --github-output "$CLASS_TMP"; then
  echo "::error::path classifier exited non-zero — refusing the default lanes" >&2
  CLASSIFIER_OK=false
fi

# Load only the 14 known keys (never blind-source tool output).
# Pre-initialised so `set -u`-style readers and shellcheck (SC2154) see
# real assignments; printf -v below only overwrites them.
docs_only=false ci_only=false code_changed=false ai_changed=false
mcp_changed=false cli_changed=false core_changed=false crawler_changed=false
downloader_changed=false tests_changed=false release_changed=false
lock_changed=false lib_src_changed=false all=false
CLASS_KEYS=(docs_only ci_only code_changed ai_changed mcp_changed cli_changed
  core_changed crawler_changed downloader_changed tests_changed
  release_changed lock_changed lib_src_changed all)
for key in "${CLASS_KEYS[@]}"; do
  val="$(grep -E "^${key}=" "$CLASS_TMP" | tail -1 | cut -d= -f2)"
  printf -v "$key" '%s' "${val:-false}"
done

# An absent key is as untrustworthy as a failed run: `false` is a verdict the
# classifier never gave. Assert all 14 before trusting any of them.
MISSING_KEYS=()
for key in "${CLASS_KEYS[@]}"; do
  grep -qE "^${key}=" "$CLASS_TMP" || MISSING_KEYS+=("$key")
done
if [[ ${#MISSING_KEYS[@]} -gt 0 ]]; then
  echo "::error::path classifier output is missing ${#MISSING_KEYS[@]} key(s): ${MISSING_KEYS[*]}" >&2
  CLASSIFIER_OK=false
fi

if [[ "$CLASSIFIER_OK" != "true" ]]; then
  FAIL=$((FAIL + 1))
  FAILED_STEPS+=("path classification")
fi

# A base that could not be resolved means the changed-file list is not the
# PR's: widen instead of guessing (#2058, S4).
if [[ "$BASE_UNRESOLVED" == "true" ]]; then
  FAIL=$((FAIL + 1))
  FAILED_STEPS+=("base resolution")
fi

if [[ "$CLASSIFIER_OK" != "true" || "$BASE_UNRESOLVED" == "true" ]]; then
  all=true
fi

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
      ! grep -Eq '\\.rs$|Cargo\\.toml|Cargo\\.lock|^crates/|^tests/|^benches/|^examples/|^fuzz/|\\.github/|scripts/' '$UNION_TMP'
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
  # Docs version pins (#2017): a `vX.Y.Z` in an install recipe is a claim about
  # which release exists, and it goes stale the moment the workspace version
  # moves — the reader pastes it and gets an old binary. Blocking, like the
  # orphan-snapshot guard above: real drift, not style. This lane is where a
  # markdown edit lands; the Cargo.toml side of the same drift is covered by
  # the same guard in lane_fmt_and_guards.
  run_guard "docs version pins gate (#2017)" scripts/check_docs_version_pins.sh \
    bash scripts/check_docs_version_pins.sh
  # AI providers doc drift (#2136): docs/architecture/ai-providers-design.md is
  # a normative spec cited by 13 code files and it lives OUTSIDE the mdBook —
  # no gate read it, and that is how a signature that did not compile survived
  # months. Blocking since phase 2: the CI step is required, so this mirror has
  # to block too — a guard that blocks in CI and not here is a surprise the
  # developer only discovers in the CI run. This lane is the DOC side of the
  # drift; the CODE side is covered by the same guard in lane_fmt_and_guards.
  run_guard "ai providers doc drift gate (#2136)" scripts/check_ai_providers_doc.sh \
    bash scripts/check_ai_providers_doc.sh
}

# collect_shell_files: print, one per line, the changed files that are shell.
#
# Two shapes count (B51):
#   - every `scripts/**.sh` (the historical filter);
#   - every OTHER file under `scripts/` that starts with a shell shebang.
#     `scripts/githooks/pre-push` and `scripts/githooks/pre-commit` are bash
#     WITHOUT the `.sh` suffix, so the `^scripts/.*\.sh$` filter never saw
#     them: they were classified `ci_only` and then skipped as "no changed
#     scripts", so `bash -n` and shellcheck never looked at them and the
#     local gate linted LESS than CI's repo-guards job lints.
#
# A path that is gone (a deleted script) is not a script to lint (#A15), and a
# file that is not shell (a python helper, a markdown file) is not either.
collect_shell_files() {
  local f first interp
  local -a shebang
  while IFS= read -r f; do
    [[ -n "$f" ]] || continue
    [[ -f "$f" ]] || continue
    case "$f" in
      scripts/*.sh) printf '%s\n' "$f"; continue ;;
      scripts/*) ;;
      *) continue ;;
    esac
    first="$(head -n 1 "$f" 2>/dev/null || true)"
    case "$first" in
      '#!'*) ;;
      *) continue ;;
    esac
    # `#!/usr/bin/env bash` and `#!/bin/bash` are both bash; with `env` the
    # interpreter is the word AFTER it.
    shebang=()
    read -r -a shebang <<<"${first#\#!}"
    interp="${shebang[0]:-}"
    if [[ "$interp" == "env" || "$interp" == */env ]]; then
      interp="${shebang[1]:-}"
    fi
    interp="$(basename "$interp" 2>/dev/null || printf '%s' "$interp")"
    case "$interp" in
      bash | sh | dash | ksh | zsh) printf '%s\n' "$f" ;;
    esac
  done <"$UNION_TMP"
}

# --- lane: ci-only (NO cargo) ----------------------------------------------------
# CI_HARNESS_MODE decides how the semantics harnesses below are triggered:
#   "triggered" (the default) runs a harness when the files it pins changed;
#   "always" runs EVERY harness, unconditionally.
# The FULL lane sets it to "always" (B52): its scope is unknown, so "the files
# that pin this harness changed" is a question an unknown scope cannot answer
# — and a harness that only ever runs in the cheap CI lane is skipped exactly
# when the scope is unknown, while the FULL lane's banner claims "nothing
# skipped". The trigger itself stays a plain `grep -Eq` over the changed-file
# union, so the wiring harnesses that read this lane keep seeing it.
CI_HARNESS_MODE="triggered"

lane_ci() {
  if [[ "$CI_HARNESS_MODE" == "always" ]]; then
    echo "fast-gate lane: FULL (the CI-ONLY lane's semantics harnesses, run unconditionally)"
  else
    echo "fast-gate lane: CI-ONLY (shell + workflow validation, no cargo)"
  fi
  # Same ignore vocabulary as the repo-guards job in .github/workflows/ci.yml
  # (#1070): structural findings stay fatal, style debt stays ignored.
  local ignore="SC2010,SC2012,SC2027,SC2034,SC2046,SC2086,SC2126,SC2129"
  local -a script_files=()
  local -a workflow_files=()
  # mapfile returns non-zero on empty input, hence `|| true` under `set -u`.
  # scripts/ is matched at any depth: scripts/githooks/ and scripts/tests/ hold
  # real shell that CI's repo-guards job lints, so a top-level-only glob left
  # nested scripts unlinted locally (#1635). The shebang files without a .sh
  # suffix count too (B51) — see collect_shell_files above.
  # The union lists DELETED paths too (git diff --name-only reports them), and a
  # deleted file is not something `bash -n` or shellcheck can read. Measured
  # 2026-10-20: retiring one script made BOTH steps fail with "No such file or
  # directory" — a legitimate deletion reported as a defect, which blocks the
  # local gate and with it the pre-push. A path that is gone is not a script to
  # lint; only the ones still on disk are.
  mapfile -t script_files < <(collect_shell_files) || true
  mapfile -t workflow_files < <(grep -E '^\.github/workflows/[^/]*\.ya?ml$' "$UNION_TMP" | while read -r f; do [ -f "$f" ] && printf '%s\n' "$f"; done) || true
  if [[ "$CI_HARNESS_MODE" == "always" ]]; then
    # The FULL lane already ran this lint through lane_fmt_and_guards
    # (run_pinned_lint_changed_scope): running it twice would report every
    # step twice for no extra coverage.
    skip_step "shell syntax/shellcheck (CI lane)" "already covered by the FULL lane's pinned lint"
  elif [[ ${#script_files[@]} -gt 0 ]]; then
    run_step "bash syntax (changed scripts)" bash -n "${script_files[@]}"
    if command -v shellcheck >/dev/null 2>&1; then
      run_step "shellcheck (changed scripts, shared ignore list)" shellcheck -x --exclude="$ignore" --format=gcc "${script_files[@]}"
    else
      skip_step "shellcheck" "not installed"
    fi
  else
    skip_step "shell syntax/shellcheck" "no changed scripts"
  fi
  if [[ "$CI_HARNESS_MODE" == "always" ]]; then
    skip_step "workflow validation (CI lane)" "already covered by the FULL lane's pinned lint"
  elif [[ ${#workflow_files[@]} -gt 0 ]]; then
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
  # CI Health close semantics harness (slice4-branch-guard): offline fake-gh
  # proof that the shared close_mine owns all close behavior. Runs only when a
  # file in the close path changed; cheap enough (<1s) to never skip otherwise.
  if grep -Eq '^(scripts/ci-health-close\.sh|scripts/test_ci_health_close\.sh|\.github/workflows/ci-health\.yml)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "ci-health close harness (offline)" bash scripts/test_ci_health_close.sh
  else
    skip_step "ci-health close harness" "no ci-health close files changed"
  fi
  # Compatibility-harness semantics (#1635): hermetic, cargo-free proof that
  # the probe failure paths stay fail-closed — the binary's own output reaches
  # the log before a FAIL, the crawl availability branch never degrades into a
  # compile-only pass, and the --output-vectors contract is derived per combo
  # instead of matched against a list of combos. <1s, so it never skips.
  if grep -Eq '^(scripts/check_compatibility\.sh|scripts/tests/test_check_compatibility\.sh)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "compatibility harness semantics (offline)" bash scripts/tests/test_check_compatibility.sh
  else
    skip_step "compatibility harness semantics" "no compatibility harness files changed"
  fi
  # Target-isolation guard semantics (#1679): hermetic mktemp git-tree proof
  # that the build-cache target policy rejects unset/uncanonicalisable
  # targets, the seed and quarantine stores, targets owned by ANY other live
  # worktree (collision-first; the opt-out cannot bypass a collision), and
  # unregistered targets without the explicit per-invocation opt-out.
  # Triggered by either the guard or its harness changing — both are
  # `scripts/**`, so a PR touching only one of them still lands in this
  # CI-ONLY lane. Hermetic, no cargo, ~1s — cheap enough that it is never
  # skipped once triggered.
  if grep -Eq '^(scripts/check_target_isolation\.sh|scripts/tests/test_check_target_isolation\.sh)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "target isolation guard semantics (offline)" bash scripts/tests/test_check_target_isolation.sh
  else
    skip_step "target isolation guard semantics" "no target-guard files changed"
  fi
  # Path-classifier regression harness (#1707; coverage for #1643): the
  # `$(...)` NUL-dropping capture collapsed a multi-file diff into one
  # pseudo-path, which could flip `run_code_jobs` false and cascade-skip every
  # code lane while CI Gate stayed green. Triggered by either the classifier or
  # its harness changing — both are `scripts/**`, so a PR touching only one of
  # them still lands in this CI-ONLY lane. Hermetic (mktemp git trees), no
  # cargo, <1s — cheap enough that it is never skipped once triggered.
  # Mirrors the `repo-guards` step of the same
  # harness in .github/workflows/ci.yml.
  if grep -Eq '^(scripts/ci_path_classifier\.sh|scripts/ci_path_classifier_test\.sh|scripts/ci_plan\.sh|scripts/ci_crossplatform_scope\.sh|scripts/test_ci_plan\.sh)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "path classifier harness semantics (offline)" bash scripts/ci_path_classifier_test.sh
    run_step "CI plan semantics harness (offline)" bash scripts/test_ci_plan.sh
  else
    skip_step "path classifier harness semantics" "no path classifier files changed"
  fi
  # Docs version pins semantics (#2017): hermetic, cargo-free proof that the
  # guard still recognises a stale pin, in both directions of the RC window —
  # a prerelease pin against a stable workspace, and a stable pin against a
  # prerelease workspace (that second direction used to abort the guard before
  # it compared anything, leaving the tree unguarded for a whole RC window).
  # Triggered by either the guard or its harness changing — both are
  # `scripts/**`, so a PR touching only one of them still lands in this
  # CI-ONLY lane. That is also why the harness lives here and NOT in lane_docs
  # beside the gate it pins: lane_docs never runs for a `scripts/**` change, so
  # it would never exercise the harness for the one PR that can break it.
  # Mirrors the `repo-guards` steps of both harnesses in
  # .github/workflows/ci.yml. Offline mktemp fixtures, no cargo, <1s — cheap
  # enough that they are never skipped once triggered.
  #
  # The wiring harness (#2017) rides the same trigger for the same reason: it
  # pins the EDGES of this gate rather than its decisions, and those edges are
  # exactly what a `scripts/**`-only change can break. It asserts that
  # `repo-guards` stays unfiltered and unconditional, that `gate` waits on it in
  # both scopes, and that both steps below are still wired into this block.
  if grep -Eq '^(scripts/check_docs_version_pins\.sh|scripts/test_docs_version_pins\.sh|scripts/test_docs_version_pins_wiring\.sh)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "docs version pins semantics (offline)" bash scripts/test_docs_version_pins.sh
    run_step "docs version pins wiring (offline)" bash scripts/test_docs_version_pins_wiring.sh
  else
    skip_step "docs version pins semantics" "no docs version pins files changed"
  fi
  # AI providers doc drift (#2136): the same blocking mirror the other two
  # lanes run, here for the lane that fires when `scripts/**` changed. A
  # `scripts/**` change can move the code the doc cites, so this lane sees the
  # CODE side of the drift. Blocking since phase 2: the CI step is required,
  # and a local lane that stayed green would hide the drift until CI.
  run_guard "ai providers doc drift gate (#2136)" scripts/check_ai_providers_doc.sh \
    bash scripts/check_ai_providers_doc.sh
  # Release-pins harnesses (#2028): the release flow owns the bump of the docs
  # version pins, and these three pin the pieces that make that ownership real
  # — the WRITER (the exact inverse of the gate above), the ORCHESTRATOR (the
  # step .github/workflows/release-plz.yml invokes) and the WIRING of both into
  # that workflow. The trigger names the new scripts AND the two workflows
  # they read, so a PR touching only one side still lands in this CI-ONLY
  # lane; `lane_docs` never runs for a `scripts/**` change, which is the only
  # kind of change that can break them. Offline: mktemp fixtures, a local bare
  # git repo and a fake `gh` on PATH — no cargo, no network.
  if grep -Eq '^(scripts/(bump_docs_version_pins|test_bump_docs_version_pins|apply_release_pins|test_apply_release_pins|test_release_pins_wiring|declare_pending_publication|test_declare_pending_publication)\.sh|\.github/workflows/(release-plz|ci)\.yml)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "docs pins writer semantics (offline)" bash scripts/test_bump_docs_version_pins.sh
    run_step "release pins orchestrator semantics (offline)" bash scripts/test_apply_release_pins.sh
    run_step "release pins wiring (offline)" bash scripts/test_release_pins_wiring.sh
    run_step "pending publication declaration semantics (offline)" bash scripts/test_declare_pending_publication.sh
  else
    skip_step "release pins harnesses" "no release pins files changed"
  fi
  # Wiring harness for the MCP version-marker gate (#2038): same class as the
  # version-pins harness above. A `scripts/**`-only change can strip the step,
  # add a `paths:` filter to the job, drop `repo-guards` from the narrow scope,
  # or feed the guard a base that makes the comparison vacuous — and every one of
  # those shapes is valid YAML that runs green. `ci.yml` is in the trigger
  # because it is the file the harness reads.
  if grep -Eq '^(scripts/check_mcp_public_surface\.sh|scripts/test_mcp_surface_wiring\.sh|\.github/workflows/ci\.yml)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "mcp surface wiring (offline)" bash scripts/test_mcp_surface_wiring.sh
  else
    skip_step "mcp surface wiring" "no mcp surface guard files changed"
  fi
  # Breaking-marker guard harness (#2081): the squash path silently destroyed a
  # breaking declaration twice (ADR-0004 paso 6, #2072 and #2078), and both
  # shipped as patches. `merge-when-green.sh` now refuses that merge; this
  # harness pins the refusal AND its negation (title marker + --squash must not
  # merge). Triggered by the script or its harness, both `scripts/**` — a PR
  # touching only one of them still lands in this lane. Offline fake `gh`,
  # mktemp fixtures, no cargo, <1s.
  if grep -Eq '^(scripts/merge-when-green\.sh|scripts/test_merge_when_green_breaking\.sh)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "breaking-marker guard semantics (offline)" bash scripts/test_merge_when_green_breaking.sh
  else
    skip_step "breaking-marker guard semantics" "no merge-when-green files changed"
  fi
  # The wait bounds of the merge waiter: every branch must be bounded by the
  # effective timeout, and a cancelled required check is terminal. Before the
  # fix, the fallback watch (the one that runs when branch protection cannot
  # be read) had no timeout at all, so `--timeout` was silently ignored and
  # the watch blocked forever. Offline fake `gh`, no network, <1s.
  if grep -Eq '^(scripts/merge-when-green\.sh|scripts/test_ci_merge_when_green\.sh)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "merge-when-green wait bounds harness (offline)" bash scripts/test_ci_merge_when_green.sh
  else
    skip_step "merge-when-green wait bounds harness" "no merge-when-green files changed"
  fi
  # The tag ruleset writer must never turn an API failure into "there is no
  # ruleset": that path POSTed a DUPLICATE ruleset on a 500. Offline fake
  # `gh`, no network, <1s.
  if grep -Eq '^(scripts/apply-tag-ruleset\.sh|scripts/test_apply_tag_ruleset\.sh)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "tag ruleset idempotency harness (offline)" bash scripts/test_apply_tag_ruleset.sh
  else
    skip_step "tag ruleset idempotency harness" "no apply-tag-ruleset files changed"
  fi
  # The metrics snapshot must report every required context and account for
  # every listed run: a `jq` failure used to shrink the sample in silence
  # while the snapshot still printed as complete. Offline fake `gh`, <1s.
  if grep -Eq '^(scripts/ci_metrics\.sh|scripts/test_ci_metrics\.sh)$' "$UNION_TMP" 2>/dev/null || [[ "$CI_HARNESS_MODE" == "always" ]]; then
    run_step "ci metrics snapshot harness (offline)" bash scripts/test_ci_metrics.sh
  else
    skip_step "ci metrics snapshot harness" "no ci_metrics files changed"
  fi
}

# run_pinned_lint_changed_scope: degraded local equivalent of CI's
# "Lint GitHub Actions workflows and shell scripts" step. CI downloads
# pinned actionlint+shellcheck and scans the whole repo; that download is
# forbidden pre-push, so this lints ONLY changed files (from $UNION_TMP)
# with already-installed tools and degrades loudly otherwise: workflows
# fall back to a python3-yaml parse, scripts warn-skip. NEVER curl-installs
# pinned binaries locally.
run_pinned_lint_changed_scope() {
  # Same ignore vocabulary as the repo-guards job in .github/workflows/ci.yml
  # and lane_ci above: structural findings stay fatal, style debt stays ignored.
  local ignore="SC2010,SC2012,SC2027,SC2034,SC2046,SC2086,SC2126,SC2129"
  local -a script_files=()
  local -a workflow_files=()
  # mapfile returns non-zero on empty input, hence `|| true` under `set -u`.
  # Any depth under scripts/, for the same reason as lane_ci (#1635), and the
  # shebang files without a .sh suffix too (B51) — see collect_shell_files.
  # The union lists DELETED paths too (git diff --name-only reports them), and a
  # deleted file is not something `bash -n` or shellcheck can read. Measured
  # 2026-10-20: retiring one script made BOTH steps fail with "No such file or
  # directory" — a legitimate deletion reported as a defect, which blocks the
  # local gate and with it the pre-push. A path that is gone is not a script to
  # lint; only the ones still on disk are.
  mapfile -t script_files < <(collect_shell_files) || true
  mapfile -t workflow_files < <(grep -E '^\.github/workflows/[^/]*\.ya?ml$' "$UNION_TMP" | while read -r f; do [ -f "$f" ] && printf '%s\n' "$f"; done) || true
  if [[ ${#script_files[@]} -eq 0 && ${#workflow_files[@]} -eq 0 ]]; then
    skip_step "pinned lint (changed scope)" "no changed scripts/workflows"
    return 0
  fi
  if [[ ${#script_files[@]} -gt 0 ]]; then
    run_step "bash syntax (changed scripts, pinned lint scope)" bash -n "${script_files[@]}"
    if command -v shellcheck >/dev/null 2>&1; then
      run_step "shellcheck (changed scripts, shared ignore list)" shellcheck -x --exclude="$ignore" --format=gcc "${script_files[@]}"
    else
      skip_step "shellcheck (changed scripts)" "not installed (no download pre-push)"
    fi
  else
    skip_step "shell syntax/shellcheck (changed scope)" "no changed scripts"
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
      skip_step "workflow validation (changed scope)" "neither actionlint nor python3-yaml available"
    fi
  else
    skip_step "workflow validation (changed scope)" "no changed workflows"
  fi
}

# run_zizmor_cached_only: degraded local equivalent of CI's zizmor audit.
# Runs ONLY when a zizmor binary is already cached on PATH, with the repo's
# .github/zizmor.yml config unchanged and `--min-severity high` (fail-closed
# on findings). Otherwise warn-skips with reason. NEVER downloads pre-push.
run_zizmor_cached_only() {
  if ! command -v zizmor >/dev/null 2>&1; then
    skip_step "zizmor audit (cached only)" "zizmor not installed (no download pre-push)"
    return 0
  fi
  if [[ ! -f .github/zizmor.yml ]]; then
    skip_step "zizmor audit (cached only)" ".github/zizmor.yml absent"
    return 0
  fi
  run_step "zizmor audit (cached, --min-severity high)" zizmor --config .github/zizmor.yml --min-severity high .github/workflows
}

# --- shared: fmt + repo guards ----------------------------------------------------
# Mirrors the `repo-guards` job + `fmt` tier of .github/workflows/ci.yml.
# Cheap greps/scripts only; every guard present in scripts/ runs, missing
# ones warn-skip (never silently dropped: the skip is logged).
# (The rustdoc step is not a guard: it runs in targeted_cargo/lane_full,
# gated on the classifier lib_src_changed signal — see run_rustdoc_gate.)

# run_guard <name> <script> <cmd...>: run_step when the guard script
# exists and is readable, else a logged skip (never a silent drop).
# Readability, NOT executability (#1302): every wrapped command invokes
# the script through `bash`, exactly like CI does, so a missing exec bit
# (e.g. check_dependency_direction.sh is -rw-r--r-- in main) must not
# skip the guard locally while CI still runs it. Avoids the
# `A && B || C` idiom (shellcheck SC2015).
# ensure_intra_crate_lint: make the AST extractor available for the
# intra-crate direction gate.
#
# The gate CONSUMES the extractor (tools/intra-crate-lint) and fails CLOSED
# when the binary is absent: by design there is no fallback scanner, because
# a gate that degrades to the weak scanner when the strong one is missing is
# the same fail-open this campaign closed. CI's repo-guards job builds the
# extractor before it runs the gate; the local gate does the same, and points
# the gate at the binary through the variable the gate documents.
#
# The resolution mirrors the gate's own, so the two never disagree about
# where the binary is. The build keeps CARGO_TARGET_DIR as it is (the
# repository's isolation guard requires it) and therefore lands in the
# tree's own target dir, not in the tool's; the gate is pointed at it
# explicitly instead.
INTRA_CRATE_LINT_BIN_PATH=""
ensure_intra_crate_lint() {
  if [[ -n "${INTRA_CRATE_LINT_BIN:-}" ]]; then
    if [[ -x "$INTRA_CRATE_LINT_BIN" ]]; then
      INTRA_CRATE_LINT_BIN_PATH="$INTRA_CRATE_LINT_BIN"
      return 0
    fi
    echo "::error::INTRA_CRATE_LINT_BIN is set to '$INTRA_CRATE_LINT_BIN' but that path is not an executable file" >&2
    return 1
  fi
  local candidate
  for candidate in "$ROOT/tools/intra-crate-lint/target/release/intra-crate-lint" \
                   "$ROOT/tools/intra-crate-lint/target/debug/intra-crate-lint"; do
    if [[ -x "$candidate" ]]; then
      INTRA_CRATE_LINT_BIN_PATH="$candidate"
      return 0
    fi
  done
  candidate="$(command -v intra-crate-lint || true)"
  if [[ -n "$candidate" && -x "$candidate" ]]; then
    INTRA_CRATE_LINT_BIN_PATH="$candidate"
    return 0
  fi
  echo "building the intra-crate lint extractor (the gate needs it and has no fallback scanner)"
  cargo build --manifest-path tools/intra-crate-lint/Cargo.toml --locked || return 1
  local target="${CARGO_TARGET_DIR:-$ROOT/tools/intra-crate-lint/target}"
  for candidate in "$target/release/intra-crate-lint" "$target/debug/intra-crate-lint"; do
    if [[ -x "$candidate" ]]; then
      INTRA_CRATE_LINT_BIN_PATH="$candidate"
      return 0
    fi
  done
  echo "::error::the intra-crate lint extractor was built but its binary is not where it was expected" >&2
  return 1
}

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
  # The anyhow guard is a PINNED grep, and `if grep ...; then fail; fi; echo OK`
  # conflates grep's exit 1 (nothing found) with its exit 2 (the scan could
  # not run): an unreadable source tree printed the clean message and the
  # step passed. The exit status is inspected explicitly — >= 2 means the scan
  # did not run, which is a FAILURE, never "nothing found" (B53/S9).
  # The payload is single-quoted ON PURPOSE: inside double quotes the
  # enclosing shell would expand `$?` before the payload ever runs, and the
  # status under test would be the wrong one.
  run_step "forbid anyhow in webfang_core (#428)" bash -c '
    if grep -rn "use anyhow" crates/webfang_core/src/ --include="*.rs"; then
      echo "webfang_core must use typed errors, not anyhow"; exit 1
    elif [ $? -ge 2 ]; then echo "cannot scan crates/webfang_core/src/ - the scan did not run, failing closed"; exit 1; fi
    echo "OK: no anyhow imports in webfang_core/src"; exit 0
  '
  run_guard "dependency direction gate (#513)" scripts/check_dependency_direction.sh \
    bash scripts/check_dependency_direction.sh
  run_step "build the intra-crate lint extractor" ensure_intra_crate_lint
  run_guard "intra-crate direction gate, strict (ADR-0010)" scripts/check_intra_crate_direction.sh \
    env INTRA_CRATE_MODE=strict INTRA_CRATE_LINT_BIN="$INTRA_CRATE_LINT_BIN_PATH" bash scripts/check_intra_crate_direction.sh
  run_guard "ignored-test inventory guard" scripts/check_ignored_guard.sh \
    bash scripts/check_ignored_guard.sh
  run_guard "ignored-test guard semantics harness (#1328)" scripts/test_ignored_guard.sh \
    bash scripts/test_ignored_guard.sh
  run_guard "concurrency lint gate (#1616 CC-L1/CC-L2)" scripts/check_concurrency_lints.sh \
    bash scripts/check_concurrency_lints.sh
  run_guard "concurrency lint semantics harness (#1616)" scripts/test_concurrency_lints.sh \
    bash scripts/test_concurrency_lints.sh
  run_guard "nextest invocation lint gate (#1784)" scripts/check_nextest_invocation_lint.sh \
    bash scripts/check_nextest_invocation_lint.sh
  run_guard "nextest invocation lint semantics harness (#1784)" scripts/test_nextest_invocation_lint.sh \
    bash scripts/test_nextest_invocation_lint.sh
  # The nightly Benches scope decides what gets measured, and `cargo bench -p`
  # is an allow-list: a crate left off it is silently never measured while the
  # nightly still reports green. Offline, mktemp fixtures only.
  run_guard "bench scope derivation semantics harness (#1913)" scripts/test_derive_bench_scope.sh \
    bash scripts/test_derive_bench_scope.sh
  # Phase 3: same guard as the docs lane — blocking here because every
  # surrounding repo-guard step is blocking (fail-closed on findings,
  # warn-skip only when the script itself is absent).
  run_guard "orphan snapshot guard" scripts/check_orphan_snapshots.sh \
    bash scripts/check_orphan_snapshots.sh
  # Docs version pins (#2017): the same guard the docs lane runs, and it has to
  # run here too — the drift starts on the Cargo.toml side, so a code PR that
  # bumps [workspace.package] version breaks a recipe with no markdown touched
  # at all, and the docs lane never runs for that PR. Mirrors the `repo-guards`
  # step of ci.yml, like every other guard in this function.
  run_guard "docs version pins gate (#2017)" scripts/check_docs_version_pins.sh \
    bash scripts/check_docs_version_pins.sh
  # AI providers doc drift (#2136): the same guard the docs lane runs, and it
  # has to run here too — the drift has TWO trigger surfaces (the doc and the
  # code), so a lane that only sees one of them would leave half the drift
  # invisible: a code PR that moves a cited signature changes no markdown and
  # never reaches lane_docs. Blocking since phase 2, like the CI step it
  # mirrors: required in CI, required here.
  run_guard "ai providers doc drift gate (#2136)" scripts/check_ai_providers_doc.sh \
    bash scripts/check_ai_providers_doc.sh
  # Issue #1614: the version-marker half of the MCP public-surface policy. The
  # other half (live schema vs the committed fixture) is a test and runs in the
  # cargo lanes; this one needs the base commit, which a test cannot see.
  run_guard "mcp public-surface version marker (#1614)" scripts/check_mcp_public_surface.sh \
    bash scripts/check_mcp_public_surface.sh
  # Same shape as the anyhow guard above, for the same reason (B53/S9): a
  # scan that could not run is a failure, not a clean result. The payload is
  # single-quoted so `$?` reaches the payload unexpanded.
  run_step "forbid sitemap string-match coupling" bash -c '
    if grep -rn "contains(\"no URLs found\")" crates/*/src/ --include="*.rs"; then
      echo "use ScraperError::SitemapEmpty, never message matching"; exit 1
    elif [ $? -ge 2 ]; then echo "cannot scan crates/*/src/ - the scan did not run, failing closed"; exit 1; fi
    echo "OK: no string-match sitemap coupling"; exit 0
  '
  # Release hand-off gate (as-is via run_guard, same pass/fail as CI).
  run_guard "release hand-off gate" scripts/check_release_dispatch.sh \
    bash scripts/check_release_dispatch.sh
  # Release reconcile semantics harness (hermetic mktemp fixtures, no network).
  run_guard "release reconcile semantics harness" scripts/test_release_reconcile.sh \
    bash scripts/test_release_reconcile.sh
  # Release provenance L1 acceptance matrix (hermetic mktemp fixtures, no network).
  run_guard "release provenance L1 acceptance matrix" scripts/test_release_provenance.sh \
    bash scripts/test_release_provenance.sh
  # Release-owner migration (#2281): stale-owner guard + historical tag policy.
  run_guard "release owner regression guard" scripts/test_release_owner.sh \
    bash scripts/test_release_owner.sh
  run_guard "release signature identity semantics" scripts/test_verify_release_signatures.sh \
    bash scripts/test_verify_release_signatures.sh
  # CI planner semantics (#2282): event -> comparison-SHA contract and the
  # crossplatform table. Hermetic mktemp git trees; no cargo, no network.
  run_guard "CI plan semantics harness (#2282)" scripts/test_ci_plan.sh \
    bash scripts/test_ci_plan.sh
  # Degraded lint + audit guards (changed-scope / cached-only, never download).
  run_pinned_lint_changed_scope
  run_zizmor_cached_only
}

# run_rustdoc_gate <mode>: mirrors the CI `doc-quality` rustdoc step
# EXACTLY (`cargo doc --workspace --all-features --no-deps` with
# `RUSTDOCFLAGS=-D warnings`; cf. AGENTS.md lesson from #516/#1006: a
# local gate narrower than CI is a hole). mode=auto runs only when the
# classifier saw library source (lib_src_changed=true); mode=always runs
# unconditionally (FULL lane — unknown scope must never skip a
# CI-required check). Fail-open on a MISSING cargo (warn + skip), never
# red-block; fail-closed on real rustdoc findings. `env` scopes
# RUSTDOCFLAGS to this one command (never `export`). A skip is counted
# and printed via skip_step, never as PASS (see the summary below).
run_rustdoc_gate() {
  local mode="${1:-auto}"
  if ! command -v cargo >/dev/null 2>&1; then
    skip_step "rustdoc" "cargo not installed (warn-only)"
  elif [[ "$mode" == "always" || "$lib_src_changed" == "true" ]]; then
    run_step "rustdoc (links + missing_docs, RUSTDOCFLAGS=-D warnings)" env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps
  else
    skip_step "rustdoc" "no crates/*/src/** change"
  fi
}

# --- shared: crate-targeted check/clippy/rustdoc/build/tests ----------------------
# Packages derive from the classifier flags. crawler/downloader live inside
# webfang_core, so those flags fold into -p webfang_core. No package matched
# but code changed (e.g. only root Cargo.toml/clippy.toml) -> workspace scope
# (fail to full at the check level, still no AI/coverage/release/mutation).
# The rustdoc step runs last via run_rustdoc_gate (auto mode: only on
# lib_src_changed), so compile failures surface before doc findings.

# log_nextest_env: echo ambient nextest env (#1786). Width/profile/retries
# can be influenced by the environment now that no CLI flag pins them.
# Informative only — no exit, no unset, no PASS/FAIL touch. Prints in
# dry-run too, like the lane echoes. Empty-or-unset renders as unset.
log_nextest_env() {
  local -a vars=(NEXTEST_PROFILE NEXTEST_TEST_THREADS NEXTEST_RETRIES)
  local v vals=""
  for v in "${vars[@]}"; do
    if [[ -n "${!v:-}" ]]; then
      vals+="${v}=${!v} "
    fi
  done
  echo "fast-gate: nextest env: ${vals:-none (profile=default, width from .config/nextest.toml)}"
}

targeted_cargo() {
  log_nextest_env
  local -a pkgs=()
  [[ "$core_changed" == "true" || "$crawler_changed" == "true" || "$downloader_changed" == "true" ]] && pkgs+=(webfang_core)
  [[ "$cli_changed" == "true" ]] && pkgs+=(webfang_cli)
  [[ "$mcp_changed" == "true" ]] && pkgs+=(webfang_mcp)
  # The `ai` feature/crate is gone (ADR-0004 paso 6): the semantic-cleaning
  # surface lives in webfang_core now, so an AI-area change inside core/mcp/cli
  # is already covered by those packages and needs no package of its own. An
  # AI-area change outside them falls through to the workspace-scope branch
  # below, which is the honest scope for it.
  if [[ "$code_changed" == "true" && ${#pkgs[@]} -eq 0 ]]; then
    echo "fast-gate: code changed outside known crates -> workspace scope"
    run_step "cargo check (workspace, all targets+features)" cargo check --workspace --all-targets --all-features
    run_step "clippy strict (workspace)" cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines
    run_step "nextest unit (workspace lib)" cargo nextest run --workspace --lib --retries 2
    run_rustdoc_gate auto
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
  # nextest runs NEVER include #[ignore] tests, and the only ones left are
  # network/model-budget rows, so no local inference happens here;
  # coverage/release/mutation are main-tier concerns.
  # --all-features is load-bearing, not strictness: feature-gated code (the
  # `chromium` subtree in webfang_core/webfang_mcp, the `mcp` and
  # `persistence` gates) is compiled out without it, so a targeted -p run
  # discovers 0 tests in those subtrees and fails vacuous-green — and, worse,
  # a clippy run without it sees the gated imports as dead code.
  # shellcheck disable=SC2068
  run_step "nextest unit (-p ${pkgs[*]} --lib, all features)" cargo nextest run $(printf -- '-p %s ' ${pkgs[@]}) --all-features --lib --retries 2
  if [[ "$tests_changed" == "true" || "$crawler_changed" == "true" || "$downloader_changed" == "true" || "$cli_changed" == "true" || "$mcp_changed" == "true" ]]; then
    # shellcheck disable=SC2068
    run_step "nextest integration (-p ${pkgs[*]} --tests, all features)" cargo nextest run $(printf -- '-p %s ' ${pkgs[@]}) --all-features --tests --retries 2
  else
    skip_step "nextest integration" "no runtime-area flags (crawler/downloader/cli/mcp/tests)"
  fi
  run_rustdoc_gate auto
  # Release provenance L1 behavioral tests (hermetic, no cargo needed)
  run_step "release provenance L1 tests" bash scripts/test_release_provenance.sh
}

# --- lane: full local gate (unknown scope — fail to full, never skip) --------------
# Mirrors the AGENTS.md pre-commit gate plus the workspace test surface
# (test-core + test-full minus AI inference). Still no coverage, release,
# cross-platform, sanitizers, fuzz, or mutation: those are main/nightly tiers.
# The rustdoc step always runs here (unknown scope must never skip a
# CI-required check), via run_rustdoc_gate always.

lane_full() {
  echo "fast-gate lane: FULL (unknown scope — nothing skipped)"
  log_nextest_env
  lane_fmt_and_guards
  # The CI-ONLY lane's semantics harnesses belong here too (B52): this lane
  # claims "nothing skipped", and a harness that only ever runs in the cheap
  # CI lane is skipped exactly when the scope is unknown — the case this lane
  # exists for. The mode is "always": the scope IS unknown, so every harness
  # runs, unconditionally.
  CI_HARNESS_MODE="always"
  lane_ci
  run_step "cargo check (workspace, all targets+features)" cargo check --workspace --all-targets --all-features
  run_step "clippy strict (workspace)" cargo clippy --workspace --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines
  run_step "pre-build webfang binary" cargo build -p webfang_cli --bin webfang --all-features
  run_step "nextest (workspace, all features)" cargo nextest run --workspace --all-features --retries 2
  run_rustdoc_gate always
  # Release provenance L1 behavioral tests (hermetic, no cargo needed)
  run_step "release provenance L1 tests" bash scripts/test_release_provenance.sh
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
