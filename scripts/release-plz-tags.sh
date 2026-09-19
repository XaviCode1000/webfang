#!/usr/bin/env bash
#
# Print release-plz tags, one per line (empty = none).
#
# Single source of truth for "which tags may be TRUSTED as a release-plz release
# tag". Three callers depend on that decision:
#   * release-plz.yml / dispatch-release        — which tag to hand to release.yml
#   * release-plz.yml / Verify the release tag  — whether the release job succeeded
#   * release-reconcile.yml / reconcile         — which historical tags to sweep
# A trust predicate duplicated across callers drifts; one copy cannot.
#
# Fingerprint of a release-plz-created tag (verified in-repo on v2.1.0 and
# v2.1.1): annotated, tagger `github-actions[bot]`, subject
# "chore: Release package <crate> version X". It deliberately EXCLUDES human
# tags: v2.0.0 is annotated by the maintainer with the subject equal to the tag
# name, and v1.0.0 has no GitHub Release at all. Trusting or dispatching one of
# those would attach binaries built from unrelated code.
#
# Modes:
#   (none)   TRUSTED release-plz tags at HEAD — the push-triggered path
#   --any    every v* tag at HEAD, fingerprint ignored — diagnostics, so a caller
#            can tell "nothing to release" apart from "an unrecognised tag landed"
#   --all    every TRUSTED release-plz tag in history — the reconciliation sweep,
#            whose whole point is a tag that is old by definition
#
# For the HEAD-scoped modes, `--points-at HEAD` is the second half of the
# predicate: release-plz tags the commit it just released, so a tag at HEAD
# belongs to THIS run, whereas listing history there would drag in every
# historical tag.
#
# Exit status: 0 for every mode, including an empty list (a normal outcome).
# Non-zero only for a usage error, so callers never mistake "no tags" for a
# trust failure.
set -euo pipefail

MODE="${1:-}"
case "$MODE" in
  "" | --any | --all) ;;
  *)
    echo "usage: $(basename "$0") [--any|--all]" >&2
    exit 2
    ;;
esac

# A release-plz tag NAME, before the fingerprint check. Anchored, so a
# branch-like ref can never match.
TAG_NAME_RE='^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$'

if [[ "$MODE" == "--all" ]]; then
  mapfile -t candidates < <(git tag -l | grep -E "$TAG_NAME_RE" || true)
else
  mapfile -t candidates < <(git tag --points-at HEAD | grep -E "$TAG_NAME_RE" || true)
fi

for tag in "${candidates[@]}"; do
  [[ -n "$tag" ]] || continue

  # `--any` skips the fingerprint on purpose: it answers "did any tag land here",
  # which is exactly the question the fingerprint cannot answer.
  if [[ "$MODE" == "--any" ]]; then
    printf '%s\n' "$tag"
    continue
  fi

  tagger="$(git for-each-ref --format='%(taggername)' "refs/tags/$tag")"
  subject="$(git tag -l --format='%(contents:subject)' "$tag")"
  if [[ "$tagger" == "github-actions[bot]" && "$subject" == "chore: Release package "* ]]; then
    printf '%s\n' "$tag"
  fi
done
