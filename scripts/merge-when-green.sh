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
#   2. Verifies mergeStateStatus is CLEAN or UNSTABLE (not BEHIND, BLOCKED, or
#      CONFLICT). UNSTABLE with required checks green is this repo's normal
#      green state: several jobs are skipped by design on PRs (Deploy Docs,
#      Miri shards), and each SKIPPED pushes the state to UNSTABLE (#823).
#   3. Squash-merges with `gh pr merge --squash`, then deletes the remote head
#      branch for same-owner PRs. Local branch/worktree cleanup is left to the
#      post-merge runbook (see 'Post-merge runbook' below).
#
# Exit codes:
#   0  PR merged.
#   1  Invalid arguments or missing gh.
#   2  Required check FAILED or was CANCELLED.
#   3  All checks green but merge state is not mergeable (e.g. BEHIND — rebase first).
#   4  gh command failure (network, auth, etc.).
#
# Notes:
#   - Respects branch protection: uses `gh pr merge --squash` (NOT --admin, NOT
#     the synchronous PUT bypass). Required checks must be green before merge.
#   - Never passes `--delete-branch`: in the worktree flow the head branch is
#     checked out in a sibling worktree, and Git refuses to delete a branch that
#     is any worktree's HEAD (linked-worktree invariant). `gh` would return
#     rc=1 after a successful merge trying to delete the local branch. Remote
#     cleanup here + local cleanup in the runbook keep exit codes truthful.
#   - Does NOT auto-rebase if BEHIND. The maintainer should rebase and re-push,
#     then re-run the script. Single maintainer, ~30s, no automation needed.
#   - Requires `gh` authenticated with repo scope.
#
# Post-merge runbook — run from the main checkout (branch `main`) after this
# script succeeds:
#   1. Verify the merge landed:
#      gh pr view <N> --json state,mergeCommit          # state: MERGED
#   2. Sync main (ff-only):
#      git fetch origin && git merge --ff-only origin/main
#   3. Remove the worktree, then the local branch (order matters):
#      git worktree remove ~/Projects/webfang-worktrees/<dir>
#      git branch -D <branch>
#      git worktree prune
#   4. Verify the handoff:
#      git worktree list          # only ~/Projects/webfang
#      git status --short         # empty
#   Verification checklist:
#     - gh pr view <N>                 -> state: MERGED
#     - git ls-remote origin <branch>  -> empty (script already deleted the remote)
#     - git worktree list              -> only ~/Projects/webfang
#     - git status --short             -> empty
#     - Linked issue closes itself     -> Closes #N in the PR body
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
# NOTE: capture the exit status with `cmd || rc=$?`, NOT `if ! cmd; then rc=$?`.
# Inside the `then` block of a negated command, `$?` is the status of the
# negated pipeline (always 0 when the body runs), so the real gh status would
# be lost (#938).
rc=0
gh pr checks "$pr_number" --watch --required --fail-fast >/dev/null || rc=$?
if [[ $rc -ne 0 ]]; then
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
  CLEAN|UNSTABLE)
    : ;;  # good — UNSTABLE with required green is mergeable (skipped jobs, #823)
  BEHIND)
    echo "error: PR is BEHIND. Rebase onto main and re-push, then re-run this script." >&2
    exit 3
    ;;
  BLOCKED|CONFLICT|DIRTY|HAS_HOOKS)
    echo "error: mergeStateStatus is ${state} (not CLEAN). Resolve and re-run." >&2
    exit 3
    ;;
  *)
    echo "error: unexpected mergeStateStatus='${state}'" >&2
    exit 3
    ;;
esac

# --- 3. Merge (or report dry-run) -------------------------------------------
# Resolve head branch + owner up front. The remote head branch is deleted only
# for same-owner PRs; fork PRs keep their branch on the fork's remote. The local
# branch is NEVER deleted here (see the worktree note in the header).
head_branch="$(gh pr view "$pr_number" --json headRefName --jq .headRefName)"
head_owner="$(gh pr view "$pr_number" --json headRepositoryOwner --jq .headRepositoryOwner.login)"
base_owner="$(gh repo view --json owner --jq .owner.login)"

delete_remote=0
if [[ -n "$head_branch" && "$head_owner" == "$base_owner" ]]; then
  delete_remote=1
fi

if [[ $dry_run -eq 1 ]]; then
  echo "==> [DRY RUN] would run: gh pr merge ${pr_number} --squash"
  if [[ $delete_remote -eq 1 ]]; then
    echo "==> [DRY RUN] would delete remote branch: ${head_branch}"
  fi
  echo "==> [DRY RUN] local branch/worktree cleanup remains in post-merge runbook."
  exit 0
fi

echo "==> Merging PR #${pr_number} (squash)..."
rc=0
gh pr merge "$pr_number" --squash || rc=$?
if [[ $rc -ne 0 ]]; then
  echo "error: gh pr merge failed (rc=${rc})" >&2
  exit 4
fi
echo "    merged."

if [[ $delete_remote -eq 1 ]]; then
  echo "==> Deleting remote branch '${head_branch}' (local cleanup: runbook)."
  # Check BEFORE pushing: after the merge the ref may already be gone (e.g.
  # deleted by another process), and `git push --delete` on an absent ref
  # prints a noisy "[remote rejected]" error even though nothing is wrong.
  if ! refs="$(git ls-remote origin "refs/heads/${head_branch}")"; then
    echo "warning: merge succeeded, but remote branch cleanup could not be verified." >&2
    exit 0
  fi

  if [[ -z "$refs" ]]; then
    echo "    remote branch already absent."
  elif git push origin --delete "refs/heads/${head_branch}"; then
    echo "    remote branch deleted."
  else
    # Residual check-then-delete race: re-verify quietly before warning.
    if refs="$(git ls-remote origin "refs/heads/${head_branch}")" && [[ -z "$refs" ]]; then
      echo "    remote branch already absent."
    else
      echo "warning: merge succeeded but remote branch '${head_branch}' could not be deleted." >&2
      exit 0
    fi
  fi
fi

exit 0
