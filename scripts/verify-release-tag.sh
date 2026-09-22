#!/usr/bin/env bash
#
# Decide whether release-plz's `release` job succeeded — by verifying the tag.
#
# WHY NOT THE EXIT CODE
#   The duplicate-tag 422 (webfang#1341) makes `release-plz release` exit 1 AFTER
#   it has already created the correct tag, inside the SAME run. Measured on
#   v2.1.1: the tag is authored by `github-actions[bot]` at 18:04:23, inside the
#   run window 18:03:58-18:04:43, while the job failed on
#   `422 Reference already exists`.
#
# WHY THE TAG IS A COMPLETE SUCCESS CRITERION HERE
#   release-plz.toml sets git_only = true, publish = false and
#   git_release_enable = false, so the only side effect of `release-plz release`
#   is the git tag. Verifying that tag therefore decides the whole job.
#
# WHY THIS IS NOT THE `continue-on-error` ANTI-PATTERN (webfang#1476)
#   There, the job reported success with NOTHING re-evaluating the failure — the
#   signal was removed and the green kept. Here the failure is re-evaluated by
#   this script, which fails the job on anything that is not a verified correct
#   tag. Green means a verified tag, never a suppressed error.
#
# Input:  RELEASE_OUTCOME  mirrors `steps.release.outcome` (success|failure|...).
# Exit:   0 when the release is verifiably fine, 1 otherwise.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAGS_SCRIPT="$REPO_ROOT/scripts/release-plz-tags.sh"

# `steps.release.outcome` is the RAW result, before `continue-on-error` rewrites
# it — which is exactly what we need, since the whole point is to re-evaluate it.
OUTCOME="${RELEASE_OUTCOME:-unknown}"

# Capture the producer's exit status instead of letting `mapfile` discard it. With a
# broken trust predicate the list comes back EMPTY, and an empty list under
# OUTCOME=success is exactly the "nothing to release" branch below - so a dead filter
# would report green. That is the webfang#1476 shape (failure signal removed, green
# kept), and it is why this script exists at all.
rc=0
trusted_list="$(bash "$TAGS_SCRIPT")" || rc=$?
if [[ "$rc" -ne 0 ]]; then
  echo "::error::the trust predicate could not run (exit $rc) - refusing to decide the release outcome from an unreadable filter." >&2
  exit 1
fi
if [[ -n "$trusted_list" ]]; then
  mapfile -t trusted <<<"$trusted_list"
else
  trusted=()
fi

case "${#trusted[@]}" in
  1)
    echo "Verified: ${trusted[0]} is a release-plz tag at HEAD."
    if [[ "$OUTCOME" != "success" ]]; then
      echo "release-plz exited non-zero (outcome=$OUTCOME), which is the known duplicate-tag 422 (webfang#1341). The tag it left behind is the intended one, so this run succeeded."
    fi
    exit 0
    ;;
  0)
    if [[ "$OUTCOME" != "success" ]]; then
      echo "::error::release-plz failed (outcome=$OUTCOME) AND no trusted releasable tag exists at HEAD - a real failure, not the duplicate-tag 422. See the release-plz log above." >&2
      exit 1
    fi
    # release-plz succeeded. Before calling this a no-op, check whether a v* tag
    # landed that the fingerprint does NOT recognise: that means the tag format
    # moved and the dispatcher would refuse to hand it over too. Failing here is
    # the difference between a loud breakage and a silent tag-without-binaries
    # — the exact shape that lost v2.1.1.
    rc=0
    unrecognised_list="$(bash "$TAGS_SCRIPT" --any)" || rc=$?
    if [[ "$rc" -ne 0 ]]; then
      echo "::error::the diagnostic tag listing failed (exit $rc) - cannot tell 'no tag landed' from 'the filter is dead', so this cannot be called a no-op." >&2
      exit 1
    fi
    if [[ -n "$unrecognised_list" ]]; then
      mapfile -t unrecognised <<<"$unrecognised_list"
    else
      unrecognised=()
    fi
    if [[ "${#unrecognised[@]}" -gt 0 ]]; then
      echo "::error::${unrecognised[*]} points at HEAD but does not match the release-plz fingerprint (annotated + github-actions[bot] + 'chore: Release package ...'). Either release-plz changed its tag format - update scripts/release-plz-tags.sh - or a human tag landed here. This tag would ship with no binaries." >&2
      exit 1
    fi
    echo "No releasable tag at HEAD and release-plz succeeded: this push carried no version bump. Nothing to release."
    exit 0
    ;;
  *)
    echo "::error::multiple release-plz tags point at HEAD: ${trusted[*]} - refusing to pick one." >&2
    exit 1
    ;;
esac
