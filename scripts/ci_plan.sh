#!/usr/bin/env bash
set -euo pipefail
# ci_plan.sh — resolve the change plan for every CI event.
#
# The event does not decide the depth; the diff does. This script resolves the
# comparison SHAs for pull_request, merge_group and push, asks
# scripts/ci_path_classifier.sh for the filename verdict, and asks
# scripts/ci_crossplatform_scope.sh whether Windows/macOS is needed. schedule,
# workflow_dispatch, an unknown event, a missing/zero SHA and any detector
# failure fall back to FULL scope (fail-closed).
#
# Outputs (written to $GITHUB_OUTPUT when set, stdout otherwise):
#   run_code_jobs, needs_ai, needs_mcp, needs_mutation_hotpath,
#   snapshot_changed, needs_crossplatform, required_producers,
#   scope_reason, base_sha, head_sha,
#   docs_only, ci_only, code_changed, all, affected
#
# `required_producers` lists the downstream producers only; the planner itself
# is always required by `gate`. It is emitted now so #2286 can compare it
# against `gate.needs` without re-deriving scope.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLASSIFIER="${CI_PLAN_CLASSIFIER:-$SCRIPT_DIR/ci_path_classifier.sh}"
CROSSPLATFORM="${CI_PLAN_CROSSPLATFORM:-$SCRIPT_DIR/ci_crossplatform_scope.sh}"

FULL_PRODUCERS="toolchain,repo-guards,artifact-guards,merge-metrics-guards,fmt,clippy,code-quality,test-core,test-full,test-release-provenance,security,doc-quality"
NARROW_PRODUCERS="toolchain,repo-guards,artifact-guards,merge-metrics-guards,security,doc-quality"

EVENT_NAME="${EVENT_NAME:-${GITHUB_EVENT_NAME:-}}"
BASE_REF=""
HEAD_REF=""

usage() {
  cat <<'EOF'
usage: ci_plan.sh

Reads EVENT_NAME plus the matching event SHA variables from the environment:
  pull_request  PR_BASE_SHA, PR_HEAD_SHA
  merge_group   MERGE_GROUP_BASE_SHA, MERGE_GROUP_HEAD_SHA
  push          PUSH_BEFORE_SHA, PUSH_AFTER_SHA
Writes the plan to $GITHUB_OUTPUT when set, otherwise stdout.
EOF
}

case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
  "") ;;
  *)
    echo "error: unknown argument '$1' (see --help)" >&2
    exit 2
    ;;
esac

emit() {
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf '%s=%s\n' "$1" "$2" >> "$GITHUB_OUTPUT"
  else
    printf '%s=%s\n' "$1" "$2"
  fi
}

valid_sha() {
  [[ "$1" =~ ^[0-9a-f]{40}$ ]] || return 1
  [[ "$1" =~ ^0+$ ]] && return 1
  printf '%s' "$1"
}

repo_root() {
  git rev-parse --show-toplevel 2>/dev/null || pwd
}

# full_scope <reason> [base] [head]
full_scope() {
  local reason="$1"
  local base="${2:-}" head="${3:-}" base_out="" head_out=""

  if [[ -n "$base" ]] && valid_sha "$base" >/dev/null; then base_out="$base"; fi
  if [[ -n "$head" ]] && valid_sha "$head" >/dev/null; then head_out="$head"; fi

  emit docs_only false
  emit ci_only false
  emit code_changed true
  emit all true
  emit affected all
  emit run_code_jobs true
  emit needs_ai true
  emit needs_mcp true
  emit needs_mutation_hotpath true
  emit snapshot_changed true
  emit needs_crossplatform true
  emit required_producers "$FULL_PRODUCERS"
  emit scope_reason "event=${EVENT_NAME:-unknown} mode=full reason=$reason"
  emit base_sha "$base_out"
  emit head_sha "$head_out"
  echo "ci-plan: event=${EVENT_NAME:-unknown} -> full scope ($reason)" >&2
  exit 0
}

