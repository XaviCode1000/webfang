#!/usr/bin/env bash
#
# Print the release-plz tags that point at HEAD, one per line (empty = none).
#
# Single source of truth for "which tags may be TRUSTED as a release-plz
# release tag". Three callers depend on that decision:
#   * release-plz.yml / dispatch-release      — which tag to hand to release.yml
#   * release-plz.yml / Verify the release tag — whether the job succeeded
#   * (diagnostics) --any — did ANY tag land at HEAD regardless of fingerprint
# A trust predicate duplicated across callers drifts; one copy cannot.
#
# Fingerprint of a release-plz-created tag (verified in-repo on v2.1.0 and
# v2.1.1): annotated, tagger `github-actions[bot]`, subject
# "chore: Release package <crate> version X". It deliberately EXCLUDES human
# tags: v2.0.0 is annotated by the maintainer with the subject equal to the tag
# name, and v1.0.0 has no GitHub Release at all. Trusting or dispatching one of
# those would attach binaries built from unrelated code.
#
# `--points-at HEAD` is the second half of the predicate: release-plz tags the
# commit it just released, so a tag at HEAD belongs to THIS run. Listing history
# instead (`git tag -l 'v*'`) would drag in every historical tag.
#
# Usage:
#   release-plz-tags-at-head.sh          trusted release-plz tags at HEAD
#   release-plz-tags-at-head.sh --any    every v* tag at HEAD, fingerprint ignored
#
# --any exists so a caller can tell "nothing to release" apart from "a tag
# landed that we do not recognise" — the latter means the fingerprint moved and
# must fail loudly rather than leave a tag with no binaries behind it.
#
# Exit status: 0 for both modes, including an empty list (a normal outcome).
# Non-zero only for a usage error, so callers never mistake "no tags" for a
# trust failure.
set -euo pipefail

MODE="${1:-}"
case "$MODE" in
  "" | --any) ;;
  *)
    echo "usage: $(basename "$0") [--any]" >&2
    exit 2
    ;;
esac

# A release-plz tag NAME, before the fingerprint check. Anchored, so a
# branch-like ref can never match.
TAG_NAME_RE='^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$'

mapfile -t at_head < <(git tag --points-at HEAD | grep -E "$TAG_NAME_RE" || true)

for tag in "${at_head[@]}"; do
  [[ -n "$tag" ]] || continue

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
