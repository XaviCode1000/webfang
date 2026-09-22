#!/usr/bin/env bash
#
# Shared trust predicate for release-plz tags.
#
# SOURCED, NEVER EXECUTED DIRECTLY. Safe to source from a script that already has
# `set -euo pipefail`.
#
# Exports:
#   RELEASE_TAG_NAME_RE        — anchored tag-name regex for release tags
#   release_tag_is_trusted     — predicate: 0 iff tag matches release-plz fingerprint
#   release_tag_commit         — resolves tag to commit SHA (exits non-zero if missing)
#
# WHY THIS EXISTS
#   The release-plz fingerprint (annotated + github-actions[bot] tagger + "chore:
#   Release package ..." subject) was duplicated across release-plz-tags.sh,
#   verify-release-tag.sh, dispatch-release, and reconcile-releases.sh. Copies
#   drift; one copy cannot. Centralising it here means a format change in
#   release-plz updates exactly one place.
#
# SECURITY HONESTY (misfeature guard, NOT an authentication boundary)
#   This fingerprint is a MISFEATURE GUARD — it prevents accidental dispatch of
#   human tags (v1.0.0, v2.0.0) that would publish binaries built from unrelated
#   code. It is NOT an authentication boundary because:
#     * A tag object's tagger name comes from local `git config user.name` —
#       anyone with write access can set `git config user.name "github-actions[bot]"`
#       and compose any subject.
#     * The real authentication boundary is the TAG RULESET (enforced by
#       scripts/apply-tag-ruleset.sh) plus branch protection on main. Those
#       control WHO can create or move a `v*` ref. This predicate only filters
#       tags that ALREADY exist.
#   A future reader must not mistake the fingerprint for a security control.

# Anchored tag-name regex for release tags (vX.Y.Z or vX.Y.Z-rc.N).
# Matches the pattern used by release-plz and Cargo semantic versioning.
# Exported for consumers that source this library.
export RELEASE_TAG_NAME_RE='^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$'

# release_tag_is_trusted <tag>
# Returns 0 iff the tag is annotated AND the tagger is exactly
# "github-actions[bot]" AND the subject starts with "chore: Release package ".
# This predicate is semantically IDENTICAL to the inline check that previously
# lived in release-plz-tags.sh — it is security-relevant and copies of it drift,
# which is exactly why it is being centralised.
release_tag_is_trusted() {
  local tag="${1:?tag required}"
  local tagger subject

  # Annotated tags have a tagger; lightweight tags have an empty tagger.
  tagger="$(git for-each-ref --format='%(taggername)' "refs/tags/$tag" 2>/dev/null || true)"
  [[ -n "$tagger" ]] || return 1

  subject="$(git tag -l --format='%(contents:subject)' "$tag" 2>/dev/null || true)"
  [[ "$tagger" == "github-actions[bot]" && "$subject" == "chore: Release package "* ]]
}

# release_tag_commit <tag>
# Echoes the commit the tag resolves to (dereferences annotated tags).
# Exits non-zero if the tag does not exist.
release_tag_commit() {
  local tag="${1:?tag required}"
  git rev-parse "refs/tags/$tag^{commit}"
}