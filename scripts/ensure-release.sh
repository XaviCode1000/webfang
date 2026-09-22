#!/usr/bin/env bash
#
# Ensure a GitHub Release exists and is COMPLETE for a release tag.
#
# Idempotent by construction: a Release that already carries every expected asset
# is a no-op, so this is safe to call both from the push-triggered path and from
# the reconciliation sweep. A Release left incomplete by a failed upload is
# re-dispatched rather than skipped.
#
# Why the retry (webfang#1484): the failure class here is transient — a GitHub
# API hiccup, or `actions: write` regressing mid-flight. With a single
# `gh workflow run`, a failure left the tag with no binaries and nothing to
# observe, because a dispatch that never happened produces no workflow run.
# A terminal failure is now loud and names the tag.
#
# The tag travels as an INPUT, and the dispatch deliberately does NOT pin `--ref`.
# What pins the artifact is the input: release.yml points BOTH of its checkouts at
# `inputs.tag || github.ref_name`. Passing `--ref <tag>` would instead make the run
# execute THAT TAG'S HISTORICAL release.yml — and this helper is shared with the
# reconciliation sweep, whose entire subject is old tags. A sweep for v2.0.0 would
# then run v2.0.0's release.yml, turning the pipeline into a function of when the
# tag was cut (and reintroducing pre-hardening definitions). Dispatching from the
# default branch always runs the current definition, hardened, with the input
# pinning the build. This is the same choice the v2.1.1 backfill made by hand.
#
# L1 Provenance: the dispatch now also passes `expected_sha` (the commit the tag
# resolves to) so release.yml's preflight can verify L1.1 Identity (Shape 2:
# default-branch dispatch requires PROV_EXPECTED_SHA).
#
# Usage: ensure-release.sh <tag> [expected_sha]
set -euo pipefail

TAG="${1:-}"
EXPECTED_SHA="${2:-}"
if [[ -z "$TAG" ]]; then
  echo "usage: $(basename "$0") <tag> [expected_sha]" >&2
  exit 2
fi

# The assets release.yml must publish for a release to count as complete (its
# build matrix plus SHA256SUMS.txt). This list is the single definition of
# "complete": the push path and the sweep cannot disagree about it.
EXPECTED_ASSETS=(
  webfang-x86_64-unknown-linux-gnu.tar.gz
  webfang-aarch64-unknown-linux-gnu.tar.gz
  webfang-aarch64-apple-darwin.tar.gz
  webfang-x86_64-pc-windows-msvc.zip
  SHA256SUMS.txt
)

if present=$(gh release view "$TAG" --repo "$GITHUB_REPOSITORY" \
              --json assets --jq '.assets[].name' 2>/dev/null); then
  missing=()
  for asset in "${EXPECTED_ASSETS[@]}"; do
    grep -qxF "$asset" <<<"$present" || missing+=("$asset")
  done
  if [[ "${#missing[@]}" -eq 0 ]]; then
    echo "Release $TAG is already complete; nothing to do."
    exit 0
  fi
  echo "Release $TAG exists but is incomplete; missing: ${missing[*]}"
else
  echo "Release $TAG does not exist yet."
fi

# Bounded retry with backoff. Three attempts covers a transient API failure
# without turning a real outage into an unbounded loop.
ATTEMPTS=3
RELEASE_DISPATCH_BACKOFF_SECONDS="${RELEASE_DISPATCH_BACKOFF_SECONDS:-10}"
for (( attempt = 1; attempt <= ATTEMPTS; attempt++ )); do
  # Build the dispatch command with optional expected_sha
  # Format matches the extraction regex in check_release_dispatch.sh:
  # backslash-continued lines ending with ; OR ${DISPATCH_CMD[@]} form
  DISPATCH_CMD=(
    gh workflow run release.yml
    --repo "$GITHUB_REPOSITORY"
    -f tag="$TAG"
  )
  if [[ -n "$EXPECTED_SHA" ]]; then
    DISPATCH_CMD+=(-f expected_sha="$EXPECTED_SHA")
  fi

  if "${DISPATCH_CMD[@]}"; then
    echo "Dispatched release.yml for $TAG (attempt $attempt)${EXPECTED_SHA:+ with expected_sha=$EXPECTED_SHA}."
    exit 0
  fi
  echo "::warning::dispatch attempt $attempt/$ATTEMPTS for $TAG failed." >&2
  if (( attempt < ATTEMPTS )); then
    # Overridable so the semantics test does not spend real time waiting;
    # production keeps a real pause between attempts.
    sleep $(( attempt * RELEASE_DISPATCH_BACKOFF_SECONDS ))
  fi
done

echo "::error::could not dispatch release.yml for $TAG after $ATTEMPTS attempts - that tag now has no binaries. Recover with: gh workflow run release.yml --repo $GITHUB_REPOSITORY -f tag=$TAG${EXPECTED_SHA:+ -f expected_sha=$EXPECTED_SHA}" >&2
exit 1