#!/usr/bin/env bash
# merge-when-green.sh — wait for required CI checks to pass, then merge.
#
# Usage:
#   scripts/merge-when-green.sh <PR-NUMBER>
#   scripts/merge-when-green.sh <PR-NUMBER> --dry-run
#   scripts/merge-when-green.sh <PR-NUMBER> --merge     # merge commit (batch PRs)
#   scripts/merge-when-green.sh <PR-NUMBER> --squash    # explicit default
#   scripts/merge-when-green.sh <PR-NUMBER> --queue     # enqueue via the merge queue
#   scripts/merge-when-green.sh <PR-NUMBER> --queue --queue-timeout=7200
#   scripts/merge-when-green.sh <PR-NUMBER> --timeout=1800   # large PRs w/ slow jobs (#1206)
#   MERGE_WHEN_GREEN_TIMEOUT=1800 scripts/merge-when-green.sh <PR-NUMBER>  # same via env
#
# Behavior:
#   0. If the PR is ALREADY MERGED (state == MERGED), reports "already merged"
#      and exits 0: the merge already happened, so there is nothing to wait for
#      and nothing to merge. GitHub answers mergeStateStatus=UNKNOWN for a
#      closed PR FOREVER (the mergeability of a closed PR is never computed),
#      so the UNKNOWN retry loop used to burn its 90s window and then FAIL on a
#      PR whose merge was already done (#1970). `state` is the field that tells
#      the two apart, and it is read in the SAME call as mergeStateStatus.
#   1. Refuses a squash-merge that would destroy a breaking declaration. A
#      breaking marker lives in a commit subject (`feat(ai)!: ...`) or in a
#      `BREAKING CHANGE:` footer, and `gh pr merge --squash` rebuilds the subject
#      from the PR TITLE — dropping the `!` — while release-plz ignores a footer
#      buried in the squashed body. Measured on #2072 and #2078: both shipped as
#      patches with the marker gone. So this runs BEFORE the wait: if the PR
#      title or any branch commit declares a break and the strategy is squash,
#      it exits 5 and names the remedy (--merge). See #2081.
#   2. Reads the required status-check contexts from branch protection, then polls
#      `gh pr checks --json name,bucket` until EVERY one of them has REPORTED with
#      a terminal bucket, and all report `pass`. Waiting on
#      `gh pr checks --watch --required` alone is not enough: it evaluates against
#      the checks reported so far, so right after a push it can declare GREEN from a
#      subset while a slow aggregator context has not been queued yet (#1011). If
#      the required-context list cannot be read, it falls back to that older watch
#      behaviour and says so on stderr. That fallback is bounded by the SAME
#      effective deadline as the polling branch (B46): a deadline that applies to
#      one branch only is not a deadline.
#   3. Verifies mergeStateStatus is CLEAN or UNSTABLE (not BEHIND, BLOCKED, or
#      CONFLICT). UNSTABLE with required checks green is this repo's normal
#      green state: several jobs are skipped by design on PRs (Deploy Docs,
#      Miri shards), and each SKIPPED pushes the state to UNSTABLE (#823).
#   4. Merges with `gh pr merge --squash` (default) or `gh pr merge --merge` when
#      `--merge` is passed, then deletes the remote head
#      branch for same-owner PRs. Local branch/worktree cleanup is left to the
#      post-merge runbook (see 'Post-merge runbook' below). A remote-cleanup
#      failure never changes the exit code; it is reported on stdout as
#      `cleanup: failed branch=<name> reason=<reason>` (#1072).
#      With --queue, `gh pr merge` ENQUEUES the PR instead (no strategy flag:
#      the queue owns the merge method), waits for the queue to merge it, and
#      only then runs the same remote cleanup. BEHIND is not fatal in queue
#      mode: the queue builds the entry against the current base.
#
# Exit codes:
#   0  PR merged, OR the PR was already merged (nothing to do — step 0). A
#      remote-branch cleanup failure after a successful merge also
#      exits 0 — by design: a non-zero exit post-merge teaches callers the
#      merge failed and re-arms the #819 retry trap (double merge), which is
#      worse than a stale branch. Parse the `cleanup: failed` stdout line
#      instead (#1072).
#   1  Invalid arguments (including --squash together with --merge) or missing gh.
#   2  Required check FAILED, was CANCELLED, or SKIPPED.
#   3  All checks green but merge state is not mergeable (e.g. BEHIND outside
#      queue mode; in --queue mode the queue handles BEHIND).
#   4  gh command failure (network, auth, etc.) or a required context that never
#      reported before the deadline.
#   5  Refused: this PR declares a breaking change and the chosen strategy would
#      destroy the marker (squash). Nothing was merged. Re-run with --merge, or
#      move the declaration into a commit. See #2081.
#
# Notes:
#   - Respects branch protection: uses `gh pr merge --squash` (or `--merge`), NOT
#     --admin, NOT the synchronous PUT bypass. Required checks must be green before
#     merge.
#   - An already merged PR is NOT a failure. The desired state is reached, so the
#     script says "already merged" and exits 0 — no wait, no second merge (#1970).
#     Re-running the script on a merged PR is normal (the runbook asks for
#     `gh pr view` afterwards) and must not burn the 90s UNKNOWN window nor
#     report a failure. No new exit code: a caller would have to learn a second
#     table for a state the repo already calls "merged".
#   - `--merge` exists for batch PRs (see AGENTS.md "Batch merge of multiple green
#     PRs"): squashing N independent fixes into one commit destroys per-fix revert
#     granularity, so a batch needs a merge commit. It is the same merge with a
#     different strategy flag — every guard above still applies. `--squash` stays
#     the default so no existing invocation changes behaviour.
#   - The breaking-marker guard (step 1) has NO override flag on purpose. A
#     bypass would re-arm exactly the silent failure it exists to stop: the
#     release would ship as a patch and nobody would notice until the tag was
#     public. The marker is a property of the branch, so the remedy is always
#     available — use `--merge`, or put the `!` in a commit instead of only in
#     the PR title.
#   - Never passes `--delete-branch`: in the worktree flow the head branch is
#     checked out in a sibling worktree, and Git refuses to delete a branch that
#     is any worktree's HEAD (linked-worktree invariant). `gh` would return
#     rc=1 after a successful merge trying to delete the local branch. Remote
#     cleanup here + local cleanup in the runbook keep exit codes truthful.
#   - Does NOT auto-rebase if BEHIND. The maintainer should rebase and re-push,
#     then re-run the script. Single maintainer, ~30s, no automation needed.
#   - Requires `gh` authenticated with repo scope.
#   - After an actual merge, prints one machine-parseable RESULT line last on
#     stdout: `RESULT: merged remote_branch=<deleted|absent|stale>` — deleted:
#     remote head branch deleted; absent: head ref already gone (or fork PR — no
#     head ref on origin to delete); stale: head ref could not be deleted, or its
#     absence could not be verified. Cleanup-failure lines (`cleanup: failed
#     branch=<name> reason=<reason>`, contract defined in #1161), if any,
#     precede the RESULT line, so the machine stream may hold two lines. Every
#     post-merge path still exits 0 (#819); automation must parse these markers,
#     not prose, to learn the cleanup outcome.
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
# Watcher deadline in seconds: flag --timeout wins over env
# MERGE_WHEN_GREEN_TIMEOUT, which wins over the 600s default (#1206).
# Flag value is captured raw here and validated after arg parsing so both
# sources share one numeric check.
timeout_secs=""
# Set to 1 when --timeout is passed, so an explicit empty value fails
# validation instead of silently falling back to the default.
timeout_given=0
# Merge strategy passed to `gh pr merge`: "squash" (default) or "merge".
# `strategy_explicit` records that the user named one, so passing both flags can
# be rejected instead of silently letting the last one win.
merge_strategy="squash"
strategy_explicit=""
# --queue: enqueue through GitHub's merge queue instead of merging directly.
# The queue owns the merge method (read from its configuration) and the
# base-relative build, so BEHIND is not fatal and no strategy flag is passed.
queue_mode=0
queue_timeout_secs=""
queue_timeout_given=0

