#!/usr/bin/env bash
#
# Ensure every release-plz tag has a complete GitHub Release — the ONE place
# that decides which tags still need a dispatch (webfang#1674).
#
# WHY THIS FILE EXISTS
#   Two callers need that decision and they must never disagree:
#     * release-plz.yml / dispatch-release   — the IMMEDIATE path, on the push
#       that carried (or should have carried) the tag;
#     * release-reconcile.yml / reconcile    — the daily backstop (#1484), for
#       a dispatch that never happened at all.
#   Before this file each caller resolved its own candidates, and the
#   immediate path resolved them with `git tag --points-at HEAD`. That is
#   correct ONLY for a fast-forwarded tag. A Release PR merged by SQUASH
#   creates a NEW commit on main; the tag release-plz pushed still points at
#   the PR's pre-squash head, which is unreachable from HEAD. Measured on
#   v2.4.0 (PR #1554): HEAD be7dc8a6 (squash merge), tag v2.4.0 -> 008b7431
#   (pre-squash), `git tag --points-at HEAD` -> 0 candidates, job "Dispatch
#   binary release" green in 6 s, and ZERO release.yml runs created.
#
#   So the immediate path now resolves candidates the same way the backstop
#   does — over history, through the shared trust predicate — and delegates
#   "does this tag already have a complete Release?" to ensure-release.sh,
#   which is the same helper the backstop uses and which is idempotent. A tag
#   that already shipped is a no-op; a tag that did not, ships now.
#
#   MEASURED, AND WIDER THAN REPORTED (#1674, PR #1668). The issue framed the
#   defect as squash-specific. It is not: it is "the tag does not sit on the
#   merge's HEAD commit", which a MERGE commit breaks identically. PR #1668
#   (`chore: release v2.4.1`) was merged with a merge commit (dec59501), and
#   the tag still sat on that merge's second parent (829f03e) — the pre-merge
#   head — so the HEAD-scoped lookup returned 0 there too. Run 36625232646:
#   "Dispatch binary release" success in 6 s (20:55:04Z -> 20:55:10Z), nothing
#   dispatched. v2.4.1 shipped only when the daily sweep (run 36716910362,
#   schedule, 12:46Z) dispatched it; the Release published at 12:56:42Z with
#   all 5 assets — about 16.5 h after the tag. So the old lookup handled exactly
#   one shape (a tag on the pushed commit) and reported success on every other.
#
# WHY NOT `exit 1` ON AN EMPTY CANDIDATE LIST
#   A squash-merged Release PR is an EXPECTED case, not an error; red there
#   would be noise, not signal. But "nothing to do" must never be a silent
#   success either — the false green of #1674 is worse than a red, because it
#   fires no alert. So the no-op case is an EXPLICIT, VISIBLE SKIP: a `::notice::`
#   annotation plus a GITHUB_STEP_SUMMARY block that names what was inspected
#   and what was left alone. The one shape that IS a hard error is a release
#   commit on HEAD with no trusted tag anywhere in history: there the tag was
#   never created, and no sweep on any later day can invent it.
#
# SCOPE NOTE
#   The trust predicate (scripts/release-plz-tags.sh --all), never
#   `git tag -l 'v*'`: v1.0.0 is a human tag with no Release, and release.yml
#   builds the tag it is handed, so dispatching it would publish binaries built
#   from unrelated code.
#
# Usage: sweep-releases.sh [--head-commit-subject <subject>]
#   --head-commit-subject feeds the subject of the push being processed, so the
#   "release commit with no tag" check works even when the caller runs from a
#   different checkout. An EMPTY value is accepted and falls back to
#   `git log -1 --format=%s HEAD` — a caller that has no subject (a push event
#   without head_commit, a re-run) must degrade to the local HEAD, never to a
#   usage error that would red the job for an argument it did not choose.
#
# Exit status:
#   0  every trusted release-plz tag has a complete GitHub Release (or none
#      exist and HEAD does not look like a release commit) — a VISIBLE skip;
#   1  at least one tag could not be reconciled, or HEAD looks like a release
#      commit while history holds no trusted tag at all.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

