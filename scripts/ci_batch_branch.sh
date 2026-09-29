#!/usr/bin/env bash
#
# ci_batch_branch.sh — Phase 5 local integration-branch helper.
#
# Design source: docs/research/webfang-workflow-transformation-blueprint.md
# Phase 5 "Merge batching without native queue" + AGENTS.md "Batch merge of
# multiple green PRs" (procedure step 1-2: branch from main in a new
# worktree, merge each PR's REMOTE head SHA with --no-ff).
#
# Usage:
#   scripts/ci_batch_branch.sh [--dry-run] <branch-name> <SHA1> [SHA2...]
#   scripts/ci_batch_branch.sh -h | --help
#
# Behaviour: creates <branch-name> from the current `main` state WITHOUT
# switching the current worktree (`git worktree add` into the standard
# sibling location ~/Projects/Rust/webfang-worktrees/<dir>, `/` -> `-`),
# then merges each SHA with `git -C <worktree> merge --no-ff <sha>`.
# SHAs must be remote PR head SHAs already fetched locally (verify with
# `gh pr view <N> --json headRefOid`).
#
# Exit codes:
#   0  Branch created and all SHAs merged cleanly (or --dry-run valid).
#   1  A merge conflicted: worktree is left in place, nothing auto-committed.
#   2  Usage error or input validation failure (branch/dir exists, bad SHA,
#      gh/git missing, base ref unresolvable).
#
# Scope notes:
#   - Never resolves conflicts automatically and never commits a conflict
#     resolution. A conflict stops the loop immediately.
#   - Local-only: no push, no PR creation, no remote mutation.
#   - After success, run the fast gate in the new worktree and open the
#     batch PR manually (next steps are printed).
set -euo pipefail

WORKTREE_PARENT="$HOME/Projects/Rust/webfang-worktrees"

usage() {
  sed -n '2,27p' "$0"
}

dry_run=0
args=()
for arg in "$@"; do
  case "$arg" in
    -h|--help) usage; exit 0 ;;
    --dry-run) dry_run=1 ;;
    *) args+=("$arg") ;;
  esac
done

if [[ ${#args[@]} -lt 2 ]]; then
  echo "error: branch name and at least one SHA are required" >&2
  usage >&2
  exit 2
fi

branch="${args[0]}"
shas=("${args[@]:1}")

if ! [[ "$branch" =~ ^(feat|fix|chore|docs|style|refactor|perf|test|build|ci|revert)/[a-z0-9._-]+$ ]]; then
  echo "error: branch '$branch' does not match the conventional pattern" >&2
  echo "       ^(feat|fix|chore|docs|style|refactor|perf|test|build|ci|revert)/[a-z0-9._-]+$" >&2
  exit 2
fi

for sha in "${shas[@]}"; do
  if ! [[ "$sha" =~ ^[0-9a-f]{40}$ ]]; then
    echo "error: invalid SHA '$sha' (expected 40 lowercase hex chars)" >&2
    exit 2
  fi
done

if ! command -v git >/dev/null 2>&1; then
  echo "error: git is not installed or not on PATH" >&2
  exit 2
fi

dir="${branch//\//-}"
target="$WORKTREE_PARENT/$dir"

if git rev-parse --verify --quiet "refs/heads/$branch" >/dev/null 2>&1; then
  echo "error: branch '$branch' already exists locally" >&2
  exit 2
fi
if [[ -e "$target" ]]; then
  echo "error: target path '$target' already exists" >&2
  exit 2
fi

# Resolve the base: local `main` state (never the current worktree's HEAD,
# which may be a feature branch).
base="main"
if ! git rev-parse --verify --quiet "refs/heads/$base" >/dev/null 2>&1; then
  echo "error: base ref '$base' does not resolve locally" >&2
  exit 2
fi

for sha in "${shas[@]}"; do
  if ! git cat-file -e "$sha" 2>/dev/null; then
    echo "error: SHA $sha is not present locally (fetch the PR head first)" >&2
    exit 2
  fi
done

if [[ $dry_run -eq 1 ]]; then
  echo "[dry-run] base: $base ($(git rev-parse --short "$base"))"
  echo "[dry-run] would run: git worktree add -b $branch $target $base"
  for sha in "${shas[@]}"; do
    echo "[dry-run] would run: git -C $target merge --no-ff $sha -m \"Merge $sha (batch slice)\""
  done
  echo "[dry-run] validation OK — no worktree created."
  exit 0
fi

echo "==> Creating worktree: git worktree add -b $branch $target $base"
git worktree add -b "$branch" "$target" "$base"

# `git worktree add` does NOT materialise .envrc: the file is gitignored, so the
# new tree starts without one. That is worse than a missing convenience — the
# tree then inherits CARGO_TARGET_DIR from the invoking shell, which is main's
# target dir, and builds a batch worktree straight into it: the exact #1267
# hazard. From this PR the fast gate also fails closed (exit 2). Bootstrap it
# here, while we still know the path.
main_repo="$(dirname "$(git -C "$target" rev-parse --path-format=absolute --git-common-dir)")"
if [[ ! -f "$target/.envrc" ]]; then
  if [[ -f "$main_repo/.envrc" ]]; then
    echo "==> NOTE: $target/.envrc is absent (gitignored, never copied by 'git worktree add')."
    echo "    Bootstrap it before the fast gate, or this tree inherits main's target dir:"
    echo "      cd $target"
    echo "      sed -e 's#cargo-target/webfang#cargo-target/${dir}#' \\"
    echo "          -e 's#^export CARGO_INCREMENTAL=1#export CARGO_INCREMENTAL=0#' \\"
    echo "          '$main_repo/.envrc' > .envrc"
    echo "      direnv allow"
  else
    echo "    WARNING: no .envrc in $target nor in $main_repo — write one by hand"
    echo "             (see AGENTS.md § Worktree lifecycle) before building."
  fi
fi

failed=""
for sha in "${shas[@]}"; do
  echo "==> Merging $sha"
  if git -C "$target" merge --no-ff "$sha" -m "Merge $sha (batch slice)"; then
    echo "    merged $sha"
  else
    failed="$sha"
    break
  fi
done

if [[ -n "$failed" ]]; then
  cat >&2 <<EOF
error: merge conflict on SHA $failed — stopped, worktree left in place.
Recovery (run INSIDE the new worktree):
  cd $target
  git status --short            # inspect conflicted paths
  # ... resolve conflicts by hand ...
  git add <resolved paths>
  git commit                    # complete the --no-ff merge commit
  # To abandon instead:
  git merge --abort
  cd <previous worktree>
  git worktree remove --force $target
  git branch -D $branch
EOF
  exit 1
fi

cat <<EOF
==> Batch branch ready: $branch @ $target
Next steps:
  1. If the bootstrap above was printed, run it (it writes .envrc + direnv allow):
       cd $target && direnv allow
  2. Fast gate in the new worktree (fails closed with exit 2 while
     CARGO_TARGET_DIR is unset — #1677):
       cd $target && bash scripts/ci_fast_gate.sh
  3. Write CHANGELOG entries there (the ONE place they are written).
  4. Push + open the batch PR manually (see docs/merge-queue-manual.md).
EOF
exit 0