usage() {
  cat <<'EOF'
Usage: scripts/merge-when-green.sh <PR-NUMBER> [--dry-run] [--queue] [--squash | --merge] [--timeout=<sec>] [--queue-timeout=<sec>]

Waits for all required CI checks on the PR to pass, then merges (or enqueues).

Flags:
  --dry-run   Poll and report status, but do not merge or enqueue.
  --queue     Enqueue through the repository's merge queue instead of merging
              directly. The queue owns the merge method and the base-relative
              build, so BEHIND is not fatal and no --squash/--merge is passed.
              Requires a merge queue configured on the PR's base branch.
  --squash    Squash-merge into one commit. This is the default.
  --merge     Merge commit instead of squash. Use for batch PRs, where squashing
              N independent fixes would destroy per-fix revert granularity.
              Mutually exclusive with --squash.
              REQUIRED when the PR declares a breaking change (a '!' before the
              ':' in the title or a commit subject, or a 'BREAKING CHANGE:'
              footer): squash would drop the marker and the release would ship
              as a patch.
  --timeout=<sec>  Watcher deadline in seconds (default 600). Accepts
              '--timeout 300' or '--timeout=300'. Overrides the
              MERGE_WHEN_GREEN_TIMEOUT env var. Useful for large PRs with
              slow jobs (e.g. cargo-mutants) that need more than 10 min (#1206).
  --queue-timeout=<sec>  How long to wait for the merge queue to merge the PR
              (default 7200). Requires --queue.
  -h, --help  Show this help.

Environment:
  MERGE_WHEN_GREEN_TIMEOUT  Same as --timeout but with lower precedence:
              flag wins over env, env wins over the 600s default.
  MERGE_WHEN_GREEN_QUEUE_TIMEOUT  Same as --queue-timeout but with lower
              precedence: flag wins over env, env wins over the 7200s default.

Exit codes:
  0  Merged, or the PR was already merged (nothing to do). A --dry-run that
     reports green exits 0 too.
  1  Bad args (including --squash together with --merge, or a queue strategy
     that conflicts with the repository's queue configuration).
  2  A required check failed/cancelled, or the queue ejected the PR without
     merging (a merge-group check failed).
  3  Checks green but PR is BEHIND or BLOCKED — rebase first (outside --queue;
     the queue handles BEHIND). A PR closed without merging also exits 3.
  4  gh network/auth failure, or the queue did not merge within the timeout.
  5  Refused before merging: the PR declares a breaking change and the chosen
     strategy would destroy the marker. Nothing was merged. Outside --queue,
     re-run with --merge.

Result marker:
  After an actual merge (not --dry-run) the script prints this machine line last:
    RESULT: merged remote_branch=<deleted|absent|stale>
  deleted = remote head branch deleted; absent = head ref already gone (or fork
  PR: no head ref on origin); stale = head ref could not be deleted or its
  absence could not be verified. Cleanup-failure lines (`cleanup: failed
  branch=<name> reason=<reason>` — full contract in #1161), if any, precede the
  RESULT line. Cleanup never changes the exit code (#819).
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --dry-run) dry_run=1; shift ;;
    --queue) queue_mode=1; shift ;;
    --queue-timeout=*)
      queue_timeout_secs="${1#--queue-timeout=}"
      queue_timeout_given=1
      if [[ -z "$queue_timeout_secs" ]]; then
        echo "error: --queue-timeout requires a value in seconds" >&2
        usage >&2
        exit 1
      fi
      shift
      ;;
    --queue-timeout)
      if [[ $# -lt 2 ]]; then
        echo "error: --queue-timeout requires a value in seconds" >&2
        usage >&2
        exit 1
      fi
      queue_timeout_secs="$2"
      queue_timeout_given=1
      shift 2
      ;;
    --timeout=*)
      timeout_secs="${1#--timeout=}"
      timeout_given=1
      if [[ -z "$timeout_secs" ]]; then
        echo "error: --timeout requires a value in seconds" >&2
        usage >&2
        exit 1
      fi
      shift
      ;;
    --timeout)
      if [[ $# -lt 2 ]]; then
        echo "error: --timeout requires a value in seconds" >&2
        usage >&2
        exit 1
      fi
      timeout_secs="$2"
      timeout_given=1
      shift 2
      ;;
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

if [[ $queue_timeout_given -eq 1 && $queue_mode -eq 0 ]]; then
  echo "error: --queue-timeout requires --queue" >&2
  usage >&2
  exit 1
fi

# Resolve the watcher deadline: --timeout flag wins over
# MERGE_WHEN_GREEN_TIMEOUT, which wins over the 600s default (#1206).
# Validated before the gh check so bad values fail as bad args (rc=1)
# even when gh is missing.
timeout_effective=600
if [[ -n "${MERGE_WHEN_GREEN_TIMEOUT:-}" ]]; then
  timeout_effective="${MERGE_WHEN_GREEN_TIMEOUT}"
fi
if [[ -n "$timeout_secs" || $timeout_given -eq 1 ]]; then
  timeout_effective="$timeout_secs"
fi
if ! [[ "$timeout_effective" =~ ^[0-9]+$ ]] || [[ "$timeout_effective" -le 0 ]]; then
  echo "error: timeout must be a positive integer number of seconds (got '${timeout_effective}')" >&2
  usage >&2
  exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "error: gh CLI is not installed or not on PATH" >&2
  exit 1
fi

# --- Read the PR state in ONE call ------------------------------------------
# `state` (MERGED / OPEN / CLOSED) and `mergeStateStatus` (the mergeability
# verdict) are read TOGETHER, so the two can never come from two different
# points in time and so a single call can tell "this PR is already merged"
# from "GitHub is still computing mergeability" (#1970).
#
# ONE reader, no fallback: a second projection of the same fact is the shape
# this repo's own triage calls out ("one fact, two readers — the second one
# wins"). `gh pr view` serves both fields in one call since the API version
# that introduced `mergeStateStatus` (the field this script already reads),
# so a single-field fallback would only exist to serve a stub — and it would
# silently DISABLE the already-merged detection, which is the defect this
# change fixes.
pr_state=""
merge_status=""
read_pr_merge_state() {
  local raw
  raw="$(gh pr view "$pr_number" --json state,mergeStateStatus)"
  pr_state="$(printf '%s' "$raw" | jq -r '.state // ""')"
  merge_status="$(printf '%s' "$raw" | jq -r '.mergeStateStatus // ""')"
}

# The single message for the already-merged case. Exit 0 by design: the state
# the caller wanted ("the PR is merged") is ALREADY reached, and a new exit
# code would only teach the caller a second table.
exit_already_merged() {
  echo "PR #${pr_number} is already merged — nothing to merge (state=MERGED)."
  exit 0
}

# --- 0. Already merged? (#1970) ---------------------------------------------
# A merged PR is a terminal fact: the merge already happened and this script has
# nothing left to do. GitHub answers mergeStateStatus=UNKNOWN for a closed PR
# FOREVER (the mergeability of a closed PR is never computed), so the UNKNOWN
# retry loop below used to burn its 90s window and then exit 4 on a PR whose
# merge was already done. `state` is the field that tells the two apart.
#
# The check runs BEFORE the wait: a merged PR must not sit in the check wait
# either, and it must not reach the breaking-marker guard, whose refusal
# ("nothing was merged") would be a lie about a PR that IS merged.
if read_pr_merge_state && [[ "$pr_state" == "MERGED" ]]; then
  exit_already_merged
fi

# --- 0b. Queue mode: read the queue's merge method ---------------------------
# The queue owns the merge method. Reading it (instead of trusting a flag)
# makes the breaking-marker guard below apply the right rule: a SQUASH queue
# drops a title-only '!'; a MERGE/REBASE queue drops the PR title too (its
# merge commit subject is not the PR title). The script's flags cannot
# override the queue.
if [[ $queue_mode -eq 1 ]]; then
  repo_full="$(gh repo view --json nameWithOwner --jq '.nameWithOwner')" \
    || { echo "error: could not read the repository name." >&2; exit 4; }
  repo="${repo_full}"
  repo_owner="${repo_full%%/*}"
  repo_name="${repo_full#*/}"
  base_branch="$(gh pr view "$pr_number" --json baseRefName --jq '.baseRefName')" \
    || { echo "error: could not read the PR base branch." >&2; exit 4; }
  # shellcheck disable=SC2016  # $owner/$repo/$branch are GraphQL variables
  queue_query='query($owner:String!,$repo:String!,$branch:String!){repository(owner:$owner,name:$repo){mergeQueue(branch:$branch){configuration{mergeMethod}}}}'
  if ! queue_method="$(gh api graphql \
    -f query="$queue_query" \
    -F owner="$repo_owner" -F repo="$repo_name" -F branch="$base_branch" \
    --jq '.data.repository.mergeQueue.configuration.mergeMethod // empty' 2>/dev/null)"; then
    echo "error: could not read the merge queue configuration for '${base_branch}' (gh/API failure)." >&2
    exit 4
  fi
  if [[ -z "$queue_method" ]]; then
    echo "error: --queue was passed but '${base_branch}' has no merge queue configured." >&2
    exit 1
  fi
  case "$queue_method" in
    SQUASH) queue_strategy="squash" ;;
    MERGE|REBASE) queue_strategy="merge" ;;
    *)
      echo "error: unsupported merge queue method '${queue_method}'." >&2
      exit 1
      ;;
  esac
  if [[ -n "$strategy_explicit" && "$strategy_explicit" != "$queue_strategy" ]]; then
    echo "error: --queue uses the repository's merge method (${queue_method}); do not pass --${strategy_explicit}." >&2
    exit 1
  fi
  merge_strategy="$queue_strategy"

  queue_timeout_effective=7200
  if [[ -n "${MERGE_WHEN_GREEN_QUEUE_TIMEOUT:-}" ]]; then
    queue_timeout_effective="${MERGE_WHEN_GREEN_QUEUE_TIMEOUT}"
  fi
  if [[ -n "$queue_timeout_secs" || $queue_timeout_given -eq 1 ]]; then
    queue_timeout_effective="$queue_timeout_secs"
  fi
  if ! [[ "$queue_timeout_effective" =~ ^[0-9]+$ ]] || [[ "$queue_timeout_effective" -le 0 ]]; then
    echo "error: queue timeout must be a positive integer number of seconds (got '${queue_timeout_effective}')" >&2
    exit 1
  fi
