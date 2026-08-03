#!/usr/bin/env bash
# merge-when-green.sh — wait for required CI checks to pass, then squash-merge.
#
# Usage:
#   scripts/merge-when-green.sh <PR-NUMBER>
#   scripts/merge-when-green.sh <PR-NUMBER> --dry-run
#
# Behavior:
#   1. Polls `gh pr checks <N> --watch --required --fail-fast` until all required
#      checks are SUCCESS or one FAILS/CANCELS.
#   2. Verifies mergeStateStatus is CLEAN (not BEHIND, BLOCKED, or CONFLICT).
#   3. Squash-merges with `gh pr merge --squash --delete-branch`.
#
# Exit codes:
#   0  PR merged.
#   1  Invalid arguments or missing gh.
#   2  Required check FAILED or was CANCELLED.
#   3  All checks green but merge state is not CLEAN (e.g. BEHIND — rebase first).
#   4  gh command failure (network, auth, etc.).
#
# Notes:
#   - Respects branch protection: uses `gh pr merge --squash` (NOT --admin, NOT
#     the synchronous PUT bypass). Required checks must be green before merge.
#   - Does NOT auto-rebase if BEHIND. The maintainer should rebase and re-push,
#     then re-run the script. Single maintainer, ~30s, no automation needed.
#   - Requires `gh` authenticated with repo scope.
#
# See: docs of Fase 0 measurement (AGENTS.md, project memory) — CI is ~8.5 min,
# this script saves the human loop of checking back every few minutes.
set -euo pipefail

pr_number=""
dry_run=0

usage() {
  cat <<'EOF'
Usage: scripts/merge-when-green.sh <PR-NUMBER> [--dry-run]

Waits for all required CI checks on the PR to pass, then squash-merges.

Flags:
  --dry-run   Poll and report status, but do not merge.
  -h, --help  Show this help.

Exit codes:
  0  Merged (or dry-run reports green).
  1  Bad args.
  2  A required check failed/cancelled.
  3  Checks green but PR is BEHIND or BLOCKED — rebase first.
  4  gh network/auth failure.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --dry-run) dry_run=1; shift ;;
    *)
      if [[ -z "$pr_number" ]]; then
        pr_number="$1"
      else
        echo "error: unexpected argument '$1'" >&2
        usage >&2
        exit 1
      fi
      shift
      ;;
  esac
done

if [[ -z "$pr_number" ]]; then
  echo "error: PR number is required" >&2
  usage >&2
  exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "error: gh CLI is not installed or not on PATH" >&2
  exit 1
fi

# --- 1. Wait for required checks to finish -----------------------------------
# `gh pr checks --watch --required --fail-fast` exits:
#   0  All required checks SUCCESS.
#   8  Still pending (timeout unlikely — watch blocks).
#   1 Some check FAILED or gh error.
echo "==> Waiting for required checks on PR #${pr_number}..."
if ! gh pr checks "$pr_number" --watch --required --fail-fast >/dev/null; then
  rc=$?
  if [[ $rc -eq 8 ]]; then
    echo "error: checks still pending after watch exit" >&2
    exit 4
  fi
  echo "error: one or more required checks FAILED or were CANCELLED (rc=${rc})" >&2
  exit 2
fi
echo "    all required checks are GREEN."

# --- 2. Verify merge state is CLEAN ------------------------------------------
state=$(gh pr view "$pr_number" --json mergeStateStatus --jq .mergeStateStatus)
case "$state" in
  CLEAN)
    : ;;  # good
  BEHIND)
    echo "error: PR is BEHIND. Rebase onto main and re-push, then re-run this script." >&2
    exit 3
    ;;
  BLOCKED|CONFLICT|DIRTY|HAS_HOOKS|UNSTABLE)
    echo "error: mergeStateStatus is ${state} (not CLEAN). Resolve and re-run." >&2
    exit 3
    ;;
  *)
    echo "error: unexpected mergeStateStatus='${state}'" >&2
    exit 3
    ;;
esac

# --- 3. Merge (or report dry-run) -------------------------------------------
if [[ $dry_run -eq 1 ]]; then
  echo "==> [DRY RUN] would run: gh pr merge ${pr_number} --squash --delete-branch"
  exit 0
fi

echo "==> Merging PR #${pr_number} (squash) and deleting branch..."
if gh pr merge "$pr_number" --squash --delete-branch; then
  echo "    merged ✓"
  exit 0
else
  rc=$?
  echo "error: gh pr merge failed (rc=${rc})" >&2
  exit 4
fi