HEAD_SUBJECT=""
if [[ "${1:-}" == "--head-commit-subject" ]]; then
  HEAD_SUBJECT="${2:-}"
  [[ $# -ge 2 ]] || { echo "usage: $(basename "$0") [--head-commit-subject <subject>]" >&2; exit 2; }
  shift 2
fi
if [[ $# -ne 0 ]]; then
  echo "usage: $(basename "$0") [--head-commit-subject <subject>]" >&2
  exit 2
fi
if [[ -z "$HEAD_SUBJECT" ]]; then
  HEAD_SUBJECT="$(git log -1 --format=%s HEAD 2>/dev/null || true)"
fi

# Append to the job summary when running under Actions. Never required: the
# same text is always written to stdout, and the ::notice:: annotation below
# is what makes the skip visible in the run log itself.
summary() {
  printf '%s\n' "$1"
  if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    printf '%s\n' "$1" >>"$GITHUB_STEP_SUMMARY"
  fi
}

# Is the commit being processed a version-bump commit? Squash merges append the
# PR number ("chore: release v2.4.1 (#1668)"), hence the unanchored tail.
head_is_release_commit() {
  [[ "$HEAD_SUBJECT" == "chore: release v"* ]]
}

# Capture the exit status EXPLICITLY. `mapfile -t tags < <(...)` discards it:
# with a broken trust predicate the producer exits non-zero, the list is empty,
# and this script would report "nothing to sweep" and exit 0 — a green safety
# net sitting on top of a dead filter (webfang#1476). An empty list is only
# trustworthy when the producer said so.
rc=0
tag_list="$(bash "$REPO_ROOT/scripts/release-plz-tags.sh" --all)" || rc=$?
if [[ "$rc" -ne 0 ]]; then
  echo "::error::the trust predicate could not run (exit $rc) - refusing to sweep, because an unreadable filter is indistinguishable from 'no tags'." >&2
  exit 1
fi

if [[ -n "$tag_list" ]]; then
  mapfile -t tags <<<"$tag_list"
else
  tags=()
fi

if [[ "${#tags[@]}" -eq 0 ]]; then
  if head_is_release_commit; then
    echo "::error::HEAD is a release commit ('$HEAD_SUBJECT') but history contains NO trusted release-plz tag. The tag for this release was never created, so no dispatch can be built and no later sweep can repair it. Re-tag the release commit and dispatch it by hand: gh workflow run release.yml --repo \$GITHUB_REPOSITORY -f tag=<vX.Y.Z> -f expected_sha=<commit> (webfang#1674)." >&2
    summary "### Release dispatch: FAILED
No trusted release-plz tag in history, but HEAD is a release commit (\`$HEAD_SUBJECT\`).
Nothing was dispatched, and nothing can be: the tag does not exist. See the \`::error::\` line in the log."
    exit 1
  fi
  echo "::notice title=Release dispatch::No release-plz tag exists in history and HEAD ('$HEAD_SUBJECT') is not a release commit - nothing to publish. This is an explicit skip, not a silent success."
  summary "### Release dispatch: skipped
No release-plz tag exists in history, and HEAD (\`$HEAD_SUBJECT\`) is not a release commit.
Nothing to publish — an explicit skip, not a silent success."
  exit 0
fi

echo "Sweeping ${#tags[@]} release-plz tag(s): ${tags[*]}"

failed=0
dispatched=0
already_complete=0
for tag in "${tags[@]}"; do
  echo "--- $tag"
  # L1.1 Identity (Shape 2): a default-branch dispatch MUST carry expected_sha —
  # release.yml fails closed without PROV_EXPECTED_SHA (webfang#1540). Resolve it
  # the same way every caller does: the commit the trusted tag points at. A tag
  # we cannot resolve is not dispatchable — fail this entry closed rather than
  # send a dispatch that is doomed to die at preflight.
  if ! expected_sha="$(git rev-parse "refs/tags/${tag}^{commit}" 2>/dev/null)"; then
    echo "::error::could not resolve commit for trusted tag $tag - refusing to dispatch without expected_sha (webfang#1540)." >&2
    failed=$((failed + 1))
    continue
  fi
  if ! entry_out="$(bash "$REPO_ROOT/scripts/ensure-release.sh" "$tag" "$expected_sha")"; then
    printf '%s\n' "$entry_out"
    failed=$((failed + 1))
    continue
  fi
  printf '%s\n' "$entry_out"
  # Classify what ensure-release.sh decided, so "this run dispatched nothing"
  # can be reported as the visible skip it has to be. The counting is
  # deliberately FAIL-SAFE: anything the helper says that is not the explicit
  # "already complete" line is counted as a dispatch. A wording change in the
  # helper can therefore only ever over-report, never hide a dispatch behind a
  # green no-op — the failure this whole file exists to close.
  if [[ "$entry_out" == *"already complete"* ]]; then
    already_complete=$((already_complete + 1))
  else
    dispatched=$((dispatched + 1))
  fi
done

if (( failed > 0 )); then
  echo "::error::$failed release-plz tag(s) could not be reconciled and may have no binaries. See the ::error:: lines above." >&2
  summary "### Release dispatch: FAILED
$failed of ${#tags[@]} trusted tag(s) could not be reconciled and may have no binaries.
Inspected: ${tags[*]}
Dispatched: $dispatched. Already complete: $already_complete."
  exit 1
fi

if (( dispatched == 0 )); then
  # The false green of webfang#1674: the run did nothing and reported success.
  # Nothing about the outcome is wrong here — every tag really is shipped — but
  # an operator reading "Dispatch binary release: success" must be able to tell
  # that apart from a run that dispatched, so the no-op is announced, counted,
  # and written to the job summary. Visible skip, not a silent success.
  echo "::notice title=Release dispatch::Nothing to publish: all ${#tags[@]} trusted release-plz tag(s) already carry a complete GitHub Release. Inspected: ${tags[*]}. This run performed no dispatch — an explicit skip, not a build."
  summary "### Release dispatch: skipped (nothing to publish)
Inspected ${#tags[@]} trusted release-plz tag(s): ${tags[*]}
All of them already carry a complete GitHub Release, so **no dispatch was made by this run**.
This is an explicit skip, not a successful build."
  exit 0
fi

echo "Sweep complete: dispatched $dispatched tag(s); $already_complete already complete and left alone."
summary "### Release dispatch: complete
Inspected ${#tags[@]} trusted release-plz tag(s): ${tags[*]}
Dispatched: $dispatched. Already complete: $already_complete.
Every trusted tag now carries a complete GitHub Release. The sweep is idempotent
(\`ensure-release.sh\` makes a complete Release a no-op), so it is safe to run on every push
to main as well as on the daily #1484 schedule."
