#!/usr/bin/env bash
#
# Regression harness for merge_group ref handling in the required workflows.
#
# The old PR-resolution step queried `pulls?head=<owner>:<synthetic ref>`,
# which can never match: the queue ref is
# `refs/heads/gh-readonly-queue/<base>/pr-<N>-<sha>`, not the PR head. This
# harness pins the synthetic-ref parsing (both payload forms), the topology
# lookup through the resolved PR, and the base-ref normalization in
# mutants.yml. Offline; reads the workflow files only.
# shellcheck disable=SC2016  # the workflow snippets below are literal text
set -euo pipefail
REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
WF="$REPO_ROOT/.github/workflows/pr-validation.yml"
MUT="$REPO_ROOT/.github/workflows/mutants.yml"
fails=0
ok()   { echo "  OK   $1"; }
bad()  { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { local desc="$1"; shift; if "$@"; then ok "$desc"; else bad "$desc"; fi; }

echo "test_pr_validation_merge_group_ref: merge_group resolution (offline)"
check "workflow file exists" test -f "$WF"
check "workflow triggers on merge_group" grep -q '^  merge_group:' "$WF"
expr="$(grep -oE 's#.*#p' "$WF" | head -1)"
check "synthetic-ref sed expression exists" test -n "$expr"
resolve() { printf '%s' "$1" | sed -nE "$expr"; }
check "refs/heads/gh-readonly-queue/main/pr-123-<sha> -> 123" test "$(resolve 'refs/heads/gh-readonly-queue/main/pr-123-abcdef123')" = 123
check "gh-readonly-queue/main/pr-123-<sha> -> 123" test "$(resolve 'gh-readonly-queue/main/pr-123-abcdef123')" = 123
check "release/1.2 base -> 77" test "$(resolve 'refs/heads/gh-readonly-queue/release/1.2/pr-77-deadbeef')" = 77
check "malformed PR id -> empty" test -z "$(resolve 'refs/heads/gh-readonly-queue/main/pr-abc-def')"
check "no live pulls?head lookup" bash -c '! grep -vE "^[[:space:]]*#" "$1" | grep -q "pulls?head="' _ "$WF"
check "topology reads refs from the resolved PR" grep -q 'gh pr view "$PR_NUMBER" --repo "$REPO" --json headRefName,baseRefName' "$WF"
check "topology no longer reads merge_group.base_ref" bash -c '! grep -q "merge_group.base_ref" "$1"' _ "$WF"
check "resolved PR is checked OPEN" grep -q 'if \[ "$state" != "OPEN" \]' "$WF"

check "mutants.yml triggers on merge_group" grep -q '^  merge_group:' "$MUT"
check "mutants.yml strips refs/heads/ from merge_group base_ref" grep -Fq 'BASE="${BASE_REF#refs/heads/}"' "$MUT"
check "mutants.yml diff uses the normalized branch" grep -Fq 'git diff "origin/$BASE"...HEAD' "$MUT"
echo "test_pr_validation_merge_group_ref: FAIL=$fails"
[[ $fails -eq 0 ]]