fi

# --- 1. Breaking-marker guard (before the wait, not after) -------------------
# The marker has to SURVIVE the merge, and squash is the strategy that kills it:
#   - `gh pr merge --squash` rebuilds the subject from the PR TITLE and drops the
#     `!`: `fix(cli)!: ...` landed as `fix(cli): ... (#2078)`;
#   - release-plz does not honour a `BREAKING CHANGE:` footer inside the squashed
#     body: the squashed #2078 commit carried it and `release-plz update` still
#     computed 2.13.1.
# Both outcomes are SILENT — the major ships as a patch and nobody sees it until
# the tag is public. So the refusal is here, before the ~9 minute wait and long
# before the tag (#2081).
#
# The marker is the `!` that belongs to the TYPE: `type!:`, `type(scope)!:`, and
# also the malformed `type!(scope):` that #2072 used. Both positions are
# recognised because both express the same intent, and the malformed one is the
# one that already burned this repo. A `!` that is not in that position
# (`fix: don't retry`) is prose, not a marker, and must not block a merge.
#
# NEGATION (scripts/test_merge_when_green_breaking.sh): before this guard, the
# title-with-`!` + --squash case exited 0 and merged. That is the case pinned as
# the mandatory failing test.
breaking_subject_re='^[a-zA-Z]+(\([^)]*\))?!(\([^)]*\))?:'

