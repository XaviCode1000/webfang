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
# WHY THE COMPLETENESS CHECK READS ASSETS BY RELEASE ID (webfang#1535):
# it used to read them from the by-tag/by-name view (`gh release view --json
# assets`, the same data as GET .../releases/tags/<tag>'s embedded .assets).
# For release id 394502300 / tag v2.3.0 that view returned an EMPTY .assets
# array while the release actually had all 5 assets: GET .../releases,
# GET .../releases/{id} and GET .../releases/{id}/assets each returned 5
# (control v2.2.0: 5 everywhere). A COMPLETE release was therefore reported
# as incomplete and dispatched spuriously (run 35844323775), which then failed
# closed at L1 preflight for a missing expected_sha — that fail-closed was
# correct, but the dispatch should never have happened.
# So: by-tag is used ONLY to resolve the release id (it returned a valid id
# even when its embedded assets were empty), and assets are read EXCLUSIVELY
# from GET /repos/$GITHUB_REPOSITORY/releases/<id>/assets — the one endpoint
# measured to return the full list. Three states stay distinct: no release ->
# "does not exist yet"; resolvable id with missing assets -> "incomplete";
# resolvable id with every expected asset -> "already complete", exit 0 with
# NO dispatch. A present-but-empty assets array on a resolvable id is
# "incomplete", never "release missing".
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

# Capture the existence probe's stderr, so a 404 can be told apart from any
# other API failure: 404 means "no release" (dispatch path), anything else is
# unknown state and must fail closed rather than dispatch on an error (guessing
# "missing" here is the spurious-dispatch class fixed above).
API_ERR="$(mktemp)"
trap 'rm -f "$API_ERR"' EXIT

release_id=""
if release_id="$(gh api "repos/${GITHUB_REPOSITORY}/releases/tags/${TAG}" \
      --jq '.id' 2>"$API_ERR")"; then
  if [[ -z "$release_id" || "$release_id" == "null" ]]; then
    echo "::error::release $TAG resolved without an id - refusing to guess whether the release exists (webfang#1535)." >&2
    exit 1
  fi
  # The release exists. Read its assets BY ID — never from the by-tag object,
  # whose embedded .assets were measured empty while this endpoint returned 5.
  if ! present="$(gh api "repos/${GITHUB_REPOSITORY}/releases/${release_id}/assets" \
        --jq '.[].name' 2>"$API_ERR")"; then
    echo "::error::release $TAG exists (id $release_id) but its assets could not be read ($(tr '\n' ' ' <"$API_ERR")) - failing closed instead of guessing: calling it 'incomplete' is the spurious dispatch of run 35844323775, calling it 'complete' would hide missing binaries." >&2
    exit 1
  fi
  missing=()
  for asset in "${EXPECTED_ASSETS[@]}"; do
    grep -qxF "$asset" <<<"$present" || missing+=("$asset")
  done
  if [[ "${#missing[@]}" -eq 0 ]]; then
    echo "Release $TAG is already complete; nothing to do."
    exit 0
  fi
  echo "Release $TAG exists but is incomplete; missing: ${missing[*]}"
elif grep -q 'HTTP 404' "$API_ERR"; then
  echo "Release $TAG does not exist yet."
else
  echo "::error::could not determine whether release $TAG exists: $(tr '\n' ' ' <"$API_ERR") - failing closed instead of dispatching on an unreadable API response (webfang#1535)." >&2
  exit 1
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