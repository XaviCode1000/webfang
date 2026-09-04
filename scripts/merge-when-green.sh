#!/usr/bin/env bash
# merge-when-green.sh — wait for required CI checks to pass, then merge.
#
# Usage:
#   scripts/merge-when-green.sh <PR-NUMBER>
#   scripts/merge-when-green.sh <PR-NUMBER> --dry-run
#   scripts/merge-when-green.sh <PR-NUMBER> --merge     # merge commit (batch PRs)
#   scripts/merge-when-green.sh <PR-NUMBER> --squash    # explicit default
#
# Behavior:
#   1. Reads the required status-check contexts from branch protection, then polls
#      `gh pr checks --json name,bucket` until EVERY one of them has REPORTED with
#      a terminal bucket, and all report `pass`. Waiting on
#      `gh pr checks --watch --required` alone is not enough: it evaluates against
#      the checks reported so far, so right after a push it can declare GREEN from a
#      subset while a slow aggregator context has not been queued yet (#1011). If
#      the required-context list cannot be read, it falls back to that older watch
#      behaviour and says so on stderr.
#   2. Verifies mergeStateStatus is CLEAN or UNSTABLE (not BEHIND, BLOCKED, or
#      CONFLICT). UNSTABLE with required checks green is this repo's normal
#      green state: several jobs are skipped by design on PRs (Deploy Docs,
#      Miri shards), and each SKIPPED pushes the state to UNSTABLE (#823).
#   3. Merges with `gh pr merge --squash` (default) or `gh pr merge --merge` when
#      `--merge` is passed, then deletes the remote head
#      branch for same-owner PRs. Local branch/worktree cleanup is left to the
#      post-merge runbook (see 'Post-merge runbook' below).
#
# Exit codes:
#   0  PR merged.
#   1  Invalid arguments (including --squash together with --merge) or missing gh.
#   2  Required check FAILED, was CANCELLED, or SKIPPED.
#   3  All checks green but merge state is not mergeable (e.g. BEHIND — rebase first).
#   4  gh command failure (network, auth, etc.) or a required context that never
#      reported before the deadline.
#
# Notes:
#   - Respects branch protection: uses `gh pr merge --squash` (or `--merge`), NOT
#     --admin, NOT the synchronous PUT bypass. Required checks must be green before
#     merge.
#   - `--merge` exists for batch PRs (see AGENTS.md "Batch merge of multiple green
#     PRs"): squashing N independent fixes into one commit destroys per-fix revert
#     granularity, so a batch needs a merge commit. It is the same merge with a
#     different strategy flag — every guard above still applies. `--squash` stays
#     the default so no existing invocation changes behaviour.
#   - Never passes `--delete-branch`: in the worktree flow the head branch is
#     checked out in a sibling worktree, and Git refuses to delete a branch that
#     is any worktree's HEAD (linked-worktree invariant). `gh` would return
#     rc=1 after a successful merge trying to delete the local branch. Remote
#     cleanup here + local cleanup in the runbook keep exit codes truthful.
#   - Does NOT auto-rebase if BEHIND. The maintainer should rebase and re-push,
#     then re-run the script. Single maintainer, ~30s, no automation needed.
#   - Requires `gh` authenticated with repo scope.
#   - After an actual merge, prints exactly ONE machine-parseable marker line on
#     stdout: `RESULT: merged remote_branch=<deleted|absent|stale>` — deleted:
#     remote head branch deleted; absent: head ref already gone (or fork PR — no
#     head ref on origin to delete); stale: head ref could not be deleted, or its
#     absence could not be verified. Every post-merge path still exits 0 (#819);
#     automation must parse this line, not prose, to learn the cleanup outcome.
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
# Merge strategy passed to `gh pr merge`: "squash" (default) or "merge".
# `strategy_explicit` records that the user named one, so passing both flags can
# be rejected instead of silently letting the last one win.
merge_strategy="squash"
strategy_explicit=""

