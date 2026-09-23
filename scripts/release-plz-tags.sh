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
#   --tag <name>  print the commit <name> resolves to IFF it is trusted;
#            exit 3 if the tag exists but is not trusted; exit 4 if missing.
#            Entry point for the provenance gate.
#
# For the HEAD-scoped modes, `--points-at HEAD` is the second half of the
# predicate: release-plz tags the commit it just released, so a tag at HEAD
# belongs to THIS run, whereas listing history there would drag in every
# historical tag.
#
# Exit status: 0 for every mode, including an empty list (a normal outcome).
# Non-zero only for a usage error (2), untrusted existing tag (3), or missing tag (4).
# Callers never mistake "no tags" for a trust failure.
set -euo pipefail

# Source the shared trust predicate library.
#
# The guard is load-bearing, not tidiness. A bare `source` of a missing file makes
# this script exit 1 with an EMPTY stdout, and the one caller that matters
# (scripts/reconcile-releases.sh) reads that empty list through `mapfile`, which
# swallows the exit status and reports "nothing to reconcile" with exit 0. Measured:
#   rm release-tag-trust.sh && bash scripts/reconcile-releases.sh
#   -> "No release-plz tags in history; nothing to reconcile."   rc=0
# A broken trust predicate must never look like a clean sweep — that is the
# green-over-failure pattern removed in webfang#1476. Fail loudly instead.
TRUST_LIB="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release-tag-trust.sh"
if [[ ! -f "$TRUST_LIB" ]]; then
  echo "::error::$(basename "$0"): missing trust predicate library ${TRUST_LIB} - refusing to report an empty tag list, because callers read empty as 'nothing to release'." >&2
  exit 5
fi
# shellcheck source=scripts/release-tag-trust.sh
source "$TRUST_LIB"

MODE="${1:-}"
TAG_ARG="${2:-}"

case "$MODE" in
  "" | --any | --all | --tag) ;;
  *)
    echo "usage: $(basename "$0") [--any|--all|--tag <name>]" >&2
    exit 2
    ;;
esac

if [[ "$MODE" == "--tag" ]]; then
  [[ -n "$TAG_ARG" ]] || { echo "usage: $(basename "$0") --tag <name>" >&2; exit 2; }
  if git rev-parse "refs/tags/$TAG_ARG" >/dev/null 2>&1; then
    if release_tag_is_trusted "$TAG_ARG"; then
      release_tag_commit "$TAG_ARG"
      exit 0
    else
      # Tag exists but is not trusted — exit 3 for the provenance gate.
      exit 3
    fi
  else
    # Tag does not exist — exit 4 for the provenance gate.
    exit 4
  fi
fi

if [[ "$MODE" == "--all" ]]; then
  mapfile -t candidates < <(git tag -l | grep -E "$RELEASE_TAG_NAME_RE" || true)
else
  mapfile -t candidates < <(git tag --points-at HEAD | grep -E "$RELEASE_TAG_NAME_RE" || true)
fi

for tag in "${candidates[@]}"; do
  [[ -n "$tag" ]] || continue

  # `--any` skips the fingerprint on purpose: it answers "did any tag land here",
  # which is exactly the question the fingerprint cannot answer.
  if [[ "$MODE" == "--any" ]]; then
    printf '%s\n' "$tag"
    continue
  fi

  if release_tag_is_trusted "$tag"; then
    printf '%s\n' "$tag"
  fi
done