case "$EVENT_NAME" in
  pull_request)
    BASE_REF="${PR_BASE_SHA:-}"
    HEAD_REF="${PR_HEAD_SHA:-}"
    [[ -n "$BASE_REF" && -n "$HEAD_REF" ]] ||
      full_scope "pull_request-missing-sha" "$BASE_REF" "$HEAD_REF"
    ;;
  merge_group)
    BASE_REF="${MERGE_GROUP_BASE_SHA:-}"
    HEAD_REF="${MERGE_GROUP_HEAD_SHA:-}"
    [[ -n "$BASE_REF" && -n "$HEAD_REF" ]] ||
      full_scope "merge_group-missing-sha" "$BASE_REF" "$HEAD_REF"
    ;;
  push)
    BASE_REF="${PUSH_BEFORE_SHA:-}"
    HEAD_REF="${PUSH_AFTER_SHA:-}"
    [[ -n "$HEAD_REF" ]] || full_scope "push-missing-after" "$BASE_REF" "$HEAD_REF"
    if [[ -z "$BASE_REF" ]] || ! valid_sha "$BASE_REF" >/dev/null; then
      full_scope "push-before-zero-or-missing" "$BASE_REF" "$HEAD_REF"
    fi
    ;;
  schedule | workflow_dispatch)
    full_scope "non-diff-event"
    ;;
  *)
    full_scope "unknown-event"
    ;;
esac

# Path-aware events must carry a valid head; a bogus value would make the
# classifier silently widen or crash, so fail closed before it runs.
valid_sha "$HEAD_REF" >/dev/null || full_scope "head-not-a-40-hex-sha" "$BASE_REF" "$HEAD_REF"
valid_sha "$BASE_REF" >/dev/null || full_scope "base-not-a-40-hex-sha" "$BASE_REF" "$HEAD_REF"

ROOT="$(repo_root)"
ERR="$(mktemp)"
trap 'rm -f "$ERR"' EXIT

if ! CLASSIFIER_OUT="$(cd "$ROOT" && bash "$CLASSIFIER" \
  --base "$BASE_REF" --head "$HEAD_REF" --format github 2>"$ERR")"; then
  echo "ci-plan: classifier failed; failing closed" >&2
  sed 's/^/  /' "$ERR" >&2 || true
  full_scope "classifier-failed" "$BASE_REF" "$HEAD_REF"
fi

declare -A KV=()
while IFS='=' read -r key value; do
  [[ -n "$key" ]] || continue
  KV["$key"]="$value"
done <<< "$CLASSIFIER_OUT"

RUN_CODE_JOBS="${KV[run_code_jobs]:-}"
if [[ "$RUN_CODE_JOBS" != "true" && "$RUN_CODE_JOBS" != "false" ]]; then
  full_scope "classifier-missing-run_code_jobs" "$BASE_REF" "$HEAD_REF"
fi

NEEDS_CROSSPLATFORM="false"
if [[ "$RUN_CODE_JOBS" == "true" ]]; then
  if ! CROSSPLATFORM_OUT="$(cd "$ROOT" && bash "$CROSSPLATFORM" \
    --base "$BASE_REF" --head "$HEAD_REF" 2>"$ERR")"; then
    echo "ci-plan: crossplatform detector failed; failing closed" >&2
    sed 's/^/  /' "$ERR" >&2 || true
    NEEDS_CROSSPLATFORM="true"
  else
    case "$CROSSPLATFORM_OUT" in
      true | false) NEEDS_CROSSPLATFORM="$CROSSPLATFORM_OUT" ;;
      *)
        echo "ci-plan: crossplatform detector returned '$CROSSPLATFORM_OUT'; failing closed" >&2
        NEEDS_CROSSPLATFORM="true"
        ;;
    esac
  fi
fi

for key in docs_only ci_only code_changed all affected; do
  emit "$key" "${KV[$key]:-false}"
done
emit run_code_jobs "$RUN_CODE_JOBS"
emit needs_ai "${KV[needs_ai]:-true}"
emit needs_mcp "${KV[needs_mcp]:-true}"
emit needs_mutation_hotpath "${KV[needs_mutation_hotpath]:-true}"
emit snapshot_changed "${KV[snapshot_changed]:-true}"
emit needs_crossplatform "$NEEDS_CROSSPLATFORM"
if [[ "$RUN_CODE_JOBS" == "true" ]]; then
  emit required_producers "$FULL_PRODUCERS"
else
  emit required_producers "$NARROW_PRODUCERS"
fi
emit scope_reason "event=$EVENT_NAME mode=path-aware"
emit base_sha "$BASE_REF"
emit head_sha "$HEAD_REF"
echo "ci-plan: event=$EVENT_NAME base=$BASE_REF head=$HEAD_REF -> run_code_jobs=$RUN_CODE_JOBS needs_crossplatform=$NEEDS_CROSSPLATFORM" >&2