usage() {
  cat <<'EOF'
Usage: scripts/merge-when-green.sh <PR-NUMBER> [--dry-run] [--squash | --merge]

Waits for all required CI checks on the PR to pass, then merges.

Flags:
  --dry-run   Poll and report status, but do not merge.
  --squash    Squash-merge into one commit. This is the default.
  --merge     Merge commit instead of squash. Use for batch PRs, where squashing
              N independent fixes would destroy per-fix revert granularity.
              Mutually exclusive with --squash.
  -h, --help  Show this help.

Exit codes:
  0  Merged (or dry-run reports green).
  1  Bad args (including --squash together with --merge).
  2  A required check failed/cancelled.
  3  Checks green but PR is BEHIND or BLOCKED — rebase first.
  4  gh network/auth failure.

Result marker:
  After an actual merge (not --dry-run) the script prints one final stdout line:
    RESULT: merged remote_branch=<deleted|absent|stale>
  deleted = remote head branch deleted; absent = head ref already gone (or fork
  PR: no head ref on origin); stale = head ref could not be deleted or its
  absence could not be verified. Cleanup never changes the exit code (#819).
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --dry-run) dry_run=1; shift ;;
    --squash|--merge)
      requested="${1#--}"
      if [[ -n "$strategy_explicit" && "$strategy_explicit" != "$requested" ]]; then
        echo "error: --$strategy_explicit and --$requested are mutually exclusive" >&2
        usage >&2
        exit 1
      fi
      strategy_explicit="$requested"
      merge_strategy="$requested"
      shift
    ;;
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
# Do NOT trust `gh pr checks --watch --required` as the "all done" signal.
# `--required` evaluates against the checks GitHub has REPORTED SO FAR, not
# against the required-context list from branch protection. Immediately after a
# push only the fast jobs have reported; a slow aggregator like `CI Gate` is not
# yet QUEUED, so it is absent rather than pending and "all required checks are
# green" is trivially satisfied by a subset. That reported GREEN while
# mergeStateStatus was still BLOCKED (#1011).
#
# Instead: read the required contexts from branch protection, then poll until
# every one of them has REPORTED with a terminal bucket.
echo "==> Waiting for required checks on PR #${pr_number}..."

repo="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"

# Newer protection configs use checks[].context; older ones use contexts[].
required="$(gh api "repos/${repo}/branches/main/protection" \
  --jq '.required_status_checks.checks[]?.context // empty' 2>/dev/null || true)"
if [[ -z "$required" ]]; then
  required="$(gh api "repos/${repo}/branches/main/protection" \
    --jq '.required_status_checks.contexts[]? // empty' 2>/dev/null || true)"
fi

if [[ -z "$required" ]]; then
  echo "warning: could not read required contexts from branch protection." >&2
  echo "         Falling back to 'gh pr checks --watch --required', which is the" >&2
  echo "         behaviour that reported a false GREEN in #1011." >&2
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
else
  required_count=$(printf '%s\n' "$required" | sed '/^[[:space:]]*$/d' | wc -l)
  echo "    required contexts from branch protection: $(printf '%s\n' "$required" | tr '\n' ',' | sed 's/,$//')"

  # ~10 min at 15s. CI is ~8.5 min on this repo, so this bounds the loop without
  # pre-empting a normal run.
  deadline=$((SECONDS + 600))
  while :; do
    checks_json="$(gh pr checks "$pr_number" --json name,bucket 2>/dev/null || echo '[]')"
    buckets="$(printf '%s' "$checks_json" \
      | jq -r '.[] | "\(.bucket)\t\(.name)"' 2>/dev/null || true)"

    missing=0
    notgreen=""
    while IFS= read -r name; do
      [[ -z "$name" ]] && continue
      bucket="$(printf '%s\n' "$buckets" | awk -F'\t' -v n="$name" '$2 == n { print $1; exit }')"
      case "$bucket" in
        pass) ;;
        fail) notgreen+=" ${name}=failed" ;;
        cancelled) notgreen+=" ${name}=cancelled" ;;
        # A REQUIRED check that skipped is not a pass. Fail closed rather than
        # merge on a check that never ran.
        skipping) notgreen+=" ${name}=skipped" ;;
        *) missing=1 ;;
      esac
    done <<< "$required"

    if [[ -n "$notgreen" ]]; then
      echo "error: required checks not green:${notgreen}" >&2
      exit 2
    fi

    if [[ $missing -eq 0 ]]; then
      echo "    all ${required_count} required checks reported and GREEN."
      break
    fi

    if [[ $SECONDS -ge $deadline ]]; then
      echo "error: timed out waiting for every required context to REPORT (not merely to pass)." >&2
      echo "       A required context that never appears in the rollup is the #1011 failure mode." >&2
      exit 4
    fi
    sleep 15
  done
