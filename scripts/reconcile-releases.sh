#!/usr/bin/env bash
#
# Reconciliation sweep: ensure a complete GitHub Release for every release-plz
# tag in history (webfang#1484).
#
# WHY THIS EXISTS
#   dispatch-release fires ONCE, on the push that carries the tag. If its call to
#   `gh workflow run` fails, nothing retries, and the failure is silent BY
#   CONSTRUCTION: a dispatch that never happened produces no workflow run, so an
#   observer that reads run states (ci-health.yml) has nothing to observe — there
#   is no red run to report. Measured cost of that gap: v2.1.1 sat as a tag with
#   no GitHub Release until a human backfilled it by hand.
#
#   The other half of the fix is the bounded retry in scripts/ensure-release.sh.
#   This sweep is the part that retry cannot cover: a tag whose dispatch was
#   never attempted at all, for any reason.
#
# WHY HISTORY IS THE RIGHT SCOPE HERE
#   Unlike the push-triggered path (HEAD-scoped, because a tag at HEAD belongs to
#   this run), a *missed* dispatch is old by definition. The candidate list still
#   comes from the shared trust predicate, never from `git tag -l 'v*'`: v1.0.0
#   is a human tag with no Release, and dispatching it would publish binaries
#   built from unrelated code.
#
# Idempotent: a complete Release is a no-op, so re-running costs API calls and
# changes nothing. Exit 0 when every trusted tag ends up complete.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

mapfile -t tags < <(bash "$REPO_ROOT/scripts/release-plz-tags.sh" --all)

if [[ "${#tags[@]}" -eq 0 ]]; then
  echo "No release-plz tags in history; nothing to reconcile."
  exit 0
fi

echo "Sweeping ${#tags[@]} release-plz tag(s): ${tags[*]}"

failed=0
for tag in "${tags[@]}"; do
  echo "--- $tag"
  if ! bash "$REPO_ROOT/scripts/ensure-release.sh" "$tag"; then
    failed=$((failed + 1))
  fi
done

if (( failed > 0 )); then
  echo "::error::$failed release-plz tag(s) could not be reconciled and may have no binaries. See the ::error:: lines above." >&2
  exit 1
fi

echo "Reconciliation complete: every release-plz tag has a complete GitHub Release."
