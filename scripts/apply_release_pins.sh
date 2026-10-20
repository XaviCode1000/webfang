#!/usr/bin/env bash
#
# Apply the docs version pins to the Release PR branch.
#
# WHY THIS EXISTS
#   The release flow owns the version, so it owns the narrative that claims
#   which version exists. `scripts/check_docs_version_pins.sh` compares the
#   `vX.Y.Z` literals in README.md and docs/src/*.md against the workspace
#   version and fails when they disagree — it can report the drift but it
#   cannot repair it. A Release PR (`chore/release-*`) bumps the workspace
#   version and leaves the narrative behind, so the gate is red by
#   construction and the Release PR is structurally unexecutable without a
#   human commit.
#
#   That human commit is EPHEMERAL: release-plz regenerates the Release PR
#   branch on every push to `main`, so the bump is deleted and the PR comes
#   back red (measured three times in one afternoon: v2.12.0 twice, v3.0.1
#   once). This script is the durable answer: the same flow that regenerates
#   the branch also re-applies the pins, in the same run, idempotently.
#
# WHAT IT DOES
#   1. finds the open Release PR (the one whose head branch carries the
#      release-branch prefix);
#   2. re-reads that PR before touching anything, and gives up quietly if it
#      was merged in the meantime;
#   3. fetches the branch and checks it out in a THROWAWAY worktree (no
#      `git checkout`/`git switch` — see AGENTS.md § Git Worktree Isolation);
#   4. runs the writer (scripts/bump_docs_version_pins.sh) inside that
#      worktree — it resolves its own root, so it edits the branch's tree and
#      not this one;
#   5. commits and pushes ONLY when something actually changed, so a second
#      run is a no-op with no commit, no push and no CI triggered;
#   6. retries ONCE when the push is rejected, because release-plz may have
#      regenerated the branch between the fetch and the push.
#
# THE PUSH USES THE APP TOKEN
#   The token travels in an `http.extraheader` header for that one git
#   invocation: never in the remote URL, never in `.git/config`. A push made
#   with GITHUB_TOKEN would not deliver `pull_request` events, so the head
#   SHA of the Release PR would sit without checks and branch protection would
#   block the merge (the same suppression that cost v2.1.1 its binaries).
#
# INPUTS (all optional — they exist so the flow can be exercised offline)
#   RELEASE_PINS_REPO_ROOT      repository to work in (default: this repo)
#   RELEASE_PINS_BRANCH_PREFIX  Release PR branch prefix (default: chore/release-)
#   RELEASE_PINS_REMOTE         remote to fetch from and push to (default: origin)
#
# EXIT: 0 nothing to do / done · 1 failed (with `::error::` on stderr)
set -euo pipefail

REPO_ROOT="${RELEASE_PINS_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
BRANCH_PREFIX="${RELEASE_PINS_BRANCH_PREFIX:-chore/release-}"
REMOTE="${RELEASE_PINS_REMOTE:-origin}"

note() { printf '::notice::%s\n' "$*"; }
err() { printf '::error::%s\n' "$*" >&2; exit 1; }

WT_DIR=""
TMP_ROOT=""

cleanup() {
  if [[ -n ${WT_DIR:-} && -d ${WT_DIR:-} ]]; then
    if ! git -C "$REPO_ROOT" worktree remove --force "$WT_DIR" >/dev/null 2>&1; then
      printf '::warning::could not remove the temporary worktree %s\n' "$WT_DIR" >&2
    fi
  fi
  if [[ -n ${TMP_ROOT:-} && -d ${TMP_ROOT:-} ]]; then
    if ! rmdir "$TMP_ROOT" 2>/dev/null; then
      printf '::warning::could not remove the temporary directory %s\n' "$TMP_ROOT" >&2
    fi
  fi
}
trap cleanup EXIT

# ─── helpers ─────────────────────────────────────────────────────────────────