pr_meta="$(gh pr view "$pr_number" --json title,commits 2>/dev/null || true)"
if [[ -z "$pr_meta" ]]; then
  # Fail closed. An unreadable PR is not evidence that the marker is absent, and
  # acting on an incomplete answer is the #1011 failure shape.
  echo "error: could not read the title/commits of PR #${pr_number} from gh." >&2
  echo "       Refusing to merge blind: the breaking-marker guard cannot run (#2081)." >&2
  exit 4
fi

pr_title="$(printf '%s' "$pr_meta" | jq -r '.title // ""')"

title_declares_breaking=0
if [[ "$pr_title" =~ $breaking_subject_re ]]; then
  title_declares_breaking=1
fi

# Offending commits, one per line: short sha + subject. The body is tested
# separately because a footer is a line inside it, not the whole body.
breaking_commits="$(printf '%s' "$pr_meta" | jq -r '
  .commits[]?
  | select(
      ((.messageHeadline // "") | test("^[a-zA-Z]+(\\([^)]*\\))?!(\\([^)]*\\))?:"))
      or ((.messageBody // "") | test("(?im)^BREAKING[ -]CHANGE:"))
    )
  | "         \(.oid[0:8])  \(.messageHeadline)"
')"

if [[ "$merge_strategy" == "squash" ]]; then
  if [[ $title_declares_breaking -eq 1 || -n "$breaking_commits" ]]; then
    {
      if [[ $queue_mode -eq 1 ]]; then
        echo "error: this PR declares a BREAKING change and the merge queue for '${base_branch}' uses SQUASH, which would destroy the marker."
      else
        echo "error: this PR declares a BREAKING change and --squash would destroy the marker."
      fi
      echo "       Nothing was merged."
      if [[ $title_declares_breaking -eq 1 ]]; then
        echo "       PR title: ${pr_title}"
      fi
      if [[ -n "$breaking_commits" ]]; then
        echo "       commits that declare it:"
        printf '%s\n' "$breaking_commits"
      fi
      echo
      echo "       Why refused instead of warned: --squash rebuilds the subject from the PR"
      echo "       title (dropping the '!') and release-plz ignores a 'BREAKING CHANGE:' footer"
      echo "       inside the squashed body. Measured on #2072 and #2078: the major shipped as"
      echo "       a patch and nobody saw it until the tag was public."
      echo
      if [[ $queue_mode -eq 1 ]]; then
        echo "       The queue's merge method is fixed for every entry. Either change it to MERGE"
        echo "       (repository merge queue settings) or land this break outside the queue."
      else
        echo "       Re-run with a merge commit, which preserves the branch commits:"
        echo "         scripts/merge-when-green.sh ${pr_number} --merge"
      fi
    } >&2
    exit 5
  fi
elif [[ $title_declares_breaking -eq 1 && -z "$breaking_commits" ]]; then
  # The mirror image, and the reason this branch exists: a merge commit keeps the
  # branch commits and DISCARDS the PR title (its subject becomes "Merge pull
  # request #N ..."). A declaration that lives only in the title therefore never
  # reaches main either — same silent patch, different strategy.
  {
    echo "error: the PR title declares a BREAKING change but no commit does."
    echo "       PR title: ${pr_title}"
    echo "       Nothing was merged."
    echo
    echo "       A merge commit keeps the branch commits and discards the PR title, so this"
    echo "       declaration would never reach main: release-plz reads commit subjects and"
    echo "       would compute a patch."
    echo
    echo "       Declare it where the release reads it: amend the branch so a commit subject"
    echo "       carries the '!', or drop it from the title if the change is not breaking."
  } >&2
  exit 5
fi

# --- 2. Wait for required checks to finish -----------------------------------
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

repo="${repo:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"
protection_branch="${base_branch:-main}"

# Newer protection configs use checks[].context; older ones use contexts[].
# gh writes the API error envelope (e.g. the 404 "Branch not protected" body)
# to STDOUT while exiting nonzero, and a `||` INSIDE the command substitution
# would capture that envelope as if it were a context list; every read below
# uses `|| required=""` OUTSIDE it, so a nonzero rc yields no contexts (#2284).
required="$(gh api "repos/${repo}/branches/${protection_branch}/protection" \
  --jq '.required_status_checks.checks[]?.context // empty' 2>/dev/null)" || required=""
if [[ -z "$required" ]]; then
  required="$(gh api "repos/${repo}/branches/${protection_branch}/protection" \
    --jq '.required_status_checks.contexts[]? // empty' 2>/dev/null)" || required=""
fi

# Classic branch protection may be migrated to repository rulesets (the merge
# queue configuration is ruleset-only). The classic endpoint then answers 404
# while the same required contexts are exposed by the active rules, one item
# per context: GET /repos/{owner}/{repo}/rules/branches/{branch} only returns
# rules from active rulesets (#2284). An rc != 0 response (gh also writes the
# error envelope to stdout) yields no contexts: the read must not mistake the
# envelope for ruleset data.
if [[ -z "$required" ]]; then
  required="$(gh api "repos/${repo}/rules/branches/${protection_branch}" \
    --jq '.[] | select(.type == "required_status_checks") | .parameters.required_status_checks[]?.context // empty' 2>/dev/null)" || required=""
fi

if [[ -z "$required" ]]; then
  echo "warning: could not read required contexts from branch protection." >&2
  echo "         Falling back to 'gh pr checks --watch --required', which is the" >&2
  echo "         behaviour that reported a false GREEN in #1011." >&2
  # The effective deadline bounds THIS branch too (B46). The watch used to run
  # unbounded here, so `--timeout` was silently ignored and the script blocked
  # forever whenever branch protection was unreadable. A deadline that applies
  # to one branch only is not a deadline.
  echo "    watcher timeout: ${timeout_effective}s (the fallback watch is bounded too)" >&2
  watch_cmd=()
  if command -v timeout >/dev/null 2>&1; then
    watch_cmd=(timeout -k 5 "$timeout_effective" gh pr checks "$pr_number" --watch --required --fail-fast)
  elif command -v gtimeout >/dev/null 2>&1; then
    # macOS ships the GNU coreutils timeout as `gtimeout`.
    watch_cmd=(gtimeout -k 5 "$timeout_effective" gh pr checks "$pr_number" --watch --required --fail-fast)
  else
    # Fail closed: without a wrapper the watch cannot be bounded, and an
    # unbounded wait is exactly the defect this branch exists to remove.
    echo "error: neither 'timeout' nor 'gtimeout' is on PATH, so the watch cannot be bounded." >&2
    echo "       Refusing to run an unbounded 'gh pr checks --watch' (--timeout would be ignored)." >&2
    exit 4
  fi
  # NOTE: capture the exit status with `cmd || rc=$?`, NOT `if ! cmd; then rc=$?`.
  # Inside the `then` block of a negated command, `$?` is the status of the
  # negated pipeline (always 0 when the body runs), so the real gh status would
  # be lost (#938).
  rc=0
  "${watch_cmd[@]}" >/dev/null || rc=$?
  if [[ $rc -ne 0 ]]; then
    if [[ $rc -eq 124 ]]; then
      # `timeout` reports 124 when it had to kill the watch: the deadline hit
      # with the checks still unsettled. That is the same class as the
      # polling branch's deadline, so it is exit 4 (not a verdict).
      echo "error: the required checks did not settle within ${timeout_effective}s (watch fallback timed out)." >&2
      exit 4
    fi
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
  echo "    watcher timeout: ${timeout_effective}s (flag --timeout wins over MERGE_WHEN_GREEN_TIMEOUT, default 600s)"

  # CI is ~8.5 min on this repo, so the 600s default bounds the loop without
  # pre-empting a normal run. Large PRs with slow jobs (e.g. cargo-mutants)
  # can raise it via --timeout=<sec> or MERGE_WHEN_GREEN_TIMEOUT (#1206).
  deadline=$((SECONDS + timeout_effective))
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
        # The API spells the terminal bucket `cancel`; `cancelled` is accepted
        # too because a cancelled required check must be terminal in EITHER
        # spelling (B47). Before, only `cancelled` matched, so the real
        # `cancel` bucket fell through to the "not reported yet" arm and the
        # wait ran to the deadline.
        cancel|cancelled) notgreen+=" ${name}=cancelled" ;;
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

# --- 3. Verify merge state is CLEAN ------------------------------------------
# UNKNOWN is not a verdict, it is GitHub still computing mergeability — which is
# the same class of bug as #1011 (acting on an incomplete answer). Reproduced on
# PR #1017 right after its required checks all went green. Retry within a bounded
# window before treating it as a failure.
read_pr_merge_state
if [[ "$pr_state" == "MERGED" ]]; then
  exit_already_merged
fi
state="$merge_status"
state_deadline=$((SECONDS + 90))
while [[ "$state" == "UNKNOWN" ]]; do
  if [[ $SECONDS -ge $state_deadline ]]; then
    echo "error: mergeStateStatus still UNKNOWN after 90s — GitHub has not finished" >&2
    echo "       computing mergeability. Re-run this script." >&2
    exit 4
  fi
  echo "    mergeStateStatus is UNKNOWN (still computing); retrying..."
  sleep 10
  # The PR can be merged by someone else while this script waits, so the
  # re-consult reads `state` again: a PR that became MERGED is done, not
  # "still computing" (#1970).
  read_pr_merge_state
  if [[ "$pr_state" == "MERGED" ]]; then
    exit_already_merged
  fi
  state="$merge_status"
done

case "$state" in
  CLEAN|UNSTABLE)
    : ;;  # good — UNSTABLE with required green is mergeable (skipped jobs, #823)
  BEHIND)
    if [[ $queue_mode -eq 1 ]]; then
      echo "    PR is BEHIND; the merge queue will build it against ${base_branch}."
    else
      echo "error: PR is BEHIND. Rebase onto main and re-push, then re-run this script." >&2
      exit 3
    fi
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

# --- 4. Merge (or report dry-run) -------------------------------------------
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
  if [[ $queue_mode -eq 1 ]]; then
    echo "==> [DRY RUN] queue method: ${queue_method}"
    echo "==> [DRY RUN] would run: gh pr merge ${pr_number}   # enqueue"
  else
    echo "==> [DRY RUN] strategy: ${merge_strategy}"
    echo "==> [DRY RUN] would run: gh pr merge ${pr_number} --${merge_strategy}"
  fi
  if [[ $delete_remote -eq 1 ]]; then
    echo "==> [DRY RUN] would delete remote branch: ${head_branch}"
  fi
  echo "==> [DRY RUN] local branch/worktree cleanup remains in post-merge runbook."
  exit 0
fi

if [[ $queue_mode -eq 1 ]]; then
  echo "==> Enqueuing PR #${pr_number} (queue method: ${queue_method})..."
  rc=0
  # No strategy flag: on a queue-enabled branch GitHub enqueues the PR when
  # its required checks are green (the queue owns the strategy). If a check
  # is still pending, gh enables auto-merge and the queue picks it up later.
  gh pr merge "$pr_number" || rc=$?
  if [[ $rc -ne 0 ]]; then
    echo "error: gh pr merge (enqueue) failed (rc=${rc})" >&2
    exit 4
  fi
  echo "    enqueued."

  echo "==> Waiting for the merge queue (timeout: ${queue_timeout_effective}s)..."
  queue_deadline=$((SECONDS + queue_timeout_effective))
  seen_in_queue=0
  left_queue=0
  last_entry=""
  # shellcheck disable=SC2016  # $owner/$repo/$number are GraphQL variables
  queue_status_query='query($owner:String!,$repo:String!,$number:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$number){state isInMergeQueue mergeQueueEntry{state}}}}'
  while :; do
    if ! queue_json="$(gh api graphql \
      -f query="$queue_status_query" \
      -F owner="$repo_owner" -F repo="$repo_name" -F number="$pr_number" 2>/dev/null)"; then
      echo "error: could not read the merge queue state for PR #${pr_number}." >&2
      exit 4
    fi
    q_state="$(printf '%s' "$queue_json" | jq -r '.data.repository.pullRequest.state // ""')"
    q_in="$(printf '%s' "$queue_json" | jq -r '.data.repository.pullRequest.isInMergeQueue // false')"
    q_entry="$(printf '%s' "$queue_json" | jq -r '.data.repository.pullRequest.mergeQueueEntry.state // ""')"

    if [[ "$q_state" == "MERGED" ]]; then
      echo "    merged by the queue."
      break
    fi
    if [[ "$q_state" == "CLOSED" ]]; then
      echo "error: PR #${pr_number} was closed without merging." >&2
      exit 3
    fi
    if [[ "$q_entry" == "UNMERGEABLE" ]]; then
      echo "error: the queue entry for PR #${pr_number} is UNMERGEABLE." >&2
      exit 3
    fi
    if [[ "$q_in" == "true" ]]; then
      seen_in_queue=1
      left_queue=0
      if [[ "$q_entry" != "$last_entry" ]]; then
        echo "    queue entry: ${q_entry:-queued}"
        last_entry="$q_entry"
      fi
    elif [[ $seen_in_queue -eq 1 ]]; then
      # The queue removes an entry both on merge and on ejection. The state
      # above already ruled out MERGED; give it one more poll to settle before
      # declaring an ejection (a merge can flip the state a moment later).
      left_queue=$((left_queue + 1))
      if [[ $left_queue -ge 2 ]]; then
        echo "error: PR #${pr_number} left the merge queue without merging (likely a failed merge-group check)." >&2
        echo "       Inspect: gh pr checks ${pr_number}" >&2
        exit 2
      fi
    fi

    if [[ $SECONDS -ge $queue_deadline ]]; then
      echo "error: timed out after ${queue_timeout_effective}s waiting for the merge queue." >&2
      exit 4
    fi
    sleep 15
  done
else
  echo "==> Merging PR #${pr_number} (${merge_strategy})..."
  rc=0
  gh pr merge "$pr_number" "--${merge_strategy}" || rc=$?
  if [[ $rc -ne 0 ]]; then
    echo "error: gh pr merge failed (rc=${rc})" >&2
    exit 4
  fi
  echo "    merged."
fi

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
    # Machine-readable signal (#1072): cleanup outcome is reported on stdout,
    # never via the exit code. A non-zero exit AFTER a successful merge would
    # teach callers the merge failed and re-arm the #819 retry trap (double
    # merge) — strictly worse than a stale branch. Consumers that need to
    # detect pending cleanup grep for '^cleanup: failed'.
    echo "cleanup: failed branch=${head_branch} reason=ls-remote-failed"
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
      # See #1072 note above: signal on stdout, keep exit 0 (the merge is OK).
      echo "cleanup: failed branch=${head_branch} reason=push-delete-failed"
      echo "warning: merge succeeded but remote branch '${head_branch}' could not be deleted." >&2
      remote_branch_state="stale"
    fi
  fi
fi

# Final machine-parseable handoff line; any `cleanup: failed` lines (#1161)
# precede it. Every post-merge path exits 0 (#819).
echo "RESULT: merged remote_branch=${remote_branch_state}"
exit 0
