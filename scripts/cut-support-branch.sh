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

BRANCH="support/${MINOR}"

# ── B1 (#2060): the branch point comes from the TAGS, not from the declared
# `latest`. support.json is a derived artifact and can lag the reality that
# just shipped (declared 2.12.0 while v2.12.1 is published) — cutting from
# the field would silently create a hotfix line that never saw the released
# patch. Same filter chain as published_minors in check_support_drift.sh:
# strip `v`, exclude `-` (`-rc.N` is never a publication), keep only X.Y.Z,
# `sort -V`, newest last.
git fetch --tags origin
LATEST="$(git tag --list "v${MINOR}.*" | sed 's/^v//' | grep -v -- '-' \
  | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -n 1 || true)"
[[ -n "$LATEST" ]] || { echo "la línea $MINOR no tiene ningún tag vX.Y.Z publicado: no hay punto de corte para $BRANCH (fail closed)" >&2; exit 1; }
TAG="v${LATEST}"
# Kept as a fail-closed belt after the tag-derived resolution above.
git rev-parse "$TAG" >/dev/null 2>&1 || { echo "tag $TAG no existe" >&2; exit 1; }
git show-ref --verify --quiet "refs/heads/$BRANCH" && { echo "$BRANCH ya existe" >&2; exit 1; }

git branch "$BRANCH" "$TAG"
git push origin "$BRANCH"

jq --arg minor "$MINOR" --arg branch "$BRANCH" \
  '(.lines[] | select(.minor == $minor)) |= (.branch = $branch)' \
  "$SUPPORT_JSON" > "${SUPPORT_JSON}.tmp"
mv "${SUPPORT_JSON}.tmp" "$SUPPORT_JSON"
echo "OK: $BRANCH creada desde $TAG; support.json actualizado."
