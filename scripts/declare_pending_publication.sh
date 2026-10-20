#!/usr/bin/env bash
#
# Declare the PENDING PUBLICATION of a release in .github/support.json.
#
# WHY THIS EXISTS (issue #2144)
#   `latest` is a FACT about what this repository published, and the enforcing
#   gate `scripts/check_support_drift.sh --scope=published` — a REQUIRED
#   check inside the `Validate PR metadata` job — fails when the declared
#   `latest` of a line lags behind the newest published patch of that line.
#   The gate tolerates exactly ONE narrow state: a declaration that is ONE
#   patch ahead of the newest published patch of its own minor and has no tag
#   of its own yet — the in-flight window of a sanctioned release.
#
#   The support patch flow already produces that state: scripts/bump-support-
#   patch.sh declares the patch it is about to publish BEFORE the tag exists
#   (cut-patch-tag.yml can only cut the tag after the bump commit merges).
#   The release-plz path — the one that publishes from `main` — declared
#   NOTHING: the tag `vX.Y.Z` was published with the declaration still behind
#   it, and the gate turned red for EVERY pull request in the repository.
#   Measured on v4.0.1: the tag was published, the `latest` of line 4.0 stayed
#   "4.0.0", and the whole repository went red (repaired by #2143).
#
#   This script is the release-plz path's half of that contract: it declares
#   the publication that is about to happen, in the same commit that carries
#   the version bump, so a tag never exists without its declaration.
#
# WHAT IT DOES
#   The version it declares is the one the RELEASE BRANCH carries, because
#   that is the version the next tag will be cut from: the release-plz
#   `release-pr` command bumps the workspace version in the Release PR
#   branch, and the `release` command cuts the tag only after that branch
#   merges. The declaration therefore rides in the Release PR commit, in the
#   same window the support patch flow uses.
#
#   - the version is read from `[workspace.package] version` of the Cargo.toml
#     of the checkout it runs in, with the same sed discipline
#     scripts/apply_release_pins.sh uses (one grammar, not two);
#   - the line is `${VERSION%.*}`;
#   - a line that is NOT declared in .github/support.json is NOT invented:
#     rotating the support lines is a governance decision (an issue plus the
#     maintainer — see AGENTS.md "Agent routing" and scripts/rotate-stable.sh),
#     so the step warns, names the version and exits 0. A release is never
#     blocked by a support governance matter; the enforcing gate reports the
#     pending rotation with its own, more precise message;
#   - a declared line is handed to scripts/refresh_support_latest.sh, which is
#     the SINGLE WRITER of the `latest` values. This script never writes
#     support.json itself and never re-implements the writer's guards;
#   - it is IDEMPOTENT: re-running with the same version writes nothing.
#
# MODES
#   scripts/declare_pending_publication.sh           the ORCHESTRATOR — this is
#                                                    what the release-plz
#                                                    workflow step runs: it
#                                                    finds the open Release PR,
#                                                    declares the publication in
#                                                    a throwaway worktree of
#                                                    that branch, and commits and
#                                                    pushes it there.
#   scripts/declare_pending_publication.sh --write   the WRITER — declares the
#                                                    pending publication in the
#                                                    CURRENT checkout. No push,
#                                                    no commit, no remote
#                                                    access, and never `main`.
#
# WHY THE ORCHESTRATOR LIVES HERE AND NOT IN scripts/apply_release_pins.sh
#   The pins orchestrator already owns "find the Release PR → worktree →
#   write → commit → push", and reusing that exact flow was the first choice.
#   Its file is outside the edit surface of this unit, so the flow is
#   reproduced here with the SAME selection rule (the same Release PR, or the
#   pins and the declaration would land on two different branches), the SAME
#   identity, the SAME commit style and the SAME single retry on a rejected
#   push. When apply_release_pins.sh can be touched, both flows should be
#   folded into one.
#
# INPUTS (all optional — they exist so the flow can be exercised offline)
#   DECLARE_PENDING_REPO_ROOT      repository to work in (default: this repo)
#   DECLARE_PENDING_BRANCH_PREFIX  Release PR branch prefix (default: chore/release-)
#   DECLARE_PENDING_REMOTE         remote to fetch from and push to (default: origin)
#
# EXIT: 0 nothing to do / declared / skipped · 1 failed (with `::error::`)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

note() { printf '::notice::%s\n' "$*"; }
warn() { printf '::warning::%s\n' "$*"; }
err() { printf '::error::%s\n' "$*" >&2; exit 1; }