# is_local_remote <url>: true when the remote is a path on this machine. A
# local remote needs no credentials, which is what lets the harness run the
# whole flow offline.
is_local_remote() {
  local url="$1"
  case "$url" in
    *://*) return 1 ;;
  esac
  # scp-like syntax (git@github.com:owner/repo) is a network remote too.
  if [[ "$url" =~ ^[A-Za-z0-9._-]+@[^/:]+: ]]; then
    return 1
  fi
  return 0
}

# read_workspace_version <root>: [workspace.package] version, read exactly the
# way the gate and the writer read it.
read_workspace_version() {
  local root="$1"
  sed -n '/^\[workspace\.package\]/,/^\[/p' "$root/Cargo.toml" \
    | sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1
}

# push_branch <worktree>: push HEAD to the release branch. Returns non-zero
# when the remote rejects the push (the retryable case).
push_branch() {
  local wt="$1"
  local url b64
  local -a auth=()

  url="$(git -C "$REPO_ROOT" remote get-url --push "$REMOTE" 2>/dev/null || true)"
  [[ -n $url ]] || err "the remote '$REMOTE' is not configured in $REPO_ROOT"

  if ! is_local_remote "$url"; then
    if [[ -z ${GITHUB_TOKEN:-} ]]; then
      err "the remote '$REMOTE' ($url) is not local and GITHUB_TOKEN is empty: the push cannot be authenticated. The App token is mandatory here — a GITHUB_TOKEN push does not trigger the Release PR's CI."
    fi
    b64="$(printf 'x-access-token:%s' "$GITHUB_TOKEN" | base64 | tr -d '\n')"
    # Per-invocation only: the token never reaches the remote URL nor
    # .git/config.
    auth=(-c "http.extraheader=AUTHORIZATION: basic $b64")
  fi

  GIT_TERMINAL_PROMPT=0 git -C "$wt" "${auth[@]}" push "$REMOTE" "HEAD:refs/heads/$BRANCH"
}

# ─── 1. find the Release PR ─────────────────────────────────────────────────
command -v gh >/dev/null 2>&1 \
  || err "gh is not available: the Release PR cannot be discovered, so its pins cannot be bumped."

pr_rows=""
if ! pr_rows="$(gh pr list --state open --json number,headRefName,updatedAt,isCrossRepository \
  --jq '.[] | [.number, .headRefName, .updatedAt, (.isCrossRepository | tostring)] | @tsv')"; then
  err "gh pr list failed: the Release PR cannot be discovered, so its pins cannot be bumped."
fi

PR_NUMBER=""
PR_BRANCH=""
PR_UPDATED=""
while IFS=$'\t' read -r number branch updated cross; do
  [[ -n $number ]] || continue
  [[ $branch == "$BRANCH_PREFIX"* ]] || continue
  # Only a branch of THIS repository can carry the release pins. `gh pr list`
  # also returns the pull requests opened from a FORK, and their head branch
  # name looks exactly like ours while its ref does not exist in our remote:
  # picking one would fail the fetch and break the release job on every run,
  # and the real Release PR would never get its pins. An unverifiable head is
  # skipped too — the field is requested from gh, and gh does not error when
  # it is missing, it yields nothing (see the harness case (l)).
  if [[ $cross != "false" ]]; then
    if [[ $cross == "true" ]]; then
      note "ignoring PR #$number: its head branch '$branch' lives in another repository (a fork), and only a branch of this repository can carry the release pins."
    else
      note "ignoring PR #$number: the repository of its head branch '$branch' could not be verified (isCrossRepository=$cross), and an unverifiable branch is not bumped."
    fi
    continue
  fi
  # RFC-3339 timestamps sort lexicographically, so the newest one wins.
  if [[ -z $PR_UPDATED || $updated > $PR_UPDATED ]]; then
    PR_NUMBER="$number"
    PR_BRANCH="$branch"
    PR_UPDATED="$updated"
  fi
done <<< "$pr_rows"

if [[ -z $PR_NUMBER ]]; then
  note "no open PR with the '$BRANCH_PREFIX' prefix: release-plz decided there is no release to prepare, so there are no pins to bump."
  exit 0
