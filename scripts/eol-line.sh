#!/usr/bin/env bash
# Declares a line EOL: deletes the support branch (remote + local) and marks
# the entry. Branch deletion is the CONSEQUENCE of the state, not its cause:
# support.json is the source of truth, the branch is its physical projection.
# EOL means no fixes, no security fixes, no backports, no releases — except
# via an explicitly approved exception issue (which recreates the line).
#
# Run inside a chore/support-* branch (base main); the mutation travels by PR.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

MINOR="${1:?usage: eol-line.sh X.Y}"
SUPPORT_JSON=".github/support.json"
[[ -f "$SUPPORT_JSON" ]] || { echo "$SUPPORT_JSON no existe" >&2; exit 1; }
BRANCH="support/${MINOR}"

if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
  git push origin --delete "$BRANCH"
  git branch -D "$BRANCH" 2>/dev/null || true
fi

jq --arg minor "$MINOR" --arg at "$(date -Iseconds)" \
  '(.lines[] | select(.minor == $minor)) |= (
     .state = "EOL" | .branch = null | .eol_at = $at |
     .eol_reason = (.eol_reason // "auto: rotate-stable, third line back from STABLE") |
     del(.accepts)
   )' "$SUPPORT_JSON" > "${SUPPORT_JSON}.tmp"
mv "${SUPPORT_JSON}.tmp" "$SUPPORT_JSON"
echo "OK: línea $MINOR en EOL."