fi

# --- 2. Verify merge state is CLEAN ------------------------------------------
# UNKNOWN is not a verdict, it is GitHub still computing mergeability — which is
# the same class of bug as #1011 (acting on an incomplete answer). Reproduced on
# PR #1017 right after its required checks all went green. Retry within a bounded
# window before treating it as a failure.
state="$(gh pr view "$pr_number" --json mergeStateStatus --jq .mergeStateStatus)"
state_deadline=$((SECONDS + 90))
while [[ "$state" == "UNKNOWN" ]]; do
  if [[ $SECONDS -ge $state_deadline ]]; then
    echo "error: mergeStateStatus still UNKNOWN after 90s — GitHub has not finished" >&2
    echo "       computing mergeability. Re-run this script." >&2
    exit 4
  fi
  echo "    mergeStateStatus is UNKNOWN (still computing); retrying..."
  sleep 10
  state="$(gh pr view "$pr_number" --json mergeStateStatus --jq .mergeStateStatus)"
done

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
  echo "==> [DRY RUN] strategy: ${merge_strategy}"
  echo "==> [DRY RUN] would run: gh pr merge ${pr_number} --${merge_strategy}"
  if [[ $delete_remote -eq 1 ]]; then
    echo "==> [DRY RUN] would delete remote branch: ${head_branch}"
  fi
  echo "==> [DRY RUN] local branch/worktree cleanup remains in post-merge runbook."
  exit 0
fi

echo "==> Merging PR #${pr_number} (${merge_strategy})..."
rc=0
gh pr merge "$pr_number" "--${merge_strategy}" || rc=$?
if [[ $rc -ne 0 ]]; then
  echo "error: gh pr merge failed (rc=${rc})" >&2
  exit 4
fi
echo "    merged."

# Classify the remote head branch outcome for the RESULT marker: deleted
# (we deleted it), absent (already gone before/after the delete attempt, or
# fork PR — no head ref on origin), stale (still there, or unverifiable).
remote_branch_state="absent"
if [[ $delete_remote -eq 1 ]]; then
  echo "==> Deleting remote branch '${head_branch}' (local cleanup: runbook)."
  # Check BEFORE pushing: after the merge the ref may already be gone (e.g.
  # deleted by another process), and `git push --delete` on an absent ref
  # prints a noisy "[remote rejected]" error even though nothing is wrong.
  if ! refs="$(git ls-remote origin "refs/heads/${head_branch}")"; then
    echo "warning: merge succeeded, but remote branch cleanup could not be verified." >&2
    remote_branch_state="stale"
  elif [[ -z "$refs" ]]; then
    echo "    remote branch already absent."
    remote_branch_state="absent"
  elif git push origin --delete "refs/heads/${head_branch}"; then
    echo "    remote branch deleted."
    remote_branch_state="deleted"
  else
    # Residual check-then-delete race: re-verify quietly before warning.
    if refs="$(git ls-remote origin "refs/heads/${head_branch}")" && [[ -z "$refs" ]]; then
      echo "    remote branch already absent."
      remote_branch_state="absent"
    else
      echo "warning: merge succeeded but remote branch '${head_branch}' could not be deleted." >&2
      remote_branch_state="stale"
    fi
  fi
fi

# Single machine-parseable handoff line; every post-merge path exits 0 (#819).
echo "RESULT: merged remote_branch=${remote_branch_state}"
exit 0