fi

printf 'release-pins: Release PR #%s (%s, updated %s)\n' "$PR_NUMBER" "$PR_BRANCH" "$PR_UPDATED"

# ─── 2. re-read the PR before touching anything ─────────────────────────────
# The PR can be merged between the list and the push; pushing to a merged
# branch would resurrect a release that is already done.
pr_state="$(gh pr view "$PR_NUMBER" --json state --jq '.state')"
if [[ "$pr_state" != "OPEN" ]]; then
  note "Release PR #$PR_NUMBER is no longer OPEN (state=$pr_state): it was closed or merged while this run was in flight, so there are no pins to bump."
  exit 0
fi

# ─── 3. fetch, worktree, write, commit, push ────────────────────────────────
BRANCH="$PR_BRANCH"

# run_attempt returns 0 on success, 2 when the push was rejected (retryable),
# and never returns on a hard failure (err exits the script).
run_attempt() {
  local attempt="$1"
  local version

  if ! git -C "$REPO_ROOT" fetch --no-tags "$REMOTE" \
    "+refs/heads/$BRANCH:refs/remotes/$REMOTE/$BRANCH"; then
    err "could not fetch '$BRANCH' from '$REMOTE'."
  fi

  TMP_ROOT="$(mktemp -d)"
  WT_DIR="$TMP_ROOT/worktree"
  if ! git -C "$REPO_ROOT" worktree add --detach "$WT_DIR" "refs/remotes/$REMOTE/$BRANCH"; then
    err "could not create the temporary worktree for '$BRANCH'."
  fi

  # The writer resolves its own root, so it edits the worktree's tree — the
  # release branch — and never this checkout.
  if ! bash "$WT_DIR/scripts/bump_docs_version_pins.sh"; then
    err "the docs version pins writer failed on '$BRANCH': nothing was committed and nothing was pushed."
  fi

  if [[ -z "$(git -C "$WT_DIR" status --porcelain)" ]]; then
    note "no changes: the docs version pins of '$BRANCH' already declare the workspace version — no commit, no push, no CI triggered."
    return 0
  fi

  version="$(read_workspace_version "$WT_DIR")"
  [[ -n $version ]] || err "could not read [workspace.package] version from the worktree of '$BRANCH'."

  git -C "$WT_DIR" add -A
  # Explicit identity: the commit is authored by the bot, not by whoever
  # happens to have a git identity configured on the runner. Signing is off:
  # the runner has no key, and a signed bot commit is not a thing anybody
  # verifies.
  git -C "$WT_DIR" \
    -c user.name='github-actions[bot]' \
    -c user.email='bot@github.com' \
    -c commit.gpgsign=false \
    commit -m "chore(docs): actualizar los pines de versión a v$version"

  if ! push_branch "$WT_DIR"; then
    printf '::warning::the push to %s was rejected (attempt %s)\n' "$BRANCH" "$attempt" >&2
    return 2
  fi

  printf 'release-pins: pushed %s to %s (docs version pins → v%s)\n' \
    "$(git -C "$WT_DIR" rev-parse HEAD)" "$BRANCH" "$version"
  return 0
}

attempt=1
max_attempts=2
while :; do
  rc=0
  run_attempt "$attempt" || rc=$?
  if [[ $rc -eq 0 ]]; then
    exit 0
  fi
  if [[ $rc -eq 2 && $attempt -lt $max_attempts ]]; then
    note "the push was rejected — release-plz regenerated the branch. Fetching again and re-applying the pins (attempt $((attempt + 1))/$max_attempts)."
    # Drop the stale worktree before the next attempt: it sits on the branch
    # as it was BEFORE release-plz regenerated it.
    cleanup
    WT_DIR=""
    TMP_ROOT=""
    attempt=$((attempt + 1))
    continue
  fi
  err "could not push the docs version pins to '$BRANCH' after $attempt attempt(s)."
done