usage() {
  cat >&2 <<'EOF'
usage: scripts/declare_pending_publication.sh [--write]

  (no argument)  orchestrator: find the open Release PR, declare the pending
                 publication of its branch's workspace version in a throwaway
                 worktree of that branch, and commit and push it there.
  --write        writer: declare the pending publication of the workspace
                 version of the CURRENT checkout. Writes nothing else, pushes
                 nothing, and never touches `main`.
EOF
}

# ─── helpers ────────────────────────────────────────────────────────────────

# read_workspace_version <root>: [workspace.package] version, read with the
# SAME sed discipline scripts/apply_release_pins.sh uses — one grammar for the
# version, not two.
read_workspace_version() {
  local root="$1"
  sed -n '/^\[workspace\.package\]/,/^\[/p' "$root/Cargo.toml" \
    | sed -n 's/^version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1
}

# declared_minor <minor>: the minor itself when the line IS declared in
# .github/support.json, nothing when it is not.
declared_minor() {
  jq -r --arg m "$1" '.lines[]? | select(.minor == $m) | .minor' .github/support.json
}

# declared_latest <minor>: the declared `latest` of a line (empty when absent).
declared_latest() {
  jq -r --arg m "$1" '.lines[]? | select(.minor == $m) | (.latest // "")' .github/support.json
}

# ─── 1. the WRITER ──────────────────────────────────────────────────────────
# Declares the pending publication of the workspace version of the checkout it
# runs in. It writes the support declaration through the single writer and
# nothing else: no commit, no push, no remote access.
write_declaration() {
  local root
  if ! root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
    err "not a git repository: there is no checkout whose pending publication could be declared."
  fi
  cd "$root"

  [[ -f "$root/.github/support.json" ]] \
    || err "$root/.github/support.json does not exist: there is no support declaration to update."
  command -v jq >/dev/null 2>&1 \
    || err "jq is not available: the support declaration cannot be read."

  local version
  version="$(read_workspace_version "$root")"
  [[ -n $version ]] \
    || err "could not read the [workspace.package] version from $root/Cargo.toml: a release with no version cannot declare a publication."
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || err "the workspace version '$version' is not X.Y.Z: the pending publication is refused, not guessed."

  local minor="${version%.*}"
  if [[ -z "$(declared_minor "$minor")" ]]; then
    warn "the workspace version $version belongs to line $minor, which is not declared in .github/support.json: the pending publication is NOT declared. Rotating the support lines is a governance decision (an issue plus the maintainer — see AGENTS.md 'Agent routing' and scripts/rotate-stable.sh), and this step does not invent a line. The enforcing gate (scripts/check_support_drift.sh --scope=published) reports the pending rotation with its own message."
    printf 'declare-pending-publication: skipped %s (line not declared; the support rotation is a governance decision)\n' "$minor"
    return 0
  fi

  local current out rc=0
  current="$(declared_latest "$minor")"
  # The single writer of the `latest` values. Its own guards (fail closed,
  # atomic write, proof that nothing but `latest` changed) are NOT re-implemented
  # here: this script decides WHICH publication to declare, the writer decides
  # how the file is written.
  out="$(bash "$root/scripts/refresh_support_latest.sh" --set "${minor}=${version}" 2>&1)" || rc=$?
  if [[ $rc -ne 0 ]]; then
    printf '%s\n' "$out" >&2
    err "the support declaration writer refused to declare ${minor}=${version}: nothing was written."
  fi
  printf '%s\n' "$out"

  if [[ "$current" == "$version" ]]; then
    printf 'declare-pending-publication: already in sync %s=%s (nothing to write)\n' "$minor" "$version"
  else
    printf 'declare-pending-publication: declared %s=%s (pending publication; the tag is cut after the Release PR merges)\n' "$minor" "$version"
  fi
}

# ─── 2. the ORCHESTRATOR ────────────────────────────────────────────────────
# Finds the open Release PR, declares the pending publication in a throwaway
# worktree of that branch, and commits and pushes it there. The flow mirrors
# scripts/apply_release_pins.sh step for step: same selection rule, same
# identity, same commit style, same single retry on a rejected push.

BRANCH_PREFIX="${DECLARE_PENDING_BRANCH_PREFIX:-chore/release-}"
REMOTE="${DECLARE_PENDING_REMOTE:-origin}"
REPO_ROOT="${DECLARE_PENDING_REPO_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
# The Release PR branch being declared. Global on purpose: the push helper
# and the attempt loop both need it, and a local would not reach them.
BRANCH=""

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

