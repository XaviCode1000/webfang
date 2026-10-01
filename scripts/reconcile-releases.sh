#!/usr/bin/env bash
#
# Reconciliation sweep: ensure a complete GitHub Release for every release-plz
# tag in history (webfang#1484). The DAILY BACKSTOP.
#
# WHY THIS EXISTS
#   The immediate dispatch-release job fires ONCE, on the push that carried the
#   tag. If its call to `gh workflow run` fails, nothing retries, and the failure
#   is silent BY CONSTRUCTION: a dispatch that never happened produces no
#   workflow run, so an observer that reads run states (ci-health.yml) has
#   nothing to observe — there is no red run to report. Measured cost of that
#   gap: v2.1.1 sat as a tag with no GitHub Release until a human backfilled it
#   by hand.
#
#   The other half of the fix is the bounded retry in scripts/ensure-release.sh.
#   This sweep is the part that retry cannot cover: a tag whose dispatch was
#   never attempted at all, for any reason. It stays a BOUNDED backstop — it
#   does not become the primary mechanism, and the immediate path does not
#   depend on it running.
#
# WHY IT DELEGATES (webfang#1674)
#   The decision "which tags still need a dispatch" now lives in ONE place,
#   scripts/sweep-releases.sh, which the immediate dispatch-release job also
#   runs. Two callers that each resolved their own candidates could disagree
#   about what a release is, and did: the immediate path resolved with
#   `git tag --points-at HEAD`, which finds nothing after a squash merge, and
#   reported a green no-op while binaries never shipped. A wrapper, rather than
#   a second copy of the loop, is what makes that disagreement unrepresentable.
#
# Idempotent: a complete Release is a no-op, so re-running costs API calls and
# changes nothing. Exit 0 when every trusted tag ends up complete.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec bash "$REPO_ROOT/scripts/sweep-releases.sh"
