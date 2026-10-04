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
#   The release-plz fingerprint (annotated + automation tagger + "chore:
#   Release package ..." subject) was duplicated across release-plz-tags.sh,
#   verify-release-tag.sh, dispatch-release, and reconcile-releases.sh. Copies
#   drift; one copy cannot. Centralising it here means a format change in
#   release-plz updates exactly one place.
#
# WHY TWO TAGGER NAMES
#   Slice 1 of the App migration moved the Release PR job to a GitHub App
#   token; slice 2 moves the release/tag job too, so tags may now be authored
#   by EITHER `github-actions[bot]` (old GITHUB_TOKEN pushes, still in history)
#   OR the App bot `release-plz-tu-repo[bot]`. Both are accepted during the
#   migration window, and the old name must keep verifying: history is never
#   re-tagged. The exact tagger login an App push produces depends on the
#   ambient git config at push time and is unverifiable without a live push —
#   `release-plz-tu-repo[bot]` is the expected name, to be confirmed against
#   the next live tag. Either way the real boundary stays where this file
#   already documents it: the TAG RULESET plus branch protection on main
#   control WHO can create a `v*` ref; this predicate only filters tags that
#   ALREADY exist.
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
# Returns 0 iff the tag is annotated AND the tagger is the release-plz
# automation — either "github-actions[bot]" (pre-migration GITHUB_TOKEN pushes,
# still the author of every historical tag) or "release-plz-tu-repo[bot]" (the
# App token since slice 2) — AND the subject starts with
# "chore: Release package ".
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
  [[ ("$tagger" == "github-actions[bot]" || "$tagger" == "release-plz-tu-repo[bot]") && "$subject" == "chore: Release package "* ]]
}

# release_tag_commit <tag>
# Echoes the commit the tag resolves to (dereferences annotated tags).
# Exits non-zero if the tag does not exist.
release_tag_commit() {
  local tag="${1:?tag required}"
  git rev-parse "refs/tags/$tag^{commit}"
}