# is_local_remote <url>: true when the remote is a path on this machine. A
# local remote needs no credentials, which is what lets the harness run the
# whole flow offline.
is_local_remote() {
  local url="$1"
  case "$url" in
    *://*) return 1 ;;
  esac
  if [[ "$url" =~ ^[A-Za-z0-9._-]+@[^/:]+: ]]; then
    return 1
  fi
  return 0
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

orchestrate() {
  command -v gh >/dev/null 2>&1 \
    || err "gh is not available: the Release PR cannot be discovered, so its pending publication cannot be declared."

  local pr_rows
  if ! pr_rows="$(gh pr list --state open --json number,headRefName,updatedAt,isCrossRepository \
    --jq '.[] | [.number, .headRefName, .updatedAt, (.isCrossRepository | tostring)] | @tsv')"; then
    err "gh pr list failed: the Release PR cannot be discovered, so its pending publication cannot be declared."
  fi

  local pr_number="" pr_branch="" pr_updated="" number branch updated cross
  while IFS=$'\t' read -r number branch updated cross; do
    [[ -n $number ]] || continue
    [[ $branch == "$BRANCH_PREFIX"* ]] || continue
    # Only a branch of THIS repository can carry the declaration. `gh pr list`
    # also returns the pull requests opened from a FORK, and their head branch
    # name looks exactly like ours while its ref does not exist in our remote.
    if [[ $cross != "false" ]]; then
      if [[ $cross == "true" ]]; then
        note "ignoring PR #$number: its head branch '$branch' lives in another repository (a fork), and only a branch of this repository can carry the declaration."
      else
        note "ignoring PR #$number: the repository of its head branch '$branch' could not be verified (isCrossRepository=$cross), and an unverifiable branch is not declared."
      fi
      continue
    fi
    # RFC-3339 timestamps sort lexicographically, so the newest one wins.
    if [[ -z $pr_updated || $updated > $pr_updated ]]; then
      pr_number="$number"
      pr_branch="$branch"
      pr_updated="$updated"
    fi
  done <<< "$pr_rows"

  if [[ -z $pr_number ]]; then
    note "no open PR with the '$BRANCH_PREFIX' prefix: release-plz decided there is no release to prepare, so there is no pending publication to declare."
    exit 0
  fi

  printf 'declare-pending-publication: Release PR #%s (%s, updated %s)\n' "$pr_number" "$pr_branch" "$pr_updated"

  # The PR can be merged between the list and the push; pushing to a merged
  # branch would resurrect a release that is already done.
  local pr_state
  pr_state="$(gh pr view "$pr_number" --json state --jq '.state')"
  if [[ "$pr_state" != "OPEN" ]]; then
    note "Release PR #$pr_number is no longer OPEN (state=$pr_state): it was closed or merged while this run was in flight, so there is no pending publication to declare."
    exit 0
  fi

  BRANCH="$pr_branch"
  local attempt=1
  local max_attempts=2
  while :; do
    local rc=0
    run_attempt "$attempt" || rc=$?
    if [[ $rc -eq 0 ]]; then
      exit 0
    fi
    if [[ $rc -eq 2 && $attempt -lt $max_attempts ]]; then
      note "the push was rejected — release-plz regenerated the branch. Fetching again and re-declaring the publication (attempt $((attempt + 1))/$max_attempts)."
      cleanup
      WT_DIR=""
      TMP_ROOT=""
      attempt=$((attempt + 1))
      continue
    fi
    err "could not push the pending publication to '$BRANCH' after $attempt attempt(s)."
  done
}

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
  # release branch — and never this checkout. It is the copy that runs this
  # step, so the declaration never depends on the branch already carrying it.
  if ! ( cd "$WT_DIR" && bash "$SCRIPT_DIR/declare_pending_publication.sh" --write ); then
    err "the pending publication could not be declared on '$BRANCH': nothing was committed and nothing was pushed."
  fi

  if [[ -z "$(git -C "$WT_DIR" status --porcelain)" ]]; then
    note "no changes: the release branch already declares the pending publication of its workspace version — no commit, no push, no CI triggered."
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
    commit -m "chore(support): declare the pending publication of v$version"

  if ! push_branch "$WT_DIR"; then
    printf '::warning::the push to %s was rejected (attempt %s)\n' "$BRANCH" "$attempt" >&2
    return 2
  fi

  printf 'declare-pending-publication: pushed %s to %s (pending publication → v%s)\n' \
    "$(git -C "$WT_DIR" rev-parse HEAD)" "$BRANCH" "$version"
  return 0
}

# ─── dispatch ───────────────────────────────────────────────────────────────
MODE="orchestrate"
case "${1:-}" in
  "") ;;
  --write) MODE="write" ;;
  -h|--help) usage; exit 0 ;;
  *) usage; exit 2 ;;
esac

if [[ "$MODE" == "write" ]]; then
  write_declaration
else
  trap cleanup EXIT
  orchestrate
fi
