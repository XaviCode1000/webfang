#!/usr/bin/env bash
# Materializes support/X.Y from the line's latest tag. ONLY creates the
# branch and records it in .github/support.json — it never touches `state`
# (rotation STABLE->MAINTENANCE is scripts/rotate-stable.sh's job; conflating
# both events was a design bug, see the release-lifecycle investigation).
#
# Run inside a chore/support-* branch (base main); the support.json mutation
# travels by normal PR. Fails closed: EOL lines, missing tags, existing
# branches and unknown lines are all hard errors, never warnings.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

MINOR="${1:?usage: cut-support-branch.sh X.Y}"
SUPPORT_JSON=".github/support.json"
[[ -f "$SUPPORT_JSON" ]] || { echo "$SUPPORT_JSON no existe" >&2; exit 1; }

STATE="$(jq -r --arg m "$MINOR" '.lines[] | select(.minor==$m) | .state // empty' "$SUPPORT_JSON")"
[[ -n "$STATE" ]] || { echo "línea $MINOR no existe en support.json" >&2; exit 1; }
[[ "$STATE" != "EOL" ]] || { echo "línea $MINOR en EOL: requiere issue de excepción aprobada antes de recrear la rama" >&2; exit 1; }

LATEST="$(jq -r --arg m "$MINOR" '.lines[] | select(.minor==$m) | .latest' "$SUPPORT_JSON")"
TAG="v${LATEST}"
BRANCH="support/${MINOR}"

git fetch --tags origin
git rev-parse "$TAG" >/dev/null 2>&1 || { echo "tag $TAG no existe" >&2; exit 1; }
git show-ref --verify --quiet "refs/heads/$BRANCH" && { echo "$BRANCH ya existe" >&2; exit 1; }

git branch "$BRANCH" "$TAG"
git push origin "$BRANCH"

jq --arg minor "$MINOR" --arg branch "$BRANCH" \
  '(.lines[] | select(.minor == $minor)) |= (.branch = $branch)' \
  "$SUPPORT_JSON" > "${SUPPORT_JSON}.tmp"
mv "${SUPPORT_JSON}.tmp" "$SUPPORT_JSON"
echo "OK: $BRANCH creada desde $TAG; support.json actualizado."
